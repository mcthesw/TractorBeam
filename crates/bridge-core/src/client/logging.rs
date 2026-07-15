use std::{fmt::Debug, io, path::PathBuf};

use super::{
    RelayEndpoint, SessionMode, TransportChoice,
    state::{LogLevel, unix_seconds},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientSessionLogContext {
    pub route: ClientSessionLogRoute,
    pub mode: SessionMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientSessionLogRoute {
    ExternalRelay {
        relay_name: Option<String>,
        relay: RelayEndpoint,
        transport: TransportChoice,
    },
    LanDirect,
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
    let route = context.map(|context| match context.route {
        ClientSessionLogRoute::ExternalRelay { .. } => "external_relay",
        ClientSessionLogRoute::LanDirect => "lan_direct",
    });
    let relay_name = context.and_then(|context| match &context.route {
        ClientSessionLogRoute::ExternalRelay { relay_name, .. } => relay_name.as_deref(),
        ClientSessionLogRoute::LanDirect => None,
    });
    let relay = context.and_then(|context| match &context.route {
        ClientSessionLogRoute::ExternalRelay { relay, .. } => Some(relay.to_string()),
        ClientSessionLogRoute::LanDirect => None,
    });
    let transport = context.and_then(|context| match context.route {
        ClientSessionLogRoute::ExternalRelay { transport, .. } => Some(transport.to_string()),
        ClientSessionLogRoute::LanDirect => None,
    });
    let mode = context.map(|context| context.mode.to_string());
    match level {
        LogLevel::Trace => tracing::trace!(
            session_id, route, relay_name, relay, transport, mode, "{}", message
        ),
        LogLevel::Debug => tracing::debug!(
            session_id, route, relay_name, relay, transport, mode, "{}", message
        ),
        LogLevel::Info => tracing::info!(
            session_id, route, relay_name, relay, transport, mode, "{}", message
        ),
        LogLevel::Warn => tracing::warn!(
            session_id, route, relay_name, relay, transport, mode, "{}", message
        ),
        LogLevel::Error => tracing::error!(
            session_id, route, relay_name, relay, transport, mode, "{}", message
        ),
    }
}
