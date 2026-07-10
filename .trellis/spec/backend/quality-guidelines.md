# Quality Guidelines

> Code quality, linting, formatting, and testing standards for Basement
> Bridge. Written from the real workspace configuration and code.

---

## Overview

This is a small, performance- and maintainability-sensitive Rust workspace
(see `AGENTS.md`: "保持代码架构清晰精简…性能和可维护性很重要"). The bar is:
concrete typed errors, `Result` everywhere, `#[must_use]` on public APIs,
saturating counters, inline tests with real sockets, and `unsafe` confined to
the two crates that need it. The project deliberately avoids `anyhow`, feature
flags, and `#[non_exhaustive]`.

---

## Workspace & Build

Root `Cargo.toml`: workspace with `resolver = "3"`, shared
`version = "0.2.1"`, `edition = "2024"`, `license = "AGPL-3.0-or-later"`,
`repository = "https://github.com/mcthesw/TractorBeam"`. Seven members under
`crates/`.

- **Cross-compilation**: `cargo-zigbuild` is used to build the relay for the
  test server (see `AGENTS.md`). There is no `.cargo/config.toml` checked in.
- **Profiles**: no `[profile.release]` overrides currently (no lto/strip).
- **Known tech debt (document, don't fix silently)**:
  - `[workspace.dependencies]` is **not** used — each crate pins its own dep
    versions independently (e.g. `tokio = "1.52.3"` repeats). Shared dep
    versions are a future cleanup, not a bootstrap concern.
  - `[workspace.lints]` declares `unsafe_code = "forbid"` and clippy
    `all`/`pedantic` = `warn`, but **no crate has `lints.workspace = true`**, so
    these lints are currently inert. Crates do not set `#![deny(warnings)]` or
    crate-level clippy attributes either.

---

## Formatting

`rustfmt.toml`:
```toml
edition = "2024"
newline_style = "Unix"
```
`.editorconfig`: UTF-8, `end_of_line = lf`, 4-space indent, final newline,
`trim_trailing_whitespace = true` (except `*.md`).

**Implication**: contributors on Windows must keep Git from introducing CRLF
(`core.autocrlf = false` or `input`). All source uses LF.

---

## Required Patterns

- **`Result<T, ConcreteError>` on every fallible function.** No `type Result<T>`
  aliases. See [error-handling.md](./error-handling.md).
- **`thiserror` for all custom errors.** No stringly-typed errors.
- **`#[must_use]` on public constructors, accessors, and diagnostic methods**
  (59+ occurrences). Examples: `runtime.rs:23` `#[must_use] pub fn new()`, and
  most `pub fn` in `relay-protocol/src/envelope.rs`.
- **Saturating arithmetic for all counters/metrics**: `.saturating_add(1)`.
  Never `+` on counters. See `bridge-relay/src/state.rs:87`, `server.rs:317`,
  `bridge-core/src/client/state.rs:26`.
- **Protocol versioning**: `relay-protocol` owns Relay Protocol v2 bounded
  bootstrap/control messages and fixed `b"TBR2"` data frames. Local IPC v2 independently owns
  `b"TBI2"`, roles, features, payload limits, and Postcard COBS framing in
  `hook-ipc`. Never reuse one protocol's constants to version the other.
- **Config structs with `Default`** for sensible defaults: `RelayConfig`,
  `SessionHealthConfig`, `BridgeClient::default()`.
- **Async runtime**: `tokio` only.
  - `bridge-relay`: `#[tokio::main]`, `JoinSet`, `tokio::sync::Mutex`,
    `tokio::time::interval` (`MissedTickBehavior::Delay`), `tokio::select!`
    with `CancellationToken`, `tokio_util::codec::LengthDelimitedCodec`+`Framed`
    for TCP, `tokio::sync::mpsc`.
  - `bridge-core`: spawns a dedicated 2-thread multi-thread runtime
    (`Builder::new_multi_thread().worker_threads(2).enable_all()`) in a
    background std thread. Blocking injector work uses `spawn_blocking`.
  - `bridge-gui`: **synchronous** eframe/egui presentation; a dedicated
    application thread owns `BridgeClient` behind bounded commands and an
    authoritative snapshot using `std::sync`, with no tokio in the GUI crate.
    Slow I/O and joins never run in egui callbacks.
  - `isaac-injector`, `native-hook`: synchronous, no async.

---

## Forbidden / Avoided Patterns

- **`unwrap()` in production code** — only in `#[cfg(test)]` or safe fallbacks
  (`unwrap_or_default`, `unwrap_or`). See [error-handling.md](./error-handling.md).
- **`todo!()` / `unimplemented!()`** — zero in the codebase. Don't add them.
- **`panic!` / `unreachable!` outside tests** — only inside `#[cfg(test)]`
  helpers.
- **`anyhow` / `eyre`** — not used; concrete errors only.
- **`#[non_exhaustive]`** — not used; protocol enums are exhaustive. Match the
  existing style unless introducing a genuinely extensible wire enum.
- **Feature flags** — none defined; conditional compilation is by
  `#[cfg(target_os = "...")]` / `#[cfg(target_arch = "...")]` only. Don't add
  `[features]` for new code without a strong reason.
- **`unsafe` outside `native-hook` and `isaac-injector`** — `bridge-core`,
  `bridge-relay`, `bridge-gui` contain zero `unsafe`. Keep it that way.
- **Blocking calls inside async tasks** — use `spawn_blocking`
  (`session.rs:456`).
- **`+` on counters** — use `.saturating_add()`.

---

## Testing Requirements

- **Inline `#[cfg(test)] mod tests`** in the source file. No top-level `tests/`
  integration directory.
- Large test modules move to a sibling `*_tests.rs` included via
  `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;`
  (`session.rs:598` → `session_tests.rs`, `server.rs:639` → `server_tests.rs`).
- Async tests use `#[tokio::test]` (e.g. `server_tests.rs`); sync tests use
  `#[test]`.
- **Real sockets on `127.0.0.1:0`**, no mocking framework. Tests that bind fixed
  ports are serialized with a `Mutex<()>` (`SESSION_TEST_LOCK` in
  `session_tests.rs`).
- Lightweight in-process helpers: `TestRelay` / `SilentRelay` (UDP relay
  simulators in threads), `TestPeer` (wraps UDP/TCP transport).
- Local IPC tests use real `interprocess` namespaced Local Sockets. On Windows,
  test nonblocking Named Pipe behavior explicitly; stream timeout APIs are not
  supported and `Ok(0)` can mean no bytes are currently available.
- **What to test**: encode/decode round-trips, error cases (bad magic, bad
  header, timeouts), state-machine transitions (join challenge/complete, room
  full, duplicate peer replacement), rate limiting, CIDR blocking, health
  ping/pong, session health snapshots, config parse/validation.
- **`unwrap()`/`expect()` are fine in test code.**

When adding a feature, add inline tests next to the code for: the happy path,
at least one error/edge case, and any state transition. If the module is
network-shaped, prefer a real-socket test over a mock.

---

## Code Review Checklist

- [ ] Fallible functions return `Result<T, ConcreteError>`; no bare `unwrap`/`expect` in prod paths.
- [ ] New errors are `thiserror` enums with `#[error("…")]`; wrapped sources use `#[from]`.
- [ ] Public constructors/accessors/diagnostics carry `#[must_use]`.
- [ ] Counters/metrics use `.saturating_add()`.
- [ ] Hot-path logs sampled; nothing sensitive beyond what `redact_text()` covers; new operational state added to the Diagnostics Bundle.
- [ ] No `unsafe` outside `native-hook` / `isaac-injector`; no blocking in async.
- [ ] Tests added inline (or `*_tests.rs` via `#[path]`) covering happy + error + transition cases.
- [ ] File uses LF, 4-space indent, final newline; matches `rustfmt.toml`/`.editorconfig`.
- [ ] Shared runtime code placed in `bridge-core`, wire contracts in the narrow protocol crate; no GUI↔relay cross-deps; crate boundary respected (see [directory-structure.md](./directory-structure.md)).
