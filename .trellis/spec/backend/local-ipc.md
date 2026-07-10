# Native Hook Local IPC Contract

## 1. Scope / Trigger

This contract applies whenever `bridge-core`, `native-hook`, or `hook-ipc`
changes communication between the x86 Native Hook and the x86_64 Bridge
Client. Local IPC v2 is independent of Relay Protocol v2; changing one must not
silently change the other.

The local transport is one full-duplex `interprocess` Local Socket. On Windows
it is a Named Pipe. `hook-ipc` owns typed messages and Postcard COBS framing;
endpoint crates own their workers and never duplicate wire tags or offsets.

## 2. Signatures

Shared entry points in `tractor-beam-hook-ipc`:

```rust
pub fn encode<T: WireMessage>(message: &T) -> Result<Vec<u8>, ProtocolError>;
pub fn decode<T: WireMessage>(frame: &mut [u8]) -> Result<T, ProtocolError>;
pub fn endpoint_name(session_id: SessionId) -> String;
pub fn FrameDecoder::push<T: WireMessage>(&mut self, input: &[u8])
    -> Result<Vec<T>, ProtocolError>;
```

The Bridge Client adapter exposes typed queues and application operations, not
`interprocess` streams. The Native Hook callback boundary is:

```rust
pub fn send_packet(peer: u64, data: *const u8, len: u32,
                   send_type: i32, channel: i32) -> bool;
```

It may copy the opaque payload and call one bounded `try_send`; connection,
framing, logging, retries, and writes belong to `ipc_worker.rs`.

## 3. Contracts

Launch parameters written beside the Hook DLL:

| Key | Type | Constraint |
|-----|------|------------|
| `mode` | string | `replace` for a Native Hook session |
| `fallback_to_steam` | bool-like integer | `0` or `1` |
| `ipc_endpoint` | string | Per-session Local Socket name |
| `ipc_session` | 32 hex chars | Random 16-byte session identity |

Handshake fields are magic `TBI2`, major/minor version, peer role, session
identity, required feature bits, and maximum game payload. Both peers must
validate the opposite role and the exact session before data is accepted.

Directional messages:

- Hook to Client: `Handshake`, `Ready`, `Game`, `InputDelayResult`, `Pong`,
  `Health`, `Goodbye`.
- Client to Hook: `Handshake`, `Game`, `InputDelay`, `Ping`, `Shutdown`.

Game payloads are opaque and limited to 65,535 bytes. Frames use
`postcard::to_stdvec_cobs`; a zero terminator is part of every encoded frame.
Data queues are bounded to 1,024 entries and drop the newest message on
overflow. The control queue is a separate 32-entry priority path.

## 4. Validation & Error Matrix

| Condition | Required result |
|-----------|-----------------|
| Bad magic, wrong role, wrong session, wrong major, missing features, or payload-limit disagreement | Terminal `InvalidData`; publish failed IPC state; no legacy fallback |
| Oversized payload/frame, unknown enum tag, malformed or truncated COBS | Typed `ProtocolError`; no panic |
| Initial Hook absent beyond the lifecycle budget | Terminal timeout and unified session teardown |
| Connected pipe drops | Publish reconnecting; discard stale queued data; accept only a fresh handshake for the same session within 3 seconds |
| Reconnect timeout or liveness timeout | End the session; never bind a relaunched Client/Isaac |
| Data queue full | Immediate `false`/drop-newest and saturating counter increment |
| Control queue full or disconnected | Bounded `WouldBlock`/`BrokenPipe` error; never wait behind game traffic |
| Input Delay response exceeds 750 ms | Bounded timeout surfaced through `InputDelayError::Io` |

Windows Named Pipes do not support stream read/write timeouts through
`interprocess`. Both workers therefore set streams nonblocking and implement
deadlines with bounded polling and partial-write handling. On Windows a
nonblocking Named Pipe read may return `Ok(0)` when no bytes are available;
liveness ping/pong plus write failures distinguish a silent disconnect.

## 5. Good / Base / Bad Cases

- Good: matching x86 Hook and x86_64 Client exchange handshake/ready, then
  game packets and Input Delay on one connection; health counters reach
  diagnostics without payload or session identity.
- Base: Isaac has not started yet; the listener and process supervisor remain
  cancellable, egui stays responsive, and no callback exists to block.
- Bad: an old Hook connects with another version/session; the Client rejects it
  immediately and the application returns to Idle instead of silently losing
  packets or trying UDP/TCP compatibility.

## 6. Tests Required

- Codec: directional round trips, fragmented/multiple frames, unknown tags,
  malformed/truncated/oversized input, exact encoded sizes for 16/40/1100/max.
- Real Windows Local Socket: handshake, bidirectional game packets, Input Delay
  request/result, graceful shutdown, same-session reconnect, wrong session,
  malformed post-handshake frame, absent peer timeout.
- Backpressure: fill each bounded data queue, assert drop-newest, immediate
  return, and saturating counter; verify control completes during data bursts.
- Cross architecture: compile the Native Hook for `i686-pc-windows-msvc` and
  run the x86_64 Local Socket harness using the same shared message crate.
- Diagnostics: assert connection/version/reconnect/drop/malformed fields are
  present while `ipc_session` and `ipc_endpoint` values are redacted.

## 7. Wrong vs Correct

Wrong:

```rust
// Steam callback connects, serializes, logs, and writes directly.
let mut stream = LocalSocketStream::connect(name)?;
stream.write_all(&hand_written_packet(payload))?;
```

Correct:

```rust
// Steam callback performs one bounded enqueue; the owned worker does I/O.
match data_tx.try_send(GamePacket { payload: payload.to_vec(), ..packet }) {
    Ok(()) => true,
    Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
        saturating_increment(&counters.hook_data_dropped);
        false
    }
}
```
