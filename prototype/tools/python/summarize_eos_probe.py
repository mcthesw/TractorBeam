#!/usr/bin/env python3
"""Summarize isaac_eos_probe JSONL traces."""

from __future__ import annotations

import argparse
import collections
import json
import statistics
from pathlib import Path
from typing import Iterable


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = int(round((pct / 100.0) * (len(ordered) - 1)))
    return ordered[index]


def format_ms(value: float) -> str:
    return f"{value / 1000.0:.3f} ms"


def load_events(path: Path) -> Iterable[dict]:
    with path.open("r", encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError as exc:
                raise SystemExit(f"{path}:{line_number}: invalid JSON: {exc}") from exc


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("trace", type=Path)
    args = parser.parse_args()

    events = list(load_events(args.trace))
    if not events:
        print("No events.")
        return 1

    by_event = collections.Counter(event.get("event", "?") for event in events)
    print("Events")
    for name, count in by_event.most_common():
        print(f"  {name}: {count}")

    for event_name in (
        "send",
        "recv",
        "next_size",
        "steam_send",
        "steam_available",
        "steam_recv",
    ):
        subset = [event for event in events if event.get("event") == event_name]
        if not subset:
            continue

        channels = collections.Counter(event.get("channel", "?") for event in subset)
        sizes = collections.Counter(event.get("bytes", 0) for event in subset)
        results = collections.Counter(event.get("result", "?") for event in subset)

        print()
        print(event_name)
        print("  results:", dict(results.most_common()))
        if event_name in ("send", "recv", "steam_send", "steam_available", "steam_recv"):
            print("  channels:", dict(channels.most_common(12)))
        if event_name == "steam_send":
            send_types = collections.Counter(
                event.get("send_type", "?") for event in subset
            )
            print("  send types:", dict(send_types.most_common()))
        peers = collections.Counter(
            event.get("peer", "?") for event in subset if "peer" in event
        )
        if peers:
            print("  peers:", dict(peers.most_common(12)))
        print("  top sizes:", dict(sizes.most_common(12)))

        timestamps = [
            int(event["ts_us"])
            for event in subset
            if isinstance(event.get("ts_us"), int)
        ]
        gaps = [
            later - earlier
            for earlier, later in zip(timestamps, timestamps[1:])
            if later >= earlier
        ]
        if gaps:
            print("  gap p50:", format_ms(statistics.median(gaps)))
            print("  gap p95:", format_ms(percentile(gaps, 95)))
            print("  gap p99:", format_ms(percentile(gaps, 99)))
            print("  gap max:", format_ms(max(gaps)))
            print("  gaps >50ms:", sum(1 for gap in gaps if gap > 50_000))
            print("  gaps >100ms:", sum(1 for gap in gaps if gap > 100_000))

    steam_interfaces = [
        str(event.get("version", ""))
        for event in events
        if event.get("event") == "steam_interface"
    ]
    if steam_interfaces:
        print()
        print("steam interfaces")
        for version, count in collections.Counter(steam_interfaces).most_common():
            marker = ""
            lowered = version.lower()
            if "networking" in lowered or "socket" in lowered:
                marker = "  <-- networking candidate"
            print(f"  {version}: {count}{marker}")

    if any(event.get("event") == "steam_send" for event in events):
        print()
        print("steam send type legend")
        print("  0: k_EP2PSendUnreliable")
        print("  1: k_EP2PSendUnreliableNoDelay")
        print("  2: k_EP2PSendReliable")
        print("  3: k_EP2PSendReliableWithBuffering")

    eos_calls = [
        str(event.get("event", ""))
        for event in events
        if str(event.get("event", "")).startswith("eos_")
    ]
    if eos_calls:
        print()
        print("eos calls")
        for name, count in collections.Counter(eos_calls).most_common():
            print(f"  {name}: {count}")

    session_states = [
        event for event in events if event.get("event") == "steam_session_state"
    ]
    if session_states:
        print()
        print("steam session state")
        relays = collections.Counter(event.get("relay", "?") for event in session_states)
        errors = collections.Counter(event.get("error", "?") for event in session_states)
        max_queued_bytes = max(int(event.get("queued_bytes", 0)) for event in session_states)
        max_queued_packets = max(
            int(event.get("queued_packets", 0)) for event in session_states
        )
        print("  relay:", dict(relays.most_common()))
        print("  errors:", dict(errors.most_common()))
        print("  max queued bytes:", max_queued_bytes)
        print("  max queued packets:", max_queued_packets)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
