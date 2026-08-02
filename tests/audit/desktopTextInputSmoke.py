"""Gate: the production Astryx composer preserves IME input and selectable text.

Hosted runners do not promise a configured Korean input method or an interactive desktop. This gate therefore
drives composition events through the production bundle in a real browser on every run. The Windows-only
``desktopImeDrive.ps1`` companion drives the operating system IME and clipboard when an interactive desktop is
available.

Usage::

    python -X utf8 tests/audit/desktopTextInputSmoke.py --selftest
    python -X utf8 tests/audit/desktopTextInputSmoke.py
"""

from __future__ import annotations

import sys
from typing import Any

from desktopPerformance import measurement

EXPECTED = (
    "draftPreservedDuringComposition",
    "composingEnterBlocked",
    "commitEndedEnterBlocked",
    "commitNativeDefaultAllowed",
    "commitParagraphBreakBlocked",
    "commitLineBreakBlocked",
    "commitBreakOneShot",
    "nonCancelableBreakIgnored",
    "unmatchedInputIgnored",
    "foreignTargetIgnored",
    "staleTimerPreservesNewGuard",
    "expiredGuardIgnored",
    "commitBreaksLeaveExactText",
    "commitFallbackTraceRecorded",
    "commitBreakTraceRecorded",
    "shiftedBreakAllowed",
    "ordinaryBreakAllowed",
    "tokenNodePreserved",
    "selectionCreated",
    "copyEventReached",
    "listenerSingleAfterSessionSwitch",
    "normalEnterSubmitted",
    "unmountCleanup",
    "compositionStartSessionSwitchReset",
    "compositionEndSessionSwitchReset",
    "editableRemountCompositionReset",
)


def problems(result: dict[str, Any]) -> list[str]:
    """Return every text behaviour absent from one browser journey."""
    return [name for name in EXPECTED if result.get(name) is not True]


def selftest() -> int:
    """Prove every independent text regression makes this gate red."""
    green = {name: True for name in EXPECTED}
    if problems(green):
        print("[desktopTextInputSmoke --selftest] FAIL. a green fixture was rejected.", file=sys.stderr)
        return 2
    for name in EXPECTED:
        fixture = dict(green)
        fixture[name] = False
        if problems(fixture) != [name]:
            print(f"[desktopTextInputSmoke --selftest] FAIL. {name} escaped.", file=sys.stderr)
            return 2
    print(f"[desktopTextInputSmoke --selftest] OK. all {len(EXPECTED)} text regressions make the gate red.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or exercise the production bundle."""
    if "--selftest" in argv:
        return selftest()
    result, diagnostic = measurement("text-input")
    if result is None:
        print(f"[desktopTextInputSmoke] FAIL. {diagnostic}", file=sys.stderr)
        return 2
    if diagnostic.strip():
        print(diagnostic, file=sys.stderr, end="" if diagnostic.endswith("\n") else "\n")
    found = problems(result)
    if found:
        print("[desktopTextInputSmoke] FAIL. desktop text regressions found:", file=sys.stderr)
        for name in found:
            print(f"  - {name}", file=sys.stderr)
        return 2
    print("[desktopTextInputSmoke] OK. composition Enter, selection, copy, and submit stay distinct.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
