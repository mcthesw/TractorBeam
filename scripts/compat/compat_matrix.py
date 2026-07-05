from __future__ import annotations

import dataclasses
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


CLIENT_ONLY_ENV = "TRACTOR_BEAM_COMPAT_CLIENT_ONLY"


@dataclasses.dataclass
class CaseResult:
    suite: str
    client: str
    server: str
    name: str
    status: str
    message: str
    duration_ms: int


def pair_label(client: str, server: str) -> str:
    return f"{client}/{server}"


def split_pair_label(label: str, fallback_client: str) -> tuple[str, str]:
    if "/" not in label:
        return fallback_client, label
    client, server = label.split("/", 1)
    return client or fallback_client, server or label


def add_result(
    results: list[CaseResult],
    client: str,
    server: str,
    name: str,
    status: str,
    message: str,
    duration_ms: int = 0,
) -> None:
    results.append(
        CaseResult(
            suite=pair_label(client, server),
            client=client,
            server=server,
            name=name,
            status=status,
            message=message,
            duration_ms=duration_ms,
        )
    )


def summarize(results: list[CaseResult]) -> dict[str, int]:
    summary = {"pass": 0, "fail": 0, "skip": 0}
    for result in results:
        summary[result.status] = summary.get(result.status, 0) + 1
    return summary


