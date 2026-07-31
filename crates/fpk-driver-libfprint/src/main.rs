//! A match-on-chip driver worker backed by libfprint.
//!
//! Reads `fingerprint-kit` protocol requests as JSON Lines on stdin and writes responses on
//! stdout. Everything device-specific is on this side of the pipe; the host learns only what the
//! protocol says.
//!
//! The template it hands back is an **FP3 serialization of libfprint's own `Print`**, not the raw
//! device bytes. FP3 round-trips a complete print — including the driver and device identity
//! libfprint checks before it will compare anything — so the blob the host stores is exactly what
//! is needed to reconstruct the print at verification time. It is also the format whose encoding
//! was proven byte-identical to libfprint's own against real hardware.
//!
//! # Running it
//!
//! It needs a libfprint with a driver for the attached sensor, and access to the USB bus. See the
//! repository's `docker/` notes; the host side of this pipe is `fingerprint-kit`'s
//! `LibfprintWorkerDevice`.

use std::io::{BufRead, Write};

use fingerprint_kit::protocol::{
    DeviceCapability, DeviceInfo, PROTOCOL_VERSION, Request, Response, SessionValidator,
    decode_request, decode_template_data, encode_response, encode_template_data,
};
use fprint_core::{Backend, Device, Finger, Print};

/// Everything that can go wrong is reported to the host as a protocol error, so the worker's own
/// error type is just a code and a message.
struct WorkerError {
    code: &'static str,
    message: String,
}

impl WorkerError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

type WorkerResult<T> = std::result::Result<T, WorkerError>;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fpk-driver-libfprint: {}: {}", error.code, error.message);
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> WorkerResult<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut validator = SessionValidator::new();

    for line in stdin.lock().lines() {
        let line = line.map_err(|error| WorkerError::new("io", error.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }

        let request = match decode_request(line.as_bytes()) {
            Ok(request) => request,
            Err(error) => {
                // A line we cannot parse has no request id to attribute the failure to.
                emit(
                    &mut stdout,
                    &mut validator,
                    protocol_error(None, None, "malformed", &error),
                )?;
                continue;
            }
        };
        if let Err(error) = validator.accept_request(&request) {
            let id = request_id_of(&request).to_owned();
            emit(
                &mut stdout,
                &mut validator,
                protocol_error(Some(id), None, "out_of_order", &error),
            )?;
            continue;
        }

        if matches!(request, Request::Shutdown { .. }) {
            return Ok(());
        }
        serve(&mut stdout, &mut validator, &request)?;
    }
    Ok(())
}

/// Handle one request, reporting any device failure to the host rather than dying on it.
fn serve(
    out: &mut impl Write,
    validator: &mut SessionValidator,
    request: &Request,
) -> WorkerResult<()> {
    let responses = match dispatch(request) {
        Ok(responses) => responses,
        Err(error) => vec![Response::Error {
            request_id: Some(request_id_of(request).to_owned()),
            operation_id: operation_id_of(request).map(str::to_owned),
            code: error.code.to_owned(),
            message: error.message,
        }],
    };
    for response in responses {
        emit(out, validator, response)?;
    }
    Ok(())
}

fn dispatch(request: &Request) -> WorkerResult<Vec<Response>> {
    match request {
        Request::Hello {
            request_id,
            protocol_version,
        } => {
            if *protocol_version != PROTOCOL_VERSION {
                return Err(WorkerError::new(
                    "version",
                    format!("this worker speaks protocol {PROTOCOL_VERSION}"),
                ));
            }
            Ok(vec![Response::HelloAck {
                request_id: request_id.clone(),
                protocol_version: PROTOCOL_VERSION,
            }])
        }
        Request::Enumerate { request_id } => Ok(vec![Response::Devices {
            request_id: request_id.clone(),
            devices: enumerate()?,
        }]),
        Request::StartEnroll {
            request_id,
            operation_id,
            device_id,
        } => enroll(request_id, operation_id, device_id),
        Request::StartVerify {
            request_id,
            operation_id,
            device_id,
            template_base64,
        } => verify(request_id, operation_id, device_id, template_base64),
        Request::StartCapture { .. } => Err(WorkerError::new(
            "unsupported",
            "this worker drives match-on-chip devices; it emits no frames",
        )),
        Request::Cancel { .. } => Err(WorkerError::new(
            "unsupported",
            "operations here are synchronous and cannot be cancelled mid-flight",
        )),
        Request::Shutdown { .. } => Ok(Vec::new()),
    }
}

fn enumerate() -> WorkerResult<Vec<DeviceInfo>> {
    let backend = fprint_backend_libfprint::LibfprintBackend::new();
    let devices = pollster::block_on(backend.enumerate())
        .map_err(|error| WorkerError::new("enumerate", error.to_string()))?;
    Ok(devices
        .iter()
        .map(|device| {
            let info = device.info();
            DeviceInfo {
                device_id: info.id.as_str().to_owned(),
                capture_profile_id: info.driver.as_str().to_owned(),
                // Every device this worker serves is match-on-chip by construction: it exposes
                // no capture path at all, so claiming host-image would be a lie the host would
                // then act on.
                capability: DeviceCapability::MatchOnChip,
                enroll_stages: info.enroll_stages,
            }
        })
        .collect())
}

