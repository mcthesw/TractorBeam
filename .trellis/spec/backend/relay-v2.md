# Relay Protocol v2 Contract

## 1. Scope / Trigger

Apply this specification whenever changing Client/Relay negotiation, admission,
forwarding, reconnect, path validation, selected profile, or Relay wire bytes.

## 2. Signatures

```rust
fn select_protocol(client: &[ProtocolRange], relay: &[ProtocolRange]) -> Result<ProtocolVersion, ProtocolSelectionError>;
fn select_capabilities(required: u64, optional: u64, available: u64) -> Result<u64, CapabilityError>;
fn route_data(&mut self, request: RouteData) -> Result<DataDestination, StateError>;
async fn reconnect(&mut self) -> io::Result<RecoveryKind>;
```

## 3. Contracts

- `relay-protocol/src/v2/` owns bounded bootstrap/control types, fixed binary
  Data/Probe Frames, version/capability selection, redacted secrets, duplicate
  windows, and golden fixtures. It owns no sockets or Room state.
- `bridge-relay` owns TCP/UDP sockets, credential-keyed in-memory Rooms,
  admission, limits, path validation, routing, presence, and 120-second grace.
- `bridge-core::client::relay_transport` owns Client socket/wire adaptation;
  session orchestration owns retry, queue/drop, logging, and UI events.
- TCP control is mandatory. TCP data shares it; UDP data uses a validated tuple.
  Profile is fixed for the running session and never falls back automatically.
- Join Code v5 carries Relay selection and exactly one 128-bit Session Credential.
- Resume retains connection identity, Room membership, and duplicate window.
  Outage packets are dropped and counted, never replayed.
- Native Hook Local IPC remains route-agnostic and independent.
- `CAP_ROOM_PATH_PROBE` is optional. Capable Bridge Clients send fixed Probe
  request/echo frames only to capable Peers over the selected data profile.
  Relay validates/forwards; Bridge Core owns measurement windows and GUI state.
  Probe Frames never enter Native Hook or Isaac game counters and never fall
  back from UDP to TCP.

## 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Bootstrap schema differs | `UnsupportedBootstrapSchema` before Join |
| No common protocol | `UnsupportedProtocol` before Join |
| Required capability unavailable | `MissingRequiredCapabilities` before Join |
| UDP tuple/token invalid | `PathValidationFailed`; never switch to TCP |
| Sender/connection/profile mismatch | Reject; do not forward |
| Duplicate/too-old frame id | Drop and increment duplicate metric |
| Packet/byte budget exceeded | `RateLimited`; do not forward |
| Resume unknown/invalid/expired | Stable Resume rejection; policy may allow full Join |
| Explicit Stop | Remove immediately, with no grace window |

Secrets, connection ids, SteamID64 values, display names, and source tuples must
not enter logs or exported diagnostics.

## 5. Good / Base / Bad Cases

- Good: a detached Peer resumes inside 120 seconds with its connection id and
  duplicate window intact; Room peers see Reconnecting then Connected.
- Base: Relay restart loses state; Client full-Joins the same Relay with the same
  Session Credential and selected profile.
- Bad: reconnect replays Hook packets, changes UDP to TCP, logs bearer material,
  or puts extensible JSON/negotiation on each gameplay packet.

## 6. Tests Required

Protect exact golden bytes, bootstrap/control bounds, capability selection,
source/profile/path validation, duplicate preservation across Resume, grace
expiry exactly once, cancellation, bounded outage drops, Room Path Quality
window semantics, and real TCP/UDP Probe sockets.
Run workspace tests, Clippy with warnings denied, rustfmt, and `git diff --check`.

## Relay OpenTelemetry boundary

- Relay OTel metrics and traces are enabled only by an explicit `[telemetry]`
  section and use OTLP/gRPC. Ambient `OTEL_*` values cannot enable export.
- Exporter/provider assembly belongs to the Relay application boundary. Domain
  state receives no provider and reads no telemetry/global configuration.
- Relay metrics use instance-owned counters and bounded enum attributes. Never
  attach Session Credential/SessionKey, Room ID, connection ID, SteamID64,
  display name, address/tuple, credentials/tokens, or payload.
- Use only static span names. Do not create connection-, Session-, or Room-life
  spans or accept/propagate Client trace context.
- OTel exporter failure is lossy and non-fatal after startup. It must not enter
  the network I/O result path; orderly flush has a two-second total cap and no
  persistent queue.
- Keep local structured logs, but do not export them as OTLP logs or span
  events. `docs/relay-observability.md` is the canonical signal catalog.

## 7. Wrong vs Correct

Wrong: domain helpers read global config, UDP silently falls back to TCP, or
gameplay frames carry build/capability/reconnect/FEC/UI fields.

Correct: socket/application code supplies explicit `RouteData` and configured
limits to credential-keyed domain state; bootstrap occurs once on TCP; gameplay
stays a bounded fixed binary frame on the explicitly selected profile.

## Deliberate exclusions

V2 has no QUIC, generic transport plugins, automatic fallback, TLS/AEAD/PAKE/MAC
fields, FEC, duplication, or specified-LAN routing. UDP FEC/duplication may later
wrap complete Data Frames as bounded delivery profiles. Encryption requires v3.
