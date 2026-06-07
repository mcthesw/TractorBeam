#!/usr/bin/env python3
"""Shared packet formats for the Isaac bridge relay tools."""

from __future__ import annotations

import dataclasses
import hashlib
import struct


LOCAL_MAGIC = b"IBR1"
RELAY_MAGIC = b"IBR2"
VERSION = 1
TYPE_REGISTER = 1
TYPE_DATA = 2
TYPE_HEARTBEAT = 3
TYPE_OUTGOING = 1
TYPE_INCOMING = 2

LOCAL_HEADER = struct.Struct("<4sBBHQIiiI")
RELAY_HEADER = struct.Struct("!4sBBH16sQQIiiI")


@dataclasses.dataclass(frozen=True)
class LocalPacket:
    packet_type: int
    peer: int
    sequence: int
    channel: int
    send_type: int
    payload: bytes


@dataclasses.dataclass(frozen=True)
class RelayPacket:
    packet_type: int
    room_id: bytes
    from_steam: int
    to_steam: int
    sequence: int
    channel: int
    send_type: int
    payload: bytes


def room_id(room: str) -> bytes:
    return hashlib.sha256(room.encode("utf-8")).digest()[:16]


def pack_local(packet: LocalPacket) -> bytes:
    header = LOCAL_HEADER.pack(
        LOCAL_MAGIC,
        VERSION,
        packet.packet_type,
        LOCAL_HEADER.size,
        packet.peer,
        packet.sequence,
        packet.channel,
        packet.send_type,
        len(packet.payload),
    )
    return header + packet.payload


def unpack_local(data: bytes) -> LocalPacket | None:
    if len(data) < LOCAL_HEADER.size:
        return None
    (
        magic,
        version,
        packet_type,
        header_size,
        peer,
        sequence,
        channel,
        send_type,
        payload_size,
    ) = LOCAL_HEADER.unpack_from(data)
    if magic != LOCAL_MAGIC or version != VERSION or header_size != LOCAL_HEADER.size:
        return None
    if len(data) < header_size + payload_size:
        return None
    payload = data[header_size : header_size + payload_size]
    return LocalPacket(packet_type, peer, sequence, channel, send_type, payload)


def pack_relay(packet: RelayPacket) -> bytes:
    header = RELAY_HEADER.pack(
        RELAY_MAGIC,
        VERSION,
        packet.packet_type,
        RELAY_HEADER.size,
        packet.room_id,
        packet.from_steam,
        packet.to_steam,
        packet.sequence,
        packet.channel,
        packet.send_type,
        len(packet.payload),
    )
    return header + packet.payload


def unpack_relay(data: bytes) -> RelayPacket | None:
    if len(data) < RELAY_HEADER.size:
        return None
    (
        magic,
        version,
        packet_type,
        header_size,
        rid,
        from_steam,
        to_steam,
        sequence,
        channel,
        send_type,
        payload_size,
    ) = RELAY_HEADER.unpack_from(data)
    if magic != RELAY_MAGIC or version != VERSION or header_size != RELAY_HEADER.size:
        return None
    if len(data) < header_size + payload_size:
        return None
    payload = data[header_size : header_size + payload_size]
    return RelayPacket(
        packet_type,
        rid,
        from_steam,
        to_steam,
        sequence,
        channel,
        send_type,
        payload,
    )
