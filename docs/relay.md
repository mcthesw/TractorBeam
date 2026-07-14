# Relay Server

For OTLP/gRPC metrics, traces, exact signal names, and the minimal capacity
guide, see [Relay observability](relay-observability.md).

The Relay is a stateless, in-memory Relay Protocol v2 forwarder. It requires a
TCP listener for the reliable control plane and may expose UDP on the same public
port for the UDP data profile. The test host only opens `25910/TCP` and
`25910/UDP`, so both listeners should normally use that port.

## Configuration

```toml
[relay_server]
tcp_bind = "0.0.0.0:25910"
udp_bind = "0.0.0.0:25910" # omit for TCP-only operation

[admission]
pow_difficulty_bits = 18

[room_limits]
max_rooms = 256

[traffic_limits]
rate_limit_per_second = 5000
byte_rate_limit_per_second = 8388608
byte_rate_limit_burst = 16777216

[access_control]
blocked_cidrs = []
```

TCP cannot be disabled in v2. A Relay without UDP accepts only Clients whose
required capabilities allow the TCP data profile. Listener, packet-size, queue,
Room, traffic, and admission values are validated at startup.

## Session lifecycle

The Client first performs the bounded compatibility bootstrap, then Join with a
128-bit Session Credential and proof-of-work challenge. The Relay creates or
finds the credential-keyed Room and issues opaque connection/resume material.
UDP sessions additionally validate a one-time path token from the observed UDP
source tuple before any UDP forwarding.

Unexpected TCP loss marks the Peer Reconnecting and retains Room membership,
connection identity, duplicate window, and validated UDP tuple for 120 seconds.
Resume restores Connected presence. Explicit Stop removes immediately; expiry
removes once. Relay restart intentionally loses all in-memory state, after which
Clients perform a full Join with the existing Session Credential.

## Protection and observability

Relay applies packet and byte limits per connection, validates sender identity,
Room membership, selected profile, UDP tuple, target membership, and monotonic
frame window before forwarding. Duplicate and too-old frames are discarded.

Startup/bootstrap transitions, disconnect/resume outcomes, expiry, and anomalies
are structured logs. Continuous workload, latency, queue pressure, and active
state belong to metrics and are not repeated as periodic INFO logs. Never log
Session Credentials, connection ids, resume/path credentials, SteamID64 values,
display names, or peer addresses.

Relay Protocol v2 also forwards capability-gated Room Path Quality Probe Frames.
They use the selected TCP or validated UDP data path and remain separate from
Isaac Data Frames. Relay validates source, Room, target, capability, path, and a
small per-connection probe budget; Bridge Clients own echo timing and the
user-visible RTT/jitter/loss calculation.

## Compatibility

The Relay implements only Relay Protocol v2 (`TBR2`); there is no legacy v1
listener, fallback, runtime module, or API namespace. Old Clients and Join Codes
are rejected by the hard-cut release. Bootstrap incompatibility is returned
with a stable schema/protocol/capability reason before admission.
