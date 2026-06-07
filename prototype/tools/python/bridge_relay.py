#!/usr/bin/env python3
"""UDP relay for the Isaac Steam P2P bridge experiment."""

from __future__ import annotations

import argparse
import collections
import socket
import time

from bridge_protocol import (
    TYPE_DATA,
    TYPE_HEARTBEAT,
    TYPE_REGISTER,
    RelayPacket,
    pack_relay,
    unpack_relay,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bind", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=25910)
    parser.add_argument("--peer-timeout", type=float, default=30.0)
    args = parser.parse_args()

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind((args.bind, args.port))
    sock.settimeout(1.0)

    peers: dict[tuple[bytes, int], tuple[tuple[str, int], float]] = {}
    counters = collections.Counter()
    last_report = time.monotonic()

    print(f"bridge relay listening on {args.bind}:{args.port}", flush=True)
    while True:
        now = time.monotonic()
        try:
            data, address = sock.recvfrom(65535)
        except socket.timeout:
            data = b""
            address = ("", 0)

        if data:
            packet = unpack_relay(data)
            if packet is None:
                counters["bad_packet"] += 1
            elif packet.packet_type in {TYPE_REGISTER, TYPE_HEARTBEAT}:
                peers[(packet.room_id, packet.from_steam)] = (address, now)
                counters["register"] += 1
            elif packet.packet_type == TYPE_DATA:
                peers[(packet.room_id, packet.from_steam)] = (address, now)
                target = peers.get((packet.room_id, packet.to_steam))
                if target is None:
                    counters["missing_target"] += 1
                else:
                    sock.sendto(data, target[0])
                    counters["forward"] += 1
                    counters["bytes"] += len(packet.payload)

        expired = [
            key
            for key, (_, seen_at) in peers.items()
            if now - seen_at > args.peer_timeout
        ]
        for key in expired:
            peers.pop(key, None)

        if now - last_report >= 5.0:
            print(
                "stats "
                f"peers={len(peers)} "
                f"forward={counters['forward']} "
                f"missing={counters['missing_target']} "
                f"bytes={counters['bytes']}",
                flush=True,
            )
            last_report = now


if __name__ == "__main__":
    raise SystemExit(main())
