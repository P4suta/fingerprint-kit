//! Experimental in-process model of a future driver/process JSON Lines protocol.
//!
//! M0 defines types, bounded codecs, and ordering validation only. It does not spawn a worker or
//! expose a stable driver ABI.

use crate::{Error, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

/// Experimental protocol version.
pub const PROTOCOL_VERSION: u32 = 0;
/// Maximum encoded JSON line size, excluding the terminating newline.
pub const MAX_LINE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum decoded frame size.
pub const MAX_DECODED_FRAME_BYTES: usize = 1024 * 1024;
/// Maximum decoded device-template size, matching the domain's own bound.
pub const MAX_DECODED_TEMPLATE_BYTES: usize = crate::MAX_DEVICE_TEMPLATE_BYTES;
const MAX_TRACKED_IDENTIFIERS: usize = 4096;

/// Host-to-driver JSON Lines message.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Negotiate the experimental protocol version.
    Hello {
        /// Unique request identifier.
        request_id: String,
        /// Must equal [`PROTOCOL_VERSION`].
        protocol_version: u32,
    },
    /// Enumerate devices.
    Enumerate {
        /// Unique request identifier.
        request_id: String,
    },
    /// Start one capture operation.
    StartCapture {
        /// Unique request identifier.
        request_id: String,
        /// Operation identifier shared by capture events.
        operation_id: String,
        /// Driver-defined device identifier.
        device_id: String,
    },
    /// Start one enrollment on a match-on-chip device.
    StartEnroll {
        /// Unique request identifier.
        request_id: String,
        /// Operation identifier shared by enrollment events.
        operation_id: String,
        /// Driver-defined device identifier.
        device_id: String,
    },
    /// Ask a match-on-chip device to compare a live finger against a template it produced.
    StartVerify {
        /// Unique request identifier.
        request_id: String,
        /// Operation identifier shared by verification events.
        operation_id: String,
        /// Driver-defined device identifier.
        device_id: String,
        /// The device's own template, standard-alphabet padded base64.
        template_base64: String,
    },
    /// Cancel the active operation.
    Cancel {
        /// Unique request identifier, distinct from the operation identifier.
        request_id: String,
        /// Active operation identifier.
        operation_id: String,
    },
    /// Ask the future worker to shut down.
    Shutdown {
        /// Unique request identifier.
        request_id: String,
    },
}

impl fmt::Debug for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Request")
            .field(&self.kind())
            .finish()
    }
}

impl Request {
    fn kind(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::Enumerate { .. } => "enumerate",
            Self::StartCapture { .. } => "start_capture",
            Self::StartEnroll { .. } => "start_enroll",
            Self::StartVerify { .. } => "start_verify",
            Self::Cancel { .. } => "cancel",
            Self::Shutdown { .. } => "shutdown",
        }
    }

    fn request_id(&self) -> &str {
        match self {
            Self::Hello { request_id, .. }
            | Self::Enumerate { request_id }
            | Self::StartCapture { request_id, .. }
            | Self::StartEnroll { request_id, .. }
            | Self::StartVerify { request_id, .. }
            | Self::Cancel { request_id, .. }
            | Self::Shutdown { request_id } => request_id,
        }
    }

    fn started_operation_id(&self) -> Option<&str> {
        match self {
            Self::StartCapture { operation_id, .. }
            | Self::StartEnroll { operation_id, .. }
            | Self::StartVerify { operation_id, .. } => Some(operation_id),
            _ => None,
        }
    }

    /// Which kind of operation this request starts, if it starts one.
    fn starts(&self) -> Option<OperationKind> {
        match self {
            Self::StartCapture { .. } => Some(OperationKind::Capture),
            Self::StartEnroll { .. } => Some(OperationKind::Enroll),
            Self::StartVerify { .. } => Some(OperationKind::Verify),
            _ => None,
        }
    }
}

/// What an active operation is doing, which decides the events that may follow `Started`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationKind {
    Capture,
    Enroll,
    Verify,
}

/// One enumerated experimental device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceInfo {
    /// Driver-defined device identifier.
    pub device_id: String,
    /// Capture profile emitted by the device.
    pub capture_profile_id: String,
    /// Where this device does its matching, which decides what operations it can serve.
    pub capability: DeviceCapability,
    /// Presentations a full enrollment needs. `0` where the device does not enroll.
    pub enroll_stages: u32,
}

/// Where a device does its matching.
///
/// This is not decoration: the two are disjoint interaction models. A host-image device answers
/// [`Request::StartCapture`] with pixels and knows nothing about templates; a match-on-chip device
/// answers [`Request::StartEnroll`]/[`Request::StartVerify`] and never emits a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceCapability {
    /// Streams frames to the host, which extracts and matches.
    HostImage,
    /// Enrolls and matches internally, exchanging opaque templates.
    MatchOnChip,
}

/// Driver-to-host JSON Lines message.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    /// Successful protocol negotiation.
    HelloAck {
        /// Request being acknowledged.
        request_id: String,
        /// Negotiated version.
        protocol_version: u32,
    },
    /// Enumeration result.
    Devices {
        /// Enumeration request identifier.
        request_id: String,
        /// Available devices.
        devices: Vec<DeviceInfo>,
    },
    /// Capture operation accepted.
    Started {
        /// Start request identifier.
        request_id: String,
        /// Active operation identifier.
        operation_id: String,
    },
    /// A finger became present.
    FingerPresent {
        /// Active operation identifier.
        operation_id: String,
    },
    /// One base64 Gray8 capture frame.
    Frame {
        /// Active operation identifier.
        operation_id: String,
        /// Monotonic frame sequence within the operation.
        sequence: u64,
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
        /// Horizontal scan resolution.
        x_resolution_ppi: u16,
        /// Vertical scan resolution.
        y_resolution_ppi: u16,
        /// Standard-alphabet padded base64 frame bytes.
        data_base64: String,
    },
    /// A finger was removed.
    FingerRemoved {
        /// Active operation identifier.
        operation_id: String,
    },
    /// One enrollment stage was accepted.
    ///
    /// **A driver need not report the final stage.** libfprint's `upekts` reports a stage only once
    /// the following poll asks for another presentation, so the poll after the last presentation is
    /// "complete" and reports nothing: a 3-stage enrollment emits two of these. A host waiting for
    /// `completed_stages == total_stages` hangs. [`Response::Enrolled`] is the completion signal.
    EnrollProgress {
        /// Active operation identifier.
        operation_id: String,
        /// Stages accepted so far.
        completed_stages: u32,
        /// Stages a full enrollment needs.
        total_stages: u32,
    },
    /// Enrollment produced the device's own template.
    Enrolled {
        /// Active operation identifier.
        operation_id: String,
        /// The device's opaque template, standard-alphabet padded base64.
        template_base64: String,
    },
    /// The device compared a live finger against a template and reached a verdict.
    ///
    /// Carries no score: the sensor does not publish one, and inventing a host-side number here
    /// would be fabricating precision the device never reported.
    MatchResult {
        /// Active operation identifier.
        operation_id: String,
        /// Whether the device recognised the finger.
        matched: bool,
    },
    /// Capture completed successfully.
    Completed {
        /// Completed operation identifier.
        operation_id: String,
    },
    /// Cancellation completed.
    Cancelled {
        /// Cancellation request identifier.
        request_id: String,
        /// Cancelled operation identifier.
        operation_id: String,
    },
    /// Structured protocol or driver failure.
    Error {
        /// Related request, when available.
        request_id: Option<String>,
        /// Related operation, when available.
        operation_id: Option<String>,
        /// Stable machine-readable code.
        code: String,
        /// Human-readable diagnostic with no frame payload.
        message: String,
    },
}

