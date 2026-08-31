"""Gate: installed provider TUIs share one live terminal across independent Runtime clients.

The provider executable, daemon, ConPTY or PTY, public Runtime protocol, control lease, and terminal renderer are
real. The gate submits no line, starts no model turn, and reads no provider transcript. It measures only terminal
bytes and bounded public descriptors.

Usage::

    python -X utf8 tests/audit/providerTerminalParity.py --providers=claude,codex
    python -X utf8 tests/audit/providerTerminalParity.py --selftest
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
import tomllib
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

import genericAcpSmoke as daemonSupport
import vscodeRealProviderJourney as process
from vscodePerformanceBudget import loadPerformanceBudget

ROOT = Path(__file__).resolve().parents[2]
MANIFESTS = ROOT / "crates" / "runtrol-drivers" / "manifests"
MAX_ECHO_MS = loadPerformanceBudget()["realProviderTerminal"]["runtimeClientDeliveryMs"]
PROTOCOL_SCHEMA = ROOT / "crates" / "runtrol-runtime-protocol" / "schema" / "runtime.schema.json"
COMMAND_TIMEOUT_S = 120.0
BUILD_TIMEOUT_S = 300.0


class Failed(Exception):
    """The real-provider terminal fabric contract did not hold."""


def protocolScreenLimit() -> int:
    """Read the public terminal snapshot limit from the generated protocol schema."""
    schema = json.loads(PROTOCOL_SCHEMA.read_text(encoding="utf-8"))
    value = schema.get("x-runtrol-limits", {}).get("maxTerminalScreenBytes")
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise ValueError("the public Runtime schema has no positive maxTerminalScreenBytes limit")
    return value


MAX_SCREEN_BYTES = protocolScreenLimit()


@dataclass(frozen=True)
class Evidence:
    """One bounded record for identity, fan-out, writer handoff, latency, lifecycle, and cleanup."""

    provider: str
    descriptor_provider: bool
    workspace_bound: bool
    process_running: bool
    mode: str
    parity: bool
    first_fanout: bool
    first_screen_changed: bool
    viewer_closed: bool
    writer_handoff: bool
    handoff_screen_changed: bool
    first_echo_ms_a: float
    first_echo_ms_b: float
    handoff_echo_ms: float
    bounded_screen: bool
    exact_stop: bool
    terminal_gone: bool
    cleanup_complete: bool


def evidenceProblems(evidence: Evidence) -> list[str]:
    """Name each independent way the real-provider proof can fail."""
    problems: list[str] = []
    checks = (
        (evidence.descriptor_provider, "the terminal descriptor named a different provider"),
        (evidence.workspace_bound, "the terminal descriptor named a different workspace"),
        (evidence.process_running, "Runtime did not report a running provider TUI process"),
        (
            evidence.mode in {"rawText", "reversibleNavigation"},
            f"the probe used an unknown input mode {evidence.mode!r}",
        ),
        (evidence.parity, "fresh viewers disagreed on the shared terminal screen"),
        (evidence.first_fanout, "the first input did not reach both live viewer streams"),
        (evidence.first_screen_changed, "the first input did not change the shared screen"),
        (evidence.viewer_closed, "the first viewer was not closed before writer handoff"),
        (evidence.writer_handoff, "a new writer did not reach the remaining live viewer"),
        (evidence.handoff_screen_changed, "writer handoff did not change the shared screen"),
        (evidence.bounded_screen, "the provider screen was empty or exceeded the public Runtime screen limit"),
        (evidence.exact_stop, "the exact terminal stop did not succeed in its owning generation"),
        (evidence.terminal_gone, "the stopped terminal remained attachable"),
        (evidence.cleanup_complete, "a task-owned daemon or provider process survived cleanup"),
    )
    problems.extend(message for held, message in checks if not held)
    for label, measured in (
        ("viewer A fan-out", evidence.first_echo_ms_a),
        ("viewer B fan-out", evidence.first_echo_ms_b),
        ("writer handoff", evidence.handoff_echo_ms),
    ):
        if not math.isfinite(measured) or measured < 0 or measured > MAX_ECHO_MS:
            problems.append(f"{label} took {measured:.1f} ms, outside the {MAX_ECHO_MS:.0f} ms bound")
    return problems


def shippedProviders() -> dict[str, list[str]]:
    """Discover provider IDs and executable candidates from the manifests compiled into Runtime."""
    shipped: dict[str, list[str]] = {}
    for path in sorted(MANIFESTS.glob("*.toml")):
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
        identifier = manifest.get("id")
        names = (manifest.get("bin") or {}).get("names") or []
        if isinstance(identifier, str) and identifier and isinstance(names, list) and names:
            shipped[identifier] = [str(name) for name in names]
    return shipped


def providerScope(argv: list[str], shipped: dict[str, list[str]]) -> list[str]:
    """Select an explicit subset while leaving provider discovery authoritative."""
    arguments = [word for word in argv if word != "--selftest"]
    if not arguments:
        return [provider for provider, names in shipped.items() if any(shutil.which(name) for name in names)]
    if len(arguments) != 1 or not arguments[0].startswith("--providers="):
        raise Failed("usage: providerTerminalParity.py [--selftest] [--providers=id,id]")
    selected = arguments[0].removeprefix("--providers=").split(",")
    if not selected or any(not provider for provider in selected):
        raise Failed("--providers must name at least one provider id")
    if len(set(selected)) != len(selected):
        raise Failed("--providers contains a duplicate provider id")
    unknown = [provider for provider in selected if provider not in shipped]
    if unknown:
        raise Failed(f"--providers names an unshipped provider: {', '.join(unknown)}")
    missing = [
        provider
        for provider in selected
        if not any(shutil.which(name) for name in shipped[provider])
    ]
    if missing:
        raise Failed(f"selected provider executable is not installed: {', '.join(missing)}")
    return selected


def selftest() -> int:
    """Prove every identity, topology, latency, lifecycle, and cleanup defect turns the gate red."""
    valid = Evidence(
        provider="provider",
        descriptor_provider=True,
        workspace_bound=True,
        process_running=True,
        mode="rawText",
        parity=True,
        first_fanout=True,
        first_screen_changed=True,
        viewer_closed=True,
        writer_handoff=True,
        handoff_screen_changed=True,
        first_echo_ms_a=5.0,
        first_echo_ms_b=6.0,
        handoff_echo_ms=7.0,
        bounded_screen=True,
        exact_stop=True,
        terminal_gone=True,
        cleanup_complete=True,
    )
    defects = [
        replace(valid, **{field: False})
        for field in (
            "descriptor_provider",
            "workspace_bound",
            "process_running",
            "parity",
            "first_fanout",
            "first_screen_changed",
            "viewer_closed",
            "writer_handoff",
            "handoff_screen_changed",
            "bounded_screen",
            "exact_stop",
            "terminal_gone",
            "cleanup_complete",
        )
    ]
    defects.append(replace(valid, mode="unknown"))
    defects.extend(
        replace(valid, **{field: MAX_ECHO_MS + 1.0})
        for field in ("first_echo_ms_a", "first_echo_ms_b", "handoff_echo_ms")
    )
    defects.extend(
        replace(valid, **{field: -1.0})
        for field in ("first_echo_ms_a", "first_echo_ms_b", "handoff_echo_ms")
    )
    defects.extend(
        replace(valid, **{field: math.nan})
        for field in ("first_echo_ms_a", "first_echo_ms_b", "handoff_echo_ms")
    )
    if evidenceProblems(valid):
        print("[providerTerminalParity:selftest] FAIL. valid evidence was rejected", file=sys.stderr)
        return 2
    for defect in defects:
        if evidenceProblems(defect):
            continue
        print(f"[providerTerminalParity:selftest] FAIL. injected defect escaped: {defect}", file=sys.stderr)
        return 2
    scope = {"claude": ["missing-claude"], "codex": ["missing-codex"]}
    if providerScope([], scope):
        print("[providerTerminalParity:selftest] FAIL. absent default providers were selected", file=sys.stderr)
        return 2
    for arguments in (["--providers=claude,claude"], ["--providers=unknown"], ["--providers="]):
        try:
            providerScope(arguments, scope)
        except Failed:
            # ok: refusal is the assertion, and the next independent invalid scope must still be checked.
            continue
        print(f"[providerTerminalParity:selftest] FAIL. invalid scope escaped: {arguments}", file=sys.stderr)
        return 2
    print(f"[providerTerminalParity:selftest] OK. all {len(defects) + 4} injected defects make the gate red.")
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


def build(root: Path) -> tuple[Path, Path]:
    """Build the product and its public-wire probe into this gate's disposable root."""
    configured_target = os.environ.get("CARGO_TARGET_DIR")
    target = Path(configured_target).resolve() if configured_target else root / "target"
    environment = dict(os.environ)
    environment["CARGO_TARGET_DIR"] = str(target)
    built = subprocess.run(
        ["cargo", "build", "-p", "runtrol", "--bin", "runtrol", "--example", "handoverProbe"],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=BUILD_TIMEOUT_S,
        check=False,
    )
    if built.returncode != 0:
        raise Failed(f"the product and probe did not build:\n{(built.stderr or built.stdout)[-8_000:]}")
    suffix = ".exe" if sys.platform == "win32" else ""
    core = target / "debug" / f"runtrol{suffix}"
    probe = target / "debug" / "examples" / f"handoverProbe{suffix}"
    if not core.is_file() or not probe.is_file():
        raise Failed("cargo succeeded without producing the product and handover probe")
    return core, probe


