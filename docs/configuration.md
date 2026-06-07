# Configuration

## Relay Address

The public relay address belongs to the Bridge Client layer, not the Native
Hook.

Relay addresses use `host:port` form:

```text
<relay-host>:25910
```

Concrete relay addresses are intentionally not baked into the repository. Get
them from the relay operator out of band.

## Hook Config

The Native Hook only talks to the local Bridge Client. It reads:

```text
%USERPROFILE%\Documents\My Games\Binding of Isaac Repentance+\online_logs\isaac_bridge_config.txt
```

The Bridge Client writes values like:

```text
mode=replace
fallback_to_steam=0
sidecar=127.0.0.1:25900
bind=127.0.0.1:25901
```

- `sidecar` is the local hook-to-client endpoint.
- `bind` is the local client-to-hook endpoint.
- The public relay IP/host is not written here.

The Rust Bridge Client owns relay selection, room setup, and this hook config
file.
