"""Gate: the headless phone PWA drives a real installed provider through the production daemon.

The phone process uses the shipped WebCrypto, Noise, record, and Core client modules. The daemon substitutes only
the already approved device row, then applies production authentication, scope, workspace, provider, session, and
event-watch behavior. The provider CLI is real and its model endpoint is a bounded loopback fixture that discards
request bodies.

Usage::

    python -X utf8 tests/audit/phoneDrivesPcSmoke.py
    python -X utf8 tests/audit/phoneDrivesPcSmoke.py --require-external
    python -X utf8 tests/audit/phoneDrivesPcSmoke.py --selftest
"""

from __future__ import annotations

import os
import signal
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, replace
from pathlib import Path

import claudeApprovalSmoke as approval
import missionLiveJourney as mission

ROOT = Path(__file__).resolve().parents[2]
TIMEOUT_S = 150.0


class Failed(Exception):
    """The live phone journey did not hold."""


@dataclass(frozen=True)
class Evidence:
    """Bounded process facts retained after the live journey."""

    cli_probed: bool
    cargo_passed: bool
    model_requests: int
    expected_requests: int
    sentinel_auth: bool
    endpoint_contract: bool
    target_absent: bool
    cleanup_complete: bool


def verifyEvidence(evidence: Evidence) -> None:
    """Require every independent phone-to-PC boundary."""
    if not evidence.cli_probed:
        raise Failed("the installed provider parser and version were not probed")
    if not evidence.cargo_passed:
        raise Failed("the headless PWA and production daemon journey failed")
    if evidence.model_requests != evidence.expected_requests:
        raise Failed(
            f"the provider made {evidence.model_requests} model requests, expected {evidence.expected_requests}"
        )
    if not evidence.sentinel_auth:
        raise Failed("the model fixture observed a credential other than its fixed sentinel")
    if not evidence.endpoint_contract:
        raise Failed("the real provider used an unexpected model endpoint")
    if not evidence.target_absent:
        raise Failed("the phone-denied file change reached disk")
    if not evidence.cleanup_complete:
        raise Failed("a process started by the live phone gate survived cleanup")


def selftest(mode: str, label: str) -> int:
    """Prove every retained acceptance fact can make the gate red."""
    expected = {"drive": 1, "approval": 2, "resilience": 3}[mode]
    valid = Evidence(True, True, expected, expected, True, True, True, True)
    defects = (
        replace(valid, cli_probed=False),
        replace(valid, cargo_passed=False),
        replace(valid, model_requests=expected - 1),
        replace(valid, model_requests=expected + 1),
        replace(valid, sentinel_auth=False),
        replace(valid, endpoint_contract=False),
        replace(valid, target_absent=False),
        replace(valid, cleanup_complete=False),
    )
    try:
        verifyEvidence(valid)
        for defect in defects:
            try:
                verifyEvidence(defect)
            except Failed:
                # ok: rejection is the assertion, and the next independent mutation is still checked.
                continue
            raise Failed(f"an injected evidence defect escaped: {defect}")
    except Failed as error:
        print(f"[{label}:selftest] FAIL: {error}", file=sys.stderr)
        return 2
    print(f"[{label}:selftest] OK. all {len(defects)} evidence mutations made the gate red.")
    return 0


def stopObserved(pids: set[int]) -> bool:
    """Terminate only exact process identifiers observed under this gate's cargo child."""
    alive = pids.intersection(approval.processTable())
    alive.discard(os.getpid())
    for pid in sorted(alive, reverse=True):
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            # ok: the exact observed process already exited, and survivor inspection follows.
            continue
    survivors = approval.waitGone(alive)
    for pid in sorted(survivors, reverse=True):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            # ok: the exact observed process exited between inspection and the final signal.
            continue
    return not approval.waitGone(survivors)


