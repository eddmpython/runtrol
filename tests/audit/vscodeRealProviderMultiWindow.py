"""Gate: two actual VS Code windows share one installed Claude or Codex TUI.

The production extension bundle opens an editor terminal in the first isolated Extension Host and attaches a second
isolated Extension Host to the exact Runtime generation and terminal identity. Input is reversible navigation on the
provider's startup modal, so the gate submits no line, starts no model turn, and parses no provider text.
Provider redraws are autonomous, so this gate caps the post-close writer handoff. The deterministic fixture gate owns
the causal cross-window delivery latency contract.

Usage::

    python -X utf8 tests/audit/vscodeRealProviderMultiWindow.py --providers=claude,codex
    python -X utf8 tests/audit/vscodeRealProviderMultiWindow.py --selftest
"""

from __future__ import annotations

import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, replace
from pathlib import Path

import genericAcpSmoke as daemonSupport
import providerTerminalParity as terminalParity
import vscodeMultiWindowTerminal as fixtureWindow
import vscodeRealProviderJourney as process

ROOT = Path(__file__).resolve().parents[2]
TIMEOUT_S = 180.0
MAX_HANDOFF_MS = fixtureWindow.PERFORMANCE_BUDGET["firstUseDeliveryMs"]


class Failed(Exception):
    """The real-provider, real-window terminal contract did not hold."""


@dataclass(frozen=True)
class Evidence:
    """One bounded record for exact identity, two-window fan-out, handoff, stop, and cleanup."""

    provider: str
    descriptor_provider: bool
    workspace_bound: bool
    same_terminal: bool
    owner_received_input: bool
    mirror_received_owner_input: bool
    mirror_wrote_after_owner_closed: bool
    mirror_received_handoff_input: bool
    owner_input_ms: float
    mirror_input_ms: float
    handoff_input_ms: float
    provider_stopped: bool
    both_vscode_hosts: bool
    cleanup_complete: bool


def evidenceProblems(evidence: Evidence) -> list[str]:
    """Name each independent way the real two-window claim can fail."""
    problems: list[str] = []
    checks = (
        (evidence.descriptor_provider, "the shared terminal descriptor named a different provider"),
        (evidence.workspace_bound, "the shared terminal descriptor named a different workspace"),
        (evidence.same_terminal, "the two VS Code windows attached to different terminal identities"),
        (evidence.owner_received_input, "the first VS Code window did not receive its provider output"),
        (evidence.mirror_received_owner_input, "the second VS Code window did not receive the first input"),
        (
            evidence.mirror_wrote_after_owner_closed,
            "the second VS Code window could not write after the first window closed",
        ),
        (evidence.mirror_received_handoff_input, "the remaining window did not receive its own provider output"),
        (evidence.provider_stopped, "the remaining window's exact terminal stop was not accepted"),
        (evidence.both_vscode_hosts, "both isolated Extension Hosts did not complete the journey"),
        (evidence.cleanup_complete, "a task-owned daemon, provider, Node, or VS Code process survived cleanup"),
    )
    problems.extend(message for held, message in checks if not held)
    if not math.isfinite(evidence.handoff_input_ms) or not 0 <= evidence.handoff_input_ms <= MAX_HANDOFF_MS:
        problems.append(
            f"writer handoff took {evidence.handoff_input_ms:.1f} ms, outside the {MAX_HANDOFF_MS:.0f} ms bound"
        )
    for label, measured in (
        ("first-window observation", evidence.owner_input_ms),
        ("second-window observation", evidence.mirror_input_ms),
    ):
        if not math.isfinite(measured) or measured < 0:
            problems.append(f"{label} reported no bounded duration")
    return problems


