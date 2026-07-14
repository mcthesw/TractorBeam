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
```

- `otlp_endpoint` is the OTLP/gRPC receiver endpoint. Version 1 does not
  implement OTLP/HTTP.
- `service_instance_id` must be stable and unique among simultaneously running
  Relay instances.

Establishment traces are emitted at connection/admission rate. Gameplay and
probe forwarding never create spans, so there is no data-plane sampling knob.
The configuration parser rejects obsolete or unknown telemetry fields; remove
`data_trace_sample_ratio` before deploying this binary over an older config.

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
| `tractor_beam.relay.connection.operation` | Counter | `{connection}` | accepted, blocked, and closed TCP connections | `outcome` |
| `tractor_beam.relay.connection.active` | UpDownCounter | `{connection}` | currently active accepted TCP connections | none |
| `tractor_beam.relay.control.operation` | Counter | `{operation}` | control outcomes | `operation`, `outcome` |
| `tractor_beam.relay.control.operation.duration` | Histogram | `s` | control handling latency | `operation` |
| `tractor_beam.relay.session.establishment.duration` | Histogram | `s` | Join/Resume establishment time and outcome | `operation`, `network.transport`, `outcome` |
| `tractor_beam.relay.data.frame` | Counter | `{frame}` | admitted/rejected/forwarded frames | `network.transport`, `direction`, `frame.type`, `outcome` |
| `tractor_beam.relay.data.io` | Counter | `By` | bytes admitted/forwarded | same as data frames |
| `tractor_beam.relay.data.dispatch.duration` | Histogram | `s` | route plus egress dispatch latency | `network.transport`, `frame.type` |
| `tractor_beam.relay.tcp.egress.queue.max_utilization` | Gauge | `1` | highest current per-peer TCP queue utilization | none |
| `tractor_beam.relay.tcp.egress.queue.full` | Counter | `{frame}` | frames refused by a full TCP egress queue | `frame.type` |

All duration histograms use explicit second boundaries:
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

The 30-second task only records gauges; it emits no periodic log. The Relay
exports no OTLP logs.

## Traces

Span names are static:

| Span | Boundary | Important bounded/correlation fields |
|---|---|---|
| `relay.session.establish` | one TCP accept through successful/rejected Join or Resume and required UDP path validation | process-local attempt ID, `session.operation`, `network.transport`, `outcome`, `error.type` |
| `relay.bootstrap` | compatibility bootstrap child | `outcome`, `error.type` |
| `relay.join.begin` | Join challenge child | `outcome`, `error.type` |
| `relay.join.proof` | Join proof child | `outcome`, `error.type` |
| `relay.resume` | Resume child | `outcome`, `error.type` |
| `relay.udp.validate` | required UDP path-validation child | `outcome`, `error.type` |

The establishment root ends on success, rejection, failure, disconnect, or a
15-second trace-only deadline. That deadline never disconnects or rejects a
Client. Routine ping/presence/Stop controls, gameplay, and probe forwarding
create no spans. Span names never contain runtime values. Traces never contain
credentials, Room or wire connection IDs, Steam IDs, display names, addresses,
tokens, or payloads. Client trace context is neither accepted nor propagated.

Local tracing events are deliberately excluded from the OTel trace layer. This
keeps useful operator logs local and prevents their address or peer fields from
becoming exported span events.

## Capacity and server-purchase guide

Use sustained trends, not one spike:

- Rising `data.frame{outcome="rate_limited"}` with otherwise healthy queue and
  host resources usually means a peer is exceeding configured admission limits;
  adding servers does not fix that peer behavior.
- A sustained TCP queue maximum above `0.8` is a warning, not an automatic
  scaling command. Corroborate it with queue-full events or rising dispatch
  latency and host pressure.
- Rising dispatch latency on both TCP and UDP with host CPU saturation suggests
  Relay compute pressure. Rising TCP-only latency and queue utilization points
  first to TCP egress/backpressure.
- Compare inbound accepted frames/bytes with outbound forwarded frames/bytes.
  A sustained widening gap plus rejected outcomes identifies Relay-side loss;
  do not infer available bandwidth from these counters.
- Scale or resize only when pressure is sustained and at least one Relay signal
  and one deployment resource signal agree. Room/peer counts alone are demand,
  not proof of resource exhaustion.

The Collector must add host evidence that the Relay cannot infer: CPU
utilization/throttling, RSS and memory pressure, scheduler run queue or steal
time, interface throughput, packet drops/errors, and provider bandwidth or
traffic quota. Evaluate comparable peak windows (for example 15-minute and
24-hour views), not a single scrape.

Use the correlated bottleneck to guide a purchase:

| Sustained evidence | Likely bottleneck | Purchase direction |
|---|---|---|
| dispatch latency rises on TCP and UDP, queue-full rises, CPU/run queue saturated | compute/scheduling | more or faster CPU before adding bandwidth |
| TCP/UDP bytes approach interface/provider limit, drops/errors rise, CPU healthy | network | higher guaranteed bandwidth/traffic quota or a better network route |
| RSS/memory pressure rises with active rooms/peers, CPU/network healthy | memory/concurrency | more RAM after confirming retention is expected |
| rooms/peers or traffic rise with no pressure signal | demand only | no resize conclusion |

Do not derive a vendor SKU, bandwidth claim, or safe peer ceiling from one
Relay metric. Preserve the observed sustained workload and headroom when
comparing candidate servers, then load-test the candidate with the same mix.

These signals describe Relay behavior. They do not determine an ideal Client
Input Delay, estimate available bandwidth, or replace Client end-to-end Room
Path Quality shown to players.
