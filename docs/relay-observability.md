# Relay observability

The Relay Server can export OpenTelemetry metrics and traces through standard
OTLP/gRPC. Export is Relay-only: Bridge Clients neither run an OTel exporter nor
propagate trace context. Client Room Path Quality remains a local player-facing
measurement.

The receiver may be a local OpenTelemetry Collector, Vector, a SigNoz
collector, or another OTLP-compatible component. Tractor Beam does not depend
on that deployment topology or on a particular observability backend.

## Local log format

Relay logs always go to standard output and remain independent of OTLP export.
Set `LOG_FORMAT=json` for newline-delimited JSON intended for journald, Docker,
or another structured log collector. Existing `tracing` event fields are
top-level JSON properties, while timestamp, level, target, message, and current
span context use the standard `tracing-subscriber` JSON representation. JSON
output never contains ANSI escape sequences.

`LOG_FORMAT` supports exactly `text` and `json`. It defaults to `text` when
unset so direct local runs remain readable. An unsupported value fails startup
before the Relay binds its sockets. `RUST_LOG` controls filtering and defaults
to `info` when absent or invalid.

For a native systemd deployment, run the Relay binary directly with
`Environment=LOG_FORMAT=json` and let journald capture stdout. A Collector
Contrib `journald` receiver can conditionally apply a `json_parser` to
`body.MESSAGE`; non-JSON systemd messages and older Relay output should pass
through unchanged. This is simpler than wrapping the Relay in a shell pipeline,
which changes journald process metadata and adds buffering.

## Configuration

Telemetry is disabled unless the `[telemetry]` section is present. Environment
variables cannot enable it by themselves.

```toml
[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = "relay-guangzhou-1"
data_trace_sample_ratio = 0.001
```

- `otlp_endpoint` is the OTLP/gRPC receiver endpoint. Version 1 does not
  implement OTLP/HTTP.
- `service_instance_id` must be stable and unique among simultaneously running
  Relay instances.
- `data_trace_sample_ratio` is between `0` and `1`, defaults to `0.001`, and
  affects only successful game/probe dispatch spans. `0` disables those spans.
  Control-plane spans remain enabled.

Invalid explicit configuration or provider construction fails startup. Once
started, an unavailable or slow receiver is lossy and non-fatal: it cannot
block forwarding. Shutdown attempts to flush metrics and traces for at most two
seconds. A crash, kill, or power loss can lose the tail; no disk queue exists.

Every signal has these standard resource attributes:

| Attribute | Value |
|---|---|
| `service.name` | `tractor-beam-relay` |
| `service.version` | Relay build version |
| `service.instance.id` | configured stable instance ID |

## Metrics

All attributes are bounded enums. Metrics never contain Session Credential,
SessionKey, Room ID, connection ID, SteamID64, display name, IP address, socket
tuple, resume credential, path token, or packet payload.

| Name | Type | Unit | Meaning | Attributes |
|---|---|---:|---|---|
| `tractor_beam.relay.room.active` | Gauge | `{room}` | active rooms | none |
| `tractor_beam.relay.peer.active` | Gauge | `{peer}` | active peers | `network.transport`, `peer.presence` |
| `tractor_beam.relay.control.operation` | Counter | `{operation}` | control outcomes | `operation`, `outcome` |
| `tractor_beam.relay.control.operation.duration` | Histogram | `s` | control handling latency | `operation` |
| `tractor_beam.relay.data.frame` | Counter | `{frame}` | admitted/rejected/forwarded frames | `network.transport`, `direction`, `frame.type`, `outcome` |
| `tractor_beam.relay.data.io` | Counter | `By` | bytes admitted/forwarded | same as data frames |
| `tractor_beam.relay.data.dispatch.duration` | Histogram | `s` | route plus egress dispatch latency | `network.transport`, `frame.type` |
| `tractor_beam.relay.tcp.egress.queue.max_utilization` | Gauge | `1` | highest current per-peer TCP queue utilization | none |

Both duration histograms use explicit second boundaries:
`0.00025`, `0.0005`, `0.001`, `0.0025`, `0.005`, `0.01`, `0.025`, `0.05`,
`0.1`, `0.25`, `0.5`, `1`, and `2.5`.

Bounded values are:

- `network.transport`: `tcp`, `udp`
- `peer.presence`: `connected`, `reconnecting`
- `direction`: `inbound`, `outbound`
- `frame.type`: `game`, `probe`
- data `outcome`: `accepted`, `forwarded`, `duplicate`, `rate_limited`, `rejected`
- control `operation`: `bootstrap`, `join_begin`, `join_proof`, `resume`,
  `udp_path_request`, `ping`, `pong`, `stop`, `udp_path_hello`, `detach`,
  `session_expire`
- control `outcome`: `attempted`, `accepted`, `rejected`

The existing 30-second structured local log uses the same instance-owned
counters. It is not a second measurement source. The Relay exports no OTLP
logs.

## Traces

Span names are static:

| Span | Boundary | Important bounded/correlation fields |
|---|---|---|
| `relay.bootstrap` | one v2 bootstrap negotiation | `network.transport=tcp` |
| `relay.control` | one decoded control operation | `operation` |
| `relay.data.dispatch` | sampled successful game dispatch | transport, frame type, process-local room metric ID, frame ID |
| `relay.probe.dispatch` | sampled probe dispatch | transport, frame type, process-local room metric ID, probe ID and phase |

There are no connection-, Session-, or Room-lifetime spans. Span names never
contain runtime values. The process-local room metric ID and bounded probe/frame
IDs are trace correlation fields only, not metric attributes and not player
identity. Matching probe request/echo decisions are deterministic for the same
Relay instance, room metric ID, and probe ID. Client trace context is neither
accepted nor propagated.

Local tracing events are deliberately excluded from the OTel trace layer. This
keeps useful operator logs local and prevents their address or peer fields from
becoming exported span events.

## Minimal operations guide

Use sustained trends, not one spike:

- Rising `data.frame{outcome="rate_limited"}` with otherwise healthy queue and
  host resources usually means a peer is exceeding configured admission limits;
  adding servers does not fix that peer behavior.
- A sustained TCP queue maximum above `0.8` is a warning, not an automatic
  scaling command. Corroborate it with rising dispatch latency, forwarding
  failures, host CPU saturation, scheduler delay, or network-interface pressure.
- Rising dispatch latency on both TCP and UDP with host CPU saturation suggests
  Relay compute pressure. Rising TCP-only latency and queue utilization points
  first to TCP egress/backpressure.
- Compare inbound accepted frames/bytes with outbound forwarded frames/bytes.
  A sustained widening gap plus rejected outcomes identifies Relay-side loss;
  do not infer available bandwidth from these counters.
- Scale or resize only when pressure is sustained and at least one Relay signal
  and one deployment resource signal agree. Room/peer counts alone are demand,
  not proof of resource exhaustion.

These signals describe Relay behavior. They do not determine an ideal Client
Input Delay, estimate available bandwidth, or replace Client end-to-end Room
Path Quality shown to players.
