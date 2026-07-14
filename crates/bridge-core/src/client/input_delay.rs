use std::{
    io,
    sync::atomic::{AtomicU32, Ordering},
};

use serde::Serialize;
use thiserror::Error;
use tractor_beam_hook_ipc::{ErrorCode, InputDelayCommand};

use super::{
    LogLevel, SessionMode, SessionStatus,
    state::{self, HookIpcConnectionState},
};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputDelayEvidenceBlocker {
    SessionNotRunning,
    UnsupportedMode,
    HookNotReady,
    CurrentDelayUnknown,
    SmoothnessUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InputDelayEvidence {
    pub recommendation_possible: bool,
    pub blocker: Option<InputDelayEvidenceBlocker>,
    pub current_delay: Option<i32>,
    pub current_delay_observed_at: Option<u64>,
    pub smoothness: super::SmoothnessSnapshot,
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
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl super::BridgeClient {
    #[must_use]
    pub fn input_delay_evidence(&self) -> InputDelayEvidence {
        let current_delay = self
            .state
            .latest_input_delay_status
            .as_ref()
            .and_then(|status| status.result.as_ref().ok().copied());
        let current_delay_observed_at = current_delay.and(
            self.state
                .latest_input_delay_status
                .as_ref()
                .map(|status| status.updated_at),
        );
        let blocker = if self.state.status != SessionStatus::Running {
            Some(InputDelayEvidenceBlocker::SessionNotRunning)
        } else if !matches!(
            self.state.active_session_mode,
            Some(SessionMode::Fallback | SessionMode::Pure)
        ) {
            Some(InputDelayEvidenceBlocker::UnsupportedMode)
        } else if self.state.hook_ipc.connection != HookIpcConnectionState::Connected {
            Some(InputDelayEvidenceBlocker::HookNotReady)
        } else if current_delay.is_none() {
            Some(InputDelayEvidenceBlocker::CurrentDelayUnknown)
        } else if self.state.smoothness.level == super::SessionQuality::Unavailable {
            Some(InputDelayEvidenceBlocker::SmoothnessUnavailable)
        } else {
            None
        };
        InputDelayEvidence {
            recommendation_possible: blocker.is_none(),
            blocker,
            current_delay,
            current_delay_observed_at,
            smoothness: self.state.smoothness.clone(),
        }
    }

    pub fn read_input_delay(&mut self) -> Result<InputDelayReport, InputDelayError> {
        self.ensure_input_delay_available()?;
        let id = next_request_id();
        let result = self.request_input_delay(id, InputDelayCommand::Read);
        self.record_input_delay_result(InputDelayOperation::Read, result)
    }

    pub fn write_input_delay(&mut self, value: i32) -> Result<InputDelayReport, InputDelayError> {
        self.ensure_input_delay_available()?;
        let id = next_request_id();
        let result = self.request_input_delay(id, InputDelayCommand::Write(value));
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
        if self.state.hook_ipc.connection != HookIpcConnectionState::Connected {
            return Err(InputDelayError::HookNotReady);
        }
        Ok(())
    }

    fn request_input_delay(
        &self,
        id: u32,
        command: InputDelayCommand,
    ) -> Result<InputDelayReport, InputDelayError> {
        let session = self
            .session
            .as_ref()
            .ok_or(InputDelayError::SessionNotRunning)?;
        match session.request_input_delay(id, command)? {
            Ok(value) => Ok(InputDelayReport { value }),
            Err(code) => Err(InputDelayError::Hook(code)),
        }
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

fn next_request_id() -> u32 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{BridgeClient, LoadedClientConfig, QualityConfidence, SessionQuality};

    #[test]
    fn evidence_is_read_only_and_requires_current_delay_and_quality() {
        let mut client = BridgeClient::with_config(LoadedClientConfig::default());
        assert_eq!(
            client.input_delay_evidence().blocker,
            Some(InputDelayEvidenceBlocker::SessionNotRunning)
        );

        client.state.status = SessionStatus::Running;
        client.state.active_session_mode = Some(SessionMode::Pure);
        client.state.hook_ipc.connection = HookIpcConnectionState::Connected;
        assert_eq!(
            client.input_delay_evidence().blocker,
            Some(InputDelayEvidenceBlocker::CurrentDelayUnknown)
        );

        client.state.latest_input_delay_status = Some(InputDelayStatus {
            operation: InputDelayOperation::Read,
            result: Ok(3),
            updated_at: 42,
        });
        client.state.smoothness.level = SessionQuality::Good;
        client.state.smoothness.confidence = QualityConfidence::High;
        let evidence = client.input_delay_evidence();
        assert!(evidence.recommendation_possible);
        assert_eq!(evidence.current_delay, Some(3));
        assert_eq!(evidence.current_delay_observed_at, Some(42));
    }
}
