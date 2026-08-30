"""Gate: two simultaneous real VS Code windows share one hosted provider terminal.

The daemon, public Runtime clients, production extension bundle, VS Code terminal tabs, PTY, and separate provider
process are real. The provider is the deterministic external ACP fixture in its declared TUI mode, so the proof
needs no credential, network request, or model token.

Usage::

    python -X utf8 tests/audit/vscodeMultiWindowTerminal.py
    python -X utf8 tests/audit/vscodeMultiWindowTerminal.py --selftest
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

import genericAcpSmoke as acp
import vscodeRealProviderJourney as process

ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"
MARKER = "RUNTROL_VSCODE_MULTI_WINDOW "
PROVIDER = acp.PROVIDER
TIMEOUT_S = 180.0
MAX_DELIVERY_MS = 5_000.0


class Failed(Exception):
    """The simultaneous-window terminal contract did not hold."""


@dataclass(frozen=True)
class Evidence:
    """One bounded record for identity, fan-out, writer handoff, lifecycle, and cleanup."""

    same_terminal: bool
    one_owner_pid: bool
    owner_alive_before_mirror: bool
    owner_alive_while_both_open: bool
    owner_alive_after_owner_window_closed: bool
    owner_saw_owner_input: bool
    mirror_saw_owner_input: bool
    mirror_wrote_after_owner_window_closed: bool
    mirror_saw_own_input: bool
    owner_input_ms: float
    mirror_saw_owner_ms: float
    mirror_input_after_handoff_ms: float
    provider_stopped: bool
    exact_owner_generation_stopped: bool
    cleanup_complete: bool


def evidenceProblems(evidence: Evidence) -> list[str]:
    """Name each independent way the direct multi-window claim can fail."""
    problems: list[str] = []
    checks = (
        (evidence.same_terminal, "the two VS Code windows attached to different terminal identities"),
        (evidence.one_owner_pid, "the journey did not establish one provider owner PID"),
        (evidence.owner_alive_before_mirror, "the provider owner exited before the second window opened"),
        (evidence.owner_alive_while_both_open, "the provider owner was not alive while both windows were open"),
        (
            evidence.owner_alive_after_owner_window_closed,
            "closing the first VS Code window ended the shared provider owner",
        ),
        (evidence.owner_saw_owner_input, "the first window did not receive its input back from the provider"),
        (evidence.mirror_saw_owner_input, "the second window did not receive the first window's provider output"),
        (
            evidence.mirror_wrote_after_owner_window_closed,
            "the second window could not write after the first window detached",
        ),
        (evidence.mirror_saw_own_input, "the second window did not receive its own provider output"),
        (evidence.provider_stopped, "the exact hosted provider did not stop through the remaining window"),
        (evidence.exact_owner_generation_stopped, "the provider PID generation survived or was reused ambiguously"),
        (evidence.cleanup_complete, "a task-owned daemon, provider, Node, or VS Code process survived cleanup"),
    )
    problems.extend(message for held, message in checks if not held)
    for label, measured in (
        ("owner input", evidence.owner_input_ms),
        ("mirror fan-out", evidence.mirror_saw_owner_ms),
        ("writer handoff", evidence.mirror_input_after_handoff_ms),
    ):
        if measured < 0 or measured > MAX_DELIVERY_MS:
            problems.append(f"{label} took {measured:.1f} ms, outside the {MAX_DELIVERY_MS:.0f} ms gate bound")
    return problems


def selftest() -> int:
    """Prove identity, lifecycle, delivery, latency, and cleanup defects each turn the gate red."""
    valid = Evidence(
        True,
        True,
        True,
        True,
        True,
        True,
        True,
        True,
        True,
        10.0,
        20.0,
        30.0,
        True,
        True,
        True,
    )
    defects = [
        replace(valid, **{field: False})
        for field in (
            "same_terminal",
            "one_owner_pid",
            "owner_alive_before_mirror",
            "owner_alive_while_both_open",
            "owner_alive_after_owner_window_closed",
            "owner_saw_owner_input",
            "mirror_saw_owner_input",
            "mirror_wrote_after_owner_window_closed",
            "mirror_saw_own_input",
            "provider_stopped",
            "exact_owner_generation_stopped",
            "cleanup_complete",
        )
    ]
    defects.extend(
        replace(valid, **{field: MAX_DELIVERY_MS + 1.0})
        for field in ("owner_input_ms", "mirror_saw_owner_ms", "mirror_input_after_handoff_ms")
    )
    defects.extend(
        replace(valid, **{field: -1.0})
        for field in ("owner_input_ms", "mirror_saw_owner_ms", "mirror_input_after_handoff_ms")
    )
    if evidenceProblems(valid):
        print("[vscodeMultiWindowTerminal:selftest] FAIL. valid evidence was rejected", file=sys.stderr)
        return 2
    for defect in defects:
        if evidenceProblems(defect):
            continue
        print(f"[vscodeMultiWindowTerminal:selftest] FAIL. injected defect escaped: {defect}", file=sys.stderr)
        return 2
    print(f"[vscodeMultiWindowTerminal:selftest] OK. all {len(defects)} defects make the gate red.")
    return 0


def executionRoot() -> Path:
    """The machine-wide disposable root required by the development hygiene contract."""
    if sys.platform == "win32":
        local = os.environ.get("LOCALAPPDATA")
        if not local:
            raise Failed("LOCALAPPDATA is required for the shared execution root")
        root = Path(local) / "dev-workspace"
    else:
        root = Path.home() / ".local" / "share" / "dev-workspace"
    root.mkdir(parents=True, exist_ok=True)
    return root


def writeManifest(home: Path, fixture: Path) -> None:
    """Declare the fixture only through the provider manifest, including its provider-owned TUI."""
    providers = home / "providers"
    providers.mkdir(parents=True)
    (providers / f"{PROVIDER}.toml").write_text(
        f'''schema = 1
id = "{PROVIDER}"
display_name = "Terminal Fixture"
kind = "acp"

[bin]
names = ["{fixture.name}"]

[probe]
version = {{ args = ["--version"], parse = "semver-anywhere" }}

[transport]
argv = []
listen = "stdio"

[tui]
new = ["--tui"]
''',
        encoding="utf-8",
    )


def hostCommand(node: str) -> list[str]:
    """Run the orchestrator under a virtual display when a Linux runner has none."""
    command = [node, str(EXTENSION / "tooling" / "multi-window-terminal.mjs")]
    if sys.platform.startswith("linux") and not os.environ.get("DISPLAY"):
        xvfb = shutil.which("xvfb-run")
        if xvfb is None:
            raise Failed("xvfb-run is required to test VS Code without a Linux display")
        return [xvfb, "-a", *command]
    return command


def resultRecord(output: str) -> dict[str, Any]:
    """Read exactly one bounded result from the Node orchestrator."""
    records = [line[len(MARKER):] for line in output.splitlines() if line.startswith(MARKER)]
    if len(records) != 1:
        raise Failed(f"expected one {MARKER.strip()} record, found {len(records)}")
    value = json.loads(records[0])
    if not isinstance(value, dict):
        raise Failed("the multi-window result is not an object")
    return value


def exercise() -> Evidence:
    """Run two simultaneous real Extension Hosts against one daemon and one provider TUI."""
    node = shutil.which("node.exe" if sys.platform == "win32" else "node") or shutil.which("node")
    if node is None:
        raise Failed("node is required to launch the VS Code Extension Hosts")
    vscode = process.vscodeExecutable(node)
    binary, fixture = acp.build()
    evidence: Evidence | None = None
    cleanup_detail = ""
    with tempfile.TemporaryDirectory(prefix="rvm-", dir=executionRoot()) as raw_root:
        root = Path(raw_root).resolve()
        home = root / "runtrol"
        workspace = root / "workspace"
        work_root = root / "vscode"
        owner_pid_path = root / "terminal-owner.pid"
        for directory in (home, workspace, work_root):
            directory.mkdir(parents=True)
        writeManifest(home, fixture)
        daemon_env = acp.environment(home, fixture)
        daemon_env["RUNTROL_ACP_FIXTURE_TUI_PID_PATH"] = str(owner_pid_path)
        daemon = acp.startDaemon(binary, daemon_env, home)
        daemon_generation = process.processGeneration(daemon.pid)
        daemon_processes: set[process.ProcessIdentity] = set()
        host_processes: set[process.ProcessIdentity] = set()
        owner_identity: process.ProcessIdentity | None = None
        host: subprocess.Popen[str] | None = None
        host_generation = ""
        output = ""
        try:
            host_env = process.hostEnvironment(dict(os.environ), daemon_env)
            host_env.update(
                {
                    "RUNTROL_TEST_CORE": str(binary),
                    "RUNTROL_TEST_INTEGRATION_ROOTS": json.dumps([str(workspace)]),
                    "RUNTROL_TEST_VSCODE_EXECUTABLE": str(vscode),
                    "RUNTROL_VSCODE_PROVIDER": PROVIDER,
                    "RUNTROL_VSCODE_WORKSPACE": str(workspace),
                    "RUNTROL_VSCODE_WORK_ROOT": str(work_root),
                    "RUNTROL_ACP_FIXTURE_TUI_PID_PATH": str(owner_pid_path),
                }
            )
            if sys.platform == "win32":
                host_env["LOCALAPPDATA"] = str(root)
            elif sys.platform == "darwin":
                host_env["HOME"] = str(root)
            else:
                host_env["XDG_STATE_HOME"] = str(root)
            with tempfile.TemporaryFile(mode="w+", encoding="utf-8", errors="replace") as host_output:
                host = subprocess.Popen(
                    hostCommand(node),
                    cwd=ROOT,
                    env=host_env,
                    stdin=subprocess.DEVNULL,
                    stdout=host_output,
                    stderr=subprocess.STDOUT,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                )
                host_generation = process.processGeneration(host.pid)
                deadline = time.monotonic() + TIMEOUT_S
                while host.poll() is None and time.monotonic() < deadline:
                    daemon_processes.update(process.ownedDescendants(daemon.pid, daemon_generation))
                    host_processes.update(process.ownedDescendants(host.pid, host_generation))
                    if owner_identity is None and owner_pid_path.is_file():
                        raw_pid = owner_pid_path.read_text(encoding="utf-8").strip()
                        if raw_pid.isdigit() and int(raw_pid) > 0:
                            owner_pid = int(raw_pid)
                            owner_identity = process.ProcessIdentity(owner_pid, process.processGeneration(owner_pid))
                    time.sleep(process.PROCESS_SAMPLE_S)
                if host.poll() is None:
                    raise Failed(f"the multi-window journey exceeded {TIMEOUT_S:.0f} seconds")
                host_output.seek(0)
                output = host_output.read()
            if host.returncode != 0:
                raise Failed(f"the multi-window orchestrator returned {host.returncode}:\n{output[-10_000:]}")
            result = resultRecord(output)
            if owner_identity is None:
                raise Failed("the journey never exposed the exact provider owner PID generation")
            exact_owner_generation_stopped = not process.aliveIdentities({owner_identity})
            evidence = Evidence(
                same_terminal=result.get("sameTerminal") is True,
                one_owner_pid=result.get("oneOwnerPid") is True,
                owner_alive_before_mirror=result.get("ownerAliveBeforeMirror") is True,
                owner_alive_while_both_open=result.get("ownerAliveWhileBothOpen") is True,
                owner_alive_after_owner_window_closed=result.get("ownerAliveAfterOwnerWindowClosed") is True,
                owner_saw_owner_input=result.get("ownerSawOwnerInput") is True,
                mirror_saw_owner_input=result.get("mirrorSawOwnerInput") is True,
                mirror_wrote_after_owner_window_closed=result.get("mirrorWroteAfterOwnerWindowClosed") is True,
                mirror_saw_own_input=result.get("mirrorSawOwnInput") is True,
                owner_input_ms=float(result.get("ownerInputMs", -1.0)),
                mirror_saw_owner_ms=float(result.get("mirrorSawOwnerMs", -1.0)),
                mirror_input_after_handoff_ms=float(result.get("mirrorInputAfterHandoffMs", -1.0)),
                provider_stopped=result.get("providerStopped") is True,
                exact_owner_generation_stopped=exact_owner_generation_stopped,
                cleanup_complete=False,
            )
        finally:
            daemon_processes.update(process.ownedDescendants(daemon.pid, daemon_generation))
            if host is not None and host_generation:
                host_processes.update(process.ownedDescendants(host.pid, host_generation))
            process.stopExactIdentities(host_processes)
            acp.stopDaemon(daemon)
            survivors = process.stopExactIdentities(daemon_processes | host_processes)
            cleanup_complete = daemon.poll() is not None and (host is None or host.poll() is not None) and not survivors
            if not cleanup_complete:
                cleanup_detail = (
                    f"daemon_exit={daemon.poll()}, host_exit={None if host is None else host.poll()}, "
                    f"survivors={sorted(identity.pid for identity in survivors)}"
                )
            if evidence is not None:
                evidence = replace(evidence, cleanup_complete=cleanup_complete)
        if evidence is None:
            raise Failed(f"the multi-window journey produced no evidence. {cleanup_detail}")
        if cleanup_detail:
            raise Failed(f"multi-window cleanup was incomplete: {cleanup_detail}")
    return evidence


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    try:
        evidence = exercise()
    except (Failed, OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"vscodeMultiWindowTerminal FAILED: {error}", file=sys.stderr)
        return 2
    problems = evidenceProblems(evidence)
    print(json.dumps(evidence.__dict__, ensure_ascii=False, indent=2))
    if problems:
        for problem in problems:
            print(f"vscodeMultiWindowTerminal FAILED: {problem}", file=sys.stderr)
        return 2
    print(
        "vscodeMultiWindowTerminal OK: two real VS Code windows shared one provider TUI, "
        "then writer ownership moved without replacing the provider process"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
