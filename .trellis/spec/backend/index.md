# Backend Development Guidelines

> Coding conventions for the Basement Bridge Rust workspace. These document
> what the code **actually does**, so sub-agents match the team's patterns
> instead of writing generic Rust. All entries reference real files.

---

## Project At A Glance

- Cargo workspace, `resolver = "3"`, `edition = "2024"`, `AGPL-3.0-or-later`.
- Seven crates under `crates/`: `bridge-core` (shared runtime), `bridge-gui`
  (eframe/egui plus an in-process application worker), `bridge-relay`
  (UDP/TCP server), `hook-ipc` (shared local protocol), `isaac-injector`
  (process/injection helper), `native-hook` (i686 Windows DLL), and
  `relay-protocol` (shared bounded Relay v2 wire contract). See
  `docs/architecture.md` for the runtime flow.
- No database. Relay state is in-memory. Only the client config (TOML) and
  log/diagnostics files are persisted.
- Active testing phase with volunteer testers — observability and simple test
  steps matter (see `AGENTS.md`).

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Directory Structure](./directory-structure.md) | Workspace, crate boundaries, module layout, test placement | Filled |
| [State & Persistence](./database-guidelines.md) | In-memory relay state; no DB; config TOML only | Filled (reframed — no database in this project) |
| [Error Handling](./error-handling.md) | thiserror enums, `io::Error` at boundaries, log-and-count | Filled |
| [Quality Guidelines](./quality-guidelines.md) | Required/forbidden patterns, formatting, testing, review checklist | Filled |
| [Logging Guidelines](./logging-guidelines.md) | tracing foundation, levels, sampling, redaction, Diagnostics Bundle | Filled |
| [Native Hook Local IPC](./local-ipc.md) | Typed TBI2 contract, Named Pipe workers, backpressure, reconnect, tests | Filled |
| [Relay Protocol v2](./relay-v2.md) | Negotiated control boundary, fixed data frames, strict profiles, reconnect contract | Filled |
| [Release & CI](./release-ci.md) | GitHub Actions, release assets, build provenance, uv compatibility harness | Filled |

Thinking guides (general best-practice prompts) live in `../guides/` and are
pre-filled; customize only if something clearly doesn't fit.

---

## Cross-Cutting Rules (read first)

1. **Crate boundary is sacred.** Shared runtime code → `bridge-core`; local and
   Relay wire contracts → `hook-ipc` and `relay-protocol`. `bridge-gui` and
   `bridge-relay` never depend on each other. No shared/library crate depends
   on a binary. `unsafe` only in `native-hook` / `isaac-injector`.
2. **Concrete typed errors via `thiserror`.** No `anyhow`/`eyre`. Convert
   protocol errors to `io::Error` at the client/relay boundary.
3. **`Result<T, ConcreteError>` everywhere fallible.** No `type Result<T>`
   aliases. No bare `unwrap()`/`todo!()`/`unimplemented!()` in production.
4. **`#[must_use]`** on public constructors, accessors, and diagnostics.
5. **`.saturating_add()`** for all counters/metrics, never `+`.
6. **Logging through `emit_client_log_event()`** in `bridge-core` (session
   context attached); bare `tracing` macros with structured fields in
   `bridge-relay`; custom file logger in `native-hook`. Sample hot-path logs.
   Never log packet payloads or tokens.
7. **Diagnostics Bundle must stay self-contained** — new operational state goes
   into counters/logs or `diagnostics_text()`.
8. **Inline tests** with real sockets on `127.0.0.1:0`; large test modules in
   `*_tests.rs` via `#[path]`.
9. **LF line endings, 4-space indent, edition 2024** (`rustfmt.toml`,
   `.editorconfig`).

---

## Known Tech Debt (document, don't "fix" silently during unrelated work)

- `[workspace.dependencies]` is not used; crates pin dep versions independently.
- `[workspace.lints]` (`unsafe_code = "forbid"`, clippy `all`/`pedantic`) is
  declared but no crate sets `lints.workspace = true`, so the lints are inert.
- No `[profile.release]` overrides; no `.cargo/config.toml` checked in (cross-
  compilation via `cargo-zigbuild` is configured outside the repo).

These are improvement candidates for a dedicated task, not side-effects to
sprinkle into feature work.

---

**Language**: documentation is written in **English**.
