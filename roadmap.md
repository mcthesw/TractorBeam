# Roadmap

This roadmap keeps the first milestone narrow: deliver the confirmed bridge path
as a small Windows player tool before expanding platform, packaging, transport,
and security scope.

## Phase 1: Windows Rust Baseline

Goal: keep the Windows bridge path on the Rust baseline and Rust Native Hook
path.

- [x] Support Windows + Steam + *The Binding of Isaac: Repentance+* only.
- [x] Use runtime crates `bridge-core`, `bridge-gui`, `bridge-relay`,
  `native-hook`, and `isaac-injector`, plus narrow shared-contract crates
  `hook-ipc` and `relay-protocol`.
- [x] Build the Rust Native Hook DLL for the i686 Isaac process.
- [x] Build the Rust Injector helper.
- [x] Build the Rust Relay Server with room join, peer forwarding, UDP/TCP listeners, timeouts, rate limits, and IP/CIDR blocklists.
- [x] Build the Rust Bridge Client runtime with async local hook bridge, selectable Relay Transport, room setup, Steam launch, injection orchestration, state, and errors.
- [x] Build the egui Bridge GUI with relay address, transport choice, room, SteamID64, mode, start/stop, status, counters, and diagnostics export.
- [x] Implement Official Mode, Fallback Mode, and Pure Mode.
- [x] Define the first relay protocol envelope, versioning, capabilities, and error codes.
- [x] Use a simple versioned envelope for Phase 1 control messages.
- [x] Produce a basic Diagnostics Bundle.
- [x] Document failure recovery for launch, injection, relay, and hook errors.
- [ ] Produce a repeatable Client Bundle layout containing the Bridge GUI, Bridge Core runtime, Native Hook, and i686 Injector helper.
- [ ] Support recent relay history.
- [x] Add focused local bridge flow tests beyond protocol, relay state, and diagnostics unit tests.
- [x] Add Relay Server runtime counters/metrics.

Deferred from Phase 1:

- [ ] Linux support.
- [ ] Non-Steam support.
- [ ] installer packaging.
- [ ] Directory Service.
- [ ] Optional bounded UDP duplication/deduplication or FEC profiles.

## Phase 2: Closed Testing

Goal: make the Windows baseline reliable across real player machines.

- [x] Prepare closed test instructions and a feedback template.
- [ ] Deploy a public test Relay Server.
- [x] Document Relay Server self-deployment.
- [x] Improve Windows Steam and Isaac path detection.
- [x] Improve launch, injection, failure recovery, and user-facing errors.
- [x] Add Relay Server logs, basic abuse limits, and an operational runbook.
- [x] Define diagnostics review workflow and log redaction rules.
- [x] Collect compatibility notes for common mod-enabled sessions.
- [x] Add Relay Server IP/CIDR blocklist support for test operations.
- [ ] Verify the Rust Native Hook and i686 Injector helper on tester machines without prototype binaries.
- [ ] Verify the Client Bundle layout can be copied to a clean machine and run from the Bridge GUI.

## Phase 3: Public Release

Goal: make the project safe and understandable for ordinary players.

- [ ] Publish GitHub Release artifacts.
- [ ] Add release-please release flow.
- [ ] Research and implement installer packaging.
- [ ] Build the Directory Service with signed Relay Server metadata.
- [ ] Add client/relay minimum and maximum protocol version policy.
- [ ] Add update and rollback policy.
- [ ] Write user documentation, FAQ, Windows security notice, and checksum guidance.
- [ ] Define public Relay Server policy.
- [ ] Add Directory Service support for relay revocation and trust metadata.

## Phase 4: UDP Delivery Experiments and Hardening

Goal: explore bounded UDP delivery improvements without disturbing the baseline
TCP-control and TCP/UDP data paths.

- [ ] Add proof-of-work or comparable anti-abuse gating.
- [ ] Research bounded UDP duplicate-send/deduplication profiles.
- [ ] Research hop-by-hop UDP FEC around complete Relay data frames.
- [ ] Measure added bandwidth, recovery rate, tail latency, and Relay CPU before
      making either profile user-facing.
- [ ] Research Linux native or Proton support.

Payload encryption is not on the current roadmap. If a future threat model ever
requires confidentiality or on-path integrity, design it as an explicit Relay
Protocol v3 rather than reserving unused crypto fields in v2.

## Local Planning

Use `.local/` for scratch notes, long research, private test records, and
unsettled experiments. Only stable decisions belong in tracked docs.
