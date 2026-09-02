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
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
import traceback
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

import genericAcpSmoke as acp
import vscodeRealProviderJourney as process
from vscodePerformanceBudget import loadPerformanceBudget

ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"
MARKER = "RUNTROL_VSCODE_MULTI_WINDOW "
PROVIDER = acp.PROVIDER
TIMEOUT_S = 180.0
PERFORMANCE_BUDGET = loadPerformanceBudget()["multiWindowTerminal"]
MAX_FIRST_DELIVERY_MS = PERFORMANCE_BUDGET["firstUseDeliveryMs"]
MAX_WARM_P95_MS = PERFORMANCE_BUDGET["warmDeliveryP95Ms"]
SAMPLE_COUNT = int(PERFORMANCE_BUDGET["latencySampleCount"])


class Failed(Exception):
    """The simultaneous-window terminal contract did not hold."""


@dataclass(frozen=True)
class Evidence:
    """One bounded record for identity, fan-out, writer handoff, lifecycle, and cleanup."""

    same_terminal: bool
    same_stream_digest: bool
    lease_transfer_ordered: bool
    follower_resize_ignored: bool
    geometry_follows_holder: bool
    no_duplicate_echo: bool
    one_owner_pid: bool
    owner_alive_before_mirror: bool
    owner_alive_while_both_open: bool
    owner_alive_after_owner_window_closed: bool
    owner_saw_owner_input: bool
    mirror_saw_owner_input: bool
    mirror_wrote_after_owner_window_closed: bool
    mirror_saw_own_input: bool
    owner_first_input_ms: float
    owner_warm_input_p95_ms: float
    mirror_first_fanout_ms: float
    mirror_warm_fanout_p95_ms: float
    handoff_first_input_ms: float
    handoff_warm_input_p95_ms: float
    provider_stopped: bool
    exact_owner_generation_stopped: bool
    cleanup_complete: bool
    owner_input_samples_ms: tuple[float, ...] = ()
    mirror_fanout_samples_ms: tuple[float, ...] = ()
    handoff_input_samples_ms: tuple[float, ...] = ()
    owner_input_timings: tuple[tuple[float, float, float], ...] = ()
    handoff_input_timings: tuple[tuple[float, float, float], ...] = ()


def evidenceProblems(evidence: Evidence) -> list[str]:
    """Name each independent way the direct multi-window claim can fail."""
    problems: list[str] = []
    checks = (
        (evidence.same_terminal, "the two VS Code windows attached to different terminal identities"),
        (
            evidence.same_stream_digest,
            "the two VS Code windows digested different raw output over the same chunk stretch",
        ),
        (evidence.lease_transfer_ordered, "control did not move only on typing with a climbing generation"),
        (evidence.follower_resize_ignored, "a follower window's pane resize changed the shared geometry"),
        (evidence.geometry_follows_holder, "the shared geometry did not follow the window that took control"),
        (evidence.no_duplicate_echo, "a typed line was echoed other than exactly once"),
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
        ("first owner input", evidence.owner_first_input_ms),
        ("first mirror fan-out", evidence.mirror_first_fanout_ms),
        ("first writer handoff", evidence.handoff_first_input_ms),
    ):
        if measured < 0 or measured > MAX_FIRST_DELIVERY_MS:
            problems.append(
                f"{label} took {measured:.1f} ms, outside the {MAX_FIRST_DELIVERY_MS:.0f} ms first-use bound"
            )
    for label, measured in (
        ("warm owner input p95", evidence.owner_warm_input_p95_ms),
        ("warm mirror fan-out p95", evidence.mirror_warm_fanout_p95_ms),
        ("warm writer handoff p95", evidence.handoff_warm_input_p95_ms),
    ):
        if measured < 0 or measured > MAX_WARM_P95_MS:
            problems.append(
                f"{label} was {measured:.1f} ms, outside the {MAX_WARM_P95_MS:.0f} ms interaction contract"
            )
    for label, samples, first, warm in (
        (
            "owner input",
            evidence.owner_input_samples_ms,
            evidence.owner_first_input_ms,
            evidence.owner_warm_input_p95_ms,
        ),
        (
            "mirror fan-out",
            evidence.mirror_fanout_samples_ms,
            evidence.mirror_first_fanout_ms,
            evidence.mirror_warm_fanout_p95_ms,
        ),
        (
            "writer handoff",
            evidence.handoff_input_samples_ms,
            evidence.handoff_first_input_ms,
            evidence.handoff_warm_input_p95_ms,
        ),
    ):
        if len(samples) != SAMPLE_COUNT or any(not math.isfinite(sample) or sample < 0 for sample in samples):
            problems.append(f"{label} did not publish exactly {SAMPLE_COUNT} finite non-negative samples")
            continue
        if not math.isclose(samples[0], first, abs_tol=0.1):
            problems.append(f"{label} first-use summary does not match its samples")
        if not math.isclose(percentile95(samples[1:]), warm, abs_tol=0.1):
            problems.append(f"{label} warm p95 summary does not match its samples")
    for label, timings in (
        ("owner input timing", evidence.owner_input_timings),
        ("writer handoff timing", evidence.handoff_input_timings),
    ):
        if len(timings) != SAMPLE_COUNT:
            problems.append(f"{label} did not publish exactly {SAMPLE_COUNT} samples")
            continue
        if any(
            not all(math.isfinite(value) for value in timing)
            or not timing[0] <= timing[1] <= timing[2]
            for timing in timings
        ):
            problems.append(f"{label} contains a non-monotonic timestamp")
    return problems


