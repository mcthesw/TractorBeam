//! Dependency-light framed IPC for Bridge Client <-> Native Hook control calls.

use std::{
    error::Error,
    fmt::{self, Display},
    io::{self, Read, Write},
};

const MAGIC: &[u8; 4] = b"TBI1";
const VERSION: u8 = 1;
const REQUEST_HEADER_LEN: usize = 12;
const RESPONSE_HEADER_LEN: usize = 12;
const MAX_PAYLOAD_LEN: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request {
    ReadInputDelay { id: u32 },
    WriteInputDelay { id: u32, value: i32 },
}

impl Request {
    #[must_use]
    pub const fn id(self) -> u32 {
        match self {
            Self::ReadInputDelay { id } | Self::WriteInputDelay { id, .. } => id,
        }
    }

    #[must_use]
    pub const fn read_input_delay(id: u32) -> Self {
        Self::ReadInputDelay { id }
    }

    #[must_use]
    pub const fn write_input_delay(id: u32, value: i32) -> Self {
        Self::WriteInputDelay { id, value }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Response {
    Ok { id: u32, value: i32 },
    Error { id: u32, code: ErrorCode },
}

impl Response {
    #[must_use]
    pub const fn ok(id: u32, value: i32) -> Self {
        Self::Ok { id, value }
    }

    #[must_use]
    pub const fn error(id: u32, code: ErrorCode) -> Self {
        Self::Error { id, code }
    }

    #[must_use]
    pub const fn id(&self) -> u32 {
        match self {
            Self::Ok { id, .. } | Self::Error { id, .. } => *id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCode {
    InvalidRequest,
    TargetNotFound,
    ReadFailed,
    WriteFailed,
    InternalError,
}

impl ErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::TargetNotFound => "target_not_found",
            Self::ReadFailed => "read_failed",
            Self::WriteFailed => "write_failed",
            Self::InternalError => "internal_error",
        }
    }

    fn from_byte(value: u8) -> Result<Self, FrameError> {
        match value {
            1 => Ok(Self::InvalidRequest),
            2 => Ok(Self::TargetNotFound),
            3 => Ok(Self::ReadFailed),
            4 => Ok(Self::WriteFailed),
            5 => Ok(Self::InternalError),
            other => Err(FrameError::UnknownStatus(other)),
        }
    }

    const fn to_byte(self) -> u8 {
        match self {
            Self::InvalidRequest => 1,
            Self::TargetNotFound => 2,
            Self::ReadFailed => 3,
            Self::WriteFailed => 4,
            Self::InternalError => 5,
        }
    }
}

impl Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    BadMagic,
    UnsupportedVersion(u8),
    UnknownOperation(u8),
    UnknownStatus(u8),
    BadPayloadLength { expected: usize, actual: usize },
    PayloadTooLarge(usize),
}

impl Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => formatter.write_str("bad IPC frame magic"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported IPC frame version: {version}")
            }
            Self::UnknownOperation(operation) => {
                write!(formatter, "unknown IPC operation: {operation}")
            }
            Self::UnknownStatus(status) => write!(formatter, "unknown IPC status: {status}"),
            Self::BadPayloadLength { expected, actual } => write!(
                formatter,
                "bad IPC payload length: expected {expected}, got {actual}"
            ),
            Self::PayloadTooLarge(length) => write!(formatter, "IPC payload too large: {length}"),
        }
    }
}

impl Error for FrameError {}

pub fn write_request(writer: &mut impl Write, request: Request) -> io::Result<()> {
    writer.write_all(&encode_request(request))
}

pub fn read_request(reader: &mut impl Read) -> io::Result<Request> {
    let mut header = [0_u8; REQUEST_HEADER_LEN];
    reader.read_exact(&mut header)?;
    let payload = read_payload(reader, payload_len(&header).map_err(io::Error::other)?)?;
    decode_request_parts(&header, &payload).map_err(io::Error::other)
}

pub fn write_response(writer: &mut impl Write, response: &Response) -> io::Result<()> {
    writer.write_all(&encode_response(response))
}

pub fn read_response(reader: &mut impl Read) -> io::Result<Response> {
    let mut header = [0_u8; RESPONSE_HEADER_LEN];
    reader.read_exact(&mut header)?;
    let payload = read_payload(reader, payload_len(&header).map_err(io::Error::other)?)?;
    decode_response_parts(&header, &payload).map_err(io::Error::other)
}

#[must_use]
pub fn encode_request(request: Request) -> Vec<u8> {
    let mut frame = Vec::with_capacity(REQUEST_HEADER_LEN + 4);
    frame.extend_from_slice(MAGIC);
    frame.push(VERSION);
    match request {
        Request::ReadInputDelay { id } => {
            frame.push(1);
            frame.extend_from_slice(&id.to_le_bytes());
            frame.extend_from_slice(&0_u16.to_le_bytes());
        }
        Request::WriteInputDelay { id, value } => {
            frame.push(2);
            frame.extend_from_slice(&id.to_le_bytes());
            frame.extend_from_slice(&4_u16.to_le_bytes());
            frame.extend_from_slice(&value.to_le_bytes());
        }
    }
    frame
}

