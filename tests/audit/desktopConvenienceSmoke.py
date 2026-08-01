"""Gate: the production desktop remembers a provider and exposes provider-owned gauges.

The browser starts a session without choosing a provider, reloads with another remembered provider,
and injects the normalized usage and account-limit frames that real drivers already emit. The gate
checks rendered product behaviour, not source tokens.

Usage::

    python -X utf8 tests/audit/desktopConvenienceSmoke.py --selftest
    python -X utf8 tests/audit/desktopConvenienceSmoke.py
"""

from __future__ import annotations

import sys
from typing import Any

from desktopPerformance import measurement

EXPECTED = (
    "defaultProvider",
    "providerRememberedAfterStart",
    "rememberedProvider",
    "usageVisible",
    "rateLimitVisible",
    "quotaReachedVisible",
)


def problems(result: dict[str, Any]) -> list[str]:
    """Return every desktop convenience that was absent."""
    return [name for name in EXPECTED if result.get(name) is not True]


def selftest() -> int:
    """Prove each independent behaviour can make the gate red."""
    green = {name: True for name in EXPECTED}
    if problems(green):
        print("[desktopConvenienceSmoke --selftest] FAIL. a green fixture was rejected.", file=sys.stderr)
        return 2
    for name in EXPECTED:
        fixture = dict(green)
        fixture[name] = False
        if problems(fixture) != [name]:
            print(f"[desktopConvenienceSmoke --selftest] FAIL. {name} escaped.", file=sys.stderr)
            return 2
    print("[desktopConvenienceSmoke --selftest] OK. all six missing behaviours make the gate red.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or exercise the production bundle in a real browser."""
    if "--selftest" in argv:
        return selftest()
    result, diagnostic = measurement("convenience")
    if result is None:
        print(f"[desktopConvenienceSmoke] FAIL. {diagnostic}", file=sys.stderr)
        return 2
    if diagnostic.strip():
        print(diagnostic, file=sys.stderr, end="" if diagnostic.endswith("\n") else "\n")
    found = problems(result)
    if found:
        print("[desktopConvenienceSmoke] FAIL. desktop conveniences are missing:", file=sys.stderr)
        for name in found:
            print(f"  - {name}", file=sys.stderr)
        return 2
    print("[desktopConvenienceSmoke] OK. provider memory and both provider gauges are visible.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
