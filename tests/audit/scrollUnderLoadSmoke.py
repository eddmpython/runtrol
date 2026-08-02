"""Gate: the production desktop stays interactive during a 3,000-frame-per-second stream.

The real browser receives raw provider-shaped frames through the same bridge the Tauri window uses.
The gate requires the full offered rate, a 60 Hz p95 frame budget with measured headroom, responsive
composition input, and a bounded DOM window.

Usage::

    python -X utf8 tests/audit/scrollUnderLoadSmoke.py --selftest
    python -X utf8 tests/audit/scrollUnderLoadSmoke.py
"""

from __future__ import annotations

import json
import sys
from typing import Any

from desktopPerformance import measurement, missingNumbers

LIMITS = {
    "producedMinimum": 29_300,
    "inputSamplesMinimum": 90,
    "frameP95Ms": 24.0,
    "frameMaxMs": 120.0,
    "inputP95Ms": 50.0,
    "renderedMessages": 48,
    "renderedCharacters": 64 * 1024,
}


def problems(metrics: dict[str, Any]) -> list[str]:
    """Return every throughput, latency, or render-bound failure."""
    names = (
        "produced",
        "inputSamples",
        "frameP95Ms",
        "frameMaxMs",
        "inputP95Ms",
        "renderedMessages",
        "renderedCharacters",
    )
    missing = missingNumbers(metrics, names)
    found = [f"{name} is missing or not numeric" for name in missing]
    produced = metrics.get("produced")
    if isinstance(produced, (int, float)) and produced < LIMITS["producedMinimum"]:
        found.append(f"produced {produced:.0f} frames is below {LIMITS['producedMinimum']}")
    input_samples = metrics.get("inputSamples")
    if isinstance(input_samples, (int, float)) and input_samples < LIMITS["inputSamplesMinimum"]:
        found.append(
            f"inputSamples {input_samples:.0f} is below {LIMITS['inputSamplesMinimum']}"
        )
    for name in names[2:]:
        value = metrics.get(name)
        limit = LIMITS[name]
        if isinstance(value, (int, float)) and value > limit:
            found.append(f"{name} {value:.1f} exceeds {limit:.1f}")
    return found


def selftest() -> int:
    """Prove each independent load contract can make the gate red."""
    green = {
        "produced": LIMITS["producedMinimum"],
        "inputSamples": LIMITS["inputSamplesMinimum"],
        "frameP95Ms": LIMITS["frameP95Ms"],
        "frameMaxMs": LIMITS["frameMaxMs"],
        "inputP95Ms": LIMITS["inputP95Ms"],
        "renderedMessages": LIMITS["renderedMessages"],
        "renderedCharacters": LIMITS["renderedCharacters"],
    }
    if problems(green):
        print("[scrollUnderLoadSmoke --selftest] FAIL. exact limits were rejected.", file=sys.stderr)
        return 2
    for name in tuple(green):
        fixture = dict(green)
        fixture[name] += -1 if name in {"produced", "inputSamples"} else 1
        if len(problems(fixture)) != 1:
            print(f"[scrollUnderLoadSmoke --selftest] FAIL. {name} failure escaped.", file=sys.stderr)
            return 2
    print("[scrollUnderLoadSmoke --selftest] OK. all six failures make the gate red.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or drive the production bundle under load."""
    if "--selftest" in argv:
        return selftest()
    metrics, diagnostic = measurement("scroll")
    if metrics is None:
        print(f"[scrollUnderLoadSmoke] FAIL. {diagnostic}", file=sys.stderr)
        return 2
    if diagnostic.strip():
        print(diagnostic, file=sys.stderr, end="" if diagnostic.endswith("\n") else "\n")
    found = problems(metrics)
    if found:
        print("[scrollUnderLoadSmoke] FAIL. the loaded desktop missed its contract.", file=sys.stderr)
        print(f"  metrics: {json.dumps(metrics, sort_keys=True)}", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print(
        "[scrollUnderLoadSmoke] OK. "
        f"{metrics['produced']:.0f} frames, p95 {metrics['frameP95Ms']:.1f} ms, "
        f"input p95 {metrics['inputP95Ms']:.1f} ms over {metrics['inputSamples']:.0f} samples, "
        f"{metrics['renderedMessages']:.0f} DOM messages."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
