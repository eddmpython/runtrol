"""Gate: reconnect drains accepted frames and cancels superseded watch setup.

The production browser bundle receives provider-shaped frames through the desktop bridge. The gate forces an OVER
beside queued frames, injects a stale generation, and abandons a watch before its acknowledgement arrives.

Usage::

    python -X utf8 tests/audit/reconnectContinuitySmoke.py --selftest
    python -X utf8 tests/audit/reconnectContinuitySmoke.py
"""

from __future__ import annotations

import sys
from typing import Any

from desktopPerformance import measurement

REQUIRED = (
    "drainedBeforeReconnect",
    "reconnectCursorExact",
    "staleOverIgnored",
    "pendingWatchCancelled",
)


def problems(metrics: dict[str, Any]) -> list[str]:
    """Return every missing or false reconnect contract."""
    return [name for name in REQUIRED if metrics.get(name) is not True]


def selftest() -> int:
    """Prove every independent reconnect contract can make the gate red."""
    green = dict.fromkeys(REQUIRED, True)
    if problems(green):
        print("[reconnectContinuitySmoke --selftest] FAIL. green fixture was rejected.", file=sys.stderr)
        return 2
    for name in REQUIRED:
        fixture = dict(green)
        fixture[name] = False
        if problems(fixture) != [name]:
            print(f"[reconnectContinuitySmoke --selftest] FAIL. {name} escaped.", file=sys.stderr)
            return 2
    print("[reconnectContinuitySmoke --selftest] OK. all four discontinuities make the gate red.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or drive the production bundle through reconnect races."""
    if "--selftest" in argv:
        return selftest()
    metrics, diagnostic = measurement("reconnect")
    if metrics is None:
        print(f"[reconnectContinuitySmoke] FAIL. {diagnostic}", file=sys.stderr)
        return 2
    if diagnostic.strip():
        print(diagnostic, file=sys.stderr, end="" if diagnostic.endswith("\n") else "\n")
    found = problems(metrics)
    if found:
        print("[reconnectContinuitySmoke] FAIL. reconnect continuity regressed.", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[reconnectContinuitySmoke] OK. frames drained, cursor exact, stale and pending watches isolated.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
