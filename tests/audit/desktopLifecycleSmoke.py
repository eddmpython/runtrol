"""Gate: the production desktop bundle completes its session lifecycle without blocking drafts.

The stateful bridge deliberately delays resume and prompt acknowledgements. The journey proves that the
metadata shell remains visible, drafts remain editable and preserved, and removal requires explicit confirmation.

Usage::

    python -X utf8 tests/audit/desktopLifecycleSmoke.py --selftest
    python -X utf8 tests/audit/desktopLifecycleSmoke.py
"""

from __future__ import annotations

import sys
from typing import Any

from desktopPerformance import measurement

EXPECTED = (
    "unifiedProviders",
    "metadataSearch",
    "titleFallbacks",
    "startOpened",
    "shellStayedVisible",
    "promptBlockedWhilePreparing",
    "preparingDraftPreserved",
    "resumeReplacedRow",
    "editableWhileSending",
    "nextDraftPreserved",
    "cancelMadeNoRequest",
    "cancelKeptRow",
    "deleteRemovedRow",
)


def problems(result: dict[str, Any]) -> list[str]:
    """Return every lifecycle behaviour that was absent."""
    return [name for name in EXPECTED if result.get(name) is not True]


def selftest() -> int:
    """Prove each independent lifecycle regression can make the gate red."""
    green = {name: True for name in EXPECTED}
    if problems(green):
        print("[desktopLifecycleSmoke --selftest] FAIL. a green fixture was rejected.", file=sys.stderr)
        return 2
    for name in EXPECTED:
        fixture = dict(green)
        fixture[name] = False
        if problems(fixture) != [name]:
            print(f"[desktopLifecycleSmoke --selftest] FAIL. {name} escaped.", file=sys.stderr)
            return 2
    print(f"[desktopLifecycleSmoke --selftest] OK. all {len(EXPECTED)} regressions make the gate red.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or exercise the production bundle in a real browser."""
    if "--selftest" in argv:
        return selftest()
    result, diagnostic = measurement("lifecycle")
    if result is None:
        print(f"[desktopLifecycleSmoke] FAIL. {diagnostic}", file=sys.stderr)
        return 2
    if diagnostic.strip():
        print(diagnostic, file=sys.stderr, end="" if diagnostic.endswith("\n") else "\n")
    found = problems(result)
    if found:
        print("[desktopLifecycleSmoke] FAIL. desktop lifecycle regressions found:", file=sys.stderr)
        for name in found:
            print(f"  - {name}", file=sys.stderr)
        return 2
    print("[desktopLifecycleSmoke] OK. start, resume, nonblocking input, and confirmed removal work.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
