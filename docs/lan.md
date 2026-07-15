# Direct LAN sessions

Direct LAN is for players who are already mutually reachable through a physical
LAN or a third-party virtual LAN. It does not contact a Tractor Beam Relay,
discover public Internet peers, traverse NAT, or fall back to a Relay.

## Player flow

1. Select a Steam account and choose **LAN Direct**. Press **Copy** and review
   the adapter list. Adapters with at least one non-link-local address are
   selected by default; link-local-only fallback adapters remain available for
   unusual networks.
2. The Client compacts each selected adapter to its best IPv4 and IPv6 address,
   gives every selected adapter one candidate before adding a second address,
   and stays inside the bounded eight-candidate protocol budget. It then binds
   TCP control and UDP gameplay sockets, ignores individual addresses that the
   operating system no longer considers available, and copies a Join Code only
   after at least one address binds. If none bind, no room or code is created.
3. Another player imports the code. If one advertised address is reachable it
   is used automatically; if several are reachable, the player chooses the
   initial entry point. That choice is only an admission preference.
4. After admission, every Peer learns the other Peers and establishes an
   independent direct UDP Peer Path. Every Peer may copy a fresh Join Code that
   names itself as Introducer. The original Room Creator is not an authority and
   may leave without ending the Room.

External Relay and Direct LAN are mutually exclusive. Stop the current session
before switching routes.

## Network and security boundary

- Join Codes disclose the selected local/virtual adapter IP addresses and a
  temporary Session Credential. Share them only with intended players.
- Control and gameplay traffic are plaintext. The Session Credential limits
  admission but does not encrypt traffic or authenticate a human identity. Use
  only a trusted LAN or virtual LAN.
- Host candidates only are supported. There is no STUN, TURN, public
  rendezvous, port mapping, embedded Relay, or Relay-assisted recovery.
- The operating-system firewall must allow the Client's dynamically bound TCP
  and UDP sockets on the selected network profiles. A probe that finds no
  reachable address usually means routing or firewall policy blocks it.
- Gameplay packets are addressed to one Peer, bounded, validated against the
  nominated path, and dropped while that path is unavailable. They are never
  queued for replay after recovery.

Diagnostics report bind, probe, admission, path, and gameplay stages plus the
nominated endpoint pair for each Peer. Exported bundles redact IP addresses and
never include the Session Credential, path identifier, or path token.
