"""Every way of finding the Runtrol home reads the same environment variable.

The Core resolves ``RUNTROL_HOME`` first and falls back to the platform's directory. Two client libraries used to
resolve only the platform directory, so one process could hold a daemon in the home the operator chose and a
locator in the default one. Nothing failed loudly: the halves simply talked to different Runtimes, and an
enrollment created by one was invisible to the other (measured 2026-08-26, an extension in a chosen home could
never finish enrolling and said ``the pending enrollment does not exist`` forever).

The rule this gate holds is not "the code looks similar". It is that **every file which builds a locator path
reads the same variable**, so a new consumer cannot quietly invent a fourth way to find the home.

    python -X utf8 tests/audit/homeIsOneRule.py
    python -X utf8 tests/audit/homeIsOneRule.py --selftest
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

HOME_ENVIRONMENT = "RUNTROL_HOME"

# Every file that turns "this machine" into a locator path. Each must consult the operator's choice first.
RESOLVERS = (
    Path("crates/runtrol-core/src/home/mod.rs"),
    Path("crates/runtrol-runtime-client/src/locator.rs"),
    Path("clients/typescript/src/locator.ts"),
)

# What a resolver that skipped the variable would use instead. Present is fine; present *without* the variable is
# the defect, because that is exactly a fallback that became the only path.
PLATFORM_MARKERS = ("LOCALAPPDATA", "XDG_STATE_HOME", "Application Support")


def offences(read: "callable[[Path], str]") -> list[str]:
    found: list[str] = []
    for relative in RESOLVERS:
        text = read(relative)
        if HOME_ENVIRONMENT not in text:
            found.append(f"{relative.as_posix()} builds a home path without reading {HOME_ENVIRONMENT}")
            continue
        if not any(marker in text for marker in PLATFORM_MARKERS):
            found.append(f"{relative.as_posix()} names no platform fallback, so an unset home has nowhere to go")
    return found


def read_repository(relative: Path) -> str:
    path = ROOT / relative
    if not path.is_file():
        raise SystemExit(f"[homeIsOneRule] missing resolver: {relative.as_posix()}")
    return path.read_text(encoding="utf-8")


def selftest() -> int:
    cases = [
        ({r: f"{HOME_ENVIRONMENT} LOCALAPPDATA" for r in RESOLVERS}, 0, "all three read the variable"),
        (
            {RESOLVERS[0]: f"{HOME_ENVIRONMENT} LOCALAPPDATA",
             RESOLVERS[1]: "LOCALAPPDATA only",
             RESOLVERS[2]: f"{HOME_ENVIRONMENT} LOCALAPPDATA"},
            1,
            "one resolver skipped the variable",
        ),
        (
            {r: HOME_ENVIRONMENT for r in RESOLVERS},
            len(RESOLVERS),
            "no resolver names a platform fallback",
        ),
    ]
    for texts, expected, why in cases:
        found = offences(lambda relative: texts[relative])
        if len(found) != expected:
            print(f"[homeIsOneRule] selftest failed: {why} gave {len(found)}, expected {expected}")
            return 1
    print(f"[homeIsOneRule] selftest OK. {len(cases)} cases.")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    found = offences(read_repository)
    if found:
        for line in found:
            print(f"[homeIsOneRule] {line}")
        return 1
    print(f"[homeIsOneRule] OK. {len(RESOLVERS)} home resolvers read {HOME_ENVIRONMENT} first.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
