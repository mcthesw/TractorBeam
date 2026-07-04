use std::{fs, io, path::PathBuf};

use clap::{ArgAction, Parser};
use ipnet::IpNet;
use serde::Deserialize;

const MAX_UDP_PACKET_SIZE: usize = 65_535;
const DEFAULT_BIND: &str = "0.0.0.0:25910";
const DEFAULT_MAX_PACKET_SIZE: usize = 2_048;
const DEFAULT_RATE_LIMIT_PER_SECOND: u32 = 100;
const DEFAULT_MAX_ROOMS: usize = 256;
const DEFAULT_MAX_PEERS_PER_ROOM: usize = 4;
const DEFAULT_MAX_ROOM_NAME_LEN: usize = 64;
const DEFAULT_PEER_IDLE_SECONDS: u64 = 30;
const DEFAULT_ROOM_IDLE_SECONDS: u64 = 120;
const DEFAULT_TCP_EGRESS_QUEUE_CAPACITY: usize = 512;
const DEFAULT_POW_DIFFICULTY_BITS: u8 = 18;

#[derive(Debug, Parser)]
#[command(author, about)]
pub(crate) struct Args {
    #[arg(short = 'V', long, action = ArgAction::SetTrue, help = "Print version information")]
    version: bool,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    bind: Option<String>,
    #[arg(long)]
    tcp_bind: Option<String>,
    #[arg(long)]
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
    pub(crate) max_rooms: usize,
    pub(crate) max_peers_per_room: usize,
    pub(crate) max_room_name_len: usize,
    pub(crate) blocked_cidrs: Vec<IpNet>,
    pub(crate) pow_difficulty_bits: u8,
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
            max_rooms: DEFAULT_MAX_ROOMS,
            max_peers_per_room: DEFAULT_MAX_PEERS_PER_ROOM,
            max_room_name_len: DEFAULT_MAX_ROOM_NAME_LEN,
            blocked_cidrs: Vec::new(),
            pow_difficulty_bits: DEFAULT_POW_DIFFICULTY_BITS,
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
        if self.udp_bind.is_none() && self.tcp_bind.is_none() {
            return invalid_config(
                "at least one of relay_server.udp_bind or relay_server.tcp_bind must be set",
            );
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
        if self.max_rooms == 0 {
            return invalid_config("max_rooms must be greater than 0");
        }
        if self.max_peers_per_room == 0 {
            return invalid_config("max_peers_per_room must be greater than 0");
        }
        if self.max_room_name_len == 0 {
            return invalid_config("max_room_name_len must be greater than 0");
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
    access_control: Option<AccessControlSection>,
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
        if let Some(access_control) = self.access_control
            && let Some(blocked_cidrs) = access_control.blocked_cidrs
        {
            config.blocked_cidrs = blocked_cidrs;
        }

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
struct AccessControlSection {
    blocked_cidrs: Option<Vec<IpNet>>,
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
mod tests {
    use super::{DEFAULT_MAX_PACKET_SIZE, DEFAULT_RATE_LIMIT_PER_SECOND, RelayConfig};

    #[test]
    fn parses_minimal_sectioned_config() {
        let config = RelayConfig::from_toml(
            r#"
[relay_server]
udp_bind = "0.0.0.0:25910"
tcp_bind = "0.0.0.0:25910"

[admission]
pow_difficulty_bits = 18

[room_limits]
max_rooms = 256

[access_control]
blocked_cidrs = ["203.0.113.10/32"]
"#,
        )
        .unwrap();

        assert_eq!(config.udp_bind.as_deref(), Some("0.0.0.0:25910"));
        assert_eq!(config.tcp_bind.as_deref(), Some("0.0.0.0:25910"));
        assert_eq!(config.pow_difficulty_bits, 18);
        assert_eq!(config.max_rooms, 256);
        assert_eq!(config.max_packet_size, DEFAULT_MAX_PACKET_SIZE);
        assert_eq!(config.rate_limit_per_second, DEFAULT_RATE_LIMIT_PER_SECOND);
        assert_eq!(config.blocked_cidrs.len(), 1);
    }

    #[test]
    fn listener_bind_presence_controls_transports() {
        let tcp_only = RelayConfig::from_toml(
            r#"
[relay_server]
tcp_bind = "127.0.0.1:25910"
"#,
        )
        .unwrap();
        assert_eq!(tcp_only.udp_bind, None);
        assert_eq!(tcp_only.tcp_bind.as_deref(), Some("127.0.0.1:25910"));

        let udp_only = RelayConfig::from_toml(
            r#"
[relay_server]
udp_bind = "127.0.0.1:25910"
"#,
        )
        .unwrap();
        assert_eq!(udp_only.udp_bind.as_deref(), Some("127.0.0.1:25910"));
        assert_eq!(udp_only.tcp_bind, None);
    }

    #[test]
    fn rejects_legacy_flat_config() {
        let error = RelayConfig::from_toml(
            r#"
bind = "127.0.0.1:25910"
tcp_enabled = false
max_packet_size = 1500
rate_limit_per_second = 240
max_rooms = 1024
blocked_cidrs = ["198.51.100.0/24"]
"#,
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn rejects_config_without_listeners() {
        let error = RelayConfig::from_toml(
            r#"
[relay_server]
"#,
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("at least one"));
    }
}
