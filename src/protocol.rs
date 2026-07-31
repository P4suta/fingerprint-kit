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
            Self::Cancel { .. } => "cancel",
            Self::Shutdown { .. } => "shutdown",
        }
    }

    fn request_id(&self) -> &str {
        match self {
            Self::Hello { request_id, .. }
            | Self::Enumerate { request_id }
            | Self::StartCapture { request_id, .. }
            | Self::Cancel { request_id, .. }
            | Self::Shutdown { request_id } => request_id,
        }
    }

    fn started_operation_id(&self) -> Option<&str> {
        match self {
            Self::StartCapture { operation_id, .. } => Some(operation_id),
            _ => None,
        }
    }
}

/// One enumerated experimental device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceInfo {
    /// Driver-defined device identifier.
    pub device_id: String,
    /// Capture profile emitted by the device.
    pub capture_profile_id: String,
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
            Self::Completed { .. } => "completed",
            Self::Cancelled { .. } => "cancelled",
            Self::Error { .. } => "error",
        }
    }
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
    },
    Capturing {
        operation_id: String,
        finger_present: bool,
        saw_frame: bool,
        last_sequence: Option<u64>,
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
                },
            ) => SessionState::AwaitStarted {
                request_id: request_id.clone(),
                operation_id: operation_id.clone(),
            },
            (SessionState::Ready, Request::Shutdown { .. }) => SessionState::Closed,
            (SessionState::Ready, Request::Cancel { .. }) => {
                return Err(Error::invalid("cancel requires an active operation"));
            }
            (SessionState::Ready, Request::Hello { .. }) => {
                return Err(Error::invalid("hello may only be sent once"));
            }
            (
                SessionState::Capturing { operation_id, .. },
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
                SessionState::Capturing { .. } | SessionState::AwaitStarted { .. },
                Request::StartCapture { .. },
            ) => {
                return Err(Error::invalid("a capture operation is already active"));
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
                },
                Response::Started {
                    request_id: started_request,
                    operation_id: started_operation,
                },
            ) => {
                if request_id != *started_request || operation_id != *started_operation {
                    return Err(Error::invalid("started event identifiers do not match"));
                }
                SessionState::Capturing {
                    operation_id,
                    finger_present: false,
                    saw_frame: false,
                    last_sequence: None,
                }
            }
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
            } if response_request_id.as_deref() == Some(request_id.as_str())
                && response_operation_id
                    .as_deref()
                    .is_none_or(|value| value == operation_id) =>
            {
                SessionState::Ready
            }
            SessionState::Capturing { operation_id, .. }
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
                    }],
                },
                "{\"type\":\"devices\",\"request_id\":\"r2\",\"devices\":[{\"device_id\":\"d1\",\"capture_profile_id\":\"profile1\"}]}\n",
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
