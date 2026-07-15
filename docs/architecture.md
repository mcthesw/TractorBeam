# Architecture

Relay OpenTelemetry is explicitly configured and exported only by the Relay
process. It is separate from Client-local, player-facing Room Path Quality. See
[Relay observability](relay-observability.md) for the signal boundary.

Tractor Beam keeps three network boundaries deliberately separate:

1. The Native Hook exchanges target-addressed game packets with Bridge Client
   over Local IPC v2 (`TBI2`). It does not know about Relays, Rooms, Join Codes,
   reconnect credentials, or TCP/UDP path selection.
2. Bridge Client owns session orchestration, the selected Relay endpoint, the
   Session Credential, retry policy, queue/drop policy, and user-visible state.
3. Relay Protocol v2 (`TBR2`) is the Client-to-Relay boundary. The Relay owns
   admission, in-memory Room membership, path validation, forwarding, limits,
   duplicate suppression, and the 120-second resume grace window.

## Relay Protocol v2

Every session has one reliable TCP control connection. A bounded JSON bootstrap
selects protocol version and capabilities before admission, and returns a
structured compatibility rejection when required behavior is unavailable.
After bootstrap, direction-specific bounded JSON control frames carry Join,
Resume, presence, path-validation, Stop, and ping messages.

Gameplay never uses JSON. It uses a fixed binary Data Frame containing the
connection id, monotonic frame id, source/target SteamID64, Hook packet metadata,
and opaque Isaac payload. TCP profile frames share the control connection; UDP
profile frames use a separately path-validated UDP tuple. The selected profile
is strict for the lifetime of a running session and never silently falls back.

Capability-gated fixed binary Probe Frames measure **Room Path Quality** between
Bridge Clients over that same selected data profile. The target Bridge Client
echoes them locally; they never enter Native Hook or Isaac packet queues. The
typed per-Peer result stays in Bridge Core. The application combines only fresh,
bounded Room Path Quality windows with recent Session Health deltas to publish a
current smoothness level, confidence, freshness, and evidence reasons. Lifetime
diagnostic counters remain available but do not permanently degrade the current
estimate after recovery. Relay-wide OTel metrics are never an input to a player
estimate.

Bridge Core also exposes read-only Input Delay evidence: current delay
availability, the current smoothness snapshot (including the worst current peer
path), and an explicit blocker when the evidence is incomplete. This contract
does not convert milliseconds to delay units, recommend a value, coordinate
peers, or issue a Hook write. Manual Read/Write remains explicitly gated on a
running Fallback/Pure session with Hook IPC Ready, and written values are not
auto-restored. Bridge Client does not export these measurements to an
observability backend.

The wire contract lives in `relay-protocol`. Socket ownership and retry policy do
not. `bridge-relay` maps wire values into its domain state, while `bridge-core`
maps Hook packets into Data Frames and owns reconnect orchestration.

## Rooms and credentials

A Room has no player-editable name or separate admission code. The 128-bit
Session Credential is both its unguessable lookup key and bearer admission
secret. Join Code v5 packages the Relay endpoint/profile and exactly one Session
Credential as an opaque copy/paste value. Secrets, connection ids, resume keys,
and path tokens must never enter logs or diagnostics.

## Recovery

An unexpected Relay failure changes the local state to Reconnecting. Bridge
Client immediately tries Resume, then retries with jittered exponential delays
from 250 ms to 2 seconds for at most 120 seconds. A failed Resume may perform a
full Join on the same Relay with the same Session Credential. It never changes
Relay, Room, or TCP/UDP profile.

Real-time Hook packets received during an unavailable path are drained and
counted, never replayed after recovery. Relay keeps the logical Peer and duplicate
window for 120 seconds, broadcasts Reconnecting/Connected presence transitions,
and removes it once on expiry. Explicit Stop removes it immediately.

## Security boundary

Relay Protocol v2 is intentionally plaintext. Session credentials resist random
Room guessing but do not protect traffic from the Relay or an on-path observer.
V2 contains no TLS, AEAD, PAKE, payload MAC, nonce reservation, or encryption
extension fields. A changed confidentiality threat model requires a separately
designed incompatible v3.

## Future delivery profiles

UDP duplication/deduplication or hop-by-hop UDP FEC may wrap complete v2 Data
Frames later. They must be negotiated on the control plane, remain bounded by
Relay packet/byte limits, and must not change Native Hook packet semantics.
Direct LAN work from issue #38 is implemented as a separate route adapter;
v2 does not choose that topology in advance.
