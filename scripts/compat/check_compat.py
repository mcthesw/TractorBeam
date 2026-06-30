#!/usr/bin/env python3
from __future__ import annotations

import argparse
import dataclasses
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Callable


ENVELOPE_MAGIC = b"BBR1"
GAME_PACKET_MAGIC = b"BBG1"
PROTOCOL_MAJOR = 1
PROTOCOL_MINOR = 0
ENVELOPE_HEADER_LEN = 42
GAME_PACKET_HEADER_LEN = 40
CAP_PATH_VALIDATION = 1 << 0

MESSAGE_JOIN = 1
MESSAGE_DATA = 4
MESSAGE_HEARTBEAT = 5
MESSAGE_ERROR = 6


class CompatError(RuntimeError):
    pass


class CompatSkip(RuntimeError):
    pass


@dataclasses.dataclass
class CaseResult:
    suite: str
    name: str
    status: str
    message: str
    duration_ms: int


class Peer:
    def send(self, raw: bytes) -> None:
        raise NotImplementedError

    def recv(self, timeout: float) -> bytes:
        raise NotImplementedError

    def close(self) -> None:
        raise NotImplementedError


class UdpPeer(Peer):
    def __init__(self, address: tuple[str, int]) -> None:
        self.address = address
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.socket.bind(("127.0.0.1", 0))

    def send(self, raw: bytes) -> None:
        self.socket.sendto(raw, self.address)

    def recv(self, timeout: float) -> bytes:
        self.socket.settimeout(timeout)
        try:
            raw, _ = self.socket.recvfrom(131_072)
            return raw
        except TimeoutError as error:
            raise CompatError("timed out waiting for UDP frame") from error

    def close(self) -> None:
        self.socket.close()


class TcpPeer(Peer):
    def __init__(self, address: tuple[str, int], timeout: float) -> None:
        self.socket = socket.create_connection(address, timeout=timeout)
        self.socket.settimeout(timeout)

    def send(self, raw: bytes) -> None:
        self.socket.sendall(struct.pack(">I", len(raw)) + raw)

    def recv(self, timeout: float) -> bytes:
        self.socket.settimeout(timeout)
        header = read_exact(self.socket, 4)
        length = struct.unpack(">I", header)[0]
        return read_exact(self.socket, length)

    def close(self) -> None:
        self.socket.close()


