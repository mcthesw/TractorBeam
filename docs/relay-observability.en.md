# Relay observability

[中文](relay-observability.md)

The Relay Server can export OpenTelemetry metrics and traces through standard
OTLP/gRPC. Only the Relay performs this export.

The receiver may be a local OpenTelemetry Collector, Vector, SigNoz Collector,
or another OTLP-compatible component. Tractor Beam does not depend on a
particular deployment topology or observability backend.

## Local log format

Relay logs always go to standard output and remain independent of OTLP export.
Set `LOG_FORMAT=json` to produce newline-delimited JSON for journald, Docker, or
another structured log collector.

`LOG_FORMAT` supports `text` and `json`, and defaults to `text`. `RUST_LOG`
controls filtering and defaults to `info` when absent or invalid.

## Configuration

Telemetry is enabled only when the `[telemetry]` section is present. Environment
variables cannot enable it by themselves.

```toml
[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = "relay-guangzhou-1"
```

- `otlp_endpoint` is the OTLP/gRPC receiver endpoint. OTLP/HTTP is not supported.
- `service_instance_id` must be stable and unique among running Relay instances.

Every signal includes these standard resource attributes:

| Attribute | Value |
|---|---|
| `service.name` | `tractor-beam-relay` |
| `service.version` | Relay build version |
| `service.instance.id` | configured stable instance ID |

## Metrics

| Name | Type | Unit | Meaning | Attributes |
|---|---|---:|---|---|
| `tractor_beam.relay.room.active` | Gauge | `{room}` | active Rooms | none |
| `tractor_beam.relay.peer.active` | Gauge | `{peer}` | active Peers | `network.transport`, `peer.presence` |
| `tractor_beam.relay.connection.operation` | Counter | `{connection}` | accepted, blocked, and closed TCP connections | `outcome` |
| `tractor_beam.relay.connection.active` | UpDownCounter | `{connection}` | currently active accepted TCP connections | none |
| `tractor_beam.relay.control.operation` | Counter | `{operation}` | control operation outcomes | `operation`, `outcome` |
| `tractor_beam.relay.control.operation.duration` | Histogram | `s` | control handling latency | `operation` |
| `tractor_beam.relay.session.establishment.duration` | Histogram | `s` | Join/Resume duration and outcome | `operation`, `network.transport`, `outcome` |
| `tractor_beam.relay.data.frame` | Counter | `{frame}` | accepted, rejected, and forwarded frames | `network.transport`, `direction`, `frame.type`, `outcome` |
| `tractor_beam.relay.data.io` | Counter | `By` | accepted and forwarded bytes | same as Data Frames |
| `tractor_beam.relay.data.dispatch.duration` | Histogram | `s` | routing and egress latency | `network.transport`, `frame.type` |
| `tractor_beam.relay.tcp.egress.queue.max_utilization` | Gauge | `1` | highest current per-Peer TCP queue utilization | none |
| `tractor_beam.relay.tcp.egress.queue.full` | Counter | `{frame}` | frames refused because a TCP egress queue was full | `frame.type` |

Duration histograms use these boundaries in seconds:
`0.00025`, `0.0005`, `0.001`, `0.0025`, `0.005`, `0.01`, `0.025`, `0.05`,
`0.1`, `0.25`, `0.5`, `1`, and `2.5`.

Bounded values are:

- `network.transport`: `tcp`, `udp`
- `peer.presence`: `connected`, `reconnecting`
- `direction`: `inbound`, `outbound`
- `frame.type`: `game`, `probe`
- queue-full `frame.type`: `control`, `game`, `probe`
- data `outcome`: `accepted`, `forwarded`, `duplicate`, `rate_limited`, `rejected`
- connection `outcome`: `accepted`, `blocked`, `closed`
- establishment `operation`: `join`, `resume`, `unknown`
- establishment `outcome`: `accepted`, `rejected`, `failed`, `disconnected`, `timeout`
- control `operation`: `bootstrap`, `join_begin`, `join_proof`, `resume`,
  `udp_path_request`, `ping`, `pong`, `stop`, `udp_path_hello`, `detach`,
  `session_expire`
- control `outcome`: `attempted`, `accepted`, `rejected`

## Traces

Span names are fixed:

| Span | Boundary | Important bounded or correlation fields |
|---|---|---|
| `relay.session.establish` | one TCP accept through Join/Resume completion and required UDP path validation | process-local attempt ID, `session.operation`, `network.transport`, `outcome`, `error.type` |
| `relay.bootstrap` | compatibility bootstrap child | `outcome`, `error.type` |
| `relay.join.begin` | Join challenge child | `outcome`, `error.type` |
| `relay.join.proof` | Join proof child | `outcome`, `error.type` |
| `relay.resume` | Resume child | `outcome`, `error.type` |
| `relay.udp.validate` | required UDP path-validation child | `outcome`, `error.type` |

The establishment root span ends on success, rejection, failure, disconnect,
or its 15-second trace deadline.

## Capacity and server selection

Do not upgrade a server based only on Room count, Peer count, or a single metric
spike. Change capacity only when Relay and host metrics consistently point to
the same bottleneck.

| Symptom | Check or upgrade first |
|---|---|
| TCP and UDP dispatch latency rise while CPU remains saturated | CPU |
| Throughput approaches NIC or provider limits and packet loss rises while CPU is healthy | bandwidth and network route |
| RSS or memory pressure grows consistently with Peer count | memory |
| Rooms, Peers, or traffic grow without resource pressure | do not upgrade yet |

`rate_limited` usually means a Peer exceeded configured limits; adding server
resources will not fix it. Validate server location and routing with actual
Client Room Path Quality because Relay metrics cannot replace player experience.
