# Roadmap

This roadmap keeps the first milestone narrow: deliver the confirmed bridge path
as a small Windows player tool before expanding platform, packaging, transport,
and security scope.

## Phase 1: Windows Rust Baseline

Goal: replace the current prototype flow with a Rust baseline and a Rust Native
Hook path.

- [x] Support Windows + Steam + *The Binding of Isaac: Repentance+* only.
- [x] Use the crate layout `bridge-core`, `bridge-gui`, `bridge-relay`, `native-hook`, and `isaac-injector`.
- [x] Build the Rust Native Hook DLL for the i686 Isaac process.
- [x] Build the Rust Injector helper.
- [x] Build the Rust Relay Server with room join, peer forwarding, timeouts, rate limits, and IP/CIDR blocklists.
- [x] Build the Rust Bridge Client runtime with local hook bridge, relay connection, room setup, Steam launch, injection orchestration, state, and errors.
- [x] Build the egui Bridge GUI with relay address, room, SteamID64, mode, start/stop, status, counters, and diagnostics export.
- [x] Implement Official Mode, Fallback Mode, and Pure Mode.
- [x] Define the first relay protocol envelope, versioning, capabilities, and error codes.
- [x] Use a simple versioned envelope for Phase 1 control messages.
- [x] Produce a basic Diagnostics Bundle.
- [x] Document failure recovery for launch, injection, relay, and hook errors.
- [ ] Produce a repeatable Client Bundle layout containing the Bridge GUI, Bridge Core runtime, Native Hook, and i686 Injector helper.
- [ ] Support recent relay history.
- [ ] Add focused local bridge flow tests beyond protocol, relay state, and diagnostics unit tests.
- [ ] Add Relay Server runtime counters/metrics.

Deferred from Phase 1:

- [ ] Linux support.
- [ ] Non-Steam support.
- [ ] installer packaging.
- [ ] Directory Service.
- [ ] end-to-end encryption.
- [ ] KCP, QUIC, FEC, or camouflage transports.

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

## Phase 4: Advanced Transports and Hardening

Goal: explore optional transport and security upgrades without disturbing the
baseline relay path.

- [ ] Add AEAD end-to-end encryption.
- [ ] Design room/session keys.
- [ ] Add Relay Server identity verification.
- [ ] Add replay protection.
- [ ] Add proof-of-work or comparable anti-abuse gating.
- [ ] Research optional FEC/redundancy.
- [ ] Research optional KCP transport.
- [ ] Research optional QUIC transport.
- [ ] Research optional TCP-like camouflage inspired by udp2raw or phantun-style approaches.
- [ ] Research Linux native or Proton support.

## Local Planning

Use `.local/` for scratch notes, long research, private test records, and
unsettled experiments. Only stable decisions belong in tracked docs.
