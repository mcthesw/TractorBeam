#!/usr/bin/env python3
import pathlib
import re
import subprocess
import sys


FORWARDER = re.compile(
    r"^\s*(\d+)\s+(?:(\S+)\s+)?\(forwarded to ([^)]+)\)$"
)
EXPECTED_ORDINALS = list(range(2, 191))


def fail(message: str) -> int:
    print(message, file=sys.stderr)
    return 1


def main() -> int:
    if len(sys.argv) != 2:
        return fail(f"usage: {pathlib.Path(sys.argv[0]).name} <winmm-proxy.dll>")

    dll = pathlib.Path(sys.argv[1])
    try:
        output = subprocess.check_output(
            ["i686-w64-mingw32-objdump", "-p", dll],
            text=True,
            stderr=subprocess.STDOUT,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        return fail(f"failed to inspect {dll}: {error}")

    if "dll name: libunwind.dll" in output.lower():
        return fail(f"{dll} must not depend on libunwind.dll")
    if "Ordinal base: 2" not in output:
        return fail(f"{dll} must export WinMM ordinals starting at 2")

    exports: list[tuple[int, str | None, str]] = []
    for line in output.split("Export Table:", maxsplit=1)[-1].splitlines():
        if match := FORWARDER.match(line):
            ordinal, name, target = match.groups()
            exports.append((int(ordinal), name, target))

    if [ordinal for ordinal, _, _ in exports] != EXPECTED_ORDINALS:
        return fail(f"{dll} must contain exactly 189 contiguous WinMM forwarders")

    for ordinal, name, target in exports:
        module, separator, symbol = target.partition(".")
        if module.lstrip("_") != "winmm_orig" or not separator:
            return fail(f"ordinal {ordinal} forwards outside winmm_orig: {target}")
        expected_symbol = name if name is not None else f"#{ordinal}"
        if symbol != expected_symbol:
            return fail(
                f"ordinal {ordinal} forwards {name or '<NONAME>'} to {target}"
            )

    print(f"Verified {len(exports)} WinMM forwarders in {dll}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
