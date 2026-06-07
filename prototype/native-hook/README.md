# Native Hook

This is the validated Windows x86 hook used by the prototype.

It is intentionally narrow:

- targets 32-bit `isaac-ng.exe`,
- hooks selected Steam P2P calls from `SteamNetworking006`,
- keeps Steam lobby/friend flow outside the hook boundary,
- forwards opaque Isaac packet payloads to a local sidecar,
- reads bridge settings from:

```text
%USERPROFILE%\Documents\My Games\Binding of Isaac Repentance+\online_logs\isaac_bridge_config.txt
```

Build:

```powershell
cmake -S prototype/native-hook --preset x86-clang-rel
cmake --build prototype/native-hook/build/x86-clang-rel
```

The current C++ namespace and include path still use the prototype name
`eos_probe`. Avoid cosmetic renames until the Rust client has replaced the
prototype tooling.
