"""Gate: rendered conversation frames disappear when the desktop page reloads.

Usage::

    python -X utf8 tests/audit/desktopPersistenceSmoke.py --selftest
    python -X utf8 tests/audit/desktopPersistenceSmoke.py
"""

from __future__ import annotations

import sys
from typing import Any

from desktopPerformance import measurement

EXPECTED = (
    "frameGoneAfterReload",
    "onlyScalarPreferences",
    "sessionStorageEmpty",
    "indexedDbEmpty",
    "cacheStorageEmpty",
)


def problems(result: dict[str, Any]) -> list[str]:
    """Return every durable browser surface that retained conversation state."""
    return [name for name in EXPECTED if result.get(name) is not True]


def selftest() -> int:
    """Prove every persistence regression can make the gate red."""
    green = {name: True for name in EXPECTED}
    if problems(green):
        print("[desktopPersistenceSmoke --selftest] FAIL. a green fixture was rejected.", file=sys.stderr)
        return 2
    for name in EXPECTED:
        fixture = dict(green)
        fixture[name] = False
        if problems(fixture) != [name]:
            print(f"[desktopPersistenceSmoke --selftest] FAIL. {name} escaped.", file=sys.stderr)
            return 2
    print("[desktopPersistenceSmoke --selftest] OK. all five persistence regressions make the gate red.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or reload the production bundle in a real browser."""
    if "--selftest" in argv:
        return selftest()
    result, diagnostic = measurement("persistence")
    if result is None:
        print(f"[desktopPersistenceSmoke] FAIL. {diagnostic}", file=sys.stderr)
        return 2
    if diagnostic.strip():
        print(diagnostic, file=sys.stderr, end="" if diagnostic.endswith("\n") else "\n")
    found = problems(result)
    if found:
        print("[desktopPersistenceSmoke] FAIL. the desktop retained conversation state:", file=sys.stderr)
        for name in found:
            print(f"  - {name}", file=sys.stderr)
        return 2
    print("[desktopPersistenceSmoke] OK. frames vanish on reload and only two scalar preferences remain.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
