use std::{fmt::Debug, io, path::PathBuf};

use super::{
    RelayEndpoint, SessionMode, TransportChoice,
    state::{LogLevel, unix_seconds},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientSessionLogContext {
    pub relay_name: Option<String>,
    pub relay: RelayEndpoint,
    pub transport: TransportChoice,
    pub room: String,
    pub mode: SessionMode,
    #[cfg(feature = "internal-test")]
    pub test_run_id: Option<String>,
}

pub trait ClientLogSink: Debug + Send + Sync {
    fn root(&self) -> Option<PathBuf> {
        None
    }

    fn warnings(&self) -> Vec<String> {
        Vec::new()
    }

    fn process_log_path(&self) -> Option<PathBuf> {
        None
    }

    fn recent_session_logs(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn start_session(
        &self,
        context: ClientSessionLogContext,
    ) -> io::Result<Box<dyn ClientSessionLog>> {
        Ok(Box::new(DefaultClientSessionLog::new(context)))
    }

    fn emit(&self, context: Option<&ClientSessionLogContext>, level: LogLevel, message: &str) {
        emit_client_log_event(context, None, level, message);
    }
}

pub trait ClientSessionLog: Debug + Send + Sync {
    fn session_id(&self) -> &str;

    fn context(&self) -> &ClientSessionLogContext;

    fn emit(&self, level: LogLevel, message: &str);
}

#[derive(Debug, Default)]
pub struct TracingClientLogSink;

impl ClientLogSink for TracingClientLogSink {}

#[derive(Debug)]
struct DefaultClientSessionLog {
    session_id: String,
    context: ClientSessionLogContext,
}

impl DefaultClientSessionLog {
    fn new(context: ClientSessionLogContext) -> Self {
        Self {
            session_id: format!("{}-{}", unix_seconds(), std::process::id()),
            context,
        }
    }
}

impl ClientSessionLog for DefaultClientSessionLog {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn context(&self) -> &ClientSessionLogContext {
        &self.context
    }

    fn emit(&self, _level: LogLevel, _message: &str) {}
}

pub fn emit_client_log_event(
    context: Option<&ClientSessionLogContext>,
    session_id: Option<&str>,
    level: LogLevel,
    message: &str,
) {
    let relay_name = context.and_then(|context| context.relay_name.as_deref());
    let relay = context.map(|context| context.relay.to_string());
    let transport = context.map(|context| context.transport.to_string());
    let room = context.map(|context| context.room.as_str());
    let mode = context.map(|context| context.mode.to_string());
    #[cfg(feature = "internal-test")]
    {
        let test_run_id = context.and_then(|context| context.test_run_id.as_deref());
        match level {
            LogLevel::Trace => tracing::trace!(
                session_id,
                relay_name,
                relay,
                transport,
                room,
                mode,
                test_run_id,
                "{}",
                message
            ),
            LogLevel::Debug => tracing::debug!(
                session_id,
                relay_name,
                relay,
                transport,
                room,
                mode,
                test_run_id,
                "{}",
                message
            ),
            LogLevel::Info => tracing::info!(
                session_id,
                relay_name,
                relay,
                transport,
                room,
                mode,
                test_run_id,
                "{}",
                message
            ),
            LogLevel::Warn => tracing::warn!(
                session_id,
                relay_name,
                relay,
                transport,
                room,
                mode,
                test_run_id,
                "{}",
                message
            ),
            LogLevel::Error => tracing::error!(
                session_id,
                relay_name,
                relay,
                transport,
                room,
                mode,
                test_run_id,
                "{}",
                message
            ),
        }
    }
    #[cfg(not(feature = "internal-test"))]
    {
        match level {
            LogLevel::Trace => tracing::trace!(
                session_id, relay_name, relay, transport, room, mode, "{}", message
            ),
            LogLevel::Debug => tracing::debug!(
                session_id, relay_name, relay, transport, room, mode, "{}", message
            ),
            LogLevel::Info => tracing::info!(
                session_id, relay_name, relay, transport, room, mode, "{}", message
            ),
            LogLevel::Warn => tracing::warn!(
                session_id, relay_name, relay, transport, room, mode, "{}", message
            ),
            LogLevel::Error => tracing::error!(
                session_id, relay_name, relay, transport, room, mode, "{}", message
            ),
        }
    }
}
