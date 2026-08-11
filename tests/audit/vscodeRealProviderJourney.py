"""Gate: an installed provider completes the full production VS Code control journey.

Claude Code, the runtrol daemon, the VS Code Extension Host, and the shipped extension bundle are real. Only the
Messages endpoint is a loopback deterministic fixture. It discards request bodies without parsing or retaining
prompts and spends no hosted model token.

Usage::

    python -X utf8 tests/audit/vscodeRealProviderJourney.py
    python -X utf8 tests/audit/vscodeRealProviderJourney.py --require-external
    python -X utf8 tests/audit/vscodeRealProviderJourney.py --selftest
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

import claudeApprovalSmoke as provider_gate
import genericAcpSmoke as process

ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"
MARKER = "RUNTROL_VSCODE_REAL_PROVIDER "
TIMEOUT_S = 240.0


class Failed(Exception):
    """The installed-provider VS Code journey did not hold."""


@dataclass(frozen=True)
class Evidence:
    """Bounded supervision facts from one isolated product journey."""

    cli_probed: bool
    provider_detected: bool
    model_requests: int
    sentinel_auth: bool
    endpoint_contract: bool
    approval_denied: bool
    reconnected: bool
    interrupted: bool
    interrupt_terminal: bool
    workspace_restored: bool
    sessions_closed: bool
    target_absent: bool
    cleanup_complete: bool


def verifyEvidence(evidence: Evidence) -> None:
    """Reject a journey that skipped any user-visible operation or exact cleanup."""
    checks = (
        (evidence.cli_probed, "the installed CLI parser and version were not probed"),
        (evidence.provider_detected, "the extension did not auto-discover the installed CLI"),
        (evidence.model_requests == 3, f"the real CLI made {evidence.model_requests} model requests, not three"),
        (evidence.sentinel_auth, "the loopback endpoint received a credential other than the sentinel"),
        (evidence.endpoint_contract, "the installed CLI used an unexpected Messages endpoint"),
        (evidence.approval_denied, "the extension did not carry the explicit approval denial"),
        (evidence.reconnected, "the selected watch did not restore after reconnect"),
        (evidence.interrupted, "the extension did not issue an interrupt"),
        (evidence.interrupt_terminal, "the provider did not declare a non-success terminal after interrupt"),
        (evidence.workspace_restored, "the selected session did not restore in its exact workspace"),
        (evidence.sessions_closed, "the extension did not close both exact sessions"),
        (evidence.target_absent, "the denied provider file change reached the filesystem"),
        (evidence.cleanup_complete, "a daemon, watcher, provider, or VS Code process survived cleanup"),
    )
    for held, message in checks:
        if not held:
            raise Failed(message)


def selftest() -> int:
    """Prove each evidence field and the exact request count can turn the gate red."""
    valid = Evidence(True, True, 3, True, True, True, True, True, True, True, True, True, True)
    try:
        verifyEvidence(valid)
    except Failed as error:
        print(f"[vscodeRealProviderJourney:selftest] FAIL. valid evidence was rejected: {error}", file=sys.stderr)
        return 2
    defects = [
        replace(valid, cli_probed=False),
        replace(valid, provider_detected=False),
        replace(valid, model_requests=2),
        replace(valid, model_requests=4),
        replace(valid, sentinel_auth=False),
        replace(valid, endpoint_contract=False),
        replace(valid, approval_denied=False),
        replace(valid, reconnected=False),
        replace(valid, interrupted=False),
        replace(valid, interrupt_terminal=False),
        replace(valid, workspace_restored=False),
        replace(valid, sessions_closed=False),
        replace(valid, target_absent=False),
        replace(valid, cleanup_complete=False),
    ]
    for defect in defects:
        try:
            verifyEvidence(defect)
        except Failed:
            # ok: rejection is the assertion, and the next injected defect is independent.
            continue
        print(f"[vscodeRealProviderJourney:selftest] FAIL. injected defect escaped: {defect}", file=sys.stderr)
        return 2
    print(f"[vscodeRealProviderJourney:selftest] OK. all {len(defects)} injected defects make the gate red.")
    return 0


def hostCommand(node: str) -> list[str]:
    """Run the Extension Host under a virtual display only when Linux has no display."""
    command = [node, str(EXTENSION / "tooling" / "real-provider-journey.mjs")]
    if sys.platform.startswith("linux") and not os.environ.get("DISPLAY"):
        xvfb = shutil.which("xvfb-run")
        if xvfb is None:
            raise Failed("xvfb-run is required to test VS Code without a Linux display")
        return [xvfb, "-a", *command]
    return command


def vscodeExecutable(node: str) -> Path:
    """Resolve or download VS Code before the provider environment denies non-loopback network traffic."""
    configured = os.environ.get("RUNTROL_TEST_VSCODE_EXECUTABLE")
    if configured:
        executable = Path(configured)
        if not executable.is_file():
            raise Failed(f"the configured VS Code executable is absent: {executable}")
        return executable.resolve()
    program = (
        "import { downloadAndUnzipVSCode } from '@vscode/test-electron';"
        "console.log(await downloadAndUnzipVSCode(process.env.RUNTROL_TEST_VSCODE_VERSION || 'stable'));"
    )
    prepared = subprocess.run(
        [node, "--input-type=module", "--eval", program],
        cwd=EXTENSION,
        env=os.environ,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=180.0,
        check=False,
    )
    if prepared.returncode != 0:
        raise Failed(f"VS Code preparation failed: {(prepared.stderr or prepared.stdout)[-4000:]}")
    lines = [line.strip() for line in prepared.stdout.splitlines() if line.strip()]
    executable = Path(lines[-1]) if lines else Path()
    if not executable.is_file():
        raise Failed("VS Code preparation returned no executable")
    return executable.resolve()


def resultRecord(output: str) -> dict[str, Any]:
    """Read the single bounded evidence record emitted by the host harness."""
    records = [line[len(MARKER):] for line in output.splitlines() if line.startswith(MARKER)]
    if len(records) != 1:
        raise Failed(f"expected one {MARKER.strip()} record, found {len(records)}")
    value = json.loads(records[0])
    if not isinstance(value, dict):
        raise Failed("the Extension Host evidence record is not an object")
    return value


def readSessionIds(result_path: Path) -> tuple[str, ...]:
    """Recover exact session identifiers for failure cleanup without retaining conversation data."""
    try:
        value = json.loads(result_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return ()
    if not isinstance(value, dict):
        return ()
    sessions = (value.get("firstSession"), value.get("secondSession"))
    return tuple(item for item in sessions if isinstance(item, str) and process.SESSION_RE.fullmatch(item))


def closeExactSessions(binary: Path, env: dict[str, str], sessions: tuple[str, ...]) -> bool:
    """Close only sessions named by the isolated journey checkpoint."""
    closed = True
    for session in reversed(sessions):
        result = subprocess.run(
            [str(binary), "close", session, "--now"],
            cwd=ROOT,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=15.0,
            check=False,
        )
        closed = result.returncode == 0 and closed
    return closed


def exercise(claude: str) -> None:
    """Drive start, prompt, approval, interrupt, reconnect, switch, restore, and close through VS Code."""
    node = shutil.which("node.exe" if sys.platform == "win32" else "node") or shutil.which("node")
    if node is None:
        raise Failed("node is required to launch the VS Code Extension Host")
    vscode = vscodeExecutable(node)
    binary = provider_gate.buildBinary()
    with tempfile.TemporaryDirectory(prefix="runtrol-vscode-real-provider-") as raw_root:
        root = Path(raw_root)
        home = root / "runtrol-home"
        first_workspace = root / "workspace-one"
        second_workspace = root / "workspace-two"
        config = root / "claude-config"
        target = first_workspace / "must-not-exist.txt"
        result_path = root / "result.json"
        user_data = root / "vscode-user"
        extensions = root / "vscode-extensions"
        for path in (first_workspace, second_workspace, config, user_data, extensions):
            path.mkdir(parents=True)

        evidence: Evidence | None = None
        with provider_gate.RunningModel(target, max_requests=3) as model:
            env = provider_gate.environment(root, home, config, model, claude)
            env.update(
                {
                    "RUNTROL_TEST_CORE": str(binary),
                    "RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY": "1",
                    "RUNTROL_VSCODE_RESULT": str(result_path),
                    "RUNTROL_VSCODE_WORKSPACE_ONE": str(first_workspace),
                    "RUNTROL_VSCODE_WORKSPACE_TWO": str(second_workspace),
                    "RUNTROL_VSCODE_DENIED_TARGET": str(target),
                    "RUNTROL_VSCODE_PROVIDER": provider_gate.PROVIDER,
                    "RUNTROL_VSCODE_MODEL": provider_gate.MODEL,
                    "RUNTROL_VSCODE_USER_DATA": str(user_data),
                    "RUNTROL_VSCODE_EXTENSIONS": str(extensions),
                    "RUNTROL_TEST_VSCODE_EXECUTABLE": str(vscode),
                }
            )
            cli_probed = provider_gate.probeClaude(claude, env)
            daemon = provider_gate.startDaemon(binary, env, home)
            daemon_pids: set[int] = set()
            host_pids: set[int] = set()
            host: subprocess.Popen[str] | None = None
            output = ""
            sessions_closed = False
            try:
                with tempfile.TemporaryFile(mode="w+", encoding="utf-8", errors="replace") as host_output:
                    host = subprocess.Popen(
                        hostCommand(node),
                        cwd=ROOT,
                        env=env,
                        stdin=subprocess.DEVNULL,
                        stdout=host_output,
                        stderr=subprocess.STDOUT,
                        text=True,
                        encoding="utf-8",
                        errors="replace",
                    )
                    deadline = time.monotonic() + TIMEOUT_S
                    while host.poll() is None and time.monotonic() < deadline:
                        daemon_pids.update(provider_gate.descendants(daemon.pid))
                        host_pids.update(provider_gate.descendants(host.pid))
                        time.sleep(0.05)
                    if host.poll() is None:
                        host.terminate()
                        try:
                            host.wait(timeout=5.0)
                        except subprocess.TimeoutExpired:
                            host.kill()
                            host.wait(timeout=5.0)
                        raise Failed(f"the Extension Host journey exceeded {TIMEOUT_S:.0f} seconds")
                    host_output.seek(0)
                    output = host_output.read()
                if host.returncode != 0:
                    crash_path = home / "daemon-crash.log"
                    crash = crash_path.read_text(encoding="utf-8", errors="replace") if crash_path.is_file() else ""
                    daemon_state = f" daemon exit {daemon.poll()}." if daemon.poll() is not None else ""
                    crash_detail = f"\nCore crash:\n{crash[-4000:]}" if crash else ""
                    raise Failed(
                        f"the Extension Host harness returned {host.returncode}.{daemon_state}\n"
                        f"{output[-8000:]}{crash_detail}"
                    )
                result = resultRecord(output)
                evidence = Evidence(
                    cli_probed=cli_probed,
                    provider_detected=result.get("providerDetected") is True,
                    model_requests=model.requests,
                    sentinel_auth=model.sentinel_auth,
                    endpoint_contract=model.endpoint_contract,
                    approval_denied=result.get("approvalDenied") is True,
                    reconnected=result.get("reconnected") is True,
                    interrupted=result.get("interrupted") is True,
                    interrupt_terminal=(
                        (
                            result.get("interruptStop") == "cancelled"
                            and result.get("interruptDeclaredBy") in ("interruptAcked", "provider")
                        )
                        or (
                            result.get("interruptStop") == "failed"
                            and result.get("interruptDeclaredBy") == "provider"
                        )
                    ),
                    workspace_restored=result.get("workspaceRestored") is True,
                    sessions_closed=result.get("sessionsClosed") is True,
                    target_absent=not target.exists(),
                    cleanup_complete=False,
                )
                sessions_closed = True
            finally:
                daemon_pids.update(provider_gate.descendants(daemon.pid))
                if host is not None:
                    host_pids.update(provider_gate.descendants(host.pid))
                sessions = readSessionIds(result_path)
                if sessions and daemon.poll() is None:
                    listing = process.command(binary, env, ["list"])
                    remaining = tuple(session for session in sessions if session in listing)
                    sessions_closed = closeExactSessions(binary, env, remaining) and sessions_closed
                provider_gate.stopProcess(host)
                process.stopDaemon(daemon)
                survivors = provider_gate.waitGone(daemon_pids | host_pids)
                cleanup_complete = (
                    sessions_closed
                    and daemon.poll() is not None
                    and (host is None or host.poll() is not None)
                    and not survivors
                )
                if evidence is not None:
                    evidence = replace(
                        evidence,
                        model_requests=model.requests,
                        sentinel_auth=model.sentinel_auth,
                        endpoint_contract=model.endpoint_contract,
                        sessions_closed=sessions_closed,
                        target_absent=not target.exists(),
                        cleanup_complete=cleanup_complete,
                    )

        if evidence is None:
            raise Failed("the installed-provider VS Code journey produced no evidence")
        verifyEvidence(evidence)
        print(
            "[vscodeRealProviderJourney] OK. installed Claude Code completed start, prompt, approval denial, "
            "interrupt, reconnect, exact workspace restore, and close through the production VS Code extension."
        )


def main(argv: list[str]) -> int:
    """Select the defect selftest or the live installed-CLI journey."""
    if argv == ["--selftest"]:
        return selftest()
    if argv not in ([], ["--require-external"]):
        print("usage: vscodeRealProviderJourney.py [--selftest|--require-external]", file=sys.stderr)
        return 1
    claude = provider_gate.claudeProgram()
    required = argv == ["--require-external"]
    if claude is None:
        message = "[vscodeRealProviderJourney] Claude Code is not installed"
        print(message, file=sys.stderr if required else sys.stdout)
        return 2 if required else 0
    try:
        exercise(claude)
    except (Failed, OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"[vscodeRealProviderJourney] FAIL: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