/// Open the named device, or say plainly that it is not here.
fn open(device_id: &str) -> WorkerResult<impl Device> {
    let backend = fprint_backend_libfprint::LibfprintBackend::new();
    let mut device = pollster::block_on(backend.open(&device_id.into()))
        .map_err(|error| WorkerError::new("open", error.to_string()))?;
    pollster::block_on(device.open())
        .map_err(|error| WorkerError::new("open", error.to_string()))?;
    Ok(device)
}

fn enroll(request_id: &str, operation_id: &str, device_id: &str) -> WorkerResult<Vec<Response>> {
    let mut device = open(device_id)?;
    let total = device.info().enroll_stages;

    let mut responses = vec![Response::Started {
        request_id: request_id.to_owned(),
        operation_id: operation_id.to_owned(),
    }];

    // Progress is collected rather than streamed because the shim's enroll blocks this thread
    // until it finishes. The ordering the host sees is unchanged; only the timing is.
    let mut progress = Vec::new();
    let print = pollster::block_on(
        device.enroll(Print::new_for_enroll(Finger::RightIndex), |p| {
            if p.retry.is_none() {
                progress.push((p.completed_stages, p.total_stages.max(total)));
            }
        }),
    )
    .map_err(|error| WorkerError::new("enroll", error.to_string()))?;

    for (completed, total_stages) in progress {
        responses.push(Response::EnrollProgress {
            operation_id: operation_id.to_owned(),
            completed_stages: completed,
            total_stages,
        });
    }

    let blob = fprint_fp3::to_bytes(&print)
        .map_err(|error| WorkerError::new("serialize", error.to_string()))?;
    responses.push(Response::Enrolled {
        operation_id: operation_id.to_owned(),
        template_base64: encode_template_data(&blob)
            .map_err(|error| WorkerError::new("serialize", error.to_string()))?,
    });
    responses.push(Response::Completed {
        operation_id: operation_id.to_owned(),
    });

    let _ = pollster::block_on(device.close());
    Ok(responses)
}

fn verify(
    request_id: &str,
    operation_id: &str,
    device_id: &str,
    template_base64: &str,
) -> WorkerResult<Vec<Response>> {
    let blob = decode_template_data(template_base64)
        .map_err(|error| WorkerError::new("template", error.to_string()))?;
    let print = fprint_fp3::from_bytes(&blob)
        .map_err(|error| WorkerError::new("template", error.to_string()))?;

    let mut device = open(device_id)?;
    let outcome = pollster::block_on(device.verify(&print))
        .map_err(|error| WorkerError::new("verify", error.to_string()))?;
    let _ = pollster::block_on(device.close());

    Ok(vec![
        Response::Started {
            request_id: request_id.to_owned(),
            operation_id: operation_id.to_owned(),
        },
        Response::MatchResult {
            operation_id: operation_id.to_owned(),
            matched: outcome.matched,
        },
        Response::Completed {
            operation_id: operation_id.to_owned(),
        },
    ])
}

fn protocol_error(
    request_id: Option<String>,
    operation_id: Option<String>,
    code: &str,
    error: &impl std::fmt::Display,
) -> Response {
    Response::Error {
        request_id,
        operation_id,
        code: code.to_owned(),
        message: error.to_string(),
    }
}

/// Write one response, keeping the worker's own view of the session honest as it goes.
fn emit(
    out: &mut impl Write,
    validator: &mut SessionValidator,
    response: Response,
) -> WorkerResult<()> {
    // Validating our own output is not ceremony: it is what stops a driver bug from becoming a
    // protocol violation the host has to defend against.
    validator
        .accept_response(&response)
        .map_err(|error| WorkerError::new("self_check", error.to_string()))?;
    let line = encode_response(&response)
        .map_err(|error| WorkerError::new("encode", error.to_string()))?;
    out.write_all(&line)
        .and_then(|()| out.flush())
        .map_err(|error| WorkerError::new("io", error.to_string()))
}

fn request_id_of(request: &Request) -> &str {
    match request {
        Request::Hello { request_id, .. }
        | Request::Enumerate { request_id }
        | Request::StartCapture { request_id, .. }
        | Request::StartEnroll { request_id, .. }
        | Request::StartVerify { request_id, .. }
        | Request::Cancel { request_id, .. }
        | Request::Shutdown { request_id } => request_id,
    }
}

fn operation_id_of(request: &Request) -> Option<&str> {
    match request {
        Request::StartCapture { operation_id, .. }
        | Request::StartEnroll { operation_id, .. }
        | Request::StartVerify { operation_id, .. }
        | Request::Cancel { operation_id, .. } => Some(operation_id),
        _ => None,
    }
}
