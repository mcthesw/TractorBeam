# Security

Tractor Beam is a work-in-progress transport moving toward a Windows
baseline. This document records the security boundary before wider distribution.

## Threat Model

Who Tractor Beam trusts, and how much:

- **Same-room Peer: semi-trusted.** A peer who joined your Room is trusted to
  play, not to behave. Bridge treats game payloads as opaque and does not
  validate Isaac semantics, so a peer can send any game-layer packet. The line
  Bridge does enforce: a peer cannot make a Relay Server forward traffic outside
  its Room, and cannot turn the relay into a general-purpose UDP forwarder toward
  third parties.
- **Relay Server: trusted with content, not availability.** The relay can observe,
  modify, delay, or drop control and Isaac packets. Tractor Beam deliberately
  provides no confidentiality or on-path integrity guarantee, so players should
  use Relay Servers whose operators they trust. Client recovery and diagnostics
  must still handle Relay failure or misbehavior without corrupting local state.
- **Malicious Client: hostile.** An attacker may try to use the relay as a
  reflection/amplification vector by spoofing a victim's source address, or to
  forward unrelated traffic through it. Defenses: protocol magic, room
  membership before any forwarding, scoped room/peer state, and a path-validation
  handshake on join (a peer must prove it can receive a token at the address it
  claims before the relay forwards anything to that address).

## Accepted Network Boundary

- Relay Protocol v2 uses plaintext TCP control and TCP/UDP data. It does not use
  TLS, PAKE, AEAD, or a payload MAC.
- A Relay Server and an on-path observer can see control messages, Session
  Credentials, Steam identities, member state, packet payloads, sizes,
  directions, and timing, and an active intermediary can modify traffic.
- Session Credentials are high-entropy bearer values that resist guessing; they
  are not secret from the Relay/network path and do not authenticate either
  endpoint against an active intermediary.
- Resume credentials and UDP path tokens prevent accidental/off-path binding;
  they do not claim on-path attack resistance.
- A future encrypted design, if ever justified by a new threat model, requires a
  separate Relay Protocol v3. V2 does not reserve nonce, tag, key epoch, or
  session-key fields for it.
- Diagnostics may contain redacted Steam/Relay/member metadata, local paths,
  counters, and error text, but never Session Credentials or recovery/path
  tokens.
- Session, resume, and path-validation credentials must never be written to
  normal logs, metrics, or Troubleshooting Packages.
- Only use trusted Relay Servers.

## Phase 1 Requirements

- Reject packets without the Tractor Beam protocol magic.
- Enforce maximum packet sizes.
- Require a peer to join a room before forwarding room traffic.
- Validate a peer's claimed address with a join handshake (the relay forwards to an address only after that address echoes a join token) to block spoofed-source reflection.
- Forward packets only among peers in the same room.
- Use bounded packet identifiers for diagnostics, ordering evidence, and
  duplicate suppression; do not describe them as cryptographic replay
  protection.
- Expire inactive peers and rooms.
- Apply basic rate limits.
- Keep room and peer state scoped so the relay cannot act as a general-purpose UDP forwarder.
- Avoid logging raw game packet payloads.
- Make Official Mode avoid bridge launch and injection.
- Restore or clearly report local state after launch, injection, relay, or hook failures.
- Produce diagnostics bundles that are useful for debugging and avoid avoidable secrets.

## Public Release Goals

- Signed Directory Service metadata for trusted Relay Servers.
- Client and Relay Server protocol compatibility ranges.
- Directory Service relay revocation for compromised, outdated, or abusive Relay Servers.
- Public Relay Server abuse and privacy policy.
- Abuse mitigation such as proof-of-work, token buckets, or equivalent gating.
- Clear user documentation for why injection is needed and how to return to Official Mode.
- Clear user documentation that Relay/control/game traffic is plaintext and
  trusted-Relay operation is the intended model.

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
