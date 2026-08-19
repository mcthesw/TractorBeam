# LAN Direct

[中文](lan.md)

LAN Direct is for players who can already reach one another through a physical
LAN or a third-party virtual LAN. It does not connect to a Relay and provides
no public discovery or NAT traversal.

## How to use it

1. The host selects a Steam account and **LAN Direct**, chooses **New Room**,
   selects the network adapters that can reach the other players, and copies
   the Join Code.
2. The other players import the Join Code. If several addresses are reachable,
   select the entry point to use.
3. Start the game after everyone has entered the Room. A player remains in the
   Room until selecting **Leave Room** or closing the Client.

The host leaving does not automatically end the Room for everyone else. Any
player in the Room can copy a new Join Code to invite another player.

## Network and security

- All players must already be mutually reachable through the selected LAN or
  virtual LAN.
- The firewall must allow the Client to use TCP and UDP on the selected network.
- A Join Code contains local or virtual-adapter addresses and a temporary Room
  credential. Share it only with the people who will play with you.
- Control and game traffic are plaintext, so use LAN Direct only on a trusted
  network.
- LAN Direct has no STUN, TURN, port mapping, or automatic Relay fallback. If a
  connection fails, check the virtual LAN, routing, and firewall settings first.

If something goes wrong, export a Diagnostics Bundle from the Client and share
it through a private channel.