def percentile95(samples: tuple[float, ...]) -> float:
    """Return the nearest-rank p95 used by the installed-host journey."""
    ordered = sorted(samples)
    index = max(0, math.ceil(len(ordered) * 0.95) - 1)
    return ordered[index]


def samplesFrom(result: dict[str, Any], key: str) -> tuple[float, ...]:
    """Read one bounded raw latency series without accepting strings or objects as numbers."""
    raw = result.get(key)
    if not isinstance(raw, list):
        return ()
    samples: list[float] = []
    for value in raw:
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            return ()
        samples.append(float(value))
    return tuple(samples)


def timingsFrom(result: dict[str, Any], key: str) -> tuple[tuple[float, float, float], ...]:
    """Read bounded receive, dispatch, and acknowledgement timestamps."""
    raw = result.get(key)
    if not isinstance(raw, list):
        return ()
    timings: list[tuple[float, float, float]] = []
    for value in raw:
        if not isinstance(value, dict):
            return ()
        fields = tuple(value.get(field) for field in ("receivedAtMs", "dispatchedAtMs", "acknowledgedAtMs"))
        if any(isinstance(field, bool) or not isinstance(field, (int, float)) for field in fields):
            return ()
        timings.append(tuple(float(field) for field in fields))
    return tuple(timings)


