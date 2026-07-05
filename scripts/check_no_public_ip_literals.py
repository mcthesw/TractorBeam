#!/usr/bin/env python3
import ipaddress
import pathlib
import re
import subprocess
import sys


IPV4_LITERAL = re.compile(r"(?<![0-9])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![0-9])")


def tracked_files() -> list[str]:
    output = subprocess.check_output(["git", "ls-files", "-z"])
    return [name.decode("utf-8") for name in output.split(b"\0") if name]


def main() -> int:
    findings: list[tuple[str, int]] = []
    for name in tracked_files():
        path = pathlib.Path(name)
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue

        for line_number, line in enumerate(text.splitlines(), start=1):
            for match in IPV4_LITERAL.finditer(line):
                try:
                    ip = ipaddress.ip_address(match.group(0))
                except ValueError:
                    continue
                if ip.is_global:
                    findings.append((name, line_number))

    if findings:
        print("Public IPv4 literals are not allowed in tracked files.", file=sys.stderr)
        for name, line_number in findings:
            print(f"{name}:{line_number}", file=sys.stderr)
        return 1

    print("No public IPv4 literals found in tracked files.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
