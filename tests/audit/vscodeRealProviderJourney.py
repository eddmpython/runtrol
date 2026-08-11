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
import signal
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


@dataclass(frozen=True)
class ProcessIdentity:
    """One process generation, not merely a reusable operating-system PID."""

    pid: int
    started: str


@dataclass(frozen=True)
class ProcessRow:
    """The bounded fields needed for parentage, generation, and creation ordering."""

    parent: int
    started: str
    age_seconds: int | None


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


def processIdentityTable() -> dict[int, ProcessRow]:
    """Read process generations without retaining arguments or environment data."""
    if sys.platform == "win32":
        command = (
            "Get-CimInstance Win32_Process | Where-Object { $_.CreationDate } | ForEach-Object { "
            "'{0}|{1}|{2}' -f $_.ProcessId,$_.ParentProcessId,$_.CreationDate.ToUniversalTime().Ticks }"
        )
        listed = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", command],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30.0,
            check=False,
        )
    else:
        listed = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,etimes=,lstart="],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=15.0,
            check=False,
        )
    if listed.returncode != 0:
        raise Failed(f"process identity enumeration failed: {listed.stderr[-2000:]}")
    found: dict[int, ProcessRow] = {}
    for line in listed.stdout.splitlines():
        fields = line.split("|", 2) if sys.platform == "win32" else line.split(maxsplit=3)
        if len(fields) != (3 if sys.platform == "win32" else 4):
            continue
        try:
            pid = int(fields[0])
            parent = int(fields[1])
            age = None if sys.platform == "win32" else int(fields[2])
        except ValueError:
            # ok: an unrelated process row raced removal; complete rows still identify every owned generation.
            continue
        started = fields[2].strip() if sys.platform == "win32" else fields[3].strip()
        if pid > 0 and started:
            found[pid] = ProcessRow(parent, started, age)
    return found


def processGeneration(pid: int) -> str:
    """Capture the start token that disambiguates later PID reuse."""
    row = processIdentityTable().get(pid)
    if row is None:
        raise Failed(f"process {pid} vanished before its generation could be captured")
    return row.started


def ownedDescendants(parent: int, started: str) -> set[ProcessIdentity]:
    """Return descendants created no earlier than one exact parent generation."""
    table = processIdentityTable()
    root = table.get(parent)
    if root is None or root.started != started:
        return set()
    found: set[int] = set()
    frontier = {parent}
    while frontier:
        children = {
            pid for pid, row in table.items()
            if row.parent in frontier and pid not in found and createdAfter(row, root)
        }
        found.update(children)
        frontier = children
    return {ProcessIdentity(pid, table[pid].started) for pid in found}


def createdAfter(candidate: ProcessRow, root: ProcessRow) -> bool:
    """Reject stale parent identifiers that point at a newer reused PID."""
    if sys.platform == "win32":
        return int(candidate.started) >= int(root.started)
    if candidate.age_seconds is None or root.age_seconds is None:
        return False
    return candidate.age_seconds <= root.age_seconds + 1


def markedProcesses(marker: Path) -> set[ProcessIdentity]:
    """Find marked VS Code roots and every child still held by those exact roots."""
    marker_text = str(marker)
    if sys.platform == "win32":
        command = (
            "$marker=[Environment]::GetEnvironmentVariable('RUNTROL_VSCODE_MARKER'); "
            "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -and "
            "$_.CommandLine.IndexOf($marker,[StringComparison]::OrdinalIgnoreCase) -ge 0 } | "
            "Select-Object -ExpandProperty ProcessId"
        )
        listed = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", command],
            env={**os.environ, "RUNTROL_VSCODE_MARKER": marker_text},
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30.0,
            check=False,
        )
        if listed.returncode != 0:
            raise Failed(f"isolated VS Code process enumeration failed: {listed.stderr[-2000:]}")
        candidates = listed.stdout.splitlines()
    else:
        listed = subprocess.run(
            ["ps", "-axo", "pid=,command="],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=15.0,
            check=False,
        )
        if listed.returncode != 0:
            raise Failed(f"isolated VS Code process enumeration failed: {listed.stderr[-2000:]}")
        candidates = [
            line.split(maxsplit=1)[0]
            for line in listed.stdout.splitlines()
            if marker_text in line and line.split(maxsplit=1)
        ]
    roots: set[int] = set()
    for value in candidates:
        try:
            pid = int(value.strip())
        except ValueError:
            # ok: a command-line row vanished while CIM or ps rendered it; remaining marked roots are still checked.
            continue
        if pid > 0 and pid != os.getpid():
            roots.add(pid)
    table = processIdentityTable()
    found: set[ProcessIdentity] = set()
    for pid in roots:
        row = table.get(pid)
        if row is None:
            continue
        found.add(ProcessIdentity(pid, row.started))
        found.update(ownedDescendants(pid, row.started))
    return found


