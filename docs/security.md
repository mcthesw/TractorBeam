# Security

Basement Bridge is a work-in-progress transport moving toward a Windows
baseline. This document records the security boundary before wider distribution.

## Threat Model

Who Basement Bridge trusts, and how much:

- **Same-room Peer: semi-trusted.** A peer who joined your Room is trusted to
  play, not to behave. Bridge treats game payloads as opaque and does not
  validate Isaac semantics, so a peer can send any game-layer packet. The line
  Bridge does enforce: a peer cannot make a Relay Server forward traffic outside
  its Room, and cannot turn the relay into a general-purpose UDP forwarder toward
  third parties.
- **Relay Server: untrusted.** The relay moves packets but is not trusted with
  their contents. It can already see metadata (peer addresses, Room name, packet
  size, direction, timing, byte counts). The durable fix is end-to-end AEAD so
  the relay only ever carries ciphertext. Until that ships, the relay operator is
  a necessary trust, so only run sessions on Relay Servers you trust.
- **Malicious Client: hostile.** An attacker may try to use the relay as a
  reflection/amplification vector by spoofing a victim's source address, or to
  forward unrelated traffic through it. Defenses: protocol magic, room
  membership before any forwarding, scoped room/peer state, and a path-validation
  handshake on join (a peer must prove it can receive a token at the address it
  claims before the relay forwards anything to that address).

## Current Test Boundary

- The current bridge path is not end-to-end encrypted.
- A Relay Server can observe metadata such as source address, room name, packet size, direction, timing, and byte counts.
- Game packet payloads are treated as opaque by Basement Bridge, but the current test protocol does not make them confidential from the Relay Server.
- Room names or invite codes should be treated as bearer secrets.
- Diagnostics may contain SteamID64, relay address, room name, local paths, counters, and error text.
- Only use trusted Relay Servers for current testing.

## Phase 1 Requirements

- Reject packets without the Basement Bridge protocol magic.
- Enforce maximum packet sizes.
- Require a peer to join a room before forwarding room traffic.
- Validate a peer's claimed address with a join handshake (the relay forwards to an address only after that address echoes a join token) to block spoofed-source reflection.
- Forward packets only among peers in the same room.
- Reserve protocol-envelope fields for a nonce and sequence number from the start, so later AEAD and replay protection do not force a breaking protocol change.
- Expire inactive peers and rooms.
- Apply basic rate limits.
- Keep room and peer state scoped so the relay cannot act as a general-purpose UDP forwarder.
- Avoid logging raw game packet payloads.
- Make Official Mode avoid bridge launch and injection.
- Restore or clearly report local state after launch, injection, relay, or hook failures.
- Produce diagnostics bundles that are useful for debugging and avoid avoidable secrets.

## Public Release Goals

- End-to-end AEAD for relayed game traffic.
- Room or session key design that does not give the Relay Server plaintext access.
- Relay Server identity verification.
- Replay protection with packet sequence tracking and replay windows.
- Signed Directory Service metadata for trusted Relay Servers.
- Client and Relay Server protocol compatibility ranges.
- Directory Service relay revocation for compromised, outdated, or abusive Relay Servers.
- Public Relay Server abuse and privacy policy.
- Abuse mitigation such as proof-of-work, token buckets, or equivalent gating.
- Clear user documentation for why injection is needed and how to return to Official Mode.

## Abuse Controls

- Phase 1 uses protocol magic, packet size limits, room membership checks, timeouts, and basic rate limits.
- Phase 2 adds Relay Server local IP/CIDR blocklists, Room limits, Peer limits, and Room name length limits for obvious abuse during closed testing.
- Public release should support Directory Service relay revocation, but should avoid a global player IP blacklist unless there is a clear privacy and governance policy.
- Proof-of-work is a public-release hardening option, not a Phase 1 requirement.

## Diagnostics Redaction

Exported Diagnostics Bundles redact SteamID64-like values, Relay Server
endpoints, Room fields, and local user profile paths. This is a guardrail, not a
promise that every possible sensitive string has been removed. Closed-test logs
should still be shared privately.
