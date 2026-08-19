# Security

[中文](security.md)

Both Tractor Beam Relay and LAN Direct use plaintext transport. Only use a
Relay or network you trust, and share Join Codes only with the people who will
play with you.

## Trust boundaries

- Players in the same Room can send arbitrary game data. Tractor Beam limits
  where that data is forwarded, but does not decide whether the game content is
  valid.
- A Relay can read, modify, delay, or drop control messages and game data that
  pass through it, so the Relay operator must be trusted.
- Credentials in a Join Code restrict Room admission. They do not encrypt
  traffic or prove the real identity of the person using them.

## Current protections

The Relay checks the protocol, packet size, proof-of-work, Room membership, and
sending path, and forwards traffic only between players in the same Room. It
also expires old state and applies rate limits, Room limits, and an IP/CIDR
blocklist so it cannot be used as an arbitrary traffic forwarder.

These protections limit abuse and accidental traffic; they are not encryption.
Players who require confidentiality should not rely on the current Relay
protocol.

## Logs and Diagnostics Bundles

Session Credentials, recovery credentials, and path tokens are not written to
normal logs or Diagnostics Bundles. Exported Diagnostics Bundles redact Steam
IDs, Relay addresses, local paths, and similar information, but may still
contain device state and error details. Share them only through a private
channel.

## LAN Direct

LAN Direct includes selected local or virtual-adapter addresses in its Join
Code and uses plaintext TCP/UDP traffic. It provides no public discovery, NAT
traversal, or automatic Relay fallback, and should be used only on a trusted
physical or virtual LAN.