def stopMarkedProcesses(marker: Path) -> bool:
    """Terminate only VS Code processes carrying the exact isolated path marker."""
    owned = markedProcesses(marker)
    for identity in owned:
        try:
            os.kill(identity.pid, signal.SIGTERM)
        except ProcessLookupError:
            # ok: this exact generation already exited, which is the cleanup outcome the next identity scan verifies.
            continue
    survivors = aliveIdentities(owned)
    deadline = time.monotonic() + 5.0
    while survivors and time.monotonic() < deadline:
        time.sleep(0.05)
        survivors = aliveIdentities(owned)
    force_signal = signal.SIGTERM if sys.platform == "win32" else signal.SIGKILL
    for identity in survivors:
        try:
            os.kill(identity.pid, force_signal)
        except ProcessLookupError:
            # ok: this exact generation exited before the forced signal, and the next identity scan verifies it.
            continue
    deadline = time.monotonic() + 5.0
    while survivors and time.monotonic() < deadline:
        time.sleep(0.05)
        survivors = aliveIdentities(owned)
    return not survivors


def aliveIdentities(identities: set[ProcessIdentity]) -> set[ProcessIdentity]:
    """Keep only the same process generations, never a later PID occupant."""
    table = processIdentityTable()
    return {
        identity for identity in identities
        if table.get(identity.pid) is not None and table[identity.pid].started == identity.started
    }


def waitIdentitiesGone(identities: set[ProcessIdentity]) -> set[ProcessIdentity]:
    """Wait briefly for exact process generations to exit."""
    deadline = time.monotonic() + 5.0
    alive = aliveIdentities(identities)
    while alive and time.monotonic() < deadline:
        time.sleep(0.05)
        alive = aliveIdentities(identities)
    return alive


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
        cleanup_detail = ""
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
            daemon_generation = processGeneration(daemon.pid)
            daemon_processes: set[ProcessIdentity] = set()
            host_processes: set[ProcessIdentity] = set()
            host_generation = ""
            host: subprocess.Popen[str] | None = None
            output = ""
            sessions_closed = False
            marked_cleanup = False
            marked_cleanup_error = ""
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
                    host_generation = processGeneration(host.pid)
                    deadline = time.monotonic() + TIMEOUT_S
                    while host.poll() is None and time.monotonic() < deadline:
                        daemon_processes.update(ownedDescendants(daemon.pid, daemon_generation))
                        host_processes.update(ownedDescendants(host.pid, host_generation))
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
                daemon_processes.update(ownedDescendants(daemon.pid, daemon_generation))
                if host is not None and host_generation:
                    host_processes.update(ownedDescendants(host.pid, host_generation))
                sessions = readSessionIds(result_path)
                if sessions and daemon.poll() is None:
                    listing = process.command(binary, env, ["list"])
                    remaining = tuple(session for session in sessions if session in listing)
                    sessions_closed = closeExactSessions(binary, env, remaining) and sessions_closed
                provider_gate.stopProcess(host)
                try:
                    marked_cleanup = stopMarkedProcesses(user_data)
                except (Failed, OSError, subprocess.SubprocessError) as error:
                    marked_cleanup_error = str(error)
                finally:
                    process.stopDaemon(daemon)
                survivors = waitIdentitiesGone(daemon_processes | host_processes)
                cleanup_complete = (
                    sessions_closed
                    and marked_cleanup
                    and daemon.poll() is not None
                    and (host is None or host.poll() is not None)
                    and not survivors
                )
                if not cleanup_complete:
                    cleanup_detail = (
                        f"sessions_closed={sessions_closed}, marked_cleanup={marked_cleanup}, "
                        f"marked_cleanup_error={marked_cleanup_error or 'none'}, "
                        f"daemon_exit={daemon.poll()}, host_exit={None if host is None else host.poll()}, "
                        f"survivors={sorted(identity.pid for identity in survivors)}, "
                        f"daemon_tracked={sorted(identity.pid for identity in daemon_processes)}, "
                        f"host_tracked={sorted(identity.pid for identity in host_processes)}"
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
        if cleanup_detail:
            raise Failed(f"exact cleanup was incomplete: {cleanup_detail}")
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
