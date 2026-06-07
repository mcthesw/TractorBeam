#!/usr/bin/env python3
"""Local sidecar that connects the Isaac hook DLL to a UDP relay."""

from __future__ import annotations

import argparse
import collections
import selectors
import socket
import time

from bridge_protocol import (
    TYPE_DATA,
    TYPE_HEARTBEAT,
    TYPE_INCOMING,
    TYPE_OUTGOING,
    TYPE_REGISTER,
    LocalPacket,
    RelayPacket,
    pack_local,
    pack_relay,
    room_id,
    unpack_local,
    unpack_relay,
)


def parse_endpoint(value: str) -> tuple[str, int]:
    host, port = value.rsplit(":", 1)
    return host, int(port)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--room", required=True)
    parser.add_argument("--steam-id", required=True, type=int)
    parser.add_argument("--relay", required=True, help="host:port")
    parser.add_argument("--hook-in", default="127.0.0.1:25900")
    parser.add_argument("--hook-out", default="127.0.0.1:25901")
    parser.add_argument("--heartbeat", type=float, default=1.0)
    args = parser.parse_args()

    rid = room_id(args.room)
    relay_address = parse_endpoint(args.relay)
    hook_in_address = parse_endpoint(args.hook_in)
    hook_out_address = parse_endpoint(args.hook_out)

    hook_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    hook_sock.bind(hook_in_address)
    hook_sock.setblocking(False)

    relay_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    relay_sock.setblocking(False)

    selector = selectors.DefaultSelector()
    selector.register(hook_sock, selectors.EVENT_READ, "hook")
    selector.register(relay_sock, selectors.EVENT_READ, "relay")

    counters = collections.Counter()
    sequence = 1
    last_heartbeat = 0.0
    last_report = time.monotonic()

    def send_register(packet_type: int) -> None:
        packet = RelayPacket(
            packet_type,
            rid,
            args.steam_id,
            0,
            0,
            0,
            0,
            b"",
        )
        relay_sock.sendto(pack_relay(packet), relay_address)

    print(
        f"sidecar steam_id={args.steam_id} room={args.room!r} "
        f"relay={args.relay} hook_in={args.hook_in} hook_out={args.hook_out}",
        flush=True,
    )

    while True:
        now = time.monotonic()
        if now - last_heartbeat >= args.heartbeat:
            send_register(TYPE_REGISTER if last_heartbeat == 0.0 else TYPE_HEARTBEAT)
            last_heartbeat = now

        for key, _ in selector.select(timeout=0.05):
            if key.data == "hook":
                data, _ = hook_sock.recvfrom(65535)
                packet = unpack_local(data)
                if packet is None or packet.packet_type != TYPE_OUTGOING:
                    counters["bad_hook_packet"] += 1
                    continue
                relay_packet = RelayPacket(
                    TYPE_DATA,
                    rid,
                    args.steam_id,
                    packet.peer,
                    packet.sequence,
                    packet.channel,
                    packet.send_type,
                    packet.payload,
                )
                relay_sock.sendto(pack_relay(relay_packet), relay_address)
                counters["hook_to_relay"] += 1
                counters["sent_bytes"] += len(packet.payload)
            elif key.data == "relay":
                data, _ = relay_sock.recvfrom(65535)
                packet = unpack_relay(data)
                if packet is None or packet.packet_type != TYPE_DATA:
                    counters["bad_relay_packet"] += 1
                    continue
                local_packet = LocalPacket(
                    TYPE_INCOMING,
                    packet.from_steam,
                    sequence,
                    packet.channel,
                    packet.send_type,
                    packet.payload,
                )
                sequence += 1
                hook_sock.sendto(pack_local(local_packet), hook_out_address)
                counters["relay_to_hook"] += 1
                counters["recv_bytes"] += len(packet.payload)

        if now - last_report >= 5.0:
            print(
                "stats "
                f"hook_to_relay={counters['hook_to_relay']} "
                f"relay_to_hook={counters['relay_to_hook']} "
                f"sent_bytes={counters['sent_bytes']} "
                f"recv_bytes={counters['recv_bytes']} "
                f"bad_hook={counters['bad_hook_packet']} "
                f"bad_relay={counters['bad_relay_packet']}",
                flush=True,
            )
            last_report = now


if __name__ == "__main__":
    raise SystemExit(main())