def runProbe(probe: Path, environment: dict[str, str], words: list[str]) -> tuple[int, str]:
    """Run one bounded public Runtime probe command and return its exact outcome."""
    completed = subprocess.run(
        [str(probe), *words],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=COMMAND_TIMEOUT_S,
        check=False,
    )
    output = (completed.stdout or "").strip() or (completed.stderr or "").strip()
    return completed.returncode, output


def runJson(probe: Path, environment: dict[str, str], words: list[str]) -> dict[str, Any]:
    """Require one successful JSON object from the public Runtime probe."""
    code, output = runProbe(probe, environment, words)
    if code != 0:
        raise Failed(f"handoverProbe {' '.join(words[:1])} failed: {output[-4_000:]}")
    value = json.loads(output)
    if not isinstance(value, dict):
        raise Failed(f"handoverProbe {' '.join(words[:1])} returned a non-object")
    return value


def processStateIsRunning(value: object) -> bool:
    """Require the public descriptor's exact structural live state."""
    return value == "Running"


def evidenceFrom(
    provider: str,
    workspace: Path,
    opened: dict[str, Any],
    parity: dict[str, Any],
    stopped: dict[str, Any],
    terminal_gone: bool,
) -> Evidence:
    """Normalize text and modal parity paths into one provider-neutral judgement."""
    mode = str(parity.get("mode", ""))
    if mode == "rawText":
        first_fanout = parity.get("nonceOnBothStreams") is True
        first_changed = parity.get("nonceOnScreen") is True
        handoff_changed = parity.get("handoffOnScreen") is True
    else:
        first_fanout = parity.get("firstInputOnBothStreams") is True
        first_changed = parity.get("firstScreenChanged") is True
        handoff_changed = parity.get("handoffScreenChanged") is True
    screen_bytes = parity.get("screenBytes")
    return Evidence(
        provider=provider,
        descriptor_provider=opened.get("provider") == provider,
        workspace_bound=Path(str(opened.get("workspace", ""))).resolve() == workspace.resolve(),
        process_running=processStateIsRunning(opened.get("processState")),
        mode=mode,
        parity=parity.get("parity") is True,
        first_fanout=first_fanout,
        first_screen_changed=first_changed,
        viewer_closed=parity.get("viewerClosed") is True,
        writer_handoff=parity.get("writerHandoff") is True,
        handoff_screen_changed=handoff_changed,
        first_echo_ms_a=float(parity.get("firstEchoMsA", -1.0)),
        first_echo_ms_b=float(parity.get("firstEchoMsB", -1.0)),
        handoff_echo_ms=float(parity.get("handoffEchoMs", -1.0)),
        bounded_screen=isinstance(screen_bytes, int) and 0 < screen_bytes <= MAX_SCREEN_BYTES,
        exact_stop=stopped.get("stopped") is True and stopped.get("generation") == opened.get("generation"),
        terminal_gone=terminal_gone,
        cleanup_complete=False,
    )