def selftest() -> int:
    """Prove identity, lifecycle, delivery, latency, and cleanup defects each turn the gate red."""
    valid = Evidence(
        same_terminal=True,
        same_stream_digest=True,
        lease_transfer_ordered=True,
        follower_resize_ignored=True,
        geometry_follows_holder=True,
        no_duplicate_echo=True,
        one_owner_pid=True,
        owner_alive_before_mirror=True,
        owner_alive_while_both_open=True,
        owner_alive_after_owner_window_closed=True,
        owner_saw_owner_input=True,
        mirror_saw_owner_input=True,
        mirror_wrote_after_owner_window_closed=True,
        mirror_saw_own_input=True,
        owner_first_input_ms=10.0,
        owner_warm_input_p95_ms=20.0,
        mirror_first_fanout_ms=30.0,
        mirror_warm_fanout_p95_ms=10.0,
        handoff_first_input_ms=20.0,
        handoff_warm_input_p95_ms=30.0,
        provider_stopped=True,
        exact_owner_generation_stopped=True,
        cleanup_complete=True,
        owner_input_samples_ms=(10.0, *([20.0] * (SAMPLE_COUNT - 1))),
        mirror_fanout_samples_ms=(30.0, *([10.0] * (SAMPLE_COUNT - 1))),
        handoff_input_samples_ms=(20.0, *([30.0] * (SAMPLE_COUNT - 1))),
        owner_input_timings=tuple((1.0, 2.0, 3.0) for _ in range(SAMPLE_COUNT)),
        handoff_input_timings=tuple((1.0, 2.0, 3.0) for _ in range(SAMPLE_COUNT)),
    )
    defects = [
        replace(valid, **{field: False})
        for field in (
            "same_terminal",
            "same_stream_digest",
            "lease_transfer_ordered",
            "follower_resize_ignored",
            "geometry_follows_holder",
            "no_duplicate_echo",
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
        replace(valid, **{field: MAX_FIRST_DELIVERY_MS + 1.0})
        for field in ("owner_first_input_ms", "mirror_first_fanout_ms", "handoff_first_input_ms")
    )
    defects.extend(
        replace(valid, **{field: ()})
        for field in (
            "owner_input_samples_ms",
            "mirror_fanout_samples_ms",
            "handoff_input_samples_ms",
            "owner_input_timings",
            "handoff_input_timings",
        )
    )
    defects.extend(
        replace(valid, **{field: MAX_WARM_P95_MS + 1.0})
        for field in ("owner_warm_input_p95_ms", "mirror_warm_fanout_p95_ms", "handoff_warm_input_p95_ms")
    )
    defects.extend(
        replace(valid, **{field: -1.0})
        for field in (
            "owner_first_input_ms",
            "owner_warm_input_p95_ms",
            "mirror_first_fanout_ms",
            "mirror_warm_fanout_p95_ms",
            "handoff_first_input_ms",
            "handoff_warm_input_p95_ms",
        )
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


def buildProbe() -> Path:
    """Build the public-wire probe the orchestrator uses to watch the Runtime's own descriptor between steps."""
    built = subprocess.run(
        ["cargo", "build", "-p", "runtrol", "--example", "handoverProbe"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=900,
        check=False,
    )
    if built.returncode != 0:
        raise Failed(f"the public-wire probe did not build: {(built.stderr or built.stdout)[-4_000:]}")
    suffix = ".exe" if sys.platform == "win32" else ""
    return acp.cargoTargetDir() / "debug" / "examples" / f"handoverProbe{suffix}"


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
    probe = buildProbe()
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
        # This latency gate owns one manifest-only provider. Keeping unrelated installed CLIs off the daemon's
        # search path prevents their background account and preparation processes from becoming unbounded,
        # machine-specific load inside a deterministic terminal transport measurement. The VS Code hosts retain
        # the operator PATH through hostEnvironment.
        daemon_env["PATH"] = str(fixture.parent)
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
                    "RUNTROL_TEST_PROBE": str(probe),
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
                same_stream_digest=result.get("sameStreamDigest") is True,
                lease_transfer_ordered=result.get("leaseTransferOrdered") is True,
                follower_resize_ignored=result.get("followerResizeIgnored") is True,
                geometry_follows_holder=result.get("geometryFollowsHolder") is True,
                no_duplicate_echo=result.get("noDuplicateEcho") is True,
                one_owner_pid=result.get("oneOwnerPid") is True,
                owner_alive_before_mirror=result.get("ownerAliveBeforeMirror") is True,
                owner_alive_while_both_open=result.get("ownerAliveWhileBothOpen") is True,
                owner_alive_after_owner_window_closed=result.get("ownerAliveAfterOwnerWindowClosed") is True,
                owner_saw_owner_input=result.get("ownerSawOwnerInput") is True,
                mirror_saw_owner_input=result.get("mirrorSawOwnerInput") is True,
                mirror_wrote_after_owner_window_closed=result.get("mirrorWroteAfterOwnerWindowClosed") is True,
                mirror_saw_own_input=result.get("mirrorSawOwnInput") is True,
                owner_first_input_ms=float(result.get("ownerFirstInputMs", -1.0)),
                owner_warm_input_p95_ms=float(result.get("ownerWarmInputP95Ms", -1.0)),
                mirror_first_fanout_ms=float(result.get("mirrorFirstFanoutMs", -1.0)),
                mirror_warm_fanout_p95_ms=float(result.get("mirrorWarmFanoutP95Ms", -1.0)),
                handoff_first_input_ms=float(result.get("handoffFirstInputMs", -1.0)),
                handoff_warm_input_p95_ms=float(result.get("handoffWarmInputP95Ms", -1.0)),
                provider_stopped=result.get("providerStopped") is True,
                exact_owner_generation_stopped=exact_owner_generation_stopped,
                cleanup_complete=False,
                owner_input_samples_ms=samplesFrom(result, "ownerInputSamplesMs"),
                mirror_fanout_samples_ms=samplesFrom(result, "mirrorFanoutSamplesMs"),
                handoff_input_samples_ms=samplesFrom(result, "handoffInputSamplesMs"),
                owner_input_timings=timingsFrom(result, "ownerInputTimings"),
                handoff_input_timings=timingsFrom(result, "handoffInputTimings"),
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
        traceback.print_exc()
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