def exercise(mode: str, require_external: bool) -> None:
    """Run one exact headless phone mode and leave no child process alive."""
    claude = approval.claudeProgram()
    node = shutil.which("node")
    if claude is None or node is None:
        if require_external:
            missing = "Claude Code" if claude is None else "Node.js"
            raise Failed(f"required external program is absent: {missing}")
        print("[phoneDrivesPcSmoke] SKIP. the optional installed provider journey is unavailable.")
        return

    with tempfile.TemporaryDirectory(prefix=f"runtrol-phone-{mode}-") as raw_root:
        root = Path(raw_root)
        home = root / "runtrol-home"
        workspace = root / "workspace"
        config = root / "claude-config"
        target = workspace / "must-not-exist.txt"
        workspace.mkdir()
        config.mkdir()
        model_context = (
            approval.RunningModel(target)
            if mode == "approval"
            else mission.RunningClaudeModel()
        )
        expected_requests = {"drive": 1, "approval": 2, "resilience": 3}[mode]
        with model_context as model:
            env = approval.environment(root, home, config, model, claude)
            operator_home = Path.home()
            env["CARGO_HOME"] = os.environ.get("CARGO_HOME", str(operator_home / ".cargo"))
            env["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(operator_home / ".rustup"))
            env["RUNTROL_PHONE_LIVE_MODE"] = mode
            env["RUNTROL_PHONE_LIVE_WORKSPACE"] = str(workspace)
            env["RUNTROL_PHONE_LIVE_PROVIDER"] = approval.PROVIDER
            env["RUNTROL_PHONE_LIVE_NODE"] = node
            cli_probed = approval.probeClaude(claude, env)
            test_name = {
                "drive": "serve::tests::phone_drives_pc_through_a_real_cli",
                "approval": "serve::tests::phone_approval_resumes_a_real_cli",
                "resilience": "serve::tests::phone_survives_network_and_core_restart",
            }[mode]
            command = ["cargo", "test", "-p", "runtrol-daemon", test_name, "--", "--exact", "--nocapture"]
            output = root / "cargo-output.txt"
            with output.open("w+", encoding="utf-8") as captured:
                child = subprocess.Popen(
                    command,
                    cwd=ROOT,
                    env=env,
                    stdin=subprocess.DEVNULL,
                    stdout=captured,
                    stderr=subprocess.STDOUT,
                    text=True,
                )
                observed = {child.pid}
                deadline = time.monotonic() + TIMEOUT_S
                while child.poll() is None and time.monotonic() < deadline:
                    observed.update(approval.descendants(child.pid))
                    time.sleep(0.05)
                if child.poll() is None:
                    observed.update(approval.descendants(child.pid))
                    child.terminate()
                    try:
                        child.wait(timeout=5.0)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=5.0)
                    if not stopObserved(observed):
                        raise Failed("the timed-out phone journey left a child process alive")
                    raise Failed("the headless phone journey timed out")
                captured.flush()
                captured.seek(0)
                detail = captured.read()
            observed.update(approval.descendants(child.pid))
            cleanup_complete = not approval.waitGone(observed)
            evidence = Evidence(
                cli_probed=cli_probed,
                cargo_passed=child.returncode == 0,
                model_requests=model.requests,
                expected_requests=expected_requests,
                sentinel_auth=model.server.sentinel_auth,
                endpoint_contract=model.server.endpoint_contract,
                target_absent=not target.exists(),
                cleanup_complete=cleanup_complete,
            )
            try:
                verifyEvidence(evidence)
            except Failed as error:
                tail = detail[-4_000:].strip()
                raise Failed(f"{error}\n{tail}") from error


def run(mode: str, args: list[str], label: str) -> int:
    """Shared entry point for the drive and approval gate names."""
    if args == ["--selftest"]:
        return selftest(mode, label)
    if args not in ([], ["--require-external"]):
        print(f"usage: {label}.py [--require-external|--selftest]", file=sys.stderr)
        return 2
    try:
        exercise(mode, args == ["--require-external"])
    except (Failed, OSError, subprocess.SubprocessError) as error:
        print(f"[{label}] FAIL: {error}", file=sys.stderr)
        return 2
    print(f"[{label}] OK. the headless phone completed the real {mode} journey and cleaned up.")
    return 0


if __name__ == "__main__":
    raise SystemExit(run("drive", sys.argv[1:], "phoneDrivesPcSmoke"))