def selftest() -> int:
    """Prove every identity, delivery, latency, lifecycle, and cleanup defect turns the gate red."""
    valid = Evidence(
        provider="provider",
        descriptor_provider=True,
        workspace_bound=True,
        same_terminal=True,
        owner_received_input=True,
        mirror_received_owner_input=True,
        mirror_wrote_after_owner_closed=True,
        mirror_received_handoff_input=True,
        owner_input_ms=10.0,
        mirror_input_ms=20.0,
        handoff_input_ms=30.0,
        provider_stopped=True,
        both_vscode_hosts=True,
        cleanup_complete=True,
    )
    defects = [
        replace(valid, **{field: False})
        for field in (
            "descriptor_provider",
            "workspace_bound",
            "same_terminal",
            "owner_received_input",
            "mirror_received_owner_input",
            "mirror_wrote_after_owner_closed",
            "mirror_received_handoff_input",
            "provider_stopped",
            "both_vscode_hosts",
            "cleanup_complete",
        )
    ]
    defects.extend(
        replace(valid, **{field: MAX_HANDOFF_MS + 1.0})
        for field in ("handoff_input_ms",)
    )
    defects.extend(
        replace(valid, **{field: -1.0})
        for field in ("owner_input_ms", "mirror_input_ms", "handoff_input_ms")
    )
    defects.extend(
        replace(valid, **{field: math.nan})
        for field in ("owner_input_ms", "mirror_input_ms", "handoff_input_ms")
    )
    if evidenceProblems(valid):
        print("[vscodeRealProviderMultiWindow:selftest] FAIL. valid evidence was rejected", file=sys.stderr)
        return 2
    for defect in defects:
        if evidenceProblems(defect):
            continue
        print(f"[vscodeRealProviderMultiWindow:selftest] FAIL. injected defect escaped: {defect}", file=sys.stderr)
        return 2
    print(f"[vscodeRealProviderMultiWindow:selftest] OK. all {len(defects)} injected defects make the gate red.")
    return 0


