/// Binary framing for ADI service protocol.
///
/// Frame layout: `[header_len: u32 BE][JSON header][payload bytes]`
///
/// The router reads only the JSON header for routing (plugin, method, request ID).
/// The payload is opaque bytes — each plugin decides its own serialization format.
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestHeader {
    /// Protocol version (currently 1)
    pub v: u8,
    /// Request identifier for correlating responses
    pub id: Uuid,
    /// Target plugin (e.g. "adi.credentials")
    pub plugin: String,
    pub method: String,
    /// Whether the client expects a streaming response
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseHeader {
    /// Protocol version (currently 1)
    pub v: u8,
    pub id: Uuid,
    pub status: ResponseStatus,
    /// Sequence number for streaming (0 for single responses)
    #[serde(default)]
    pub seq: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Success,
    Error,
    PluginNotFound,
    MethodNotFound,
    StreamChunk,
    StreamEnd,
    InvalidRequest,
}

#[derive(Debug)]
pub enum FrameError {
    TooShort,
    HeaderTooLarge { declared: u32, available: usize },
    InvalidHeaderJson(serde_json::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "frame too short (need at least 4 bytes for header length)"),
            Self::HeaderTooLarge { declared, available } => {
                write!(f, "header length {} exceeds available data {}", declared, available)
            }
            Self::InvalidHeaderJson(e) => write!(f, "invalid header JSON: {}", e),
        }
    }
}

impl std::error::Error for FrameError {}

/// Parse a binary frame into a request header and opaque payload.
///
/// Layout: `[u32 BE header_len][header_len bytes of JSON][remaining payload bytes]`
pub fn parse_request(data: &[u8]) -> Result<(RequestHeader, Bytes), FrameError> {
    if data.len() < 4 {
        return Err(FrameError::TooShort);
    }

    let mut cursor = &data[..];
    let header_len = cursor.get_u32() as usize;

    if cursor.len() < header_len {
        return Err(FrameError::HeaderTooLarge {
            declared: header_len as u32,
            available: cursor.len(),
        });
    }

    let header_bytes = &cursor[..header_len];
    let payload = Bytes::copy_from_slice(&cursor[header_len..]);

    let header: RequestHeader =
        serde_json::from_slice(header_bytes).map_err(FrameError::InvalidHeaderJson)?;

    Ok((header, payload))
}

pub fn build_response(header: &ResponseHeader, payload: &[u8]) -> Bytes {
    let header_json = serde_json::to_vec(header).expect("ResponseHeader is always serializable");
    let mut buf = BytesMut::with_capacity(4 + header_json.len() + payload.len());
    buf.put_u32(header_json.len() as u32);
    buf.put_slice(&header_json);
    buf.put_slice(payload);
    buf.freeze()
}

pub fn success_response(request_id: Uuid, payload: &[u8]) -> Bytes {
    build_response(
        &ResponseHeader { v: 1, id: request_id, status: ResponseStatus::Success, seq: 0 },
        payload,
    )
}

pub fn error_response(request_id: Uuid, payload: &[u8]) -> Bytes {
    build_response(
        &ResponseHeader { v: 1, id: request_id, status: ResponseStatus::Error, seq: 0 },
        payload,
    )
}

/// Build a router-level error response (payload is a UTF-8 message).
pub fn router_error(request_id: Uuid, status: ResponseStatus, message: &str) -> Bytes {
    build_response(
        &ResponseHeader { v: 1, id: request_id, status, seq: 0 },
        message.as_bytes(),
    )
}

pub fn stream_chunk(request_id: Uuid, seq: u32, payload: &[u8]) -> Bytes {
    build_response(
        &ResponseHeader { v: 1, id: request_id, status: ResponseStatus::StreamChunk, seq },
        payload,
    )
}

pub fn stream_end(request_id: Uuid, seq: u32, payload: &[u8]) -> Bytes {
    build_response(
        &ResponseHeader { v: 1, id: request_id, status: ResponseStatus::StreamEnd, seq },
        payload,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_request(header: &RequestHeader, payload: &[u8]) -> Vec<u8> {
        let header_json = serde_json::to_vec(header).unwrap();
        let mut buf = Vec::with_capacity(4 + header_json.len() + payload.len());
        buf.extend_from_slice(&(header_json.len() as u32).to_be_bytes());
        buf.extend_from_slice(&header_json);
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn round_trip_request() {
        let header = RequestHeader {
            v: 1,
            id: Uuid::nil(),
            plugin: "adi.credentials".to_string(),
            method: "list".to_string(),
            stream: false,
        };
        let payload = b"hello world";
        let frame = build_request(&header, payload);

        let (parsed_header, parsed_payload) = parse_request(&frame).unwrap();
        assert_eq!(parsed_header.plugin, "adi.credentials");
        assert_eq!(parsed_header.method, "list");
        assert_eq!(parsed_header.id, Uuid::nil());
        assert_eq!(parsed_payload.as_ref(), b"hello world");
    }

    #[test]
    fn round_trip_response() {
        let request_id = Uuid::new_v4();
        let payload = b"response data";
        let frame = success_response(request_id, payload);

        // Parse as response (reuse same layout)
        assert!(frame.len() >= 4);
        let header_len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        let header: ResponseHeader =
            serde_json::from_slice(&frame[4..4 + header_len]).unwrap();
        let resp_payload = &frame[4 + header_len..];

        assert_eq!(header.id, request_id);
        assert_eq!(header.status, ResponseStatus::Success);
        assert_eq!(header.seq, 0);
        assert_eq!(resp_payload, b"response data");
    }

    #[test]
    fn empty_payload() {
        let header = RequestHeader {
            v: 1,
            id: Uuid::nil(),
            plugin: "p".to_string(),
            method: "m".to_string(),
            stream: false,
        };
        let frame = build_request(&header, b"");
        let (_, payload) = parse_request(&frame).unwrap();
        assert!(payload.is_empty());
    }

    #[test]
    fn too_short_frame() {
        assert!(matches!(parse_request(&[0, 1]), Err(FrameError::TooShort)));
    }

    #[test]
    fn header_exceeds_data() {
        // header_len = 999 but only 4 bytes of data after the length prefix
        let mut frame = Vec::new();
        frame.extend_from_slice(&999u32.to_be_bytes());
        frame.extend_from_slice(b"tiny");
        assert!(matches!(
            parse_request(&frame),
            Err(FrameError::HeaderTooLarge { .. })
        ));
    }
}
