# Roadmap

[中文](roadmap.md)

The Windows + Steam baseline and player-facing test goals are complete. Future
work will prioritize keeping the current experience stable, while focused
improvements and longer-term directions remain open to community involvement.

## Completed baseline

- [x] Ship a Windows Client Bundle containing the Bridge GUI, Bridge Client,
  Native Hook, and Injector.
- [x] Support Steam + *The Binding of Isaac: Repentance+* with Official Mode,
  Fallback Mode, and Pure Mode.
- [x] Support external Relay and LAN Direct routes with selectable TCP/UDP
  Relay Transport.
- [x] Provide a self-deployable Relay Server with basic abuse limits, logs,
  metrics, traces, and operations documentation.
- [x] Provide Diagnostics Bundles, log redaction, player-facing errors, and
  recovery guidance for common failures.
- [x] Publish GitHub Release assets, maintain the Release Please flow, and
  verify that the Client Bundle runs on a clean Windows device.

## Current priorities

- Fix reproducible critical bugs, security issues, and compatibility
  regressions.
- Keep the Windows + Steam baseline, Relay self-deployment, and existing
  protocol paths maintainable.
- Improve player documentation, diagnostic evidence, and small, focused user
  experience issues.
- Review well-bounded, well-validated community contributions as time allows.

Read the [contribution guide](CONTRIBUTING.md) before contributing.

## Future directions

- A Directory Service, signed Relay metadata, revocation, and trust
  publication.
- Minimum and maximum Client/Relay protocol version policies.
- A long-term public Relay Server policy.
- Bounded UDP duplicate-send/deduplication, hop-by-hop FEC, and measurement of
  their bandwidth, tail-latency, and Relay CPU costs.
- Linux/Proton, non-Steam support, and installer packaging.
- Dynamic Input Delay and additional connection-quality visualization.

These directions are not current priorities. Interested contributors are
welcome to discuss scope and validation in an Issue. Large changes to
protocols, Relay data paths, the Native Hook, or the Injector should also start
with an Issue that aligns the scope before implementation.

Payload encryption remains out of scope.
