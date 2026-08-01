"""Gate: the production desktop bundle stays inside its local interaction ratchet.

The browser gets a transport-only mock so the measurements belong to the GUI, not a model or the
network. The page is the real production bundle, rendered by an installed Edge or Chrome binary.
The checked-in limits start above the first three-run measurements and may only move down.

Usage::

    python -X utf8 tests/audit/interactionLatencyBudget.py --selftest
    python -X utf8 tests/audit/interactionLatencyBudget.py
"""

from __future__ import annotations

import sys
from typing import Any

from desktopPerformance import measurement, missingNumbers

BUDGET_MS = {
    "listPaintP95Ms": 900.0,
    "sessionOpenMs": 100.0,
    "inputP95Ms": 50.0,
}


def problems(metrics: dict[str, Any]) -> list[str]:
    """Return every missing or regressed interaction measurement."""
    missing = missingNumbers(metrics, tuple(BUDGET_MS))
    found = [f"{name} is missing or not numeric" for name in missing]
    for name, budget in BUDGET_MS.items():
        value = metrics.get(name)
        if isinstance(value, (int, float)) and value > budget:
            found.append(f"{name} {value:.1f} ms exceeds {budget:.1f} ms")
    return found


def selftest() -> int:
    """Prove every budget can make the gate red independently."""
    green = {name: budget for name, budget in BUDGET_MS.items()}
    if problems(green):
        print("[interactionLatencyBudget --selftest] FAIL. exact budgets were rejected.", file=sys.stderr)
        return 2
    for name, budget in BUDGET_MS.items():
        fixture = dict(green)
        fixture[name] = budget + 0.1
        if len(problems(fixture)) != 1:
            print(f"[interactionLatencyBudget --selftest] FAIL. {name} regression escaped.", file=sys.stderr)
            return 2
    fixture = dict(green)
    fixture.pop("sessionOpenMs")
    if problems(fixture) != ["sessionOpenMs is missing or not numeric"]:
        print("[interactionLatencyBudget --selftest] FAIL. missing metrics escaped.", file=sys.stderr)
        return 2
    print("[interactionLatencyBudget --selftest] OK. all three regressions make the gate red.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or measure the production desktop bundle."""
    if "--selftest" in argv:
        return selftest()
    metrics, diagnostic = measurement("interaction")
    if metrics is None:
        print(f"[interactionLatencyBudget] FAIL. {diagnostic}", file=sys.stderr)
        return 2
    if diagnostic.strip():
        print(diagnostic, file=sys.stderr, end="" if diagnostic.endswith("\n") else "\n")
    found = problems(metrics)
    if found:
        print("[interactionLatencyBudget] FAIL. the desktop interaction ratchet regressed.", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print(
        "[interactionLatencyBudget] OK. "
        f"list p95 {metrics['listPaintP95Ms']:.1f} ms, "
        f"session {metrics['sessionOpenMs']:.1f} ms, input p95 {metrics['inputP95Ms']:.1f} ms."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