impl fmt::Debug for Response {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Response")
            .field(&self.kind())
            .finish()
    }
}

impl Response {
    fn kind(&self) -> &'static str {
        match self {
            Self::HelloAck { .. } => "hello_ack",
            Self::Devices { .. } => "devices",
            Self::Started { .. } => "started",
            Self::FingerPresent { .. } => "finger_present",
            Self::Frame { .. } => "frame",
            Self::FingerRemoved { .. } => "finger_removed",
            Self::EnrollProgress { .. } => "enroll_progress",
            Self::Enrolled { .. } => "enrolled",
            Self::MatchResult { .. } => "match_result",
            Self::Completed { .. } => "completed",
            Self::Cancelled { .. } => "cancelled",
            Self::Error { .. } => "error",
        }
    }
}

/// Base64-encode a device template after enforcing the template limit.
pub fn encode_template_data(template: &[u8]) -> Result<String> {
    if template.is_empty() {
        return Err(Error::invalid("protocol template is empty"));
    }
    if template.len() > MAX_DECODED_TEMPLATE_BYTES {
        return Err(Error::invalid(
            "decoded protocol template exceeds the limit",
        ));
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(template))
}

/// Decode a device template's base64 after enforcing the template limit.
///
/// Bounded far below the frame limit on purpose: a template is a reference the sensor hands back,
/// not an image, so anything approaching frame size is a malformed or hostile line.
pub fn decode_template_data(template_base64: &str) -> Result<Vec<u8>> {
    let estimated = template_base64
        .len()
        .checked_add(3)
        .and_then(|length| length.checked_div(4))
        .and_then(|blocks| blocks.checked_mul(3))
        .ok_or_else(|| Error::invalid("base64 template size overflow"))?;
    if estimated > MAX_DECODED_TEMPLATE_BYTES + 2 {
        return Err(Error::invalid(
            "decoded protocol template exceeds the limit",
        ));
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(template_base64)
        .map_err(|_| Error::invalid("protocol template contains invalid base64"))?;
    if decoded.is_empty() {
        return Err(Error::invalid("protocol template is empty"));
    }
    if decoded.len() > MAX_DECODED_TEMPLATE_BYTES {
        return Err(Error::invalid(
            "decoded protocol template exceeds the limit",
        ));
    }
    Ok(decoded)
}

/// Encode one bounded request and append a JSON Lines newline.
pub fn encode_request(request: &Request) -> Result<Vec<u8>> {
    validate_request(request)?;
    encode_line(request)
}

/// Decode exactly one bounded request line.
pub fn decode_request(line: &[u8]) -> Result<Request> {
    let request: Request = decode_line(line)?;
    validate_request(&request)?;
    Ok(request)
}

/// Encode one bounded response and append a JSON Lines newline.
pub fn encode_response(response: &Response) -> Result<Vec<u8>> {
    validate_response(response)?;
    encode_line(response)
}

/// Decode exactly one bounded response line, including base64 and frame geometry validation.
pub fn decode_response(line: &[u8]) -> Result<Response> {
    let response: Response = decode_line(line)?;
    validate_response(&response)?;
    Ok(response)
}

/// Base64-encode a decoded frame after enforcing the 1 MiB limit.
pub fn encode_frame_data(frame: &[u8]) -> Result<String> {
    if frame.len() > MAX_DECODED_FRAME_BYTES {
        return Err(Error::invalid("decoded protocol frame exceeds 1 MiB"));
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(frame))
}

/// Decode a response frame's base64 after enforcing the 1 MiB limit.
pub fn decode_frame_data(data_base64: &str) -> Result<Vec<u8>> {
    let estimated = data_base64
        .len()
        .checked_add(3)
        .and_then(|length| length.checked_div(4))
        .and_then(|blocks| blocks.checked_mul(3))
        .ok_or_else(|| Error::invalid("base64 frame size overflow"))?;
    if estimated > MAX_DECODED_FRAME_BYTES + 2 {
        return Err(Error::invalid("decoded protocol frame exceeds 1 MiB"));
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data_base64)
        .map_err(|_| Error::invalid("protocol frame contains invalid base64"))?;
    if decoded.len() > MAX_DECODED_FRAME_BYTES {
        return Err(Error::invalid("decoded protocol frame exceeds 1 MiB"));
    }
    Ok(decoded)
}

fn encode_line<T: Serialize>(message: &T) -> Result<Vec<u8>> {
    let mut line = serde_json::to_vec(message)?;
    if line.len() > MAX_LINE_BYTES {
        return Err(Error::invalid("protocol JSON line exceeds 2 MiB"));
    }
    line.push(b'\n');
    Ok(line)
}

fn decode_line<T: for<'de> Deserialize<'de>>(line: &[u8]) -> Result<T> {
    if line.len() > MAX_LINE_BYTES + 1 {
        return Err(Error::invalid("protocol JSON line exceeds 2 MiB"));
    }
    let body = line.strip_suffix(b"\n").unwrap_or(line);
    let body = body.strip_suffix(b"\r").unwrap_or(body);
    if body.len() > MAX_LINE_BYTES || body.contains(&b'\n') || body.contains(&b'\r') {
        return Err(Error::invalid("codec requires exactly one JSON line"));
    }
    serde_json::from_slice(body).map_err(Error::json)
}

fn validate_request(request: &Request) -> Result<()> {
    match request {
        Request::Hello {
            request_id,
            protocol_version: _,
        }
        | Request::Enumerate { request_id }
        | Request::Shutdown { request_id } => validate_identifier(request_id, "request ID"),
        Request::StartCapture {
            request_id,
            operation_id,
            device_id,
        }
        | Request::StartEnroll {
            request_id,
            operation_id,
            device_id,
        } => {
            validate_identifier(request_id, "request ID")?;
            validate_identifier(operation_id, "operation ID")?;
            if request_id == operation_id {
                return Err(Error::invalid(
                    "request ID and operation ID must be distinct",
                ));
            }
            validate_identifier(device_id, "device ID")
        }
        Request::StartVerify {
            request_id,
            operation_id,
            device_id,
            template_base64,
        } => {
            validate_identifier(request_id, "request ID")?;
            validate_identifier(operation_id, "operation ID")?;
            if request_id == operation_id {
                return Err(Error::invalid(
                    "request ID and operation ID must be distinct",
                ));
            }
            validate_identifier(device_id, "device ID")?;
            decode_template_data(template_base64).map(|_| ())
        }
        Request::Cancel {
            request_id,
            operation_id,
        } => {
            validate_identifier(request_id, "request ID")?;
            validate_identifier(operation_id, "operation ID")?;
            if request_id == operation_id {
                return Err(Error::invalid(
                    "request ID and operation ID must be distinct",
                ));
            }
            Ok(())
        }
    }
}

fn validate_response(response: &Response) -> Result<()> {
    match response {
        Response::HelloAck {
            request_id,
            protocol_version: _,
        }
        | Response::Devices {
            request_id,
            devices: _,
        } => validate_identifier(request_id, "request ID")?,
        Response::Started {
            request_id,
            operation_id,
        }
        | Response::Cancelled {
            request_id,
            operation_id,
        } => {
            validate_identifier(request_id, "request ID")?;
            validate_identifier(operation_id, "operation ID")?;
        }
        Response::FingerPresent { operation_id }
        | Response::FingerRemoved { operation_id }
        | Response::Completed { operation_id } => {
            validate_identifier(operation_id, "operation ID")?;
        }
        Response::MatchResult {
            operation_id,
            matched: _,
        } => {
            validate_identifier(operation_id, "operation ID")?;
        }
        Response::EnrollProgress {
            operation_id,
            completed_stages,
            total_stages,
        } => {
            validate_identifier(operation_id, "operation ID")?;
            if *total_stages == 0 {
                return Err(Error::invalid("enrollment declares no stages"));
            }
            // `completed <= total` only. Requiring the last stage to be reported would reject
            // honest drivers: upekts finishes a 3-stage enrollment having reported two.
            if *completed_stages == 0 || completed_stages > total_stages {
                return Err(Error::invalid("enrollment stage count is out of range"));
            }
        }
        Response::Enrolled {
            operation_id,
            template_base64,
        } => {
            validate_identifier(operation_id, "operation ID")?;
            decode_template_data(template_base64)?;
        }
        Response::Frame {
            operation_id,
            width,
            height,
            x_resolution_ppi,
            y_resolution_ppi,
            data_base64,
            ..
        } => {
            validate_identifier(operation_id, "operation ID")?;
            if *width == 0 || *height == 0 || *x_resolution_ppi == 0 || *y_resolution_ppi == 0 {
                return Err(Error::invalid("protocol frame geometry is invalid"));
            }
            let expected = u64::from(*width)
                .checked_mul(u64::from(*height))
                .ok_or_else(|| Error::invalid("protocol frame dimensions overflow"))?;
            let decoded = decode_frame_data(data_base64)?;
            if u64::try_from(decoded.len()).unwrap_or(u64::MAX) != expected {
                return Err(Error::invalid(
                    "decoded protocol frame length does not match geometry",
                ));
            }
        }
        Response::Error {
            request_id,
            operation_id,
            code,
            message,
        } => {
            if let Some(request_id) = request_id {
                validate_identifier(request_id, "request ID")?;
            }
            if let Some(operation_id) = operation_id {
                validate_identifier(operation_id, "operation ID")?;
            }
            validate_identifier(code, "error code")?;
            if message.len() > 4096 {
                return Err(Error::invalid("protocol error message is too long"));
            }
        }
    }
    if let Response::Devices { devices, .. } = response {
        if devices.len() > 256 {
            return Err(Error::invalid("protocol device list is too large"));
        }
        for device in devices {
            validate_identifier(&device.device_id, "device ID")?;
            validate_identifier(&device.capture_profile_id, "capture profile ID")?;
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 {
        return Err(Error::invalid(format!("{label} is empty or too long")));
    }
    Ok(())
}

#[derive(Clone)]
enum SessionState {
    NeedHello,
    AwaitHelloAck {
        request_id: String,
    },
    Ready,
    AwaitDevices {
        request_id: String,
    },
    AwaitStarted {
        request_id: String,
        operation_id: String,
        kind: OperationKind,
    },
    Capturing {
        operation_id: String,
        finger_present: bool,
        saw_frame: bool,
        last_sequence: Option<u64>,
    },
    Enrolling {
        operation_id: String,
        last_completed: Option<u32>,
        /// Set once the template arrives; only then may the operation complete.
        enrolled: bool,
    },
    Verifying {
        operation_id: String,
        /// Set once the device reports a verdict; only then may the operation complete.
        reported: bool,
    },
    AwaitCancelled {
        request_id: String,
        operation_id: String,
    },
    Closed,
}

/// Stateful validator for request/response negotiation and capture event ordering.
pub struct SessionValidator {
    state: SessionState,
    seen_request_ids: HashSet<String>,
    seen_operation_ids: HashSet<String>,
}

impl Default for SessionValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionValidator {
    /// Start a session in the hello-required state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: SessionState::NeedHello,
            seen_request_ids: HashSet::new(),
            seen_operation_ids: HashSet::new(),
        }
    }

    /// Accept a host request only when it is valid in the current state.
    pub fn accept_request(&mut self, request: &Request) -> Result<()> {
        validate_request(request)?;
        let request_id = request.request_id();
        if self.seen_request_ids.contains(request_id) {
            return Err(Error::invalid("request ID has already been used"));
        }
        if self.seen_request_ids.len() >= MAX_TRACKED_IDENTIFIERS {
            return Err(Error::invalid("protocol request ID limit reached"));
        }
        if let Some(operation_id) = request.started_operation_id() {
            if self.seen_operation_ids.contains(operation_id) {
                return Err(Error::invalid("operation ID has already been used"));
            }
            if self.seen_operation_ids.len() >= MAX_TRACKED_IDENTIFIERS {
                return Err(Error::invalid("protocol operation ID limit reached"));
            }
        }
        let next = match (self.state.clone(), request) {
            (
                SessionState::NeedHello,
                Request::Hello {
                    request_id,
                    protocol_version,
                },
            ) => {
                if *protocol_version != PROTOCOL_VERSION {
                    return Err(Error::invalid("experimental protocol version mismatch"));
                }
                SessionState::AwaitHelloAck {
                    request_id: request_id.clone(),
                }
            }
            (SessionState::NeedHello, _) => {
                return Err(Error::invalid("hello is required before any command"));
            }
            (SessionState::Ready, Request::Enumerate { request_id }) => {
                SessionState::AwaitDevices {
                    request_id: request_id.clone(),
                }
            }
            (
                SessionState::Ready,
                Request::StartCapture {
                    request_id,
                    operation_id,
                    ..
                }
                | Request::StartEnroll {
                    request_id,
                    operation_id,
                    ..
                }
                | Request::StartVerify {
                    request_id,
                    operation_id,
                    ..
                },
            ) => SessionState::AwaitStarted {
                request_id: request_id.clone(),
                operation_id: operation_id.clone(),
                kind: request
                    .starts()
                    .ok_or_else(|| Error::invalid("request does not start an operation"))?,
            },
            (SessionState::Ready, Request::Shutdown { .. }) => SessionState::Closed,
            (SessionState::Ready, Request::Cancel { .. }) => {
                return Err(Error::invalid("cancel requires an active operation"));
            }
            (SessionState::Ready, Request::Hello { .. }) => {
                return Err(Error::invalid("hello may only be sent once"));
            }
            (
                SessionState::Capturing { operation_id, .. }
                | SessionState::Enrolling { operation_id, .. }
                | SessionState::Verifying { operation_id, .. },
                Request::Cancel {
                    request_id,
                    operation_id: cancel_operation,
                },
            ) => {
                if &operation_id != cancel_operation {
                    return Err(Error::invalid(
                        "cancel operation ID does not match active operation",
                    ));
                }
                SessionState::AwaitCancelled {
                    request_id: request_id.clone(),
                    operation_id,
                }
            }
            (
                SessionState::Capturing { .. }
                | SessionState::Enrolling { .. }
                | SessionState::Verifying { .. }
                | SessionState::AwaitStarted { .. },
                Request::StartCapture { .. }
                | Request::StartEnroll { .. }
                | Request::StartVerify { .. },
            ) => {
                return Err(Error::invalid("an operation is already active"));
            }
            (SessionState::Closed, _) => {
                return Err(Error::invalid("protocol session is closed"));
            }
            _ => {
                return Err(Error::invalid(
                    "request is out of order for the protocol state",
                ));
            }
        };
        self.state = next;
        self.seen_request_ids.insert(request_id.to_owned());
        if let Some(operation_id) = request.started_operation_id() {
            self.seen_operation_ids.insert(operation_id.to_owned());
        }
        Ok(())
    }

    /// Accept a driver response/event only when identifiers and event order are valid.
    pub fn accept_response(&mut self, response: &Response) -> Result<()> {
        validate_response(response)?;
        if let Response::Error {
            request_id,
            operation_id,
            ..
        } = response
        {
            return self.accept_error_response(request_id, operation_id);
        }

        let next = match (self.state.clone(), response) {
            (
                SessionState::AwaitHelloAck { request_id },
                Response::HelloAck {
                    request_id: ack_request,
                    protocol_version,
                },
            ) => {
                if request_id != *ack_request || *protocol_version != PROTOCOL_VERSION {
                    return Err(Error::invalid(
                        "hello acknowledgement ID or version mismatch",
                    ));
                }
                SessionState::Ready
            }
            (
                SessionState::AwaitDevices { request_id },
                Response::Devices {
                    request_id: devices_request,
                    ..
                },
            ) => {
                if request_id != *devices_request {
                    return Err(Error::invalid("enumeration request ID mismatch"));
                }
                SessionState::Ready
            }
            (
                SessionState::AwaitStarted {
                    request_id,
                    operation_id,
                    kind,
                },
                Response::Started {
                    request_id: started_request,
                    operation_id: started_operation,
                },
            ) => {
                if request_id != *started_request || operation_id != *started_operation {
                    return Err(Error::invalid("started event identifiers do not match"));
                }
                // The request that opened the operation decides which events may follow. A
                // match-on-chip driver cannot answer with frames, and a capture driver cannot
                // answer with a verdict.
                match kind {
                    OperationKind::Capture => SessionState::Capturing {
                        operation_id,
                        finger_present: false,
                        saw_frame: false,
                        last_sequence: None,
                    },
                    OperationKind::Enroll => SessionState::Enrolling {
                        operation_id,
                        last_completed: None,
                        enrolled: false,
                    },
                    OperationKind::Verify => SessionState::Verifying {
                        operation_id,
                        reported: false,
                    },
                }
            }
            (
                SessionState::Enrolling {
                    operation_id,
                    last_completed,
                    enrolled,
                },
                Response::EnrollProgress {
                    operation_id: event_operation,
                    completed_stages,
                    ..
                },
            ) if operation_id == *event_operation => {
                if enrolled {
                    return Err(Error::invalid(
                        "enrollment progress after the template was delivered",
                    ));
                }
                if last_completed.is_some_and(|last| *completed_stages <= last) {
                    return Err(Error::invalid(
                        "enrollment stage count must increase monotonically",
                    ));
                }
                SessionState::Enrolling {
                    operation_id,
                    last_completed: Some(*completed_stages),
                    enrolled: false,
                }
            }
            (
                SessionState::Enrolling {
                    operation_id,
                    last_completed,
                    enrolled: false,
                },
                Response::Enrolled {
                    operation_id: event_operation,
                    ..
                },
            ) if operation_id == *event_operation => SessionState::Enrolling {
                operation_id,
                last_completed,
                enrolled: true,
            },
            (
                SessionState::Enrolling {
                    operation_id,
                    enrolled: true,
                    ..
                },
                Response::Completed {
                    operation_id: event_operation,
                },
            ) if operation_id == *event_operation => SessionState::Ready,
            (
                SessionState::Verifying {
                    operation_id,
                    reported: false,
                },
                Response::MatchResult {
                    operation_id: event_operation,
                    ..
                },
            ) if operation_id == *event_operation => SessionState::Verifying {
                operation_id,
                reported: true,
            },
            (
                SessionState::Verifying {
                    operation_id,
                    reported,
                },
                Response::FingerPresent {
                    operation_id: event_operation,
                }
                | Response::FingerRemoved {
                    operation_id: event_operation,
                },
            ) if operation_id == *event_operation => SessionState::Verifying {
                operation_id,
                reported,
            },
            (
                SessionState::Verifying {
                    operation_id,
                    reported: true,
                },
                Response::Completed {
                    operation_id: event_operation,
                },
            ) if operation_id == *event_operation => SessionState::Ready,
            (
                SessionState::Capturing {
                    operation_id,
                    finger_present: false,
                    saw_frame,
                    last_sequence,
                },
                Response::FingerPresent {
                    operation_id: event_operation,
                },
            ) if operation_id == *event_operation => SessionState::Capturing {
                operation_id,
                finger_present: true,
                saw_frame,
                last_sequence,
            },
            (
                SessionState::Capturing {
                    operation_id,
                    finger_present: true,
                    saw_frame: _,
                    last_sequence,
                },
                Response::Frame {
                    operation_id: event_operation,
                    sequence,
                    ..
                },
            ) if operation_id == *event_operation => {
                if last_sequence.is_some_and(|last| *sequence <= last) {
                    return Err(Error::invalid("frame sequence must increase monotonically"));
                }
                SessionState::Capturing {
                    operation_id,
                    finger_present: true,
                    saw_frame: true,
                    last_sequence: Some(*sequence),
                }
            }
            (
                SessionState::Capturing {
                    operation_id,
                    finger_present: true,
                    saw_frame,
                    last_sequence,
                },
                Response::FingerRemoved {
                    operation_id: event_operation,
                },
            ) if operation_id == *event_operation => SessionState::Capturing {
                operation_id,
                finger_present: false,
                saw_frame,
                last_sequence,
            },
            (
                SessionState::Capturing {
                    operation_id,
                    finger_present: false,
                    saw_frame: true,
                    last_sequence: _,
                },
                Response::Completed {
                    operation_id: event_operation,
                },
            ) if operation_id == *event_operation => SessionState::Ready,
            (
                SessionState::AwaitCancelled {
                    request_id,
                    operation_id,
                },
                Response::Cancelled {
                    request_id: cancelled_request,
                    operation_id: cancelled_operation,
                },
            ) => {
                if request_id != *cancelled_request || operation_id != *cancelled_operation {
                    return Err(Error::invalid("cancelled event identifiers do not match"));
                }
                SessionState::Ready
            }
            (SessionState::NeedHello, _) => {
                return Err(Error::invalid("response received before hello negotiation"));
            }
            (SessionState::Closed, _) => {
                return Err(Error::invalid("protocol session is closed"));
            }
            _ => {
                return Err(Error::invalid(
                    "response event is out of order for the protocol state",
                ));
            }
        };
        self.state = next;
        Ok(())
    }

    fn accept_error_response(
        &mut self,
        response_request_id: &Option<String>,
        response_operation_id: &Option<String>,
    ) -> Result<()> {
        let next = match self.state.clone() {
            SessionState::AwaitHelloAck { request_id }
                if response_request_id.as_deref() == Some(request_id.as_str())
                    && response_operation_id.is_none() =>
            {
                SessionState::NeedHello
            }
            SessionState::AwaitDevices { request_id }
                if response_request_id.as_deref() == Some(request_id.as_str())
                    && response_operation_id.is_none() =>
            {
                SessionState::Ready
            }
            SessionState::AwaitStarted {
                request_id,
                operation_id,
                ..
            } if response_request_id.as_deref() == Some(request_id.as_str())
                && response_operation_id
                    .as_deref()
                    .is_none_or(|value| value == operation_id) =>
            {
                SessionState::Ready
            }
            SessionState::Capturing { operation_id, .. }
            | SessionState::Enrolling { operation_id, .. }
            | SessionState::Verifying { operation_id, .. }
                if response_request_id.is_none()
                    && response_operation_id.as_deref() == Some(operation_id.as_str()) =>
            {
                SessionState::Ready
            }
            SessionState::AwaitCancelled {
                request_id,
                operation_id,
            } if response_request_id.as_deref() == Some(request_id.as_str())
                && response_operation_id.as_deref() == Some(operation_id.as_str()) =>
            {
                SessionState::Ready
            }
            SessionState::NeedHello => {
                return Err(Error::invalid(
                    "error response received before hello negotiation",
                ));
            }
            SessionState::Ready => {
                return Err(Error::invalid("unsolicited protocol error response"));
            }
            SessionState::Closed => {
                return Err(Error::invalid("protocol session is closed"));
            }
            _ => {
                return Err(Error::invalid(
                    "error response identifiers do not match protocol state",
                ));
            }
        };
        self.state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Request {
        Request::Hello {
            request_id: "hello-1".to_owned(),
            protocol_version: PROTOCOL_VERSION,
        }
    }

    fn hello_ack() -> Response {
        Response::HelloAck {
            request_id: "hello-1".to_owned(),
            protocol_version: PROTOCOL_VERSION,
        }
    }

    fn start() -> Request {
        Request::StartCapture {
            request_id: "request-2".to_owned(),
            operation_id: "operation-7".to_owned(),
            device_id: "device-1".to_owned(),
        }
    }

    /// A validator that has negotiated hello and is ready for a command.
    fn ready() -> SessionValidator {
        let mut validator = SessionValidator::new();
        validator.accept_request(&hello()).unwrap();
        validator.accept_response(&hello_ack()).unwrap();
        validator
    }

    fn started(request_id: &str, operation_id: &str) -> Response {
        Response::Started {
            request_id: request_id.to_owned(),
            operation_id: operation_id.to_owned(),
        }
    }

    #[test]
    fn an_enrollment_completes_without_reporting_its_final_stage() {
        // The behaviour a real sensor taught us. upekts finishes a 3-stage enrollment having
        // reported two, because the poll after the last presentation says "complete" rather than
        // "one more". A validator that demanded 3 of 3 would reject an honest driver.
        let mut validator = ready();
        validator
            .accept_request(&Request::StartEnroll {
                request_id: "request-2".to_owned(),
                operation_id: "operation-7".to_owned(),
                device_id: "device-1".to_owned(),
            })
            .unwrap();
        validator
            .accept_response(&started("request-2", "operation-7"))
            .unwrap();
        for completed in 1..=2 {
            validator
                .accept_response(&Response::EnrollProgress {
                    operation_id: "operation-7".to_owned(),
                    completed_stages: completed,
                    total_stages: 3,
                })
                .unwrap();
        }
        validator
            .accept_response(&Response::Enrolled {
                operation_id: "operation-7".to_owned(),
                template_base64: encode_template_data(&[7; 241]).unwrap(),
            })
            .unwrap();
        validator
            .accept_response(&Response::Completed {
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();
    }

    #[test]
    fn an_enrollment_cannot_complete_before_the_template_arrives() {
        let mut validator = ready();
        validator
            .accept_request(&Request::StartEnroll {
                request_id: "request-2".to_owned(),
                operation_id: "operation-7".to_owned(),
                device_id: "device-1".to_owned(),
            })
            .unwrap();
        validator
            .accept_response(&started("request-2", "operation-7"))
            .unwrap();
        // Completion with no template would leave the host with nothing to store.
        assert!(
            validator
                .accept_response(&Response::Completed {
                    operation_id: "operation-7".to_owned(),
                })
                .is_err()
        );
    }

    #[test]
    fn enrollment_progress_must_advance_and_stop_at_the_template() {
        let mut validator = ready();
        validator
            .accept_request(&Request::StartEnroll {
                request_id: "request-2".to_owned(),
                operation_id: "operation-7".to_owned(),
                device_id: "device-1".to_owned(),
            })
            .unwrap();
        validator
            .accept_response(&started("request-2", "operation-7"))
            .unwrap();
        let progress = |completed| Response::EnrollProgress {
            operation_id: "operation-7".to_owned(),
            completed_stages: completed,
            total_stages: 3,
        };
        validator.accept_response(&progress(2)).unwrap();
        assert!(validator.accept_response(&progress(2)).is_err());
        assert!(validator.accept_response(&progress(1)).is_err());
        // Out of range against the declared total.
        assert!(validator.accept_response(&progress(4)).is_err());

        validator
            .accept_response(&Response::Enrolled {
                operation_id: "operation-7".to_owned(),
                template_base64: encode_template_data(&[7; 8]).unwrap(),
            })
            .unwrap();
        assert!(validator.accept_response(&progress(3)).is_err());
    }

    #[test]
    fn a_verification_runs_to_a_verdict_and_a_capture_cannot_produce_one() {
        let mut validator = ready();
        validator
            .accept_request(&Request::StartVerify {
                request_id: "request-2".to_owned(),
                operation_id: "operation-7".to_owned(),
                device_id: "device-1".to_owned(),
                template_base64: encode_template_data(&[7; 241]).unwrap(),
            })
            .unwrap();
        validator
            .accept_response(&started("request-2", "operation-7"))
            .unwrap();
        validator
            .accept_response(&Response::FingerPresent {
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();
        validator
            .accept_response(&Response::MatchResult {
                operation_id: "operation-7".to_owned(),
                matched: true,
            })
            .unwrap();
        validator
            .accept_response(&Response::Completed {
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();

        // The operation that was opened decides the events that may follow: a capture cannot
        // answer with a verdict, and an enrollment cannot answer with a frame.
        let mut validator = ready();
        validator.accept_request(&start()).unwrap();
        validator
            .accept_response(&started("request-2", "operation-7"))
            .unwrap();
        assert!(
            validator
                .accept_response(&Response::MatchResult {
                    operation_id: "operation-7".to_owned(),
                    matched: true,
                })
                .is_err()
        );
    }

    #[test]
    fn template_payloads_are_bounded_and_never_empty() {
        assert!(encode_template_data(&[]).is_err());
        assert!(encode_template_data(&vec![0; MAX_DECODED_TEMPLATE_BYTES + 1]).is_err());
        assert!(decode_template_data("").is_err());
        assert!(decode_template_data("***").is_err());

        let boundary = vec![9_u8; MAX_DECODED_TEMPLATE_BYTES];
        let encoded = encode_template_data(&boundary).unwrap();
        assert_eq!(decode_template_data(&encoded).unwrap(), boundary);

        // A start-verify carrying an unusable template is refused at the codec, before any
        // device is asked to compare against it.
        assert!(
            encode_request(&Request::StartVerify {
                request_id: "r".to_owned(),
                operation_id: "o".to_owned(),
                device_id: "d".to_owned(),
                template_base64: String::new(),
            })
            .is_err()
        );
    }

    #[test]
    fn golden_json_lines_are_stable() {
        let requests = vec![
            (
                Request::Hello {
                    request_id: "r1".to_owned(),
                    protocol_version: 0,
                },
                "{\"type\":\"hello\",\"request_id\":\"r1\",\"protocol_version\":0}\n",
            ),
            (
                Request::Enumerate {
                    request_id: "r2".to_owned(),
                },
                "{\"type\":\"enumerate\",\"request_id\":\"r2\"}\n",
            ),
            (
                Request::StartCapture {
                    request_id: "r3".to_owned(),
                    operation_id: "op3".to_owned(),
                    device_id: "d1".to_owned(),
                },
                "{\"type\":\"start_capture\",\"request_id\":\"r3\",\"operation_id\":\"op3\",\"device_id\":\"d1\"}\n",
            ),
            (
                Request::Cancel {
                    request_id: "r4".to_owned(),
                    operation_id: "op3".to_owned(),
                },
                "{\"type\":\"cancel\",\"request_id\":\"r4\",\"operation_id\":\"op3\"}\n",
            ),
            (
                Request::Shutdown {
                    request_id: "r5".to_owned(),
                },
                "{\"type\":\"shutdown\",\"request_id\":\"r5\"}\n",
            ),
        ];
        for (message, golden) in requests {
            let line = encode_request(&message).unwrap();
            assert_eq!(line, golden.as_bytes());
            assert_eq!(decode_request(&line).unwrap(), message);
        }

        let responses = vec![
            (
                Response::HelloAck {
                    request_id: "r1".to_owned(),
                    protocol_version: 0,
                },
                "{\"type\":\"hello_ack\",\"request_id\":\"r1\",\"protocol_version\":0}\n",
            ),
            (
                Response::Devices {
                    request_id: "r2".to_owned(),
                    devices: vec![DeviceInfo {
                        device_id: "d1".to_owned(),
                        capture_profile_id: "profile1".to_owned(),
                        capability: DeviceCapability::HostImage,
                        enroll_stages: 0,
                    }],
                },
                "{\"type\":\"devices\",\"request_id\":\"r2\",\"devices\":[{\"device_id\":\"d1\",\"capture_profile_id\":\"profile1\",\"capability\":\"host_image\",\"enroll_stages\":0}]}\n",
            ),
            (
                Response::Started {
                    request_id: "r3".to_owned(),
                    operation_id: "op3".to_owned(),
                },
                "{\"type\":\"started\",\"request_id\":\"r3\",\"operation_id\":\"op3\"}\n",
            ),
            (
                Response::FingerPresent {
                    operation_id: "op3".to_owned(),
                },
                "{\"type\":\"finger_present\",\"operation_id\":\"op3\"}\n",
            ),
            (
                Response::Frame {
                    operation_id: "op3".to_owned(),
                    sequence: 2,
                    width: 2,
                    height: 1,
                    x_resolution_ppi: 500,
                    y_resolution_ppi: 500,
                    data_base64: encode_frame_data(&[0, 255]).unwrap(),
                },
                "{\"type\":\"frame\",\"operation_id\":\"op3\",\"sequence\":2,\"width\":2,\"height\":1,\"x_resolution_ppi\":500,\"y_resolution_ppi\":500,\"data_base64\":\"AP8=\"}\n",
            ),
            (
                Response::FingerRemoved {
                    operation_id: "op3".to_owned(),
                },
                "{\"type\":\"finger_removed\",\"operation_id\":\"op3\"}\n",
            ),
            (
                Response::Completed {
                    operation_id: "op3".to_owned(),
                },
                "{\"type\":\"completed\",\"operation_id\":\"op3\"}\n",
            ),
            (
                Response::Cancelled {
                    request_id: "r4".to_owned(),
                    operation_id: "op3".to_owned(),
                },
                "{\"type\":\"cancelled\",\"request_id\":\"r4\",\"operation_id\":\"op3\"}\n",
            ),
            (
                Response::Error {
                    request_id: Some("r4".to_owned()),
                    operation_id: Some("op3".to_owned()),
                    code: "capture_failed".to_owned(),
                    message: "capture failed".to_owned(),
                },
                "{\"type\":\"error\",\"request_id\":\"r4\",\"operation_id\":\"op3\",\"code\":\"capture_failed\",\"message\":\"capture failed\"}\n",
            ),
        ];
        for (message, golden) in responses {
            let line = encode_response(&message).unwrap();
            assert_eq!(line, golden.as_bytes());
            assert_eq!(decode_response(&line).unwrap(), message);
        }
    }

    #[test]
    fn codec_rejects_base64_sizes_geometry_and_versions() {
        let invalid_base64 = br#"{"type":"frame","operation_id":"op","sequence":0,"width":1,"height":1,"x_resolution_ppi":500,"y_resolution_ppi":500,"data_base64":"***"}"#;
        assert!(decode_response(invalid_base64).is_err());

        let too_large = vec![0_u8; MAX_DECODED_FRAME_BYTES + 1];
        assert!(encode_frame_data(&too_large).is_err());
        let oversized_response = Response::Frame {
            operation_id: "op".to_owned(),
            sequence: 0,
            width: 1,
            height: u32::try_from(MAX_DECODED_FRAME_BYTES + 1).unwrap(),
            x_resolution_ppi: 500,
            y_resolution_ppi: 500,
            data_base64: base64::engine::general_purpose::STANDARD.encode(&too_large),
        };
        let oversized_line = serde_json::to_vec(&oversized_response).unwrap();
        assert!(decode_response(&oversized_line).is_err());

        let boundary = vec![0_u8; MAX_DECODED_FRAME_BYTES];
        let boundary_response = Response::Frame {
            operation_id: "op".to_owned(),
            sequence: 0,
            width: 1024,
            height: 1024,
            x_resolution_ppi: 500,
            y_resolution_ppi: 500,
            data_base64: encode_frame_data(&boundary).unwrap(),
        };
        let boundary_line = encode_response(&boundary_response).unwrap();
        assert_eq!(decode_response(&boundary_line).unwrap(), boundary_response);

        let wrong_length = Response::Frame {
            operation_id: "op".to_owned(),
            sequence: 0,
            width: 2,
            height: 2,
            x_resolution_ppi: 500,
            y_resolution_ppi: 500,
            data_base64: encode_frame_data(&[0]).unwrap(),
        };
        assert!(encode_response(&wrong_length).is_err());

        let mut validator = SessionValidator::new();
        let wrong_version = Request::Hello {
            request_id: "hello".to_owned(),
            protocol_version: 1,
        };
        assert!(validator.accept_request(&wrong_version).is_err());

        validator.accept_request(&hello()).unwrap();
        let wrong_ack = Response::HelloAck {
            request_id: "hello-1".to_owned(),
            protocol_version: 1,
        };
        assert!(validator.accept_response(&wrong_ack).is_err());
        assert!(validator.accept_response(&hello_ack()).is_ok());
    }

    #[test]
    fn validator_rejects_pre_hello_double_operation_and_cancel_order() {
        let mut validator = SessionValidator::new();
        assert!(validator.accept_request(&start()).is_err());
        assert!(validator.accept_request(&hello()).is_ok());
        assert!(validator.accept_response(&hello_ack()).is_ok());
        assert!(
            validator
                .accept_request(&Request::Cancel {
                    request_id: "cancel-early".to_owned(),
                    operation_id: "operation-7".to_owned(),
                })
                .is_err()
        );

        assert!(validator.accept_request(&start()).is_ok());
        assert!(validator.accept_request(&start()).is_err());
        assert!(
            validator
                .accept_response(&Response::Started {
                    request_id: "request-2".to_owned(),
                    operation_id: "operation-7".to_owned(),
                })
                .is_ok()
        );
        assert!(
            validator
                .accept_request(&Request::StartCapture {
                    request_id: "request-3".to_owned(),
                    operation_id: "operation-8".to_owned(),
                    device_id: "device-1".to_owned(),
                })
                .is_err()
        );
        assert!(
            validator
                .accept_response(&Response::Frame {
                    operation_id: "operation-7".to_owned(),
                    sequence: 0,
                    width: 1,
                    height: 1,
                    x_resolution_ppi: 500,
                    y_resolution_ppi: 500,
                    data_base64: encode_frame_data(&[0]).unwrap(),
                })
                .is_err()
        );

        assert!(
            validator
                .accept_request(&Request::Cancel {
                    request_id: "cancel-1".to_owned(),
                    operation_id: "operation-7".to_owned(),
                })
                .is_ok()
        );
        assert!(
            validator
                .accept_response(&Response::Completed {
                    operation_id: "operation-7".to_owned(),
                })
                .is_err()
        );
        assert!(
            validator
                .accept_response(&Response::Cancelled {
                    request_id: "cancel-1".to_owned(),
                    operation_id: "operation-7".to_owned(),
                })
                .is_ok()
        );
    }

    #[test]
    fn validator_accepts_complete_capture_sequence() {
        let mut validator = SessionValidator::new();
        validator.accept_request(&hello()).unwrap();
        validator.accept_response(&hello_ack()).unwrap();
        validator.accept_request(&start()).unwrap();
        validator
            .accept_response(&Response::Started {
                request_id: "request-2".to_owned(),
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();
        validator
            .accept_response(&Response::FingerPresent {
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();
        validator
            .accept_response(&Response::Frame {
                operation_id: "operation-7".to_owned(),
                sequence: 0,
                width: 2,
                height: 1,
                x_resolution_ppi: 500,
                y_resolution_ppi: 500,
                data_base64: encode_frame_data(&[1, 2]).unwrap(),
            })
            .unwrap();
        validator
            .accept_response(&Response::FingerRemoved {
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();
        validator
            .accept_response(&Response::Completed {
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();
    }

    #[test]
    fn validator_rejects_non_monotonic_frames() {
        let mut validator = SessionValidator::new();
        validator.accept_request(&hello()).unwrap();
        validator.accept_response(&hello_ack()).unwrap();
        validator.accept_request(&start()).unwrap();
        validator
            .accept_response(&Response::Started {
                request_id: "request-2".to_owned(),
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();
        validator
            .accept_response(&Response::FingerPresent {
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();

        let frame = |sequence| Response::Frame {
            operation_id: "operation-7".to_owned(),
            sequence,
            width: 1,
            height: 1,
            x_resolution_ppi: 500,
            y_resolution_ppi: 500,
            data_base64: encode_frame_data(&[0]).unwrap(),
        };
        validator.accept_response(&frame(5)).unwrap();
        assert!(validator.accept_response(&frame(5)).is_err());
        assert!(validator.accept_response(&frame(4)).is_err());
        assert!(validator.accept_response(&frame(6)).is_ok());
    }

    #[test]
    fn validator_correlates_error_responses_without_bypassing_hello() {
        let mut validator = SessionValidator::new();
        validator.accept_request(&hello()).unwrap();
        let unrelated = Response::Error {
            request_id: Some("other-request".to_owned()),
            operation_id: None,
            code: "failure".to_owned(),
            message: "redacted diagnostic".to_owned(),
        };
        assert!(validator.accept_response(&unrelated).is_err());
        assert!(validator.accept_request(&start()).is_err());
        validator.accept_response(&hello_ack()).unwrap();

        validator.accept_request(&start()).unwrap();
        validator
            .accept_response(&Response::Started {
                request_id: "request-2".to_owned(),
                operation_id: "operation-7".to_owned(),
            })
            .unwrap();
        let wrong_operation = Response::Error {
            request_id: None,
            operation_id: Some("operation-8".to_owned()),
            code: "failure".to_owned(),
            message: "redacted diagnostic".to_owned(),
        };
        assert!(validator.accept_response(&wrong_operation).is_err());
        let correlated = Response::Error {
            request_id: None,
            operation_id: Some("operation-7".to_owned()),
            code: "failure".to_owned(),
            message: "redacted diagnostic".to_owned(),
        };
        assert!(validator.accept_response(&correlated).is_ok());

        let mut failed_hello = SessionValidator::new();
        failed_hello.accept_request(&hello()).unwrap();
        let hello_error = Response::Error {
            request_id: Some("hello-1".to_owned()),
            operation_id: None,
            code: "version_rejected".to_owned(),
            message: "redacted diagnostic".to_owned(),
        };
        failed_hello.accept_response(&hello_error).unwrap();
        assert!(failed_hello.accept_request(&start()).is_err());
    }

    #[test]
    fn validator_enforces_distinct_and_unique_identifiers() {
        let equal_identifiers = Request::StartCapture {
            request_id: "same".to_owned(),
            operation_id: "same".to_owned(),
            device_id: "device".to_owned(),
        };
        assert!(encode_request(&equal_identifiers).is_err());

        let mut validator = SessionValidator::new();
        validator.accept_request(&hello()).unwrap();
        validator.accept_response(&hello_ack()).unwrap();
        validator
            .accept_request(&Request::Enumerate {
                request_id: "enumerate-1".to_owned(),
            })
            .unwrap();
        validator
            .accept_response(&Response::Devices {
                request_id: "enumerate-1".to_owned(),
                devices: Vec::new(),
            })
            .unwrap();
        assert!(
            validator
                .accept_request(&Request::Shutdown {
                    request_id: "enumerate-1".to_owned(),
                })
                .is_err()
        );
        assert!(
            validator
                .accept_request(&Request::Shutdown {
                    request_id: "shutdown-1".to_owned(),
                })
                .is_ok()
        );
    }

    #[test]
    fn codec_rejects_oversized_or_multiple_lines() {
        assert!(decode_request(b"{}\n{}\n").is_err());
        assert!(
            decode_request(
                br#"{"type":"hello","request_id":"r1","protocol_version":0,"unexpected":true}"#
            )
            .is_err()
        );
        let oversized = vec![b' '; MAX_LINE_BYTES + 2];
        assert!(decode_request(&oversized).is_err());

        let exact_json = format!("\"{}\"", "a".repeat(MAX_LINE_BYTES - 2));
        let value: serde_json::Value = decode_line(exact_json.as_bytes()).unwrap();
        let encoded = encode_line(&value).unwrap();
        assert_eq!(encoded.len(), MAX_LINE_BYTES + 1);
        assert_eq!(encoded.last(), Some(&b'\n'));
    }

    #[test]
    fn protocol_debug_redacts_identifiers_and_frame_payload() {
        let response = Response::Frame {
            operation_id: "SENTINEL_OPERATION".to_owned(),
            sequence: 0,
            width: 1,
            height: 1,
            x_resolution_ppi: 500,
            y_resolution_ppi: 500,
            data_base64: encode_frame_data(b"S").unwrap(),
        };
        let rendered = format!("{response:?}");
        assert_eq!(rendered, "Response(\"frame\")");
        assert!(!rendered.contains("SENTINEL"));
    }
}