def exerciseProvider(
    core: Path,
    node: str,
    vscode: Path,
    root: Path,
    provider: str,
) -> Evidence:
    """Run two isolated real Extension Hosts against one installed provider TUI."""
    provider_root = root / provider
    home = provider_root / "home"
    workspace = provider_root / "workspace"
    work_root = provider_root / "vscode"
    temporary = provider_root / "temp"
    for directory in (home, workspace, work_root, temporary):
        directory.mkdir(parents=True)
    daemon_environment = dict(os.environ)
    daemon_environment.update({"RUNTROL_HOME": str(home), "TEMP": str(temporary), "TMP": str(temporary)})
    daemon = daemonSupport.startDaemon(core, daemon_environment, home)
    daemon_generation = process.processGeneration(daemon.pid)
    daemon_processes: set[process.ProcessIdentity] = set()
    host_processes: set[process.ProcessIdentity] = set()
    host: subprocess.Popen[str] | None = None
    host_generation = ""
    evidence: Evidence | None = None
    output = ""
    try:
        host_environment = process.hostEnvironment(dict(os.environ), daemon_environment)
        host_environment.update(
            {
                "RUNTROL_TEST_CORE": str(core),
                "RUNTROL_TEST_INTEGRATION_ROOTS": json.dumps([str(workspace)]),
                "RUNTROL_TEST_VSCODE_EXECUTABLE": str(vscode),
                "RUNTROL_VSCODE_PROVIDER": provider,
                "RUNTROL_VSCODE_WORKSPACE": str(workspace),
                "RUNTROL_VSCODE_WORK_ROOT": str(work_root),
                "RUNTROL_VSCODE_INPUT_MODE": "navigation",
            }
        )
        if sys.platform == "win32":
            host_environment["LOCALAPPDATA"] = str(provider_root)
        elif sys.platform == "darwin":
            host_environment["HOME"] = str(provider_root)
        else:
            host_environment["XDG_STATE_HOME"] = str(provider_root)
        with tempfile.TemporaryFile(mode="w+", encoding="utf-8", errors="replace") as host_output:
            host = subprocess.Popen(
                fixtureWindow.hostCommand(node),
                cwd=ROOT,
                env=host_environment,
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
                time.sleep(process.PROCESS_SAMPLE_S)
            if host.poll() is None:
                raise Failed(f"{provider} two-window journey exceeded {TIMEOUT_S:.0f} seconds")
            host_output.seek(0)
            output = host_output.read()
        if host.returncode != 0:
            raise Failed(f"{provider} two-window orchestrator returned {host.returncode}:\n{output[-10_000:]}")
        result = fixtureWindow.resultRecord(output)
        evidence = Evidence(
            provider=provider,
            descriptor_provider=result.get("providerId") == provider,
            workspace_bound=Path(str(result.get("workspace", ""))).resolve() == workspace.resolve(),
            same_terminal=result.get("sameTerminal") is True,
            owner_received_input=result.get("ownerSawOwnerInput") is True,
            mirror_received_owner_input=result.get("mirrorSawOwnerInput") is True,
            mirror_wrote_after_owner_closed=result.get("mirrorWroteAfterOwnerWindowClosed") is True,
            mirror_received_handoff_input=result.get("mirrorSawOwnInput") is True,
            owner_input_ms=float(result.get("ownerFirstInputMs", -1.0)),
            mirror_input_ms=float(result.get("mirrorFirstFanoutMs", -1.0)),
            handoff_input_ms=float(result.get("handoffFirstInputMs", -1.0)),
            provider_stopped=result.get("providerStopped") is True,
            both_vscode_hosts=bool(result.get("ownerVscode")) and bool(result.get("mirrorVscode")),
            cleanup_complete=False,
        )
    finally:
        daemon_processes.update(process.ownedDescendants(daemon.pid, daemon_generation))
        if host is not None and host_generation:
            host_processes.update(process.ownedDescendants(host.pid, host_generation))
        process.stopExactIdentities(host_processes)
        daemonSupport.stopDaemon(daemon)
        survivors = process.stopExactIdentities(daemon_processes | host_processes)
        cleanup_complete = daemon.poll() is not None and (host is None or host.poll() is not None) and not survivors
        diagnostics = getattr(daemon, "diagnostics", None)
        if diagnostics is not None:
            diagnostics.close()
        if evidence is not None:
            evidence = replace(evidence, cleanup_complete=cleanup_complete)
        if not cleanup_complete:
            raise Failed(
                f"{provider} cleanup was incomplete: daemon={daemon.poll()}, "
                f"host={None if host is None else host.poll()}, "
                f"survivors={sorted(identity.pid for identity in survivors)}"
            )
    if evidence is None:
        raise Failed(f"{provider} produced no real two-window evidence")
    return evidence


def exercise(providers: list[str]) -> list[Evidence]:
    """Build once, then isolate the selected real providers and their VS Code profiles."""
    if not providers:
        raise Failed("no installed provider was selected")
    node = shutil.which("node.exe" if sys.platform == "win32" else "node") or shutil.which("node")
    if node is None:
        raise Failed("node is required to launch the VS Code Extension Hosts")
    vscode = process.vscodeExecutable(node)
    with tempfile.TemporaryDirectory(prefix="rvm-real-", dir=terminalParity.executionRoot()) as raw_root:
        root = Path(raw_root).resolve()
        core, _probe = terminalParity.build(root)
        return [exerciseProvider(core, node, vscode, root, provider) for provider in providers]


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    try:
        providers = terminalParity.providerScope(argv, terminalParity.shippedProviders())
        evidence = exercise(providers)
    except (Failed, terminalParity.Failed, OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"vscodeRealProviderMultiWindow FAILED: {error}", file=sys.stderr)
        return 2
    failed = False
    for row in evidence:
        print(json.dumps(row.__dict__, ensure_ascii=False, indent=2))
        for problem in evidenceProblems(row):
            failed = True
            print(f"vscodeRealProviderMultiWindow FAILED [{row.provider}]: {problem}", file=sys.stderr)
    if failed:
        return 2
    summary = ", ".join(
        f"{row.provider} observations {row.owner_input_ms:.0f}/{row.mirror_input_ms:.0f} ms, "
        f"handoff {row.handoff_input_ms:.0f} ms"
        for row in evidence
    )
    print(
        "vscodeRealProviderMultiWindow OK: two real VS Code windows shared each real provider TUI, then the "
        f"remaining window wrote and stopped it exactly ({summary})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
