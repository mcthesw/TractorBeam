# Tractor Beam

[中文](README.md)

A desktop Client and Relay Server for improving online play in
*The Binding of Isaac: Repentance+*.

When official online play or a virtual LAN is not smooth enough, Tractor Beam
can move game data through a Relay while preserving normal Steam features.

## Project status

Maintainer time is limited, but we will do our best to fix well-defined bugs
and compatibility issues and review focused pull requests as time allows.
Community contributions remain welcome; see the
[contribution guide](CONTRIBUTING.md).

The Client supports Windows and Linux (Proton). The supported game version is
*Repentance+*. The Relay supports Windows, Linux, and macOS; formal Releases
provide Windows and Linux executables.

## Use the Client

1. Download the Client Bundle for your platform from the
   [latest release](https://github.com/mcthesw/TractorBeam/releases/latest) and
   extract the complete archive.
2. Keep the extracted files in the same directory. On Windows run
   `tractor-beam.exe`; on Linux run `tractor-beam`.
3. Select the Steam account and connection route. The host copies the Join
   Code, and the other players import it.
4. Select **Launch Game**. If something goes wrong, export a Diagnostics Bundle
   from the Client.

On Linux, the Client runs the Windows *Repentance+* build through Proton. It
writes a temporary `winmm.dll` proxy next to the game executable and temporarily
enables a Wine DLL override scoped to `isaac-ng.exe`. It restores the prior
override and removes the proxy when the session ends or the Client next starts.

The formal Client Bundle requires a self-configured Relay. A separate
public-test bundle is also available; its maintainer-funded public Relays are
intended for testing.

- [LAN Direct guide](docs/lan.en.md)
- [Relay self-deployment guide](docs/relay.en.md)

## Build

Install the Rust toolchain first.

```sh
# Build the Client
cargo build -p tractor-beam-gui

# Build the Relay Server
cargo build -p tractor-beam-relay

# Check and test
cargo check --workspace
cargo test --workspace
```

## Documentation

- [docs/architecture.en.md](docs/architecture.en.md): component boundaries and data flow.
- [docs/relay.en.md](docs/relay.en.md): Relay Server deployment.
- [docs/relay-configuration.en.md](docs/relay-configuration.en.md): Relay Server configuration.
- [docs/relay-observability.en.md](docs/relay-observability.en.md): Relay Server logs, metrics, traces, and capacity guidance.
- [docs/lan.en.md](docs/lan.en.md): LAN and virtual-LAN direct sessions.
- [docs/security.en.md](docs/security.en.md): security boundaries.
- [roadmap.en.md](roadmap.en.md): staged roadmap.
- [CONTRIBUTING.md](CONTRIBUTING.md): contribution guidance.

## License

Licensed by default under [GNU AGPL v3.0 or later](LICENSE). For alternative
licensing, commercial use, or exceptions, use the public contact information on
the author's GitHub profile.
