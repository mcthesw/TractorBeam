# Prototype Tools

These tools are copied from the validated prototype so the repository keeps the
working path while Rust crates are implemented.

- `python/bridge_protocol.py`: current local and relay packet formats.
- `python/bridge_sidecar.py`: local sidecar between hook and relay.
- `python/bridge_relay.py`: public UDP relay prototype.
- `windows/*.ps1`: launch, sidecar, injection, and log helper scripts.

Do not treat these scripts as the final UX. The Rust client should replace
them with a single managed application.
