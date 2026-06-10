use std::{fs, io, path::PathBuf};

use clap::Parser;
use ipnet::IpNet;
use serde::Deserialize;

const MAX_UDP_PACKET_SIZE: usize = 65_535;

#[derive(Debug, Parser)]
#[command(author, version, about)]
pub(crate) struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    bind: Option<String>,
    #[arg(long)]
    tcp_bind: Option<String>,
    #[arg(long)]
    disable_tcp: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RelayConfig {
    pub(crate) bind: String,
    #[serde(default = "default_tcp_enabled")]
    pub(crate) tcp_enabled: bool,
    #[serde(default = "default_tcp_bind")]
    pub(crate) tcp_bind: String,
    #[serde(default = "default_tcp_egress_queue_capacity")]
    pub(crate) tcp_egress_queue_capacity: usize,
    pub(crate) max_packet_size: usize,
    pub(crate) peer_idle_seconds: u64,
    pub(crate) room_idle_seconds: u64,
    pub(crate) rate_limit_per_second: u32,
    pub(crate) max_rooms: usize,
    pub(crate) max_peers_per_room: usize,
    pub(crate) max_room_name_len: usize,
    pub(crate) blocked_cidrs: Vec<IpNet>,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:25910".to_owned(),
            tcp_enabled: true,
            tcp_bind: default_tcp_bind(),
            tcp_egress_queue_capacity: default_tcp_egress_queue_capacity(),
            max_packet_size: MAX_UDP_PACKET_SIZE,
            peer_idle_seconds: 30,
            room_idle_seconds: 120,
            rate_limit_per_second: 2_000,
            max_rooms: 1024,
            max_peers_per_room: 4,
            max_room_name_len: 64,
            blocked_cidrs: Vec::new(),
        }
    }
}

impl RelayConfig {
    pub(crate) fn load(args: &Args) -> io::Result<Self> {
        let mut config = if let Some(path) = &args.config {
            let contents = fs::read_to_string(path)?;
            toml::from_str(&contents).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid config: {error}"),
                )
            })?
        } else {
            Self::default()
        };
        if let Some(bind) = &args.bind {
            config.bind.clone_from(bind);
        }
        if let Some(tcp_bind) = &args.tcp_bind {
            config.tcp_bind.clone_from(tcp_bind);
            config.tcp_enabled = true;
        }
        if args.disable_tcp {
            config.tcp_enabled = false;
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> io::Result<()> {
        if self.bind.trim().is_empty() {
            return invalid_config("bind must not be empty");
        }
        if self.tcp_enabled && self.tcp_bind.trim().is_empty() {
            return invalid_config("tcp_bind must not be empty when TCP is enabled");
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

fn invalid_config<T>(message: impl Into<String>) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

fn default_tcp_enabled() -> bool {
    true
}

fn default_tcp_bind() -> String {
    "0.0.0.0:25910".to_owned()
}

fn default_tcp_egress_queue_capacity() -> usize {
    512
}
