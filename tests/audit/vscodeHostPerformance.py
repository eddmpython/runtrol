"""Gate: the real VS Code Extension Host stays inside one checked-in performance budget.

The measurement launches an isolated profile on the exact tested VS Code version, the production extension bundle,
and a tracked Core daemon. Three isolated trials measure ready activation, opening the contributed view, session refresh p95,
Extension Host RSS growth, 30 managed external ACP sessions with at most eight hot, a real cold-session resume,
selected-watch plus Webview-paint switching, and exact selection restoration after VS Code restarts in another
workspace. The median rejects one hosted-runner scheduling outlier without relaxing the checked-in JSON thresholds.

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
HOST_TEST_PATH = EXTENSION / "src" / "integration" / "extensionHost.test.ts"
APPROVAL_DRIVER_PATH = EXTENSION / "tooling" / "integration-approval.mjs"
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
MEASUREMENT_TRIALS = 3
INITIALIZATION_TIMEOUT_DECLARATION = "const EXTENSION_INITIALIZATION_HANG_TIMEOUT_MS = 15_000;"
INITIALIZATION_TIMEOUT_USE = "within(api.ready, EXTENSION_INITIALIZATION_HANG_TIMEOUT_MS"


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


def medianMeasurements(measurements: list[dict[str, Any]]) -> dict[str, Any]:
    """Aggregate three complete isolated trials without weakening exact invariants."""
    if len(measurements) != MEASUREMENT_TRIALS:
        raise ValueError(f"expected {MEASUREMENT_TRIALS} VS Code measurements, found {len(measurements)}")
    result = dict(measurements[-1])
    for name in FIELDS:
        values = [measurement.get(name) for measurement in measurements]
        if any(isinstance(value, bool) or not isinstance(value, (int, float)) for value in values):
            raise ValueError(f"every VS Code measurement must contain numeric {name}")
        result[name] = sorted(float(value) for value in values)[MEASUREMENT_TRIALS // 2]
    exact = {
        "hotSessionCount": EXPECTED_HOT_SESSIONS,
        "sessionCount": EXPECTED_MANAGED_SESSIONS,
        "webviewDroppedFrames": EXPECTED_DROPPED_FRAMES,
    }
    for name, expected in exact.items():
        values = [measurement.get(name) for measurement in measurements]
        if any(value != expected for value in values):
            raise ValueError(f"every VS Code measurement must report {name}={expected}, found {values!r}")
        result[name] = expected
    return result


def hostContractProblems(source: str) -> list[str]:
    """Keep hang detection separate from the median performance ratchet."""
    found: list[str] = []
    if INITIALIZATION_TIMEOUT_DECLARATION not in source:
        found.append("the Extension Host initialization hang timeout is not the exact 15 second guard")
    if source.count(INITIALIZATION_TIMEOUT_USE) != 2:
        found.append("initial activation and reload do not share the initialization hang guard")
    return found


def approvalDriverProblems(source: str) -> list[str]:
    """Require the approval marker to follow graceful private-pipe retirement."""
    found: list[str] = []
    close_at = source.find("await connection.close();")
    publish_at = source.find("await writeFile(marker,")
    if close_at < 0 or publish_at < close_at:
        found.append("the approval marker can precede private IPC retirement")
    if "this.socket.end();" not in source:
        found.append("the approval driver does not end its private pipe gracefully")
    if 'this.socket.once("close", resolve);' not in source:
        found.append("the approval driver does not wait for the private pipe to close")
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
    measurements = [dict(green) for _unused in range(MEASUREMENT_TRIALS)]
    measurements[0]["activationMs"] = budget["activationMs"] * 10
    if problems(medianMeasurements(measurements), budget):
        print("[vscodeHostPerformance --selftest] FAIL. one scheduling outlier made the median red.", file=sys.stderr)
        return 2
    measurements[1]["activationMs"] = budget["activationMs"] * 10
    expected_median_problem = (
        f"activationMs {budget['activationMs'] * 10:.1f} exceeds {budget['activationMs']:.1f}"
    )
    if problems(medianMeasurements(measurements), budget) != [expected_median_problem]:
        print("[vscodeHostPerformance --selftest] FAIL. a majority activation regression escaped.", file=sys.stderr)
        return 2
    incomplete_rejected = False
    try:
        medianMeasurements([dict(green), dict(green)])
    except ValueError:
        incomplete_rejected = True
    if not incomplete_rejected:
        print("[vscodeHostPerformance --selftest] FAIL. an incomplete trial set was accepted.", file=sys.stderr)
        return 2
    host_source = (
        f"{INITIALIZATION_TIMEOUT_DECLARATION}\n"
        f"{INITIALIZATION_TIMEOUT_USE}, 'initial');\n"
        f"{INITIALIZATION_TIMEOUT_USE}, 'reload');\n"
    )
    if hostContractProblems(host_source):
        print("[vscodeHostPerformance --selftest] FAIL. the host contract fixture was rejected.", file=sys.stderr)
        return 2
    host_mutations = (
        host_source.replace("15_000", "5_000"),
        host_source.replace(f"{INITIALIZATION_TIMEOUT_USE}, 'reload');\n", ""),
    )
    if any(not hostContractProblems(mutation) for mutation in host_mutations):
        print("[vscodeHostPerformance --selftest] FAIL. a host timeout defect escaped.", file=sys.stderr)
        return 2
    approval_source = (
        "let settledIntegration = null;\n"
        "await connection.close();\n"
        "await writeFile(marker, integrationId);\n"
        'this.socket.once("close", resolve);\n'
        "this.socket.end();\n"
    )
    if approvalDriverProblems(approval_source):
        print("[vscodeHostPerformance --selftest] FAIL. the approval driver fixture was rejected.", file=sys.stderr)
        return 2
    approval_mutations = (
        approval_source.replace("await connection.close();", "connection.close();"),
        approval_source.replace("this.socket.end();", "this.socket.destroy();"),
        approval_source.replace('this.socket.once("close", resolve);', ""),
    )
    if any(not approvalDriverProblems(mutation) for mutation in approval_mutations):
        print("[vscodeHostPerformance --selftest] FAIL. a private pipe retirement defect escaped.", file=sys.stderr)
        return 2
    print(
        "[vscodeHostPerformance --selftest] OK. all injected defects, host guards, and trial aggregation "
        "make the gate red."
    )
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
            "--target-dir", str(target),
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


def singleMeasurement(binary: Path, fixture: Path) -> dict[str, Any]:
    """Run one isolated Extension Host and parse its single result record."""
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


def measurement(binary: Path, fixture: Path) -> dict[str, Any]:
    """Use the median of three isolated cold trials as the shared-host ratchet value."""
    measured: list[dict[str, Any]] = []
    for trial in range(1, MEASUREMENT_TRIALS + 1):
        print(f"[vscodeHostPerformance] trial {trial}/{MEASUREMENT_TRIALS}")
        measured.append(singleMeasurement(binary, fixture))
    return medianMeasurements(measured)


def run() -> int:
    """Build, measure, and enforce the shared budget."""
    try:
        budget = loadBudget()
        host_problems = hostContractProblems(HOST_TEST_PATH.read_text(encoding="utf-8"))
        if host_problems:
            raise RuntimeError("; ".join(host_problems))
        approval_problems = approvalDriverProblems(
            APPROVAL_DRIVER_PATH.read_text(encoding="utf-8")
        )
        if approval_problems:
            raise RuntimeError("; ".join(approval_problems))
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
