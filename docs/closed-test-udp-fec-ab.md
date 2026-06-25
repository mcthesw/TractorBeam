# Closed-Test UDP FEC A/B Script

Use this after the startup preflight succeeds on both machines. Keep the relay,
room, mode, and players unchanged between runs.

## Profiles

1. `UDP`
2. `UDP + FEC (experimental)`

`UDP + FEC` is still the UDP transport with `[udp_fec] enabled for the next
session. It is not a third relay transport.

## Steps

1. Both players select the same relay, room, mode, and connection profile.
2. Run the startup preflight. Continue only if injection and `25901/UDP`
   readiness are clear on both machines.
3. Play 3-5 minutes or one short room using plain `UDP`.
4. Stop the session, export or upload Diagnostics Bundles from both machines.
5. Switch both clients to `UDP + FEC (experimental)`.
6. Restart the game/session so the profile change takes effect.
7. Play the same 3-5 minute route or a comparable short room.
8. Stop the session, export or upload Diagnostics Bundles from both machines.

## Stop Conditions

Stop the current run and upload diagnostics if any of these occur:

- A player cannot pass injection or `25901/UDP` readiness.
- The game disconnects before meaningful gameplay starts.
- Runtime RTT timeouts become nonzero.
- Queue drops become nonzero.
- Sequence gaps keep increasing during active gameplay.
- The FEC diagnostics show high `oversized_passthrough_packets`, because the
  selected profile is not protecting many game datagrams.

## Compare

Compare both sides for:

- Runtime RTT p95 and timeouts.
- Queue drops.
- Source sequence gaps and duplicate or reordered packets.
- Hook out duration p95.
- UDP FEC repair packets, recovered packets, unrecovered groups, decode delay
  p95, and oversized passthrough packets.
- Relay room stats for the same room and time window.
