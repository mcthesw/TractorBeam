# Roadmap

[中文](roadmap.md)

The roadmap keeps the first milestone narrow: deliver the confirmed Windows
player tool before expanding platform, packaging, transport, and security scope.

## Phase 1: Windows Rust baseline

Goal: keep the Windows Bridge path on the Rust baseline and Rust Native Hook.

- [x] Support only Windows + Steam + *The Binding of Isaac: Repentance+*.
- [x] Use runtime crates `bridge-core`, `bridge-gui`, `bridge-relay`,
  `native-hook`, and `isaac-injector`, plus the focused shared-contract crates
  `hook-ipc` and `relay-protocol`.
- [x] Build the Rust Native Hook DLL for the i686 Isaac process.
- [x] Build the Rust Injector helper.
- [x] Build the Rust Relay Server with Room admission, Peer forwarding, UDP/TCP
  listeners, timeouts, rate limits, and IP/CIDR blocklists.
- [x] Build the Rust Bridge Client runtime with the asynchronous local Hook
  bridge, selectable Relay transport, Room setup, Steam launch, injection
  orchestration, state, and error handling.
- [x] Build the egui Bridge GUI with Relay address, transport choice, Room,
  SteamID64, mode, start/stop, status, counters, and diagnostics export.
- [x] Implement Official Mode, Fallback Mode, and Pure Mode.
- [x] Define the first Relay protocol envelope, versions, capabilities, and
  error codes.
- [x] Use a simple versioned envelope for Phase 1 control messages.
- [x] Produce a basic Diagnostics Bundle.
- [x] Document recovery from launch, injection, Relay, and Hook failures.
- [x] Add focused local Bridge flow tests beyond protocol, Relay state, and
  diagnostics unit tests.
- [x] Add Relay Server runtime counters and metrics.

Deferred from Phase 1:

- Linux support.
- Non-Steam support.
- Installer packaging.
- Directory Service.
- Optional bounded UDP duplication/deduplication or FEC profiles.

## Phase 2: Testing

Goal: make the Windows baseline reliable across real player machines.

- [x] Prepare test instructions and a feedback template.
- [x] Deploy a public test Relay Server.
- [x] Document Relay Server self-deployment.
- [x] Improve Windows Steam and Isaac path detection.
- [x] Improve launch, injection, recovery, and user-facing errors.
- [x] Add Relay Server logs, basic abuse limits, and an operations guide.
- [x] Define diagnostics review and log-redaction rules.
- [x] Collect compatibility notes for common sessions with mods.
- [x] Add local Relay Server IP/CIDR blocklists for test operations.
- [x] Verify the Rust Native Hook and i686 Injector on tester machines without
  prototype binaries.
- [x] Verify that the Client Bundle can be copied to a clean machine and run
  from the Bridge GUI.

## Phase 3: Public release

Goal: make the project safe and understandable for ordinary players.

- [x] Publish GitHub Release assets.
- [x] Add the Release Please flow.
- [ ] Build a Directory Service with signed Relay Server metadata.
- [ ] Add minimum and maximum Client/Relay protocol version policies.
- [x] Write user documentation, FAQ, Windows security notice, and checksum
  guidance.
- [ ] Define a public Relay Server policy.
- [ ] Support Relay revocation and trust metadata through the Directory Service.

## Phase 4: UDP delivery experiments and hardening

Goal: explore bounded UDP delivery improvements without disturbing the baseline
TCP control and TCP/UDP data paths.

- [x] Add proof-of-work or comparable anti-abuse gating.
- [ ] Research bounded UDP duplicate-send/deduplication profiles.
- [ ] Research hop-by-hop UDP FEC around complete Relay Data Frames.
- [ ] Measure added bandwidth, recovery rate, tail latency, and Relay CPU before
  exposing either option to users.
- [ ] Research native Linux or Proton support.

Payload encryption is not on the current roadmap.
