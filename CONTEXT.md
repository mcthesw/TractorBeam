# Tractor Beam Context

Tractor Beam is a remote co-op transport project for *The Binding of Isaac:
Repentance+*. This context defines the project language used in code, roadmap,
and user-facing documentation.

## Language

**Official Mode**:
A session mode that leaves the game on its normal Steam online path.
_Avoid_: vanilla mode, Steam mode

**Fallback Mode**:
A session mode that uses Tractor Beam transport while allowing Steam receive fallback.
_Avoid_: Bridge Safe Mode, hybrid mode, safe mode

**Pure Mode**:
A session mode that uses only Tractor Beam transport for the hooked packet path.
_Avoid_: Bridge Pure Mode, forced mode, relay-only mode

**Bridge Client**:
The local runtime that coordinates a player's Tractor Beam session without owning presentation.
_Avoid_: sidecar, GUI, app

**Bridge GUI**:
The desktop presentation layer used by players to configure and control a Tractor Beam session.
_Avoid_: client, sidecar

**Bridge Core**:
The Rust code crate that contains the Bridge Client runtime, diagnostics, Steam detection, and local configuration helpers. It consumes shared protocol crates but does not own their wire contracts.
_Avoid_: GUI, relay, hook

**Protocol**:
The versioned message language and packet formats at one component boundary. Local IPC Protocol is shared by the Bridge Client and Native Hook; Relay Protocol is shared by the Bridge Client and Relay Server. They are independent contracts. Protocol describes what is sent, not how it is carried.
_Avoid_: transport, runtime, socket loop

**Local IPC Protocol**:
The typed `TBI2` message contract used only between one Bridge Client session and its Native Hook over a Local Socket/Windows Named Pipe.
_Avoid_: Relay Protocol, UDP protocol, socket loop

**Relay Protocol v2**:
The `TBR2` compatibility bootstrap, control messages, and bounded binary data/probe frames shared by the Bridge Client and Relay Server.
_Avoid_: Local IPC Protocol, Relay Transport, room state

**Direct Protocol**:
The versioned, bounded control, path-check, and gameplay-frame contract exchanged directly between two Peers in a LAN Session.
_Avoid_: Relay Protocol, LAN discovery policy, adapter selection

**Relay Transport**:
The network carriage selected to move Protocol envelopes between a Bridge Client and a Relay Server.
_Avoid_: protocol, Relay Server, session mode

**Transport Choice**:
The single Relay Transport selected by one Bridge Client session.
_Avoid_: session mode, automatic fallback

**Input Delay**:
A player-controlled Isaac remote co-op delay value used to make remote inputs
feel stable under Relay Server latency.
_Avoid_: onlineInputDelay, manager offset

**Client Bundle**:
The versioned player-facing package that ships the Bridge GUI, Bridge Client, Native Hook, and Injector together.
_Avoid_: hook release, GUI release

**Native Hook**:
The in-process component that redirects Isaac's Steam packet path into Tractor Beam.
_Avoid_: mod, plugin

**Injector**:
The local component that places the Native Hook into the Isaac process.
_Avoid_: launcher

**Relay Server**:
The public server that forwards Tractor Beam room traffic between joined peers.
_Avoid_: server, directory

**Directory Service**:
The authority that publishes trusted Relay Server metadata.
_Avoid_: relay, server list

**Session Credential**:
A high-entropy bearer secret that identifies one Room and authorizes Peers to join it.
_Avoid_: room name, admission, session key

**Room**:
The player-facing co-op group whose peers share one Session Credential; it has no separate user-editable name.
_Avoid_: room name, relay registry key, lobby code

**Room Path Quality**:
The measured round-trip latency, variation, and probe loss between two Bridge Clients over their selected gameplay path.
_Avoid_: Relay latency, game packet loss, connection score

**Join Code**:
The single player-shareable value that carries one Room entry point and one Session Credential.
_Avoid_: room code, invite code, admission code

**Peer**:
A player endpoint admitted to one Room with a Session Credential.
_Avoid_: user, member

**LAN Session**:
A session whose Peers are already mutually IP-reachable through a physical LAN or third-party virtual LAN.
_Avoid_: Internet P2P, automatic NAT traversal

**Room Creator**:
The Peer that creates a LAN Session by generating its Session Credential and first LAN Join Code; it has no special runtime authority.
_Avoid_: Room Host, game host, coordinator

**Introducer**:
The Peer named by a LAN Join Code that supplies one joining Peer with the Room's current Peer-discovery information.
_Avoid_: Room Host, Relay Server, leader

**Peer Path**:
The nominated direct gameplay path between two Peers in a LAN Session.
_Avoid_: Relay path, room entry point

**Diagnostics Bundle**:
A support artifact containing run logs, environment facts, counters, and errors.
_Avoid_: log zip, report