def exerciseProvider(core: Path, probe: Path, root: Path, provider: str) -> Evidence:
    """Measure one installed provider TUI through two viewers and two writers, then stop it exactly."""
    provider_root = root / provider
    home = provider_root / "home"
    workspace = provider_root / "workspace"
    identity = provider_root / "identity.json"
    temporary = provider_root / "temp"
    for directory in (home, workspace, temporary):
        directory.mkdir(parents=True)
    environment = dict(os.environ)
    environment.update({"RUNTROL_HOME": str(home), "TEMP": str(temporary), "TMP": str(temporary)})
    daemon = daemonSupport.startDaemon(core, environment, home)
    daemon_generation = process.processGeneration(daemon.pid)
    owned_processes: set[process.ProcessIdentity] = set()
    opened: dict[str, Any] | None = None
    stopped_exactly = False
    evidence: Evidence | None = None
    try:
        runJson(probe, environment, ["enroll", str(home), str(core), str(identity), str(workspace)])
        opened = runJson(probe, environment, ["open", str(home), str(identity), provider, str(workspace)])
        owned_processes.update(process.ownedDescendants(daemon.pid, daemon_generation))
        terminal = str(opened.get("terminalId", ""))
        generation = str(opened.get("generation", ""))
        if not terminal or not generation:
            raise Failed(f"{provider} open returned no terminal identity or generation")
        try:
            parity = runJson(
                probe,
                environment,
                ["parity-navigation", str(home), str(identity), generation, terminal],
            )
        except Failed as error:
            raise Failed(f"{provider}: {error}") from error
        owned_processes.update(process.ownedDescendants(daemon.pid, daemon_generation))
        stopped = runJson(
            probe,
            environment,
            ["stop", str(home), str(identity), generation, terminal],
        )
        stopped_exactly = stopped.get("stopped") is True
        attach_code, attach_output = runProbe(
            probe,
            environment,
            ["attach", str(home), str(identity), generation, terminal],
        )
        terminal_gone = attach_code != 0 and "terminalgone" in attach_output.casefold()
        evidence = evidenceFrom(provider, workspace, opened, parity, stopped, terminal_gone)
    finally:
        owned_processes.update(process.ownedDescendants(daemon.pid, daemon_generation))
        if opened is not None and not stopped_exactly:
            runProbe(
                probe,
                environment,
                [
                    "stop",
                    str(home),
                    str(identity),
                    str(opened.get("generation", "")),
                    str(opened.get("terminalId", "")),
                ],
            )
        process.stopExactIdentities(owned_processes)
        daemonSupport.stopDaemon(daemon)
        survivors = process.stopExactIdentities(owned_processes)
        cleanup_complete = daemon.poll() is not None and not survivors
        diagnostics = getattr(daemon, "diagnostics", None)
        if diagnostics is not None:
            diagnostics.close()
        if evidence is not None:
            evidence = replace(evidence, cleanup_complete=cleanup_complete)
        if not cleanup_complete:
            raise Failed(
                f"{provider} cleanup was incomplete: daemon={daemon.poll()}, "
                f"survivors={sorted(identity.pid for identity in survivors)}"
            )
    if evidence is None:
        raise Failed(f"{provider} produced no terminal parity evidence")
    return evidence


