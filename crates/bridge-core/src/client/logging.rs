use std::{fmt::Debug, path::PathBuf};

use super::{RelayEndpoint, SessionMode, TransportChoice, state::LogLevel};

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

    fn log_files(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn emit(&self, context: Option<&ClientSessionLogContext>, level: LogLevel, message: &str) {
        emit_client_log_event(context, level, message);
    }
}

#[derive(Debug, Default)]
pub struct TracingClientLogSink;

impl ClientLogSink for TracingClientLogSink {}

pub fn emit_client_log_event(
    context: Option<&ClientSessionLogContext>,
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
        LogLevel::Trace => {
            tracing::trace!(route, relay_name, relay, transport, mode, "{}", message)
        }
        LogLevel::Debug => {
            tracing::debug!(route, relay_name, relay, transport, mode, "{}", message)
        }
        LogLevel::Info => {
            tracing::info!(route, relay_name, relay, transport, mode, "{}", message)
        }
        LogLevel::Warn => {
            tracing::warn!(route, relay_name, relay, transport, mode, "{}", message)
        }
        LogLevel::Error => {
            tracing::error!(route, relay_name, relay, transport, mode, "{}", message)
        }
    }
}
