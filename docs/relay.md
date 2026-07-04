# Relay Server

The Relay Server is one binary that can listen on UDP, TCP, or both using the
same Relay Protocol. It does not need a database for Phase 2.

## Run

Build and run locally:

```sh
cargo run --release -p basement-bridge-relay -- --config deploy/relay.example.toml
```

Or override the bind address directly:

```sh
cargo run --release -p basement-bridge-relay -- --bind 0.0.0.0:25910
```

Use `--tcp-bind 0.0.0.0:25910` to override TCP separately, or `--disable-tcp`
to run UDP only.

On a server, copy the release binary and a config file, then run:

```sh
RUST_LOG=info ./basement-bridge-relay --config /etc/basement-bridge/relay.toml
```

Docker build:

```sh
docker build -f deploy/Dockerfile.relay -t basement-bridge-relay .
docker run --rm -p 25910:25910/udp -p 25910:25910/tcp basement-bridge-relay
```

Open inbound firewall rules for each listener configured under
`[relay_server]`, normally `25910/udp` and `25910/tcp`.

## Config

```toml
[relay_server]
udp_bind = "0.0.0.0:25910"
tcp_bind = "0.0.0.0:25910"

[admission]
pow_difficulty_bits = 18

[room_limits]
max_rooms = 256

[access_control]
blocked_cidrs = []
```

- `relay_server.udp_bind`: UDP listener address. Omit it to disable UDP.
- `relay_server.tcp_bind`: TCP listener address. Omit it to disable TCP.
- `admission.pow_difficulty_bits`: room-admission proof-of-work difficulty.
  Use `0` only for local or private development relays.
- `room_limits.max_rooms`: maximum active Rooms.
- `blocked_cidrs`: local IP/CIDR blocklist, for example
  `["203.0.113.10/32", "2001:db8::/32"]`.

Other relay safety limits are shared built-in defaults. Public relay datagrams
are capped at 2048 bytes on the wire, each peer is capped at 100 packets/s plus
a 256 KiB/s byte-token bucket with a 512 KiB burst, HealthPing replies are
capped per source IP, rooms hold at most four peers, inactive peers expire after
30 seconds, and empty rooms expire after 120 seconds.

Restart the Relay Server after config changes.

## Runbook

- Startup is healthy when the log contains `relay listening`.
- If clients report `relay join timed out`, check the selected Transport Choice
  and the matching firewall rule first.
- If a Room fills unexpectedly, check stale Peers and whether the Room already
  has four peers.
- For obvious abuse, add the source IP or CIDR to `blocked_cidrs`, restart, and
  keep raw IPs out of public notes.
- If TCP sessions feel delayed, compare `tcp_egress_queue_full` and
  `tcp_egress_dropped_packets` in the periodic `relay stats` lines before
  changing relay internals.
- If CPU or traffic spikes, collect the Relay Server logs privately and check
  `blocked`, `rate_limited`, and room-level counters before changing defaults.
- The Relay Server keeps only in-memory Room state, so a restart drops all active
  Rooms.

The Relay Server requires clients to complete a join challenge before it
forwards room traffic, and it only forwards packets among peers in the same
room.
