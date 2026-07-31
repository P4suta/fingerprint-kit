//! Driving a match-on-chip device that lives in another process.
//!
//! This is the host end of the [`protocol`](crate::protocol) pipe. It spawns a driver worker,
//! speaks JSON Lines to it on stdio, and presents the result as an ordinary
//! [`MatchOnChipDevice`] — so nothing above this module knows or cares that the device is behind
//! a process boundary, links a C library, or is written in another language.
//!
//! That indirection is the whole design. A driver can need `unsafe`, a foreign toolchain, or a
//! licence the core will not take; none of that crosses the pipe. Replacing one worker with
//! another changes nothing here.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::moc::MatchOnChipDevice;
use crate::protocol::{
    DeviceCapability, DeviceInfo, PROTOCOL_VERSION, Request, Response, SessionValidator,
    decode_response, decode_template_data, encode_request, encode_template_data,
};
use crate::{Error, MatchVerdict, Result};

/// A match-on-chip device driven by a worker process.
pub struct WorkerDevice {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    validator: SessionValidator,
    device: DeviceInfo,
    next_id: u64,
}

impl WorkerDevice {
    /// Spawn `program`, negotiate the protocol, and bind to the first match-on-chip device it
    /// enumerates.
    ///
    /// A worker that offers only host-image devices is refused here rather than later: it cannot
    /// serve an enrollment, and finding that out mid-operation would waste a user's finger.
    pub fn spawn(program: &str, arguments: &[&str]) -> Result<Self> {
        let mut child = Command::new(program)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr is left inherited on purpose: a driver's diagnostics belong on the terminal
            // where a person can see them, not swallowed into this pipe.
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(Error::io)?;
        let stdin = child.stdin.take().ok_or_else(|| {
            Error::processing("driver worker did not provide a standard input pipe")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            Error::processing("driver worker did not provide a standard output pipe")
        })?;

        let mut worker = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            validator: SessionValidator::new(),
            device: DeviceInfo {
                device_id: String::new(),
                capture_profile_id: String::new(),
                capability: DeviceCapability::MatchOnChip,
                enroll_stages: 0,
            },
            next_id: 0,
        };

        let request_id = worker.mint("hello");
        match worker.exchange_one(Request::Hello {
            request_id: request_id.clone(),
            protocol_version: PROTOCOL_VERSION,
        })? {
            Response::HelloAck { .. } => {}
            other => return Err(unexpected("hello acknowledgement", &other)),
        }

        let request_id = worker.mint("enumerate");
        let devices = match worker.exchange_one(Request::Enumerate { request_id })? {
            Response::Devices { devices, .. } => devices,
            other => return Err(unexpected("device list", &other)),
        };
        worker.device = devices
            .into_iter()
            .find(|device| device.capability == DeviceCapability::MatchOnChip)
            .ok_or_else(|| {
                Error::processing("the driver worker enumerated no match-on-chip device")
            })?;
        if worker.device.enroll_stages == 0 {
            return Err(Error::processing(
                "the driver worker reports a device that cannot enroll",
            ));
        }
        Ok(worker)
    }

    /// The device this worker bound to.
    #[must_use]
    pub fn device(&self) -> &DeviceInfo {
        &self.device
    }

    fn mint(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{}", self.next_id)
    }

    fn send(&mut self, request: Request) -> Result<()> {
        self.validator.accept_request(&request)?;
        let line = encode_request(&request)?;
        self.stdin.write_all(&line).map_err(Error::io)?;
        self.stdin.flush().map_err(Error::io)
    }

    fn receive(&mut self) -> Result<Response> {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).map_err(Error::io)?;
        if read == 0 {
            return Err(Error::processing("driver worker closed its output"));
        }
        let response = decode_response(line.trim_end().as_bytes())?;
        self.validator.accept_response(&response)?;
        if let Response::Error { code, message, .. } = &response {
            return Err(Error::processing(format!(
                "driver worker: {code}: {message}"
            )));
        }
        Ok(response)
    }

    /// Send one request and read exactly one response.
    fn exchange_one(&mut self, request: Request) -> Result<Response> {
        self.send(request)?;
        self.receive()
    }
}

impl Drop for WorkerDevice {
    fn drop(&mut self) {
        // Ask before killing: a driver mid-transfer should get to put the sensor down.
        let request_id = format!("shutdown-{}", self.next_id + 1);
        let _ = encode_request(&Request::Shutdown { request_id }).map(|line| {
            self.stdin
                .write_all(&line)
                .and_then(|()| self.stdin.flush())
        });
        let _ = self.child.wait();
    }
}

impl MatchOnChipDevice for WorkerDevice {
    fn driver_id(&self) -> &str {
        &self.device.capture_profile_id
    }

    fn device_profile_id(&self) -> &str {
        &self.device.device_id
    }

    fn enroll_stages(&self) -> u32 {
        self.device.enroll_stages
    }

    fn enroll(&mut self, on_progress: &mut dyn FnMut(u32, u32)) -> Result<Vec<u8>> {
        let request_id = self.mint("enroll");
        let operation_id = self.mint("operation");
        let device_id = self.device.device_id.clone();
        self.send(Request::StartEnroll {
            request_id,
            operation_id,
            device_id,
        })?;

        let mut template = None;
        loop {
            match self.receive()? {
                Response::Started { .. } => {}
                Response::EnrollProgress {
                    completed_stages,
                    total_stages,
                    ..
                } => on_progress(completed_stages, total_stages),
                Response::Enrolled {
                    template_base64, ..
                } => template = Some(decode_template_data(&template_base64)?),
                // `Completed` ends the operation, not the last progress event: a driver need not
                // report its final stage. See `MatchOnChipDevice`.
                Response::Completed { .. } => break,
                other => return Err(unexpected("an enrollment event", &other)),
            }
        }
        template.ok_or_else(|| {
            Error::processing("the driver worker completed enrollment without a template")
        })
    }

    fn verify(&mut self, blob: &[u8]) -> Result<MatchVerdict> {
        let request_id = self.mint("verify");
        let operation_id = self.mint("operation");
        let device_id = self.device.device_id.clone();
        self.send(Request::StartVerify {
            request_id,
            operation_id,
            device_id,
            template_base64: encode_template_data(blob)?,
        })?;

        let mut verdict = None;
        loop {
            match self.receive()? {
                Response::Started { .. }
                | Response::FingerPresent { .. }
                | Response::FingerRemoved { .. } => {}
                Response::MatchResult { matched, .. } => verdict = Some(MatchVerdict { matched }),
                Response::Completed { .. } => break,
                other => return Err(unexpected("a verification event", &other)),
            }
        }
        verdict.ok_or_else(|| {
            Error::processing("the driver worker completed verification without a verdict")
        })
    }
}

fn unexpected(expected: &str, got: &Response) -> Error {
    // `Response`'s Debug is redacted to its kind, so this cannot leak a frame or a template.
    Error::processing(format!(
        "expected {expected} from the driver worker, got {got:?}"
    ))
}
