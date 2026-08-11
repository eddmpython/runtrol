"""Gate: the real VS Code Extension Host stays inside one checked-in performance budget.

The measurement launches an isolated VS Code profile, the production extension bundle, and a tracked Core
daemon. It measures ready activation, opening the contributed view, repeated session refresh p95, and Extension
Host RSS growth. The JSON budget beside the extension is the only threshold source used by both this gate and the
in-host test.

Usage::

    python -X utf8 tests/audit/vscodeHostPerformance.py --selftest
    python -X utf8 tests/audit/vscodeHostPerformance.py
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"
BUDGET_PATH = EXTENSION / "performance-budget.json"
MARKER = "RUNTROL_VSCODE_HOST "
FIELDS = (
    "activationMs",
    "openViewMs",
    "refreshP95Ms",
    "rssGrowthBytes",
    "webviewFrameP95Ms",
    "webviewFrameOverrunP95Ms",
    "webviewInputP95Ms",
    "webviewScrollP95Ms",
    "webviewPendingFrames",
)


def loadBudget() -> dict[str, float]:
    """Read and validate the shared ratchet."""
    raw = json.loads(BUDGET_PATH.read_text(encoding="utf-8"))
    if set(raw) != set(FIELDS):
        raise ValueError(f"{BUDGET_PATH.relative_to(ROOT)} must contain exactly {', '.join(FIELDS)}")
    budget: dict[str, float] = {}
    for name in FIELDS:
        value = raw[name]
        if isinstance(value, bool) or not isinstance(value, (int, float)) or value <= 0:
            raise ValueError(f"budget {name} must be a positive number")
        budget[name] = float(value)
    return budget


def problems(metrics: dict[str, Any], budget: dict[str, float]) -> list[str]:
    """Return every missing or over-budget measurement."""
    found: list[str] = []
    for name in FIELDS:
        value = metrics.get(name)
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            found.append(f"{name} is missing or not numeric")
        elif value > budget[name]:
            found.append(f"{name} {value:.1f} exceeds {budget[name]:.1f}")
    return found


def selftest() -> int:
    """Prove each missing and regressed metric makes the gate red independently."""
    budget = loadBudget()
    green = dict(budget)
    if problems(green, budget):
        print("[vscodeHostPerformance --selftest] FAIL. exact budgets were rejected.", file=sys.stderr)
        return 2
    for name in FIELDS:
        regressed = dict(green)
        regressed[name] = budget[name] + 0.1
        if problems(regressed, budget) != [f"{name} {regressed[name]:.1f} exceeds {budget[name]:.1f}"]:
            print(f"[vscodeHostPerformance --selftest] FAIL. {name} regression escaped.", file=sys.stderr)
            return 2
        missing = dict(green)
        missing.pop(name)
        if problems(missing, budget) != [f"{name} is missing or not numeric"]:
            print(f"[vscodeHostPerformance --selftest] FAIL. missing {name} escaped.", file=sys.stderr)
            return 2
    print("[vscodeHostPerformance --selftest] OK. all eighteen injected defects make the gate red.")
    return 0


def runCommand(command: list[str], cwd: Path, environment: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    """Run one visible gate command and retain output for marker parsing."""
    result = subprocess.run(
        command,
        cwd=cwd,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.stdout:
        print(result.stdout, end="" if result.stdout.endswith("\n") else "\n")
    if result.stderr:
        print(result.stderr, file=sys.stderr, end="" if result.stderr.endswith("\n") else "\n")
    return result


def productBinary() -> Path:
    """Build the current product Core and return its platform path."""
    target = ROOT / "target" / "vscode-performance"
    built = runCommand(
        [
            "cargo",
            "build",
            "-p",
            "runtrol",
            "--bin",
            "runtrol",
            "--no-default-features",
            "--target-dir",
            str(target),
        ],
        ROOT,
    )
    if built.returncode != 0:
        raise RuntimeError(f"cargo build returned {built.returncode}")
    suffix = ".exe" if sys.platform == "win32" else ""
    binary = target / "debug" / f"runtrol{suffix}"
    if not binary.is_file():
        raise RuntimeError(f"built Core is missing at {binary}")
    return binary


def hostCommand() -> list[str]:
    """Use a virtual display only where a Linux host has no real display."""
    npm = shutil.which("npm.cmd" if sys.platform == "win32" else "npm")
    if not npm:
        raise RuntimeError("npm is required to test the VS Code extension")
    command = [npm, "run", "test:host"]
    if sys.platform.startswith("linux") and not os.environ.get("DISPLAY"):
        xvfb = shutil.which("xvfb-run")
        if not xvfb:
            raise RuntimeError("xvfb-run is required to test VS Code without a Linux display")
        return [xvfb, "-a", *command]
    return command


def measurement(binary: Path) -> dict[str, Any]:
    """Run the isolated Extension Host and parse its single result record."""
    environment = dict(os.environ)
    environment["RUNTROL_TEST_CORE"] = str(binary)
    result = runCommand(hostCommand(), EXTENSION, environment)
    if result.returncode != 0:
        raise RuntimeError(f"VS Code Extension Host returned {result.returncode}")
    records = [line[len(MARKER):] for line in result.stdout.splitlines() if line.startswith(MARKER)]
    if len(records) != 1:
        raise RuntimeError(f"expected one {MARKER.strip()} record, found {len(records)}")
    value = json.loads(records[0])
    if not isinstance(value, dict):
        raise RuntimeError("the VS Code Extension Host record is not an object")
    return value


def run() -> int:
    """Build, measure, and enforce the shared budget."""
    try:
        budget = loadBudget()
        metrics = measurement(productBinary())
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError) as error:
        print(f"[vscodeHostPerformance] FAIL. {error}", file=sys.stderr)
        return 2
    found = problems(metrics, budget)
    if found:
        print("[vscodeHostPerformance] FAIL. the Extension Host ratchet regressed.", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print(
        "[vscodeHostPerformance] OK. "
        f"activation {metrics['activationMs']:.1f} ms, view {metrics['openViewMs']:.1f} ms, "
        f"refresh p95 {metrics['refreshP95Ms']:.1f} ms, RSS growth {metrics['rssGrowthBytes']:.0f} bytes, "
        f"Webview frame {metrics['webviewFrameP95Ms']:.1f} ms "
        f"(baseline {metrics['webviewBaselineFrameP95Ms']:.1f}, overrun {metrics['webviewFrameOverrunP95Ms']:.1f}), "
        f"input {metrics['webviewInputP95Ms']:.1f} ms, "
        f"scroll {metrics['webviewScrollP95Ms']:.1f} ms, pending {metrics['webviewPendingFrames']:.0f}."
    )
    return 0


def main(argv: list[str]) -> int:
    """Select selftest or the real gate."""
    if argv == ["--selftest"]:
        return selftest()
    if argv:
        print("usage: vscodeHostPerformance.py [--selftest]", file=sys.stderr)
        return 1
    return run()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
