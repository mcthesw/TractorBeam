# Relay configuration

[中文](relay-configuration.md)

Start from the maintained [`relay.toml`](../deploy/relay.toml). Unknown fields
and invalid values stop the Relay at startup, so keep the defaults unless a
deployment has a specific reason to change them.

## Listeners

```toml
[relay_server]
tcp_bind = "0.0.0.0:25910"
udp_bind = "0.0.0.0:25910"
```

- `tcp_bind` is required for the control plane.
- `udp_bind` enables the UDP data path. Remove it only for a TCP-only Relay.
- When either port changes, update the Docker port mapping and allow the new
  TCP or UDP port through the host and cloud firewalls.
- Use `[::]:25910` to bind IPv6. Whether an IPv6 wildcard also accepts IPv4 is
  operating-system dependent.

## Admission and room capacity

```toml
[admission]
pow_difficulty_bits = 18

[room_limits]
max_rooms = 256
```

- `pow_difficulty_bits` controls join proof-of-work. Keep `18` for a public
  Relay; use `0` only for a trusted local development Relay.
- `max_rooms` limits the number of active in-memory rooms.

## Traffic limits

```toml
[traffic_limits]
rate_limit_per_second = 5000
byte_rate_limit_per_second = 8388608
byte_rate_limit_burst = 16777216
```

These limits apply per peer. Keep the supplied values unless observed workload
and host capacity justify changing them.

## Access control

```toml
[access_control]
blocked_cidrs = []
```

`blocked_cidrs` accepts IPv4 or IPv6 addresses and CIDR ranges:

```toml
blocked_cidrs = ["203.0.113.10/32", "2001:db8::/32"]
```

## Telemetry

Telemetry is disabled when this section is absent:

```toml
[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = "relay-example-01"
```

The endpoint must accept OTLP/gRPC. Use a stable, unique
`service_instance_id` for each running Relay. See
[Relay observability](relay-observability.en.md) for the exported signals.