def read_exact(stream: socket.socket, size: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < size:
        chunk = stream.recv(size - len(chunks))
        if not chunk:
            raise CompatError("TCP connection closed")
        chunks.extend(chunk)
    return bytes(chunks)


def encode_envelope(
    message_type: int,
    payload: bytes,
    *,
    major: int = PROTOCOL_MAJOR,
    minor: int = PROTOCOL_MINOR,
    capabilities: int = CAP_PATH_VALIDATION,
) -> bytes:
    header = struct.pack(
        ">4sBBBBHIQQ12s",
        ENVELOPE_MAGIC,
        major,
        minor,
        message_type,
        0,
        ENVELOPE_HEADER_LEN,
        len(payload),
        capabilities,
        0,
        b"\0" * 12,
    )
    return header + payload


def decode_envelope(raw: bytes, *, allow_any_major: bool = False) -> tuple[int, bytes]:
    if len(raw) < ENVELOPE_HEADER_LEN:
        raise CompatError("envelope too short")
    magic, major, _minor, message_type, _flags, header_len, payload_len = struct.unpack(
        ">4sBBBBHI", raw[:14]
    )
    if magic != ENVELOPE_MAGIC:
        raise CompatError("bad envelope magic")
    if major != PROTOCOL_MAJOR and not allow_any_major:
        raise CompatError(f"unsupported envelope major in response: {major}")
    if header_len < ENVELOPE_HEADER_LEN:
        raise CompatError(f"bad response header length: {header_len}")
    if len(raw) < header_len + payload_len:
        raise CompatError("response payload is truncated")
    return message_type, raw[header_len : header_len + payload_len]


def encode_control(message: dict[str, Any], message_type: int) -> bytes:
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    return encode_envelope(message_type, payload)


def recv_control(
    peer: Peer, timeout: float, *, allow_any_major: bool = False
) -> tuple[int, dict[str, Any]]:
    message_type, payload = decode_envelope(
        peer.recv(timeout), allow_any_major=allow_any_major
    )
    try:
        message = json.loads(payload.decode("utf-8"))
    except json.JSONDecodeError as error:
        raise CompatError(f"control payload is not JSON: {error}") from error
    return message_type, message


def join_peer(
    peer: Peer,
    room: str,
    steam_id64: str,
    timeout: float,
) -> dict[str, Any]:
    send_join(peer, room, steam_id64, None)
    message_type, challenge = recv_control(peer, timeout)
    if message_type != 2 or challenge.get("type") != "challenge":
        raise CompatError(f"expected join challenge, got {challenge}")

    send_join(peer, room, steam_id64, challenge.get("token"))
    message_type, ready = recv_control(peer, timeout)
    if message_type != 3 or ready.get("type") != "ready":
        raise CompatError(f"expected join ready, got {ready}")
    return ready


def send_join(
    peer: Peer,
    room: str,
    steam_id64: str,
    challenge: str | None,
) -> None:
    peer.send(
        encode_control(
            {
                "type": "join",
                "room": room,
                "steam_id64": steam_id64,
                "display_name": None,
                "challenge": challenge,
            },
            MESSAGE_JOIN,
        )
    )


def send_health_ping(peer: Peer, timeout: float) -> None:
    peer.send(encode_control({"type": "health_ping", "id": 42}, MESSAGE_HEARTBEAT))
    message_type, message = recv_control(peer, timeout)
    if message_type != MESSAGE_HEARTBEAT or message != {"type": "health_pong", "id": 42}:
        raise CompatError(f"expected health pong, got {message}")


def encode_game_packet(from_steam_id64: str, to_steam_id64: int, payload: bytes) -> bytes:
    header = struct.pack(
        ">4sBBHQQiiII",
        GAME_PACKET_MAGIC,
        PROTOCOL_MAJOR,
        0,
        GAME_PACKET_HEADER_LEN,
        int(from_steam_id64),
        to_steam_id64,
        0,
        0,
        len(payload),
        1,
    )
    return header + payload


def decode_game_packet(raw: bytes) -> tuple[str, int, bytes]:
    if len(raw) < GAME_PACKET_HEADER_LEN:
        raise CompatError("game packet too short")
    magic, major, _reserved, header_len = struct.unpack(">4sBBH", raw[:8])
    if magic != GAME_PACKET_MAGIC:
        raise CompatError("bad game packet magic")
    if major != PROTOCOL_MAJOR:
        raise CompatError(f"bad game packet major: {major}")
    from_id, to_id, _channel, _send_type, payload_len, _sequence = struct.unpack(
        ">QQiiII", raw[8:40]
    )
    if len(raw) < header_len + payload_len:
        raise CompatError("game packet payload truncated")
    return str(from_id), to_id, raw[header_len : header_len + payload_len]


def send_game(peer: Peer, from_steam_id64: str, to_steam_id64: int, payload: bytes) -> None:
    game = encode_game_packet(from_steam_id64, to_steam_id64, payload)
    peer.send(encode_envelope(MESSAGE_DATA, game))


def recv_game(peer: Peer, timeout: float) -> tuple[str, int, bytes]:
    message_type, payload = decode_envelope(peer.recv(timeout))
    if message_type != MESSAGE_DATA:
        raise CompatError(f"expected data envelope, got message type {message_type}")
    return decode_game_packet(payload)


def make_peer(transport: str, address: tuple[str, int], timeout: float) -> Peer:
    if transport == "udp":
        return UdpPeer(address)
    if transport == "tcp":
        return TcpPeer(address, timeout)
    raise ValueError(f"unknown transport {transport}")


def case_join_heartbeat(transport: str, address: tuple[str, int], timeout: float) -> None:
    peer = make_peer(transport, address, timeout)
    try:
        join_peer(peer, new_room(), "76561198000000101", timeout)
        send_health_ping(peer, timeout)
    finally:
        peer.close()


def case_forwarding(transport: str, address: tuple[str, int], timeout: float) -> None:
    room = new_room()
    peer_a = make_peer(transport, address, timeout)
    peer_b = make_peer(transport, address, timeout)
    try:
        join_peer(peer_a, room, "76561198000000101", timeout)
        join_peer(peer_b, room, "76561198000000102", timeout)
        payload = b"compat-data"
        send_game(peer_a, "76561198000000101", 76561198000000102, payload)
        from_id, to_id, received = recv_game(peer_b, timeout)
        if (from_id, to_id, received) != ("76561198000000101", 76561198000000102, payload):
            raise CompatError("forwarded game packet did not round-trip")
    finally:
        peer_a.close()
        peer_b.close()


def case_unsupported_major(address: tuple[str, int], timeout: float) -> None:
    peer = UdpPeer(address)
    try:
        peer.send(encode_envelope(MESSAGE_HEARTBEAT, b"{}", major=99))
        message_type, message = recv_control(peer, timeout, allow_any_major=True)
        if message_type != MESSAGE_ERROR or message.get("code") != "decode_error":
            raise CompatError(f"expected decode_error, got {message}")
    finally:
        peer.close()


def case_unknown_message_type(address: tuple[str, int], timeout: float) -> None:
    peer = UdpPeer(address)
    try:
        peer.send(encode_envelope(99, b"{}"))
        message_type, message = recv_control(peer, timeout)
        if message_type != MESSAGE_ERROR or message.get("code") != "decode_error":
            raise CompatError(f"expected decode_error, got {message}")
    finally:
        peer.close()


def case_oversize_tcp_frame(address: tuple[str, int], timeout: float) -> None:
    peer = TcpPeer(address, timeout)
    try:
        peer.send(b"x" * 70_000)
        try:
            _ = peer.recv(timeout)
        except CompatError as error:
            if "closed" in str(error):
                return
            raise
        except OSError:
            return
        raise CompatError("oversize TCP frame was not rejected")
    finally:
        peer.close()


def case_join_before_forwarding_guard(address: tuple[str, int], timeout: float) -> None:
    room = new_room()
    joined = UdpPeer(address)
    stranger = UdpPeer(address)
    try:
        join_peer(joined, room, "76561198000000102", timeout)
        send_game(stranger, "76561198000000101", 76561198000000102, b"unjoined")
        try:
            _ = joined.recv(0.25)
        except CompatError:
            return
        raise CompatError("unjoined peer data was forwarded")
    finally:
        joined.close()
        stranger.close()


def new_room() -> str:
    return f"compat-{uuid.uuid4().hex[:10]}"


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def wait_for_relay(address: tuple[str, int], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(address, timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise CompatError(f"relay did not open TCP listener at {address[0]}:{address[1]}")


def add_result(
    results: list[CaseResult],
    suite: str,
    name: str,
    status: str,
    message: str,
    duration_ms: int = 0,
) -> None:
    results.append(CaseResult(suite, name, status, message, duration_ms))


def run_case(
    results: list[CaseResult],
    suite: str,
    name: str,
    func: Callable[[], None],
) -> None:
    started = time.monotonic()
    try:
        func()
    except CompatSkip as error:
        status = "skip"
        message = str(error)
    except Exception as error:  # noqa: BLE001 - keep the harness recording all cases.
        status = "fail"
        message = str(error)
    else:
        status = "pass"
        message = "ok"
    add_result(
        results, suite, name, status, message, int((time.monotonic() - started) * 1000)
    )


def launch_relay(
    relay_binary: Path, timeout: float
) -> tuple[subprocess.Popen[str], tuple[str, int]]:
    last_error = "relay did not start"
    for _attempt in range(5):
        port = free_port()
        address = ("127.0.0.1", port)
        try:
            process = subprocess.Popen(
                [
                    str(relay_binary),
                    "--bind",
                    f"{address[0]}:{address[1]}",
                    "--tcp-bind",
                    f"{address[0]}:{address[1]}",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
        except OSError as error:
            raise CompatError(f"relay process could not start: {error}") from error
        try:
            wait_for_relay(address, timeout)
            return process, address
        except Exception as error:  # noqa: BLE001 - retry startup races.
            last_error = str(error)
            stop_process(process)
            time.sleep(0.1)
    raise CompatError(last_error)


def stop_process(process: subprocess.Popen[str]) -> None:
    process.terminate()
    try:
        process.communicate(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate(timeout=2)


def run_relay_suite(
    results: list[CaseResult],
    suite: str,
    relay_binary: Path,
    timeout: float,
) -> None:
    if not relay_binary.exists():
        raise CompatSkip(f"relay binary not found: {relay_binary}")
    process, address = launch_relay(relay_binary, timeout)
    try:
        wait_for_relay(address, timeout)
        run_case(
            results,
            suite,
            "tcp_join_heartbeat",
            lambda: case_join_heartbeat("tcp", address, timeout),
        )
        run_case(
            results,
            suite,
            "udp_join_heartbeat",
            lambda: case_join_heartbeat("udp", address, timeout),
        )
        run_case(
            results,
            suite,
            "tcp_forwarding",
            lambda: case_forwarding("tcp", address, timeout),
        )
        run_case(
            results,
            suite,
            "udp_forwarding",
            lambda: case_forwarding("udp", address, timeout),
        )
        run_case(
            results,
            suite,
            "unsupported_protocol_major",
            lambda: case_unsupported_major(address, timeout),
        )
        run_case(
            results,
            suite,
            "unknown_message_type",
            lambda: case_unknown_message_type(address, timeout),
        )
        run_case(
            results,
            suite,
            "oversize_tcp_frame",
            lambda: case_oversize_tcp_frame(address, timeout),
        )
        run_case(
            results,
            suite,
            "join_before_forwarding_guard",
            lambda: case_join_before_forwarding_guard(address, timeout),
        )
    finally:
        stop_process(process)


def acquire_base_relay(
    args: argparse.Namespace, results: list[CaseResult]
) -> tuple[Path | None, bool]:
    if args.base_relay_binary:
        return Path(args.base_relay_binary), True
    if not args.previous_tag:
        run_case(
            results,
            args.base_label,
            "base_relay_acquisition",
            lambda: (_ for _ in ()).throw(CompatSkip("no base relay configured")),
        )
        return None, False
    if not args.github_repository:
        run_case(
            results,
            args.base_label,
            "base_relay_acquisition",
            lambda: (_ for _ in ()).throw(CompatSkip("github repository is not configured")),
        )
        return None, False
    if shutil.which("gh") is None:
        run_case(
            results,
            args.base_label,
            "base_relay_acquisition",
            lambda: (_ for _ in ()).throw(CompatSkip("gh CLI is unavailable")),
        )
        return None, False

    downloads = Path(args.downloads_dir)
    downloads.mkdir(parents=True, exist_ok=True)
    command = [
        "gh",
        "release",
        "download",
        args.previous_tag,
        "--repo",
        args.github_repository,
        "--pattern",
        "basement-bridge-relay-linux-x86_64",
        "--dir",
        str(downloads),
        "--clobber",
    ]
    completed = subprocess.run(command, text=True, capture_output=True, check=False)
    if completed.returncode != 0:
        message = (completed.stderr or completed.stdout).strip() or "gh release download failed"
        run_case(
            results,
            args.base_label,
            "base_relay_acquisition",
            lambda: (_ for _ in ()).throw(CompatSkip(message)),
        )
        return None, False

    relay = downloads / "basement-bridge-relay-linux-x86_64"
    if os.name != "nt":
        relay.chmod(relay.stat().st_mode | 0o111)
    run_case(results, args.base_label, "base_relay_acquisition", lambda: None)
    return relay, False


def summarize(results: list[CaseResult]) -> dict[str, int]:
    summary = {"pass": 0, "fail": 0, "skip": 0}
    for result in results:
        summary[result.status] = summary.get(result.status, 0) + 1
    return summary


def write_report(path: Path, results: list[CaseResult]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    report = {
        "schema_version": 1,
        "generated_at_unix": int(time.time()),
        "summary": summarize(results),
        "cases": [dataclasses.asdict(result) for result in results],
    }
    path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run Basement Bridge compatibility probes.")
    parser.add_argument(
        "--head-relay-binary", "--relay-binary", dest="head_relay_binary",
        help="Head/current relay binary to test.",
    )
    parser.add_argument(
        "--base-relay-binary", "--previous-relay-binary", dest="base_relay_binary",
        help="Base/merge-target relay binary to test.",
    )
    parser.add_argument("--previous-tag", help="Optional previous GitHub Release tag to download.")
    parser.add_argument(
        "--github-repository",
        default=os.environ.get("GITHUB_REPOSITORY"),
        help="GitHub repository for previous release downloads, owner/name.",
    )
    parser.add_argument("--head-label", default="head-relay")
    parser.add_argument("--base-label", default="base-relay")
    parser.add_argument("--downloads-dir", default=".local/compat/downloads")
    parser.add_argument("--json-out", default=".local/compat/compat-report.json")
    parser.add_argument("--timeout", type=float, default=2.0)
    args = parser.parse_args()
    if not args.head_relay_binary:
        parser.error("--head-relay-binary is required")
    return args


def main() -> int:
    args = parse_args()
    results: list[CaseResult] = []

    try:
        run_relay_suite(
            results, args.head_label, Path(args.head_relay_binary), args.timeout,
        )
    except Exception as error:  # noqa: BLE001 - still write the report.
        add_result(results, args.head_label, "relay_startup", "fail", str(error))
    base_relay, base_required = acquire_base_relay(args, results)
    if base_relay is not None:
        try:
            run_relay_suite(
                results, args.base_label, base_relay, args.timeout,
            )
        except Exception as error:  # noqa: BLE001 - previous evidence is optional.
            add_result(
                results,
                args.base_label,
                "base_relay_suite",
                "fail" if base_required else "skip",
                str(error),
            )

    write_report(Path(args.json_out), results)
    summary = summarize(results)
    print(
        f"compat: pass={summary.get('pass', 0)} fail={summary.get('fail', 0)} "
        f"skip={summary.get('skip', 0)} report={args.json_out}"
    )
    for result in results:
        print(f"{result.status.upper()} {result.suite}/{result.name}: {result.message}")
    return 1 if summary.get("fail", 0) else 0


if __name__ == "__main__":
    sys.exit(main())
