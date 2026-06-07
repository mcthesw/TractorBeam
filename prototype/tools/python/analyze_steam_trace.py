#!/usr/bin/env python3
"""Detailed timing analysis for isaac_eos_probe Steam P2P events."""

from __future__ import annotations

import argparse
import collections
import json
import re
from pathlib import Path
from typing import Iterable


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


def ms(us: int) -> float:
    return us / 1000.0


def analyze_stream(events: list[dict], event_name: str, threshold_ms: float) -> None:
    subset = [
        event
        for event in events
        if event.get("event") == event_name and isinstance(event.get("ts_us"), int)
    ]
    if len(subset) < 2:
        return

    first_ts = int(subset[0]["ts_us"])
    gaps = []
    for previous, current in zip(subset, subset[1:]):
        gap_us = int(current["ts_us"]) - int(previous["ts_us"])
        if gap_us < 0:
            continue
        gaps.append((gap_us, previous, current))

    long_gaps = [
        item for item in gaps if item[0] >= int(threshold_ms * 1000)
    ]
    print()
    print(f"{event_name} long gaps >= {threshold_ms:.1f} ms: {len(long_gaps)}")
    for gap_us, previous, current in sorted(
        long_gaps, key=lambda item: item[0], reverse=True
    )[:20]:
        at_ms = ms(int(current["ts_us"]) - first_ts)
        previous_size = previous.get("bytes", "?")
        current_size = current.get("bytes", "?")
        peer = current.get("peer", "?")
        channel = current.get("channel", "?")
        print(
            f"  at +{at_ms:9.3f} ms gap={ms(gap_us):8.3f} ms "
            f"peer={peer} ch={channel} prev_size={previous_size} size={current_size}"
        )

    windows = collections.Counter()
    for event in subset:
        bucket = (int(event["ts_us"]) - first_ts) // 1_000_000
        windows[int(bucket)] += 1

    if windows:
        sparse = sorted((count, second) for second, count in windows.items())[:15]
        dense = sorted(
            ((count, second) for second, count in windows.items()), reverse=True
        )[:15]
        print(f"{event_name} sparsest seconds:")
        for count, second in sparse:
            print(f"  +{second:4d}s: {count}")
        print(f"{event_name} densest seconds:")
        for count, second in dense:
            print(f"  +{second:4d}s: {count}")


def print_gap_summary(label: str, subset: list[dict]) -> None:
    if len(subset) < 2:
        print(f"   {label}: <2 events")
        return
    gaps = sorted(
        (later["ts_us"] - earlier["ts_us"]) / 1000.0
        for earlier, later in zip(subset, subset[1:])
        if later["ts_us"] >= earlier["ts_us"]
    )
    if not gaps:
        print(f"   {label}: no monotonic gaps")
        return
    p95 = gaps[int(0.95 * (len(gaps) - 1))]
    p99 = gaps[int(0.99 * (len(gaps) - 1))]
    print(
        f"   {label}: p50={gaps[len(gaps)//2]:.3f}ms "
        f"p95={p95:.3f}ms p99={p99:.3f}ms max={max(gaps):.3f}ms "
        f">100ms={sum(gap > 100 for gap in gaps)}"
    )


def analyze_segments(events: list[dict], split_gap_ms: float) -> None:
    steam_events = [
        event
        for event in events
        if event.get("event")
        in {"steam_send", "steam_recv", "steam_available", "steam_session_state"}
        and isinstance(event.get("ts_us"), int)
    ]
    steam_events.sort(key=lambda event: int(event["ts_us"]))
    if not steam_events:
        return

    split_gap_us = int(split_gap_ms * 1000)
    segments: list[list[dict]] = []
    current: list[dict] = []
    previous_ts = None
    for event in steam_events:
        ts_us = int(event["ts_us"])
        if previous_ts is not None and ts_us - previous_ts > split_gap_us and current:
            segments.append(current)
            current = []
        current.append(event)
        previous_ts = ts_us
    if current:
        segments.append(current)

    if len(segments) <= 1:
        return

    print()
    print(f"segments split by gaps > {split_gap_ms:.1f} ms")
    for index, segment in enumerate(segments, start=1):
        duration_s = (int(segment[-1]["ts_us"]) - int(segment[0]["ts_us"])) / 1_000_000
        counts = collections.Counter(event.get("event") for event in segment)
        print(f" segment {index}: duration={duration_s:.3f}s counts={dict(counts)}")
        for event_name in ("steam_send", "steam_recv"):
            subset = [event for event in segment if event.get("event") == event_name]
            print_gap_summary(event_name, subset)
        states = [
            event for event in segment if event.get("event") == "steam_session_state"
        ]
        if states:
            relays = collections.Counter(event.get("relay", "?") for event in states)
            errors = collections.Counter(event.get("error", "?") for event in states)
            max_queued_packets = max(
                int(event.get("queued_packets", 0)) for event in states
            )
            print(
                "   session_state: "
                f"relay={dict(relays.most_common())} "
                f"errors={dict(errors.most_common())} "
                f"max_queued_packets={max_queued_packets}"
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("trace", type=Path)
    parser.add_argument("--official-log", type=Path)
    parser.add_argument("--threshold-ms", type=float, default=100.0)
    parser.add_argument("--split-gap-ms", type=float, default=1000.0)
    args = parser.parse_args()

    events = list(load_events(args.trace))
    if not events:
        print("No events.")
        return 1

    timestamps = [
        int(event["ts_us"])
        for event in events
        if isinstance(event.get("ts_us"), int)
    ]
    if timestamps:
        trace_seconds = (max(timestamps) - min(timestamps)) / 1_000_000
        print(f"trace duration: {trace_seconds:.3f} s")

        if args.official_log:
            max_frame = 0
            with args.official_log.open("r", encoding="utf-8", errors="ignore") as handle:
                for line in handle:
                    for match in re.finditer(r"\[Frame:?\s*(\d+)\]", line):
                        max_frame = max(max_frame, int(match.group(1)))
            if max_frame:
                ideal_seconds = max_frame / 60.0
                print(f"official max frame: {max_frame}")
                print(f"ideal 60fps simulation time: {ideal_seconds:.3f} s")
                print(f"slowdown factor: {trace_seconds / ideal_seconds:.3f}x")

    for event_name in ("steam_recv", "steam_send", "steam_available"):
        analyze_stream(events, event_name, args.threshold_ms)

    analyze_segments(events, args.split_gap_ms)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
