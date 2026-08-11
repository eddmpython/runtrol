"""Gate: the real VS Code Extension Host stays inside one checked-in performance budget.

The measurement launches an isolated VS Code profile, the production extension bundle, and a tracked Core
daemon. It measures ready activation, opening the contributed view, repeated session refresh p95, Extension Host RSS
growth, 30 managed external ACP sessions with at most eight hot, a real cold-session resume, selected-watch plus
Webview-paint switching, and exact selection restoration after VS Code restarts in another workspace. The JSON
budget beside the extension is the only threshold source used by both this gate and the in-host test.

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
    "coldResumeMs",
    "sessionSwitchP95Ms",
    "reloadRestoreMs",
)
EXPECTED_HOT_SESSIONS = 8
EXPECTED_MANAGED_SESSIONS = 30
EXPECTED_DROPPED_FRAMES = 0


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
    if metrics.get("hotSessionCount") != EXPECTED_HOT_SESSIONS:
        found.append(
            f"hotSessionCount {metrics.get('hotSessionCount')!r} is not {EXPECTED_HOT_SESSIONS}"
        )
    if metrics.get("sessionCount") != EXPECTED_MANAGED_SESSIONS:
        found.append(
            f"sessionCount {metrics.get('sessionCount')!r} is not {EXPECTED_MANAGED_SESSIONS}"
        )
    if metrics.get("webviewDroppedFrames") != EXPECTED_DROPPED_FRAMES:
        found.append(
            f"webviewDroppedFrames {metrics.get('webviewDroppedFrames')!r} is not "
            f"{EXPECTED_DROPPED_FRAMES}"
        )
    return found


def selftest() -> int:
    """Prove each missing and regressed metric makes the gate red independently."""
    budget = loadBudget()
    green = {
        **budget,
        "hotSessionCount": EXPECTED_HOT_SESSIONS,
        "sessionCount": EXPECTED_MANAGED_SESSIONS,
        "webviewDroppedFrames": EXPECTED_DROPPED_FRAMES,
    }
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
    wrong_count = dict(green)
    wrong_count_value = EXPECTED_HOT_SESSIONS - 1
    wrong_count["hotSessionCount"] = wrong_count_value
    expected_count_problem = (
        f"hotSessionCount {wrong_count_value!r} is not {EXPECTED_HOT_SESSIONS}"
    )
    if problems(wrong_count, budget) != [expected_count_problem]:
        print("[vscodeHostPerformance --selftest] FAIL. a missing hot session escaped.", file=sys.stderr)
        return 2
    wrong_managed = dict(green)
    wrong_managed_value = EXPECTED_MANAGED_SESSIONS - 1
    wrong_managed["sessionCount"] = wrong_managed_value
    expected_managed_problem = (
        f"sessionCount {wrong_managed_value!r} is not {EXPECTED_MANAGED_SESSIONS}"
    )
    if problems(wrong_managed, budget) != [expected_managed_problem]:
        print("[vscodeHostPerformance --selftest] FAIL. a missing managed session escaped.", file=sys.stderr)
        return 2
    dropped = dict(green)
    dropped["webviewDroppedFrames"] = 1
    if problems(dropped, budget) != ["webviewDroppedFrames 1 is not 0"]:
        print("[vscodeHostPerformance --selftest] FAIL. a dropped frame escaped.", file=sys.stderr)
        return 2
    print("[vscodeHostPerformance --selftest] OK. all twenty-seven injected defects make the gate red.")
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


def productBinaries() -> tuple[Path, Path]:
    """Build the current product Core and the external ACP fixture."""
    target = ROOT / "target" / "vscode-performance"
    commands = (
        [
            "cargo", "build", "-p", "runtrol", "--bin", "runtrol",
            "--no-default-features", "--target-dir", str(target),
        ],
        [
            "cargo", "build", "-p", "runtrol-drivers", "--example", "acpFixture",
            "--target-dir", str(target),
        ],
    )
    for command in commands:
        built = runCommand(command, ROOT)
        if built.returncode != 0:
            raise RuntimeError(f"cargo build returned {built.returncode}")
    suffix = ".exe" if sys.platform == "win32" else ""
    binary = target / "debug" / f"runtrol{suffix}"
    fixture = target / "debug" / "examples" / f"acpFixture{suffix}"
    for expected in (binary, fixture):
        if not expected.is_file():
            raise RuntimeError(f"built performance binary is missing at {expected}")
    return binary, fixture


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


def measurement(binary: Path, fixture: Path) -> dict[str, Any]:
    """Run the isolated Extension Host and parse its single result record."""
    environment = dict(os.environ)
    environment["RUNTROL_TEST_CORE"] = str(binary)
    environment["RUNTROL_TEST_ACP_FIXTURE"] = str(fixture)
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
        metrics = measurement(*productBinaries())
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
        f"scroll {metrics['webviewScrollP95Ms']:.1f} ms, pending {metrics['webviewPendingFrames']:.0f}, "
        f"{metrics['sessionCount']:.0f} managed, {metrics['hotSessionCount']:.0f} hot, "
        f"cold resume {metrics['coldResumeMs']:.1f} ms, "
        f"hot-session switch p95 {metrics['sessionSwitchP95Ms']:.1f} ms, "
        f"reload restore {metrics['reloadRestoreMs']:.1f} ms."
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