#[must_use]
pub fn encode_response(response: &Response) -> Vec<u8> {
    let mut frame = Vec::with_capacity(RESPONSE_HEADER_LEN + 4);
    frame.extend_from_slice(MAGIC);
    frame.push(VERSION);
    match response {
        Response::Ok { id, value } => {
            frame.push(0);
            frame.extend_from_slice(&id.to_le_bytes());
            frame.extend_from_slice(&4_u16.to_le_bytes());
            frame.extend_from_slice(&value.to_le_bytes());
        }
        Response::Error { id, code } => {
            frame.push(code.to_byte());
            frame.extend_from_slice(&id.to_le_bytes());
            frame.extend_from_slice(&0_u16.to_le_bytes());
        }
    }
    frame
}

pub fn decode_request(bytes: &[u8]) -> Result<Request, FrameError> {
    if bytes.len() < REQUEST_HEADER_LEN {
        return Err(FrameError::BadPayloadLength {
            expected: REQUEST_HEADER_LEN,
            actual: bytes.len(),
        });
    }
    let payload_len = payload_len(&bytes[..REQUEST_HEADER_LEN])?;
    if bytes.len() != REQUEST_HEADER_LEN + payload_len {
        return Err(FrameError::BadPayloadLength {
            expected: REQUEST_HEADER_LEN + payload_len,
            actual: bytes.len(),
        });
    }
    decode_request_parts(&bytes[..REQUEST_HEADER_LEN], &bytes[REQUEST_HEADER_LEN..])
}

pub fn decode_response(bytes: &[u8]) -> Result<Response, FrameError> {
    if bytes.len() < RESPONSE_HEADER_LEN {
        return Err(FrameError::BadPayloadLength {
            expected: RESPONSE_HEADER_LEN,
            actual: bytes.len(),
        });
    }
    let payload_len = payload_len(&bytes[..RESPONSE_HEADER_LEN])?;
    if bytes.len() != RESPONSE_HEADER_LEN + payload_len {
        return Err(FrameError::BadPayloadLength {
            expected: RESPONSE_HEADER_LEN + payload_len,
            actual: bytes.len(),
        });
    }
    decode_response_parts(&bytes[..RESPONSE_HEADER_LEN], &bytes[RESPONSE_HEADER_LEN..])
}

fn decode_request_parts(header: &[u8], payload: &[u8]) -> Result<Request, FrameError> {
    validate_header(header)?;
    let operation = header[5];
    let id = frame_id(header);
    match operation {
        1 => {
            expect_payload_len(payload, 0)?;
            Ok(Request::ReadInputDelay { id })
        }
        2 => {
            expect_payload_len(payload, 4)?;
            Ok(Request::WriteInputDelay {
                id,
                value: i32::from_le_bytes(payload.try_into().expect("slice length checked")),
            })
        }
        other => Err(FrameError::UnknownOperation(other)),
    }
}

fn decode_response_parts(header: &[u8], payload: &[u8]) -> Result<Response, FrameError> {
    validate_header(header)?;
    let status = header[5];
    let id = frame_id(header);
    if status == 0 {
        expect_payload_len(payload, 4)?;
        return Ok(Response::Ok {
            id,
            value: i32::from_le_bytes(payload.try_into().expect("slice length checked")),
        });
    }
    expect_payload_len(payload, 0)?;
    Ok(Response::Error {
        id,
        code: ErrorCode::from_byte(status)?,
    })
}

fn validate_header(header: &[u8]) -> Result<(), FrameError> {
    if &header[0..4] != MAGIC {
        return Err(FrameError::BadMagic);
    }
    let version = header[4];
    if version != VERSION {
        return Err(FrameError::UnsupportedVersion(version));
    }
    Ok(())
}

fn payload_len(header: &[u8]) -> Result<usize, FrameError> {
    let length = u16::from_le_bytes([header[10], header[11]]) as usize;
    if length > MAX_PAYLOAD_LEN {
        return Err(FrameError::PayloadTooLarge(length));
    }
    Ok(length)
}

fn read_payload(reader: &mut impl Read, length: usize) -> io::Result<Vec<u8>> {
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

fn frame_id(header: &[u8]) -> u32 {
    u32::from_le_bytes(header[6..10].try_into().expect("slice length checked"))
}

fn expect_payload_len(payload: &[u8], expected: usize) -> Result<(), FrameError> {
    if payload.len() == expected {
        Ok(())
    } else {
        Err(FrameError::BadPayloadLength {
            expected,
            actual: payload.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrips_read() {
        let request = Request::read_input_delay(7);

        let decoded = decode_request(&encode_request(request)).unwrap();

        assert_eq!(decoded, request);
    }

    #[test]
    fn request_roundtrips_write() {
        let request = Request::write_input_delay(8, -1);

        let decoded = decode_request(&encode_request(request)).unwrap();

        assert_eq!(decoded, request);
    }

    #[test]
    fn response_roundtrips_ok() {
        let response = Response::ok(9, 4);

        let decoded = decode_response(&encode_response(&response)).unwrap();

        assert_eq!(decoded, response);
    }

    #[test]
    fn response_roundtrips_error() {
        let response = Response::error(10, ErrorCode::TargetNotFound);

        let decoded = decode_response(&encode_response(&response)).unwrap();

        assert_eq!(decoded, response);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = encode_request(Request::read_input_delay(1));
        bytes[0] = b'X';

        assert_eq!(decode_request(&bytes), Err(FrameError::BadMagic));
    }

    #[test]
    fn rejects_bad_payload_length() {
        let bytes = encode_request(Request::read_input_delay(1));

        assert!(matches!(
            decode_request(&bytes[..bytes.len() - 1]),
            Err(FrameError::BadPayloadLength { .. })
        ));
    }
}
