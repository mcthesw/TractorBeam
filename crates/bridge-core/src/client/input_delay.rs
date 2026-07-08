use std::{
    io,
    net::{SocketAddr, TcpStream},
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use thiserror::Error;
use tractor_beam_hook_ipc::{ErrorCode, Request, Response};

use super::{
    LogLevel, SessionMode, SessionStatus,
    hook_config::HOOK_CONTROL,
    state::{self, HookStartupPhase},
};

const CONTROL_TIMEOUT: Duration = Duration::from_millis(600);

static NEXT_REQUEST_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputDelayReport {
    pub value: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDelayOperation {
    Read,
    Write,
}

impl std::fmt::Display for InputDelayOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read => formatter.write_str("read"),
            Self::Write => formatter.write_str("write"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputDelayStatus {
    pub operation: InputDelayOperation,
    pub result: Result<i32, String>,
    pub updated_at: u64,
}

#[derive(Debug, Error)]
pub enum InputDelayError {
    #[error("session is not running")]
    SessionNotRunning,
    #[error("input delay is only available in Fallback or Pure mode")]
    UnsupportedMode,
    #[error("Native Hook is not ready")]
    HookNotReady,
    #[error("Native Hook returned {0}")]
    Hook(ErrorCode),
    #[error("unexpected Native Hook response")]
    UnexpectedResponse,
    #[error("Native Hook response id mismatch: expected {expected}, got {actual}")]
    ResponseIdMismatch { expected: u32, actual: u32 },
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl super::BridgeClient {
    pub fn read_input_delay(&mut self) -> Result<InputDelayReport, InputDelayError> {
        self.ensure_input_delay_available()?;
        let id = next_request_id();
        let result = request_input_delay(Request::read_input_delay(id));
        self.record_input_delay_result(InputDelayOperation::Read, result)
    }

    pub fn write_input_delay(&mut self, value: i32) -> Result<InputDelayReport, InputDelayError> {
        self.ensure_input_delay_available()?;
        let id = next_request_id();
        let result = request_input_delay(Request::write_input_delay(id, value));
        self.record_input_delay_result(InputDelayOperation::Write, result)
    }

    fn ensure_input_delay_available(&self) -> Result<(), InputDelayError> {
        if self.state.status != SessionStatus::Running {
            return Err(InputDelayError::SessionNotRunning);
        }
        match self.state.active_session_mode {
            Some(SessionMode::Fallback | SessionMode::Pure) => {}
            Some(SessionMode::Official) | None => return Err(InputDelayError::UnsupportedMode),
        }
        if self.state.hook_startup.phase != HookStartupPhase::Ready {
            return Err(InputDelayError::HookNotReady);
        }
        Ok(())
    }

    fn record_input_delay_result(
        &mut self,
        operation: InputDelayOperation,
        result: Result<InputDelayReport, InputDelayError>,
    ) -> Result<InputDelayReport, InputDelayError> {
        let status_result = result
            .as_ref()
            .map(|report| report.value)
            .map_err(ToString::to_string);
        self.state.latest_input_delay_status = Some(InputDelayStatus {
            operation,
            result: status_result,
            updated_at: state::unix_seconds(),
        });
        match &result {
            Ok(report) => self.log(
                LogLevel::Info,
                format!("Input delay {operation} succeeded value={}", report.value),
            ),
            Err(error) => self.log(
                LogLevel::Warn,
                format!("Input delay {operation} failed error={error}"),
            ),
        }
        result
    }
}

fn request_input_delay(request: Request) -> Result<InputDelayReport, InputDelayError> {
    let endpoint = HOOK_CONTROL
        .parse::<SocketAddr>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let expected_id = request.id();
    let mut stream = TcpStream::connect_timeout(&endpoint, CONTROL_TIMEOUT)?;
    stream.set_read_timeout(Some(CONTROL_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_TIMEOUT))?;
    tractor_beam_hook_ipc::write_request(&mut stream, request)?;
    let response = tractor_beam_hook_ipc::read_response(&mut stream)?;
    let actual_id = response.id();
    if actual_id != expected_id {
        return Err(InputDelayError::ResponseIdMismatch {
            expected: expected_id,
            actual: actual_id,
        });
    }
    match response {
        Response::Ok { value, .. } => Ok(InputDelayReport { value }),
        Response::Error { code, .. } => Err(InputDelayError::Hook(code)),
    }
}

fn next_request_id() -> u32 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}