**Readiness Preflight**:
A startup check that verifies the local Bridge path is ready before gameplay,
including configuration, injection, Native Hook initialization, and local
receive endpoints.
_Avoid_: health check, network test

**Incident Snapshot**:
A compact diagnostics record captured when the Bridge Client or Relay Server
observes an abnormal data-plane condition.
_Avoid_: crash report, trace dump

**Advanced Transport**:
An optional non-baseline transport layer for hostile networks or later hardening.
_Avoid_: normal relay mode

## Relationships

- A **Client Bundle** contains one **Bridge GUI**, one **Bridge Client**, one **Native Hook**, and one **Injector**.
- **Bridge Core** provides the code used by the **Bridge GUI** to control a **Bridge Client** session.
- A **Bridge Client** and **Native Hook** exchange **Local IPC Protocol** messages.
- A **Bridge Client** and **Relay Server** exchange **Relay Protocol v2** messages.
- Two **Bridge Clients** exchange **Direct Protocol** messages over one **Peer Path** in a **LAN Session**.
- A **Bridge GUI** controls a **Bridge Client**.
- A **Bridge Client** joins at most one **Room** using one **Session Credential** per active session.
- A **LAN Session** is initially created by exactly one **Room Creator**.
- Every admitted **Peer** is equal and may create a **Join Code** naming itself as **Introducer**.
- An **Introducer** assists discovery for one join and gains no control over membership or gameplay.
- A **LAN Session** carries baseline gameplay traffic over pairwise **Peer Paths**.
- **LAN Session** membership may change while each **Peer Path** becomes usable or recovers independently.
- Selecting an **Introducer** endpoint establishes the initial join path and a local candidate preference; it does not pin the **LAN Session** to one adapter.
- Every **Peer Path** nominates and recovers its direct candidate pair independently; different paths may use different local adapters.
- Established **Peer Paths** do not depend on the original **Room Creator** or any particular **Introducer**.
- A **Bridge Client** uses one **Transport Choice** to exchange **Protocol** envelopes with a **Relay Server** during an external Relay session.
- **Input Delay** is adjusted through the **Bridge GUI** and applied by the
  **Native Hook** when Isaac is ready.
- A **Relay Server** forwards packets only among **Peers** admitted with the same **Session Credential**.
- A **Join Code** contains exactly one Relay or LAN discovery entry point and one **Session Credential**; one LAN entry point may carry a bounded set of endpoint candidates for the same **Introducer**.
- **Room Path Quality** describes one Bridge Client's selected gameplay path to another **Peer**, whether direct or relayed.
- Creating a new **Room** rotates the **Session Credential**; stopping and restarting the same local session does not.
- A **Directory Service** publishes metadata about one or more **Relay Servers**.
- A **Diagnostics Bundle** describes one local **Bridge Client** run.
- A **Readiness Preflight** runs before a player treats a **Bridge Client**
  session as playable.
- An **Incident Snapshot** may be included in a **Diagnostics Bundle**.
- A future FEC or redundancy design would be an **Advanced Transport** and
  **Transport Choice**, not a UDP profile.

## Example Dialogue

> **Dev:** "Should the **Bridge GUI** reconnect the relay socket?"
> **Domain expert:** "No. The **Bridge GUI** asks the **Bridge Client** to start or stop a session; reconnect behavior belongs to the **Bridge Client**."

## Flagged Ambiguities

- "client" was used to mean both the user-facing application and the local bridge runtime. Resolved: use **Bridge GUI** for presentation, **Bridge Client** for runtime, and **Client Bundle** for the versioned package.
- "sidecar" was used for the early local bridge process. Resolved: use **Bridge Client** for the product term.
- "server" was used for both relay forwarding and future trusted server discovery. Resolved: use **Relay Server** for forwarding and **Directory Service** for trusted metadata.
- "protocol" and "transport" were used interchangeably. Resolved: use **Protocol** for message formats and **Relay Transport** for network carriage.
- One "Protocol" previously implied one format shared by all three runtime components. Resolved: use **Local IPC Protocol** for Bridge Client/Native Hook and **Relay Protocol v2** for Bridge Client/Relay Server.
- "mode" was used for both session behavior and network carriage. Resolved: use **Official Mode**, **Fallback Mode**, and **Pure Mode** for session behavior; use **Transport Choice** for UDP or TCP carriage.
- "room", "room name", "admission", and "join code" described overlapping parts of one player-visible join flow. Resolved: **Room** remains the player-facing co-op group, but it has no separately editable name; use **Session Credential** for its single high-entropy routing/admission secret and **Join Code** for the value players copy or import.
- "host" can mean the Isaac game/lobby host or a Tractor Beam discovery entry point. Resolved: avoid **Room Host**; use **Room Creator** only for room creation and **Introducer** for the Peer named by a particular LAN Join Code.
