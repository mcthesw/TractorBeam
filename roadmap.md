# Roadmap

This roadmap keeps the first milestone narrow: deliver the confirmed bridge path
as a small Windows player tool before expanding platform, packaging, transport,
and security scope.

## Phase 1: Windows Rust Baseline

Goal: replace the current script-driven flow with a Rust baseline while keeping
the existing Native Hook.

- [ ] Support Windows + Steam + *The Binding of Isaac: Repentance+* only.
- [ ] Keep the existing Native Hook and Injector inside the Client Bundle.
- [ ] Split `bridge-client` runtime from `bridge-gui` presentation.
- [ ] Build the Rust Relay Server with room join, peer forwarding, counters, timeouts, and basic limits.
- [ ] Build the Rust Bridge Client with local hook bridge, relay connection, room setup, launch, injection, state, and errors.
- [ ] Build the egui Bridge GUI with relay address, room, SteamID64, mode, start/stop, status, counters, and diagnostics export.
- [ ] Implement Official Mode, Fallback Mode, and Pure Mode.
- [ ] Support manual relay address entry and recent relay history.
- [ ] Define the first relay protocol envelope, version negotiation, capabilities, and error codes.
- [ ] Decide the Phase 1 control-message format after a short Cap'n Proto vs Protobuf spike.
- [ ] Produce a basic Diagnostics Bundle.
- [ ] Add focused tests for protocol encoding, relay forwarding, local bridge flow, and diagnostics packaging.
- [ ] Document failure recovery for launch, injection, relay, and hook errors.

Deferred from Phase 1:

- [ ] Linux support.
- [ ] Non-Steam support.
- [ ] installer packaging.
- [ ] Directory Service.
- [ ] end-to-end encryption.
- [ ] KCP, QUIC, FEC, or camouflage transports.

## Phase 2: Closed Testing

Goal: make the Windows baseline reliable across real player machines.

- [ ] Prepare closed test instructions and a feedback template.
- [ ] Deploy a public test Relay Server.
- [ ] Document Relay Server self-deployment.
- [ ] Improve Windows Steam and Isaac path detection.
- [ ] Improve launch, injection, failure recovery, and user-facing errors.
- [ ] Add Relay Server logs, basic abuse limits, and an operational runbook.
- [ ] Define diagnostics review workflow and log redaction rules.
- [ ] Collect compatibility notes for common mod-enabled sessions.
- [ ] Add Relay Server IP/CIDR blocklist support for test operations.

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
