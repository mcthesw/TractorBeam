use std::{fs, io, path::PathBuf};

use argh::FromArgs;
use ipnet::IpNet;
use serde::Deserialize;

const MAX_UDP_PACKET_SIZE: usize = 65_535;
const DEFAULT_BIND: &str = "0.0.0.0:25910";
const DEFAULT_MAX_PACKET_SIZE: usize = 2_048;
const DEFAULT_RATE_LIMIT_PER_SECOND: u32 = 5_000;
const DEFAULT_BYTE_RATE_LIMIT_PER_SECOND: usize = 8 * 1024 * 1024;
const DEFAULT_BYTE_RATE_LIMIT_BURST: usize = 16 * 1024 * 1024;
const DEFAULT_HEALTH_PONGS_PER_SECOND_PER_IP: u32 = 10;
const DEFAULT_MAX_ROOMS: usize = 256;
const DEFAULT_MAX_PEERS_PER_ROOM: usize = 4;
const DEFAULT_MAX_ROOM_NAME_LEN: usize = 64;
const DEFAULT_PEER_IDLE_SECONDS: u64 = 30;
const DEFAULT_ROOM_IDLE_SECONDS: u64 = 120;
const DEFAULT_TCP_EGRESS_QUEUE_CAPACITY: usize = 512;
const DEFAULT_POW_DIFFICULTY_BITS: u8 = 18;

/// Run the Tractor Beam Relay Server.
#[derive(Debug, FromArgs)]
pub(crate) struct Args {
    /// print version information
    #[argh(switch, short = 'V')]
    version: bool,
    /// load Relay settings from this TOML file
    #[argh(option)]
    config: Option<PathBuf>,
    /// override the UDP listener address
    #[argh(option)]
    bind: Option<String>,
    /// override the TCP listener address
    #[argh(option)]
    tcp_bind: Option<String>,
    /// disable the TCP listener
    #[argh(switch)]
    disable_tcp: bool,
}

impl Args {
    #[must_use]
    pub(crate) fn should_print_version(&self) -> bool {
        self.version
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RelayConfig {
    pub(crate) udp_bind: Option<String>,
    pub(crate) tcp_bind: Option<String>,
    pub(crate) tcp_egress_queue_capacity: usize,
    pub(crate) max_packet_size: usize,
    pub(crate) peer_idle_seconds: u64,
    pub(crate) room_idle_seconds: u64,
    pub(crate) rate_limit_per_second: u32,
    pub(crate) byte_rate_limit_per_second: usize,
    pub(crate) byte_rate_limit_burst: usize,
    pub(crate) health_pongs_per_second_per_ip: u32,
    pub(crate) max_rooms: usize,
    pub(crate) max_peers_per_room: usize,
    pub(crate) max_room_name_len: usize,
    pub(crate) blocked_cidrs: Vec<IpNet>,
    pub(crate) pow_difficulty_bits: u8,
    pub(crate) telemetry: Option<TelemetryConfig>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TelemetryConfig {
    pub(crate) otlp_endpoint: String,
    pub(crate) service_instance_id: String,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            udp_bind: Some(default_bind()),
            tcp_bind: Some(default_bind()),
            tcp_egress_queue_capacity: DEFAULT_TCP_EGRESS_QUEUE_CAPACITY,
            max_packet_size: DEFAULT_MAX_PACKET_SIZE,
            peer_idle_seconds: DEFAULT_PEER_IDLE_SECONDS,
            room_idle_seconds: DEFAULT_ROOM_IDLE_SECONDS,
            rate_limit_per_second: DEFAULT_RATE_LIMIT_PER_SECOND,
            byte_rate_limit_per_second: DEFAULT_BYTE_RATE_LIMIT_PER_SECOND,
            byte_rate_limit_burst: DEFAULT_BYTE_RATE_LIMIT_BURST,
            health_pongs_per_second_per_ip: DEFAULT_HEALTH_PONGS_PER_SECOND_PER_IP,
            max_rooms: DEFAULT_MAX_ROOMS,
            max_peers_per_room: DEFAULT_MAX_PEERS_PER_ROOM,
            max_room_name_len: DEFAULT_MAX_ROOM_NAME_LEN,
            blocked_cidrs: Vec::new(),
            pow_difficulty_bits: DEFAULT_POW_DIFFICULTY_BITS,
            telemetry: None,
        }
    }
}

impl RelayConfig {
    fn file_default() -> Self {
        Self {
            udp_bind: None,
            tcp_bind: None,
            ..Self::default()
        }
    }

    pub(crate) fn load(args: &Args) -> io::Result<Self> {
        let mut config = if let Some(path) = &args.config {
            let contents = fs::read_to_string(path)?;
            Self::from_toml(&contents)?
        } else {
            Self::default()
        };
        if let Some(bind) = &args.bind {
            config.udp_bind = Some(bind.clone());
        }
        if let Some(tcp_bind) = &args.tcp_bind {
            config.tcp_bind = Some(tcp_bind.clone());
        }
        if args.disable_tcp {
            config.tcp_bind = None;
        }
        config.validate()?;
        Ok(config)
    }

