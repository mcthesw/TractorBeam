# Basement Bridge

[中文](README.md)

A desktop client and relay transport for *The Binding of Isaac: Repentance+*
online play.

Official online play can be very laggy, and virtual LAN setups are often still
not smooth enough. Basement Bridge introduces a non-P2P path: it keeps the
normal Steam version features intact while moving game data transport onto a
server relay for better transmission quality.

It is still a work in progress, Windows + Steam only.

## Build

```sh
cargo check --workspace
cargo test --workspace
```

## Docs

- [docs/architecture.md](docs/architecture.md): component boundaries and data flow.
- [docs/closed-testing.md](docs/closed-testing.md): closed test flow and feedback template.
- [docs/relay.md](docs/relay.md): Relay Server deployment.
- [docs/diagnostics.md](docs/diagnostics.md): diagnostics export and redaction rules.
- [docs/security.md](docs/security.md): threat model and security boundary.
- [roadmap.md](roadmap.md): staged roadmap.

## License

Licensed by default under [GNU AGPL v3.0 or later](LICENSE). For alternative
licensing, commercial use, or exceptions, use the public contact information on
the author's GitHub profile.
