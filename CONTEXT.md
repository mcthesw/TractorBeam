# Basement Bridge Context

Basement Bridge is a remote co-op transport project for *The Binding of Isaac:
Repentance+*. This context defines the project language used in code, roadmap,
and user-facing documentation.

## Language

**Official Mode**:
A session mode that leaves the game on its normal Steam online path.
_Avoid_: vanilla mode, Steam mode

**Fallback Mode**:
A session mode that uses Basement Bridge transport while allowing Steam receive fallback.
_Avoid_: Bridge Safe Mode, hybrid mode, safe mode

**Pure Mode**:
A session mode that uses only Basement Bridge transport for the hooked packet path.
_Avoid_: Bridge Pure Mode, forced mode, relay-only mode

**Bridge Client**:
The local runtime that coordinates a player's Basement Bridge session without owning presentation.
_Avoid_: sidecar, GUI, app

**Bridge GUI**:
The desktop presentation layer used by players to configure and control a Basement Bridge session.
_Avoid_: client, sidecar

**Bridge Core**:
The Rust code crate that contains the Bridge Client runtime, protocol, diagnostics, Steam detection, and local configuration helpers.
_Avoid_: GUI, relay, hook

**Client Bundle**:
The versioned player-facing package that ships the Bridge GUI, Bridge Client, Native Hook, and Injector together.
_Avoid_: hook release, GUI release

**Native Hook**:
The in-process component that redirects Isaac's Steam packet path into Basement Bridge.
_Avoid_: mod, plugin

**Injector**:
The local component that places the Native Hook into the Isaac process.
_Avoid_: launcher

**Relay Server**:
The public server that forwards Basement Bridge room traffic between joined peers.
_Avoid_: server, directory

**Directory Service**:
The authority that publishes trusted Relay Server metadata.
_Avoid_: relay, server list

**Room**:
A named session scope that decides which peers may exchange relayed packets.
_Avoid_: lobby

**Peer**:
A player endpoint that has joined a Room through a Relay Server.
_Avoid_: user, member

**Diagnostics Bundle**:
A support artifact containing run logs, environment facts, counters, and errors.
_Avoid_: log zip, report

**Advanced Transport**:
An optional non-baseline transport layer for hostile networks or later hardening.
_Avoid_: normal relay mode

## Relationships

- A **Client Bundle** contains one **Bridge GUI**, one **Bridge Client**, one **Native Hook**, and one **Injector**.
- **Bridge Core** provides the code used by the **Bridge GUI** to control a **Bridge Client** session.
- A **Bridge GUI** controls a **Bridge Client**.
- A **Bridge Client** joins at most one **Room** on one **Relay Server** per active session.
- A **Relay Server** forwards packets only among **Peers** in the same **Room**.
- A **Directory Service** publishes metadata about one or more **Relay Servers**.
- A **Diagnostics Bundle** describes one local **Bridge Client** run.

## Example Dialogue

> **Dev:** "Should the **Bridge GUI** reconnect the relay socket?"
> **Domain expert:** "No. The **Bridge GUI** asks the **Bridge Client** to start or stop a session; reconnect behavior belongs to the **Bridge Client**."

## Flagged Ambiguities

- "client" was used to mean both the user-facing application and the local bridge runtime. Resolved: use **Bridge GUI** for presentation, **Bridge Client** for runtime, and **Client Bundle** for the versioned package.
- "sidecar" was used for the early local bridge process. Resolved: use **Bridge Client** for the product term.
- "server" was used for both relay forwarding and future trusted server discovery. Resolved: use **Relay Server** for forwarding and **Directory Service** for trusted metadata.