def write_report(
    path: Path,
    results: list[CaseResult],
    *,
    client_order: list[str] | None = None,
    server_order: list[str] | None = None,
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    report = {
        "schema_version": 2,
        "generated_at_unix": int(time.time()),
        "summary": summarize(results),
        "matrix": matrix_summary(
            results,
            client_order=client_order,
            server_order=server_order,
        ),
        "cases": [dataclasses.asdict(result) for result in results],
    }
    path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


def write_step_summary(
    path: Path,
    results: list[CaseResult],
    *,
    client_order: list[str] | None = None,
    server_order: list[str] | None = None,
) -> None:
    matrix = matrix_summary(
        results,
        client_order=client_order,
        server_order=server_order,
    )
    pair_summaries = {
        (pair["client"], pair["server"]): pair["summary"] for pair in matrix["pairs"]
    }
    lines = [
        "## Compatibility matrix",
        "",
        summary_line("Overall", summarize(results)),
        "",
        "| Client probe | "
        + " | ".join(escape_markdown(server) for server in matrix["servers"])
        + " |",
        "| --- | " + " | ".join("---" for _ in matrix["servers"]) + " |",
    ]
    for client in matrix["clients"]:
        cells = [
            summary_cell(pair_summaries.get((client, server), {}))
            for server in matrix["servers"]
        ]
        lines.append(f"| {escape_markdown(client)} | " + " | ".join(cells) + " |")
    lines.append("")
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as summary:
        summary.write("\n".join(lines))


def matrix_summary(
    results: list[CaseResult],
    *,
    client_order: list[str] | None = None,
    server_order: list[str] | None = None,
) -> dict[str, Any]:
    observed_clients = unique_in_order([result.client for result in results])
    observed_servers = unique_in_order([result.server for result in results])
    clients = preferred_order(observed_clients, client_order or [])
    servers = preferred_order(observed_servers, server_order or [])
    pairs = []
    for client in clients:
        for server in servers:
            pair_results = [
                result
                for result in results
                if result.client == client and result.server == server
            ]
            if pair_results:
                pairs.append(
                    {
                        "client": client,
                        "server": server,
                        "summary": summarize(pair_results),
                    }
                )
    return {"clients": clients, "servers": servers, "pairs": pairs}


def unique_in_order(values: list[str]) -> list[str]:
    seen: set[str] = set()
    ordered: list[str] = []
    for value in values:
        if value in seen:
            continue
        seen.add(value)
        ordered.append(value)
    return ordered


def preferred_order(observed: list[str], preferred: list[str]) -> list[str]:
    ordered = [value for value in preferred if value in observed]
    ordered.extend(value for value in observed if value not in ordered)
    return ordered


def summary_line(label: str, summary: dict[str, int]) -> str:
    return (
        f"**{label}:** pass={summary.get('pass', 0)} "
        f"fail={summary.get('fail', 0)} skip={summary.get('skip', 0)}"
    )


def summary_cell(summary: dict[str, int]) -> str:
    if not summary:
        return "not run"
    return (
        f"pass={summary.get('pass', 0)}<br>"
        f"fail={summary.get('fail', 0)}<br>"
        f"skip={summary.get('skip', 0)}"
    )


def escape_markdown(value: str) -> str:
    return value.replace("|", "\\|")


def client_only(args: Any) -> bool:
    return bool(args.client_only) or os.environ.get(CLIENT_ONLY_ENV) == "1"


def should_run_matrix(args: Any) -> bool:
    return bool(args.base_client_script) and not client_only(args)


def run_matrix(args: Any) -> list[CaseResult]:
    results: list[CaseResult] = []
    clients = [
        (args.base_client_label, Path(args.base_client_script)),
        (
            args.head_client_label,
            Path(args.head_client_script)
            if args.head_client_script
            else Path(args.default_head_client_script),
        ),
    ]
    for client_label, script in clients:
        run_client_script(results, args, client_label, script)
    return results


def run_client_script(
    results: list[CaseResult],
    args: Any,
    client_label: str,
    script: Path,
) -> None:
    if not script.exists():
        for server in active_server_labels(args):
            add_result(
                results,
                client_label,
                server,
                "client_script",
                "fail",
                f"client script not found: {script}",
            )
        return

    report_path = Path(args.json_out).with_name(
        f"{Path(args.json_out).stem}.{safe_label(client_label)}.json"
    )
    command = client_script_command(script)
    command.extend(
        [
            "--head-relay-binary",
            str(args.head_relay_binary),
            "--head-label",
            pair_label(client_label, args.head_server_label),
            "--base-label",
            pair_label(client_label, args.base_server_label),
            "--json-out",
            str(report_path),
            "--timeout",
            str(args.timeout),
        ]
    )
    if args.base_relay_binary:
        command.extend(["--base-relay-binary", str(args.base_relay_binary)])
    elif args.previous_tag:
        command.extend(["--previous-tag", str(args.previous_tag)])
        command.extend(
            [
                "--downloads-dir",
                str(Path(args.downloads_dir) / safe_label(client_label)),
            ]
        )
        if args.github_repository:
            command.extend(["--github-repository", str(args.github_repository)])

    env = os.environ.copy()
    env[CLIENT_ONLY_ENV] = "1"
    env.pop("GITHUB_STEP_SUMMARY", None)
    completed = subprocess.run(command, text=True, capture_output=True, check=False, env=env)
    if import_client_report(report_path, results, client_label):
        return

    message = subprocess_message(completed)
    for server in active_server_labels(args):
        add_result(results, client_label, server, "client_script", "fail", message)


def active_server_labels(args: Any) -> list[str]:
    labels = [args.head_server_label]
    if args.base_relay_binary or args.previous_tag:
        labels.append(args.base_server_label)
    return labels


def client_script_command(script: Path) -> list[str]:
    uv = shutil.which("uv")
    if uv and (script.parent / "pyproject.toml").exists():
        return [uv, "run", "--project", str(script.parent), "python", str(script)]
    return [sys.executable, str(script)]


def import_client_report(
    report_path: Path, results: list[CaseResult], fallback_client: str
) -> bool:
    if not report_path.exists():
        return False
    try:
        report = json.loads(report_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    cases = report.get("cases")
    if not isinstance(cases, list):
        return False
    for raw in cases:
        if isinstance(raw, dict):
            results.append(imported_case(raw, fallback_client))
    return True


def imported_case(raw: dict[str, Any], fallback_client: str) -> CaseResult:
    suite = str(raw.get("suite") or "")
    client = str(raw.get("client") or "")
    server = str(raw.get("server") or "")
    if "/" in suite:
        suite_client, suite_server = suite.split("/", 1)
        if suite_client == fallback_client:
            client = suite_client
            server = suite_server
    if not client:
        client = fallback_client
    if not server:
        server = suite or "server"
    try:
        duration_ms = int(raw.get("duration_ms") or 0)
    except (TypeError, ValueError):
        duration_ms = 0
    return CaseResult(
        suite=pair_label(client, server),
        client=client,
        server=server,
        name=str(raw.get("name") or "unknown"),
        status=str(raw.get("status") or "fail"),
        message=str(raw.get("message") or ""),
        duration_ms=duration_ms,
    )


def subprocess_message(completed: subprocess.CompletedProcess[str]) -> str:
    output = (completed.stderr or completed.stdout).strip()
    if not output:
        return f"client script exited with code {completed.returncode}"
    return output[-1_000:]


def safe_label(value: str) -> str:
    safe = "".join(
        char if char.isalnum() or char in {"-", "_"} else "-" for char in value
    ).strip("-")
    return safe or "client"