    fn from_toml(contents: &str) -> io::Result<Self> {
        let file = toml::from_str::<RelayConfigFile>(contents).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid config: {error}"),
            )
        })?;
        let config = file.into_config();
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> io::Result<()> {
        validate_listener("relay_server.udp_bind", &self.udp_bind)?;
        validate_listener("relay_server.tcp_bind", &self.tcp_bind)?;
        if self.tcp_bind.is_none() {
            return invalid_config("relay_server.tcp_bind is required for the control plane");
        }
        if self.tcp_egress_queue_capacity == 0 {
            return invalid_config("tcp_egress_queue_capacity must be greater than 0");
        }
        if self.max_packet_size == 0 {
            return invalid_config("max_packet_size must be greater than 0");
        }
        if self.max_packet_size > MAX_UDP_PACKET_SIZE {
            return invalid_config(format!(
                "max_packet_size must not exceed {MAX_UDP_PACKET_SIZE}"
            ));
        }
        if self.peer_idle_seconds == 0 {
            return invalid_config("peer_idle_seconds must be greater than 0");
        }
        if self.room_idle_seconds == 0 {
            return invalid_config("room_idle_seconds must be greater than 0");
        }
        if self.rate_limit_per_second == 0 {
            return invalid_config("rate_limit_per_second must be greater than 0");
        }
        if self.byte_rate_limit_per_second == 0 {
            return invalid_config("byte_rate_limit_per_second must be greater than 0");
        }
        if self.byte_rate_limit_burst == 0 {
            return invalid_config("byte_rate_limit_burst must be greater than 0");
        }
        if self.health_pongs_per_second_per_ip == 0 {
            return invalid_config("health_pongs_per_second_per_ip must be greater than 0");
        }
        if self.max_rooms == 0 {
            return invalid_config("max_rooms must be greater than 0");
        }
        if self.max_peers_per_room == 0 {
            return invalid_config("max_peers_per_room must be greater than 0");
        }
        if self.max_room_name_len == 0 {
            return invalid_config("max_room_name_len must be greater than 0");
        }
        if let Some(telemetry) = &self.telemetry {
            if telemetry.otlp_endpoint.trim().is_empty() {
                return invalid_config("telemetry.otlp_endpoint must not be empty");
            }
            if telemetry.service_instance_id.trim().is_empty() {
                return invalid_config("telemetry.service_instance_id must not be empty");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayConfigFile {
    relay_server: Option<RelayServerSection>,
    admission: Option<AdmissionSection>,
    room_limits: Option<RoomLimitsSection>,
    traffic_limits: Option<TrafficLimitsSection>,
    access_control: Option<AccessControlSection>,
    telemetry: Option<TelemetrySection>,
}

impl RelayConfigFile {
    fn into_config(self) -> RelayConfig {
        let mut config = RelayConfig::file_default();

        if let Some(relay_server) = self.relay_server {
            config.udp_bind = relay_server.udp_bind;
            config.tcp_bind = relay_server.tcp_bind;
        }
        if let Some(admission) = self.admission
            && let Some(pow_difficulty_bits) = admission.pow_difficulty_bits
        {
            config.pow_difficulty_bits = pow_difficulty_bits;
        }
        if let Some(room_limits) = self.room_limits
            && let Some(max_rooms) = room_limits.max_rooms
        {
            config.max_rooms = max_rooms;
        }
        if let Some(traffic_limits) = self.traffic_limits {
            if let Some(rate_limit_per_second) = traffic_limits.rate_limit_per_second {
                config.rate_limit_per_second = rate_limit_per_second;
            }
            if let Some(byte_rate_limit_per_second) = traffic_limits.byte_rate_limit_per_second {
                config.byte_rate_limit_per_second = byte_rate_limit_per_second;
            }
            if let Some(byte_rate_limit_burst) = traffic_limits.byte_rate_limit_burst {
                config.byte_rate_limit_burst = byte_rate_limit_burst;
            }
        }
        if let Some(access_control) = self.access_control
            && let Some(blocked_cidrs) = access_control.blocked_cidrs
        {
            config.blocked_cidrs = blocked_cidrs;
        }
        config.telemetry = self.telemetry.map(|telemetry| TelemetryConfig {
            otlp_endpoint: telemetry.otlp_endpoint,
            service_instance_id: telemetry.service_instance_id,
        });

        config
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayServerSection {
    udp_bind: Option<String>,
    tcp_bind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionSection {
    pow_difficulty_bits: Option<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoomLimitsSection {
    max_rooms: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrafficLimitsSection {
    rate_limit_per_second: Option<u32>,
    byte_rate_limit_per_second: Option<usize>,
    byte_rate_limit_burst: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessControlSection {
    blocked_cidrs: Option<Vec<IpNet>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TelemetrySection {
    otlp_endpoint: String,
    service_instance_id: String,
}

fn invalid_config<T>(message: impl Into<String>) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

fn validate_listener(name: &str, bind: &Option<String>) -> io::Result<()> {
    if bind.as_deref().is_some_and(|value| value.trim().is_empty()) {
        return invalid_config(format!("{name} must not be empty when set"));
    }
    Ok(())
}

fn default_bind() -> String {
    DEFAULT_BIND.to_owned()
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