def exercise(providers: list[str]) -> list[Evidence]:
    """Build once, then isolate each selected installed provider in its own Runtime generation."""
    if not providers:
        raise Failed("no installed provider was selected")
    with tempfile.TemporaryDirectory(prefix="rpt-", dir=executionRoot()) as raw_root:
        root = Path(raw_root).resolve()
        core, probe = build(root)
        return [exerciseProvider(core, probe, root, provider) for provider in providers]


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    try:
        providers = providerScope(argv, shippedProviders())
        evidence = exercise(providers)
    except (Failed, OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"providerTerminalParity FAILED: {error}", file=sys.stderr)
        return 2
    failed = False
    for row in evidence:
        print(json.dumps(row.__dict__, ensure_ascii=False, indent=2))
        for problem in evidenceProblems(row):
            failed = True
            print(f"providerTerminalParity FAILED [{row.provider}]: {problem}", file=sys.stderr)
    if failed:
        return 2
    summary = ", ".join(
        f"{row.provider} {row.first_echo_ms_a:.0f}/{row.first_echo_ms_b:.0f}/{row.handoff_echo_ms:.0f} ms"
        for row in evidence
    )
    print(
        "providerTerminalParity OK: real provider TUIs shared two viewers, survived viewer closure and writer "
        f"handoff, then stopped exactly ({summary})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
