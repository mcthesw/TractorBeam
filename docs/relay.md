# Relay Server

The Relay Server is a single UDP binary plus a TOML config. It does not need a
database for Phase 2.

## Run

Build and run locally:

```sh
cargo run --release -p basement-bridge-relay -- --config deploy/relay.example.toml
```

Or override the bind address directly:

```sh
cargo run --release -p basement-bridge-relay -- --bind 0.0.0.0:25910
```

On a server, copy the release binary and a config file, then run:

```sh
RUST_LOG=info ./basement-bridge-relay --config /etc/basement-bridge/relay.toml
```

Docker build:

```sh
docker build -f deploy/Dockerfile.relay -t basement-bridge-relay .
docker run --rm -p 25910:25910/udp basement-bridge-relay
```

Open inbound UDP for the configured port, normally `25910/udp`. No public TCP
port is required for Relay Server traffic.

## Config

```toml
bind = "0.0.0.0:25910"
max_packet_size = 1500
peer_idle_seconds = 30
room_idle_seconds = 120
rate_limit_per_second = 240
max_rooms = 1024
max_peers_per_room = 4
max_room_name_len = 64
blocked_cidrs = []
```

- `max_packet_size`: receive buffer size for one UDP datagram.
- `peer_idle_seconds`: inactive Peer expiry.
- `room_idle_seconds`: empty Room expiry.
- `rate_limit_per_second`: per-address packet limit.
- `max_rooms`: maximum active Rooms.
- `max_peers_per_room`: maximum Peers in one Room.
- `max_room_name_len`: maximum Room name length in bytes.
- `blocked_cidrs`: local IP/CIDR blocklist, for example
  `["203.0.113.10/32", "2001:db8::/32"]`.

Restart the Relay Server after config changes.

## Runbook

- Startup is healthy when the log contains `relay listening`.
- If clients report `relay join timed out`, check inbound UDP and host firewall
  first.
- If a Room fills unexpectedly, check `max_peers_per_room` and stale Peers.
- For obvious abuse, add the source IP or CIDR to `blocked_cidrs`, restart, and
  keep raw IPs out of public notes.
- If CPU or traffic spikes, lower `rate_limit_per_second` and collect the Relay
  Server logs privately.
- The Relay Server keeps only in-memory Room state, so a restart drops all active
  Rooms.

The Relay Server requires clients to complete a join challenge before it
forwards room traffic, and it only forwards packets among peers in the same
room.
