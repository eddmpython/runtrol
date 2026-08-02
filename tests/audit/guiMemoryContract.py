"""Measure the production GUI and its WebView process tree on Windows.

This gate deliberately ships without guessed byte ceilings. It has three phases:

1. ``--record`` starts the release product, waits for the real page's first-list trace, and records the GUI
   root plus every descendant WebView process. The separately started daemon is not part of this budget.
2. ``--seed`` reads clean records and prints a budget whose ceilings are the observed maxima, with no invented
   multiplier or headroom. A smoke seed needs five independent records. A campaign seed needs one complete
   24-hour record.
3. ``--budget`` repeats the same measurement core and makes any ceiling increase red. Ratchets may move down.
   Raising one requires operator approval and new records, because a larger memory contract is a product change.

The short pull-request smoke and the self-hosted 24-hour campaign differ only in duration and sampling cadence.
Both preserve private bytes and working set bytes because WebView shared pages make either number misleading by
itself.

Examples::

    python -X utf8 tests/audit/guiMemoryContract.py --selftest
    python -X utf8 tests/audit/guiMemoryContract.py --profile smoke --record smoke-1.json
    python -X utf8 tests/audit/guiMemoryContract.py --seed smoke smoke-1.json smoke-2.json smoke-3.json smoke-4.json smoke-5.json
    python -X utf8 tests/audit/guiMemoryContract.py --profile smoke --budget gui-memory-budget.json
    python -X utf8 tests/audit/guiMemoryContract.py --profile campaign --record campaign.json

``--seed`` writes JSON to stdout. Review it and redirect it to the intended tracked budget file only after the
records are available. This script never silently rewrites a contract.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import math
import os
import platform
import re
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import genericAcpSmoke as acp

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BINARY = ROOT / "target" / "release" / "runtrol.exe"
DEFAULT_FIXTURE = ROOT / "target" / "release" / "examples" / "acpFixture.exe"
SCHEMA = 1
MIB = 1024 * 1024
SMOKE_SECONDS = 60.0
CAMPAIGN_SECONDS = 24.0 * 60.0 * 60.0
SMOKE_SAMPLE_SECONDS = 1.0
CAMPAIGN_SAMPLE_SECONDS = 10.0
SMOKE_SETTLE_SECONDS = 15.0
CAMPAIGN_SETTLE_SECONDS = 60.0
SMOKE_CHURN_SECONDS = 2.0
CAMPAIGN_CHURN_SECONDS = 60.0
START_WITHIN_SECONDS = 45.0
SMOKE_SEED_RECORDS = 5
TRACE_NEEDLE = "first list at "
APPLIED_TRACE = "frame applied "
PAINTED_TRACE = "feed painted "
WEBVIEW_IMAGE = "msedgewebview2.exe"
RENDERER_REPLY_BYTES = 512 * 1024
RENDERER_RETAINED_CHARACTERS = 256 * 1024
MEMORY_CEILING_FIELDS = (
    "peak_private_bytes",
    "peak_working_set_bytes",
    "retained_private_growth_bytes",
    "retained_working_set_growth_bytes",
)
TOPOLOGY_CEILING_FIELDS = (
    "maximum_process_count",
)
CONTINUITY_FIELDS = (
    "maximum_sample_gap_seconds",
    "maximum_sample_lateness_seconds",
    "maximum_churn_gap_seconds",
    "maximum_churn_lateness_seconds",
)
RATCHET_FIELDS = (*MEMORY_CEILING_FIELDS, *TOPOLOGY_CEILING_FIELDS)
ATTESTATION_NAME = "guiMemoryBuildAttestation.json"
CHECKPOINT_RE = re.compile(
    rf"^({re.escape(APPLIED_TRACE.strip())}|{re.escape(PAINTED_TRACE.strip())}) "
    r"checkpoint=([0-9]+:[0-9]+:[0-9]+:[0-9]+) "
    r"view=([0-9]+) seq=([0-9]+) items=([0-9]+) characters=([0-9]+)$"
)


class Failed(Exception):
    """The GUI memory journey could not produce trustworthy evidence."""


@dataclass(frozen=True)
class ProcessIdentity:
    """A PID plus stable process facts that distinguish PID reuse."""

    pid: int
    created_at_ticks: int
    image_path: str


@dataclass(frozen=True)
class ProcessMemory:
    """One live process in the GUI-owned tree."""

    pid: int
    parent_pid: int
    image: str
    identity: ProcessIdentity
    private_bytes: int
    working_set_bytes: int


@dataclass(frozen=True)
class TreeMemory:
    """One aggregate sample of the GUI-owned process tree."""

    private_bytes: int
    working_set_bytes: int
    process_count: int
    images: tuple[str, ...]
    pids: tuple[int, ...]
    identities: tuple[ProcessIdentity, ...]
    image_paths: tuple[str, ...]


@dataclass(frozen=True)
class Summary:
    """Bounded evidence retained from a measurement run."""

    baseline_private_bytes: int
    baseline_working_set_bytes: int
    peak_private_bytes: int
    peak_working_set_bytes: int
    retained_private_growth_bytes: int
    retained_working_set_growth_bytes: int
    maximum_process_count: int
    sample_count: int
    maximum_sample_gap_seconds: float
    maximum_sample_lateness_seconds: float
    maximum_churn_gap_seconds: float
    maximum_churn_lateness_seconds: float


@dataclass(frozen=True)
class RunEvidence:
    """Raw measurements returned by one product journey."""

    samples: list[TreeMemory]
    trace: str
    elapsed: float
    turns: int
    sample_gaps: list[float]
    sample_lateness: list[float]
    churn_gaps: list[float]
    churn_lateness: list[float]
    applied_checkpoints: int
    painted_checkpoints: int
    completed_checkpoints: int


@dataclass(frozen=True)
class CheckpointTrace:
    """Content-free metrics for one apply or DOM-paint checkpoint."""

    checkpoint: str
    view: int
    seq: int
    items: int
    characters: int


@dataclass(frozen=True)
class CompletedCheckpoint:
    """One ordered apply-to-paint pair carrying the same checkpoint identity."""

    applied: CheckpointTrace
    painted: CheckpointTrace


@dataclass(frozen=True)
class ActionSelection:
    """The action-bearing CLI switches, isolated from argparse for failure-proof tests."""

    selftest: bool = False
    build: bool = False
    seed: bool = False
    record: bool = False
    record_auto: bool = False
    budget: bool = False
    records: int = 0


def actionProblems(selection: ActionSelection) -> list[str]:
    """Return ambiguous or incomplete CLI action combinations."""
    found: list[str] = []
    measuring = selection.record or selection.record_auto or selection.budget
    primary = sum((selection.selftest, selection.build, selection.seed, measuring))
    if primary == 0:
        found.append("choose exactly one selftest, build, seed, or measurement action")
    elif primary > 1:
        found.append("selftest, build, seed, and measurement actions are mutually exclusive")
    if selection.record and selection.record_auto:
        found.append("choose either an explicit record path or the automatic record path")
    if selection.seed and selection.records == 0:
        found.append("a seed action needs record paths")
    if not selection.seed and selection.records != 0:
        found.append("positional record paths belong only to a seed action")
    return found


def profileDefaults(profile: str) -> tuple[float, float, float]:
    """Return duration, settle time, and sample cadence for one profile."""
    if profile == "campaign":
        return CAMPAIGN_SECONDS, CAMPAIGN_SETTLE_SECONDS, CAMPAIGN_SAMPLE_SECONDS
    return SMOKE_SECONDS, SMOKE_SETTLE_SECONDS, SMOKE_SAMPLE_SECONDS


def profileEvidence(profile: str) -> tuple[float, float, float, float, int, int]:
    """Return the evidence floor derived from the profile's duration and cadence."""
    duration, settle, sample_seconds = profileDefaults(profile)
    churn_seconds = CAMPAIGN_CHURN_SECONDS if profile == "campaign" else SMOKE_CHURN_SECONDS
    minimum_samples = math.floor(duration / sample_seconds)
    minimum_turns = math.floor(duration / churn_seconds)
    return duration, settle, sample_seconds, churn_seconds, minimum_samples, minimum_turns


def continuityLimits(profile: str) -> dict[str, float]:
    """Derive fixed continuity limits from the declared profile cadence.

    One cadence of scheduler delay is tolerated. A two-cadence hole is not.
    """
    _duration, _settle, sample_seconds, churn_seconds, _samples, _turns = profileEvidence(profile)
    return {
        "maximum_sample_gap_seconds": sample_seconds * 2.0,
        "maximum_sample_lateness_seconds": sample_seconds,
        "maximum_churn_gap_seconds": churn_seconds * 2.0,
        "maximum_churn_lateness_seconds": churn_seconds,
    }


def continuityProblems(measurement: dict[str, Any], profile: str) -> list[str]:
    """Qualify cadence continuity independently from byte and topology ratchets."""
    found: list[str] = []
    for name, limit in continuityLimits(profile).items():
        observed = measurement.get(name)
        if not isinstance(observed, (int, float)) or not math.isfinite(float(observed)):
            found.append(f"{name} is not finite continuity evidence")
        elif float(observed) > limit:
            found.append(f"{name} observed {float(observed):.9f}, above profile limit {limit:.9f}")
    return found


def nextCadence(previous_due: float, completed_at: float, cadence: float) -> float:
    """Return the next future slot, skipping missed slots instead of emitting catch-up work."""
    scheduled = previous_due + cadence
    return scheduled if scheduled > completed_at else completed_at + cadence


def manifest(home: Path, fixture: Path) -> None:
    """Declare a real ACP workload whose replies exceed the renderer's 256 KiB retained-text bound."""
    providers = home / "providers"
    providers.mkdir(parents=True)
    text = f'''schema = 1
id = "{acp.PROVIDER}"
display_name = "ACP GUI Memory Fixture"
kind = "acp"

[bin]
names = [{json.dumps(fixture.name)}]

[probe]
version = {{ args = ["--version"], parse = "semver-anywhere" }}

[transport]
argv = ["--reply-bytes", "{RENDERER_REPLY_BYTES}"]
listen = "stdio"
'''
    (providers / f"{acp.PROVIDER}.toml").write_text(text, encoding="utf-8")


def median(values: list[int]) -> int:
    """Return an integer median without allowing an empty evidence window."""
    if not values:
        raise Failed("the memory evidence window was empty")
    return int(statistics.median(values))


def summarize(
    samples: list[TreeMemory],
    sample_gaps: list[float],
    sample_lateness: list[float],
    churn_gaps: list[float],
    churn_lateness: list[float],
) -> Summary:
    """Summarize start, peak, and retained growth with windows resistant to one scheduler sample."""
    if len(samples) < 5:
        raise Failed("fewer than five process-tree samples were captured")
    edge = max(1, len(samples) // 10)
    first = samples[:edge]
    last = samples[-edge:]
    baseline_private = median([sample.private_bytes for sample in first])
    baseline_working = median([sample.working_set_bytes for sample in first])
    ending_private = median([sample.private_bytes for sample in last])
    ending_working = median([sample.working_set_bytes for sample in last])
    return Summary(
        baseline_private_bytes=baseline_private,
        baseline_working_set_bytes=baseline_working,
        peak_private_bytes=max(sample.private_bytes for sample in samples),
        peak_working_set_bytes=max(sample.working_set_bytes for sample in samples),
        retained_private_growth_bytes=ending_private - baseline_private,
        retained_working_set_growth_bytes=ending_working - baseline_working,
        maximum_process_count=max(sample.process_count for sample in samples),
        sample_count=len(samples),
        maximum_sample_gap_seconds=max(sample_gaps, default=0.0),
        maximum_sample_lateness_seconds=max(sample_lateness, default=0.0),
        maximum_churn_gap_seconds=max(churn_gaps, default=0.0),
        maximum_churn_lateness_seconds=max(churn_lateness, default=0.0),
    )


def descendants(root_pid: int, parents: dict[int, int]) -> set[int]:
    """Resolve descendants from one process snapshot without following unrelated ancestors."""
    owned = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, parent_pid in parents.items():
            if parent_pid in owned and pid not in owned:
                owned.add(pid)
                changed = True
    return owned


def windowsProcesses() -> dict[int, tuple[int, str]]:
    """Snapshot process identifiers, parents, and image names through Toolhelp32."""

    class ProcessEntry(ctypes.Structure):
        _fields_ = [
            ("size", ctypes.c_ulong),
            ("usage", ctypes.c_ulong),
            ("pid", ctypes.c_ulong),
            ("default_heap", ctypes.c_void_p),
            ("module_id", ctypes.c_ulong),
            ("threads", ctypes.c_ulong),
            ("parent_pid", ctypes.c_ulong),
            ("priority", ctypes.c_long),
            ("flags", ctypes.c_ulong),
            ("image", ctypes.c_wchar * 260),
        ]

    snapshot_process = 0x00000002
    invalid_handle = ctypes.c_void_p(-1).value
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateToolhelp32Snapshot.argtypes = [ctypes.c_ulong, ctypes.c_ulong]
    kernel.CreateToolhelp32Snapshot.restype = ctypes.c_void_p
    kernel.Process32FirstW.argtypes = [ctypes.c_void_p, ctypes.POINTER(ProcessEntry)]
    kernel.Process32NextW.argtypes = [ctypes.c_void_p, ctypes.POINTER(ProcessEntry)]
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    snapshot = kernel.CreateToolhelp32Snapshot(snapshot_process, 0)
    if snapshot == invalid_handle:
        raise Failed(f"CreateToolhelp32Snapshot failed with Windows error {ctypes.get_last_error()}")
    found: dict[int, tuple[int, str]] = {}
    try:
        entry = ProcessEntry()
        entry.size = ctypes.sizeof(entry)
        present = kernel.Process32FirstW(snapshot, ctypes.byref(entry))
        while present:
            found[int(entry.pid)] = (int(entry.parent_pid), entry.image)
            present = kernel.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        kernel.CloseHandle(snapshot)
    return found


def windowsMemory(pid: int, parent_pid: int, image: str) -> ProcessMemory | None:
    """Read working set and private bytes, returning None only when a snapshotted process has exited."""

    class Counters(ctypes.Structure):
        _fields_ = [
            ("size", ctypes.c_ulong),
            ("page_fault_count", ctypes.c_ulong),
            ("peak_working_set_size", ctypes.c_size_t),
            ("working_set_size", ctypes.c_size_t),
            ("quota_peak_paged_pool_usage", ctypes.c_size_t),
            ("quota_paged_pool_usage", ctypes.c_size_t),
            ("quota_peak_non_paged_pool_usage", ctypes.c_size_t),
            ("quota_non_paged_pool_usage", ctypes.c_size_t),
            ("pagefile_usage", ctypes.c_size_t),
            ("peak_pagefile_usage", ctypes.c_size_t),
            ("private_usage", ctypes.c_size_t),
        ]

    class FileTime(ctypes.Structure):
        _fields_ = [("low", ctypes.c_ulong), ("high", ctypes.c_ulong)]

    query_information = 0x1000
    read_memory = 0x0010
    invalid_parameter = 87
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
    kernel.OpenProcess.restype = ctypes.c_void_p
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    kernel.GetProcessTimes.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(FileTime),
        ctypes.POINTER(FileTime),
        ctypes.POINTER(FileTime),
        ctypes.POINTER(FileTime),
    ]
    kernel.QueryFullProcessImageNameW.argtypes = [
        ctypes.c_void_p,
        ctypes.c_ulong,
        ctypes.c_wchar_p,
        ctypes.POINTER(ctypes.c_ulong),
    ]
    psapi.GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.POINTER(Counters), ctypes.c_ulong]
    handle = kernel.OpenProcess(query_information | read_memory, False, pid)
    if not handle:
        error = ctypes.get_last_error()
        if error == invalid_parameter:
            return None
        raise Failed(f"OpenProcess could not inspect {image} ({pid}), Windows error {error}")
    try:
        counters = Counters()
        counters.size = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.size):
            error = ctypes.get_last_error()
            if error == invalid_parameter:
                return None
            raise Failed(f"GetProcessMemoryInfo could not inspect {image} ({pid}), Windows error {error}")
        created = FileTime()
        exited = FileTime()
        kernel_time = FileTime()
        user_time = FileTime()
        if not kernel.GetProcessTimes(
            handle,
            ctypes.byref(created),
            ctypes.byref(exited),
            ctypes.byref(kernel_time),
            ctypes.byref(user_time),
        ):
            raise Failed(f"GetProcessTimes could not identify {image} ({pid}), Windows error {ctypes.get_last_error()}")
        capacity = ctypes.c_ulong(32768)
        image_path = ctypes.create_unicode_buffer(capacity.value)
        full_path = image.lower()
        if kernel.QueryFullProcessImageNameW(handle, 0, image_path, ctypes.byref(capacity)):
            full_path = image_path.value.lower()
        # Some WebView2 utility processes deny full-path queries even though memory and creation time remain
        # observable. Creation time plus the snapshotted image still prevents PID reuse from crossing cleanup.
        identity = ProcessIdentity(
            pid=pid,
            created_at_ticks=(int(created.high) << 32) | int(created.low),
            image_path=full_path,
        )
        return ProcessMemory(
            pid=pid,
            parent_pid=parent_pid,
            image=image.lower(),
            identity=identity,
            private_bytes=int(counters.private_usage),
            working_set_bytes=int(counters.working_set_size),
        )
    finally:
        kernel.CloseHandle(handle)


def sampleTree(root_pid: int, root_image: str) -> TreeMemory:
    """Measure exactly the live GUI root and descendants from one coherent process snapshot."""
    snapshot = windowsProcesses()
    if root_pid not in snapshot:
        raise Failed("the production GUI root exited during measurement")
    parents = {pid: parent for pid, (parent, _image) in snapshot.items()}
    owned = descendants(root_pid, parents)
    processes: list[ProcessMemory] = []
    for pid in sorted(owned):
        parent_pid, image = snapshot[pid]
        measured = windowsMemory(pid, parent_pid, image)
        if measured is not None:
            processes.append(measured)
    if not any(process.pid == root_pid for process in processes):
        raise Failed("the production GUI root disappeared between process snapshots")
    if any(process.image == root_image.lower() and process.pid != root_pid for process in processes):
        raise Failed("the GUI started a daemon inside its measured tree; the gate daemon was not ready")
    if not any(process.image == WEBVIEW_IMAGE for process in processes):
        raise Failed("the production GUI tree contains no WebView2 process")
    return TreeMemory(
        private_bytes=sum(process.private_bytes for process in processes),
        working_set_bytes=sum(process.working_set_bytes for process in processes),
        process_count=len(processes),
        images=tuple(sorted({process.image for process in processes})),
        pids=tuple(sorted(process.pid for process in processes)),
        identities=tuple(sorted((process.identity for process in processes), key=lambda item: item.pid)),
        image_paths=tuple(sorted({process.identity.image_path for process in processes})),
    )


def waitForTrace(gui: subprocess.Popen[bytes], trace_path: Path, needles: tuple[str, ...]) -> None:
    """Require a trace emitted after the production page draws its first real session list."""
    deadline = time.monotonic() + START_WITHIN_SECONDS
    while time.monotonic() < deadline:
        if gui.poll() is not None:
            raise Failed(f"the production GUI exited before first paint with code {gui.returncode}")
        trace = trace_path.read_text(encoding="utf-8", errors="replace") if trace_path.is_file() else ""
        if all(needle in trace for needle in needles):
            return
        time.sleep(0.05)
    trace = trace_path.read_text(encoding="utf-8", errors="replace") if trace_path.is_file() else ""
    raise Failed(f"the production page emitted incomplete trace evidence: {needles!r}; trace was {trace!r}")


def checkpointEvidence(
    trace: str,
    start_line: int = 0,
) -> tuple[int, int, tuple[CompletedCheckpoint, ...]]:
    """Return apply, paint, and ordered same-ID completion evidence from a trace suffix."""
    applied = 0
    painted = 0
    waiting: dict[str, CheckpointTrace] = {}
    completed: list[CompletedCheckpoint] = []
    for line in trace.splitlines()[start_line:]:
        match = CHECKPOINT_RE.match(line)
        if match is None:
            continue
        _kind, checkpoint, raw_view, raw_seq, raw_items, raw_characters = match.groups()
        parsed = CheckpointTrace(
            checkpoint=checkpoint,
            view=int(raw_view),
            seq=int(raw_seq),
            items=int(raw_items),
            characters=int(raw_characters),
        )
        if line.startswith(APPLIED_TRACE):
            applied += 1
            expected = f"{parsed.view}:{parsed.seq}:{parsed.items}:{parsed.characters}"
            if checkpoint == expected:
                waiting[checkpoint] = parsed
        elif line.startswith(PAINTED_TRACE):
            painted += 1
            matching = waiting.pop(checkpoint, None)
            if matching is not None and (parsed.view, parsed.seq) == (matching.view, matching.seq):
                completed.append(CompletedCheckpoint(applied=matching, painted=parsed))
    return applied, painted, tuple(completed)


def qualifiedCheckpoints(completed: tuple[CompletedCheckpoint, ...]) -> tuple[CompletedCheckpoint, ...]:
    """Keep checkpoints proving both retained feed state and actual DOM reached the bound."""
    return tuple(
        checkpoint
        for checkpoint in completed
        if checkpoint.applied.characters >= RENDERER_RETAINED_CHARACTERS
        and checkpoint.painted.characters >= RENDERER_RETAINED_CHARACTERS
    )


def waitForCheckpointGrowth(
    gui: subprocess.Popen[bytes],
    trace_path: Path,
    start_line: int,
) -> str:
    """Require one provider turn to cross apply and DOM paint with one checkpoint ID."""
    deadline = time.monotonic() + acp.TURN_WAIT_S
    latest: tuple[CompletedCheckpoint, ...] = ()
    latest_counts = (0, 0)
    while time.monotonic() < deadline:
        if gui.poll() is not None:
            raise Failed(f"the production GUI exited before applying a provider turn: {gui.returncode}")
        trace = trace_path.read_text(encoding="utf-8", errors="replace") if trace_path.is_file() else ""
        _applied, _painted, completed = checkpointEvidence(trace, start_line)
        latest_counts = (_applied, _painted)
        latest = completed
        qualified = qualifiedCheckpoints(completed)
        if qualified:
            return qualified[-1].applied.checkpoint
        time.sleep(0.025)
    diagnostic = [
        {
            "checkpoint": item.applied.checkpoint,
            "appliedItems": item.applied.items,
            "appliedCharacters": item.applied.characters,
            "paintedItems": item.painted.items,
            "paintedCharacters": item.painted.characters,
        }
        for item in latest[-4:]
    ]
    raise Failed(
        "a provider turn did not reach ordered worker apply and DOM paint with one checkpoint ID "
        f"at the {RENDERER_RETAINED_CHARACTERS}-character bound; "
        f"apply/paint counts were {latest_counts}; latest metrics were {diagnostic}"
    )


def stop(process: subprocess.Popen[bytes]) -> None:
    """Stop and reap one exact process, forcing only its verified process tree if it does not cooperate."""
    if process.poll() is not None:
        process.wait(timeout=1.0)
        return
    process.terminate()
    try:
        process.wait(timeout=5.0)
    except subprocess.TimeoutExpired:
        if sys.platform == "win32":
            subprocess.run(
                ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                capture_output=True,
                check=False,
                timeout=10.0,
            )
        else:
            process.kill()
        process.wait(timeout=10.0)


def binaryDigest(binary: Path) -> str:
    """Identify the exact product bits that generated a record."""
    digest = hashlib.sha256()
    with binary.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def isSha256(value: object) -> bool:
    """Whether a JSON value is one lowercase SHA-256 digest."""
    return isinstance(value, str) and len(value) == 64 and all(character in "0123456789abcdef" for character in value)


def isGitCommit(value: object) -> bool:
    """Whether a JSON value has a full Git object identifier shape."""
    return (
        isinstance(value, str)
        and len(value) in {40, 64}
        and all(character in "0123456789abcdef" for character in value)
    )


def relevantSourceDigest() -> str:
    """Hash build-relevant source without generated dependency or target trees."""
    candidates: set[Path] = set()
    for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "deny.toml"):
        path = ROOT / name
        if path.is_file():
            candidates.add(path)
    for base in (ROOT / "crates", ROOT / "assets" / "brand"):
        if not base.is_dir():
            continue
        for path in base.rglob("*"):
            relative_parts = path.relative_to(ROOT).parts
            if path.is_file() and not {"node_modules", "target", "dist"}.intersection(relative_parts):
                candidates.add(path)
    digest = hashlib.sha256()
    for path in sorted(candidates, key=lambda item: item.relative_to(ROOT).as_posix()):
        relative = path.relative_to(ROOT).as_posix().encode("utf-8")
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        contents = path.read_bytes()
        digest.update(len(contents).to_bytes(8, "big"))
        digest.update(contents)
    return digest.hexdigest()


def distTreeDigest() -> str:
    """Hash the exact generated frontend tree embedded by the production build."""
    root = ROOT / "crates" / "runtrol-gui" / "ui" / "dist"
    files = sorted(
        (path for path in root.rglob("*") if path.is_file()),
        key=lambda path: path.relative_to(root).as_posix(),
    )
    if not files:
        raise Failed("the production frontend dist tree is empty")
    digest = hashlib.sha256()
    for path in files:
        relative = path.relative_to(root).as_posix().encode("utf-8")
        contents = path.read_bytes()
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        digest.update(len(contents).to_bytes(8, "big"))
        digest.update(contents)
    return digest.hexdigest()


def sourceRevision() -> dict[str, Any]:
    """Record whether the measured binary came from a clean, reviewable source revision."""
    commit = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        timeout=10.0,
    )
    status = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=all"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        timeout=10.0,
    )
    diff = subprocess.run(
        ["git", "diff", "--binary", "HEAD"],
        cwd=ROOT,
        capture_output=True,
        check=False,
        timeout=30.0,
    )
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"],
        cwd=ROOT,
        capture_output=True,
        check=False,
        timeout=10.0,
    )
    if (
        commit.returncode != 0
        or status.returncode != 0
        or diff.returncode != 0
        or untracked.returncode != 0
        or not commit.stdout.strip()
    ):
        archive_digest = relevantSourceDigest()
        archive_commit = os.environ.get("RUNTROL_SOURCE_COMMIT", archive_digest).strip().lower()
        if not isGitCommit(archive_commit):
            raise Failed("RUNTROL_SOURCE_COMMIT is not a full hexadecimal source identifier")
        return {
            "commit": archive_commit,
            "workspaceClean": True,
            "workspaceStateSha256": archive_digest,
        }
    state = hashlib.sha256()
    state.update(commit.stdout.strip().encode("ascii"))
    state.update(diff.stdout)
    for raw_path in sorted(path for path in untracked.stdout.split(b"\0") if path):
        state.update(raw_path)
        path = ROOT / os.fsdecode(raw_path)
        if path.is_file():
            state.update(path.read_bytes())
    return {
        "commit": commit.stdout.strip(),
        "workspaceClean": not status.stdout.strip(),
        "workspaceStateSha256": state.hexdigest(),
    }


def attestationPath(binary: Path) -> Path:
    """Place the build attestation beside the production binary it identifies."""
    return binary.parent / ATTESTATION_NAME


def canonicalDigest(value: dict[str, Any]) -> str:
    """Hash one JSON object independent of formatting."""
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def attestationProblems(
    value: dict[str, Any],
    binary: Path,
    fixture: Path,
    revision: dict[str, Any],
    source_tree_sha256: str,
    dist_tree_sha256: str,
) -> list[str]:
    """Reject stale targets whose exact bits and source state were not built together."""
    found: list[str] = []
    if value.get("schema") != SCHEMA or value.get("kind") != "runtrol-gui-production-build":
        return [f"the build attestation is not schema {SCHEMA} production evidence"]
    if value.get("source") != revision:
        found.append("the build attestation source revision is stale")
    if value.get("sourceTreeSha256") != source_tree_sha256:
        found.append("the build attestation relevant source tree is stale")
    if value.get("distTreeSha256") != dist_tree_sha256:
        found.append("the build attestation embedded frontend dist tree is stale")
    for name, path in (("binary", binary), ("fixture", fixture)):
        artifact = value.get(name)
        if not isinstance(artifact, dict):
            found.append(f"the build attestation has no {name} artifact")
            continue
        if artifact.get("name") != path.name:
            found.append(f"the build attestation {name} name is stale")
        expected_digest = binaryDigest(path) if path.is_file() else None
        if artifact.get("sha256") != expected_digest:
            found.append(f"the build attestation {name} digest is stale")
    created_at = value.get("createdAt")
    if not isinstance(created_at, str):
        found.append("the build attestation has no creation time")
    else:
        try:
            if datetime.fromisoformat(created_at).tzinfo is None:
                found.append("the build attestation creation time has no timezone")
        except ValueError:
            found.append("the build attestation creation time is invalid")
    return found


def validatedAttestation(binary: Path, fixture: Path, path: Path) -> dict[str, Any]:
    """Load and validate production build evidence against current source and bits."""
    value = loadJson(path)
    found = attestationProblems(
        value,
        binary,
        fixture,
        sourceRevision(),
        relevantSourceDigest(),
        distTreeDigest(),
    )
    if found:
        raise Failed("; ".join(found))
    return value


def buildProduction() -> None:
    """Build the embedded production page and fixture, then attest exact source and bits."""
    revision = sourceRevision()
    source_tree_sha256 = relevantSourceDigest()
    frontend = subprocess.run(
        ["npm.cmd" if sys.platform == "win32" else "npm", "run", "build"],
        cwd=ROOT / "crates" / "runtrol-gui" / "ui",
        check=False,
    )
    if frontend.returncode != 0:
        raise Failed("production frontend build failed")
    dist_tree_sha256 = distTreeDigest()
    for command in (
        [
            "cargo",
            "build",
            "-p",
            "runtrol",
            "--bin",
            "runtrol",
            "--release",
            "--locked",
            "--features",
            "runtrol-gui/custom-protocol",
        ],
        [
            "cargo",
            "build",
            "-p",
            "runtrol-drivers",
            "--example",
            "acpFixture",
            "--release",
            "--locked",
        ],
    ):
        built = subprocess.run(
            command,
            cwd=ROOT,
            check=False,
        )
        if built.returncode != 0:
            raise Failed(f"production build failed: {' '.join(command)}")
    if sourceRevision() != revision or relevantSourceDigest() != source_tree_sha256:
        raise Failed("the source tree changed during the production build")
    value = {
        "schema": SCHEMA,
        "kind": "runtrol-gui-production-build",
        "createdAt": datetime.now(UTC).isoformat(),
        "source": revision,
        "sourceTreeSha256": source_tree_sha256,
        "distTreeSha256": dist_tree_sha256,
        "binary": {"name": DEFAULT_BINARY.name, "sha256": binaryDigest(DEFAULT_BINARY)},
        "fixture": {"name": DEFAULT_FIXTURE.name, "sha256": binaryDigest(DEFAULT_FIXTURE)},
    }
    destination = attestationPath(DEFAULT_BINARY)
    temporary = destination.with_suffix(destination.suffix + ".pending")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(destination)


def treeIdentities(root_pid: int) -> set[ProcessIdentity]:
    """Return the current identity-stable process tree rooted at one gate-owned PID."""
    snapshot = windowsProcesses()
    if root_pid not in snapshot:
        return set()
    parents = {pid: parent for pid, (parent, _image) in snapshot.items()}
    found: set[ProcessIdentity] = set()
    for pid in descendants(root_pid, parents):
        parent_pid, image = snapshot[pid]
        process = windowsMemory(pid, parent_pid, image)
        if process is not None:
            found.add(process.identity)
    return found


def survivingOwnedIdentities(
    owned: set[ProcessIdentity],
    live: dict[int, ProcessIdentity],
) -> set[ProcessIdentity]:
    """Return only exact gate-owned processes, excluding reused PIDs."""
    return {identity for identity in owned if live.get(identity.pid) == identity}


def liveOwnedIdentities(owned: set[ProcessIdentity]) -> set[ProcessIdentity]:
    """Re-identify only captured PIDs without inspecting unrelated processes."""
    snapshot = windowsProcesses()
    live: dict[int, ProcessIdentity] = {}
    for identity in owned:
        entry = snapshot.get(identity.pid)
        if entry is None:
            continue
        parent_pid, image = entry
        process = windowsMemory(identity.pid, parent_pid, image)
        if process is not None:
            live[identity.pid] = process.identity
    return survivingOwnedIdentities(owned, live)


def waitIdentitiesGone(owned: set[ProcessIdentity], timeout: float) -> set[ProcessIdentity]:
    """Wait for captured identities to disappear and return any exact survivors."""
    deadline = time.monotonic() + timeout
    remaining = set(owned)
    while time.monotonic() < deadline:
        remaining = liveOwnedIdentities(remaining)
        if not remaining:
            return set()
        time.sleep(0.05)
    return remaining


def forceCapturedIdentities(identities: set[ProcessIdentity]) -> None:
    """Force only still-matching captured identities, then require zero exact survivors."""
    force_errors: list[str] = []
    for identity in sorted(identities, key=lambda item: item.pid):
        if identity not in liveOwnedIdentities({identity}):
            continue
        try:
            subprocess.run(
                ["taskkill", "/PID", str(identity.pid), "/T", "/F"],
                capture_output=True,
                check=False,
                timeout=10.0,
            )
        except (OSError, subprocess.SubprocessError) as error:
            force_errors.append(f"{identity.pid}@{identity.created_at_ticks}: {error}")
    remaining = waitIdentitiesGone(identities, 10.0)
    if remaining:
        details = [
            f"{item.pid}@{item.created_at_ticks}:{item.image_path}"
            for item in sorted(remaining, key=lambda item: item.pid)
        ]
        suffix = f"; force errors: {force_errors}" if force_errors else ""
        raise Failed(f"gate-owned process identities survived forced cleanup: {details}{suffix}")


def cleanupOwned(
    gui: subprocess.Popen[bytes] | None,
    daemon: subprocess.Popen[str],
    session: str | None,
    binary: Path,
    environment: dict[str, str],
    observed: set[ProcessIdentity],
) -> None:
    """Attempt every cleanup action, then prove the GUI, WebView, daemon, and fixture PIDs are gone."""
    owned = set(observed)
    if gui is not None:
        owned.update(treeIdentities(gui.pid))
    owned.update(treeIdentities(daemon.pid))
    errors: list[str] = []
    if gui is not None:
        try:
            stop(gui)
        except (OSError, subprocess.SubprocessError) as error:
            errors.append(f"GUI cleanup failed: {error}")
    if session is not None:
        try:
            acp.command(binary, environment, ["close", session, "--now"])
        except (acp.Failed, OSError, subprocess.SubprocessError) as error:
            errors.append(f"session cleanup failed: {error}")
    try:
        acp.stopDaemon(daemon)
    except (OSError, subprocess.SubprocessError) as error:
        errors.append(f"daemon cleanup failed: {error}")
    try:
        survivors = waitIdentitiesGone(owned, 2.0)
        if survivors:
            forceCapturedIdentities(survivors)
    except Failed as error:
        errors.append(str(error))
    if errors:
        raise Failed("; ".join(errors))


def measure(
    binary: Path,
    fixture: Path,
    duration: float,
    settle: float,
    sample_seconds: float,
    churn_seconds: float,
) -> RunEvidence:
    """Run the real product and return process-tree samples plus its first-paint trace."""
    if sys.platform != "win32":
        raise Failed("the GUI memory contract currently requires Windows WebView2")
    if not binary.is_file():
        raise Failed(f"release product binary is missing: {binary}")
    if not fixture.is_file():
        raise Failed(f"release ACP fixture is missing: {fixture}")
    if duration <= 0 or settle < 0 or sample_seconds <= 0 or churn_seconds <= 0:
        raise Failed("duration and sample cadence must be positive, and settle time cannot be negative")

    with tempfile.TemporaryDirectory(prefix="runtrol-gui-memory-") as raw_temp:
        temp = Path(raw_temp)
        home = temp / "home"
        workspace = temp / "workspace"
        workspace.mkdir()
        trace_path = temp / "gui-trace.log"
        error_path = temp / "gui-error.log"
        manifest(home, fixture)
        environment = acp.environment(home, fixture)
        environment["RUNTROL_GUI_TRACE"] = "1"
        daemon = acp.startDaemon(binary, environment, home)
        gui: subprocess.Popen[bytes] | None = None
        session: str | None = None
        observed_identities = treeIdentities(daemon.pid)
        try:
            session = acp.command(binary, environment, ["start", acp.PROVIDER, str(workspace)])
            if acp.SESSION_RE.fullmatch(session) is None:
                raise Failed(f"the fixture start returned no session identifier: {session!r}")
            with trace_path.open("wb") as trace_output, error_path.open("wb") as error_output:
                gui = subprocess.Popen(
                    [str(binary), "gui"],
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=trace_output,
                    stderr=error_output,
                )
                waitForTrace(
                    gui,
                    trace_path,
                    (TRACE_NEEDLE, f"watching {session} view="),
                )
                warmup_trace = trace_path.read_text(encoding="utf-8", errors="replace")
                warmup_before = len(warmup_trace.splitlines())
                acp.command(binary, environment, ["say", session, "gui memory warmup"])
                waitForCheckpointGrowth(gui, trace_path, warmup_before)
                time.sleep(settle)
                samples: list[TreeMemory] = []
                turns = 0
                churn_errors: list[Exception] = []
                churn_started: list[float] = []
                churn_lateness: list[float] = []
                stop_churn = threading.Event()
                started = time.monotonic()
                deadline = started + duration

                def churn() -> None:
                    nonlocal turns
                    next_turn = started
                    try:
                        while not stop_churn.is_set() and next_turn < deadline:
                            wait = next_turn - time.monotonic()
                            if wait > 0 and stop_churn.wait(wait):
                                return
                            turn_started = time.monotonic()
                            churn_started.append(turn_started)
                            churn_lateness.append(max(0.0, turn_started - next_turn))
                            before = len(
                                trace_path.read_text(encoding="utf-8", errors="replace").splitlines()
                            )
                            acp.command(
                                binary,
                                environment,
                                ["say", session, f"gui memory churn {turns}"],
                            )
                            waitForCheckpointGrowth(gui, trace_path, before)
                            turns += 1
                            next_turn = nextCadence(next_turn, time.monotonic(), churn_seconds)
                    except (Failed, acp.Failed, OSError, subprocess.SubprocessError) as error:
                        churn_errors.append(error)

                churn_thread = threading.Thread(target=churn, name="gui-memory-churn")
                churn_thread.start()
                sample_started: list[float] = []
                sample_lateness: list[float] = []
                try:
                    next_sample = started
                    while next_sample < deadline:
                        wait = next_sample - time.monotonic()
                        if wait > 0:
                            time.sleep(wait)
                        sampled_at = time.monotonic()
                        sample_started.append(sampled_at)
                        sample_lateness.append(max(0.0, sampled_at - next_sample))
                        if gui.poll() is not None:
                            raise Failed(
                                f"the production GUI exited during measurement with code {gui.returncode}"
                            )
                        if churn_errors:
                            raise Failed(f"the real hot-session workload failed: {churn_errors[0]}")
                        sampled = sampleTree(gui.pid, binary.name)
                        samples.append(sampled)
                        observed_identities.update(sampled.identities)
                        next_sample = nextCadence(next_sample, time.monotonic(), sample_seconds)
                    remaining = deadline - time.monotonic()
                    if remaining > 0:
                        time.sleep(remaining)
                    elapsed = time.monotonic() - started
                finally:
                    stop_churn.set()
                    churn_thread.join(timeout=acp.TIMEOUT_S + 1.0)
                if churn_thread.is_alive():
                    raise Failed("the real hot-session workload did not stop")
                if churn_errors:
                    raise Failed(f"the real hot-session workload failed: {churn_errors[0]}")
            trace = trace_path.read_text(encoding="utf-8", errors="replace")
            sample_gaps = [
                current - previous for previous, current in zip(sample_started, sample_started[1:])
            ]
            churn_gaps = [
                current - previous for previous, current in zip(churn_started, churn_started[1:])
            ]
            applied, painted, completed = checkpointEvidence(trace)
            return RunEvidence(
                samples=samples,
                trace=trace,
                elapsed=elapsed,
                turns=turns,
                sample_gaps=sample_gaps,
                sample_lateness=sample_lateness,
                churn_gaps=churn_gaps,
                churn_lateness=churn_lateness,
                applied_checkpoints=applied,
                painted_checkpoints=painted,
                completed_checkpoints=len(qualifiedCheckpoints(completed)),
            )
        finally:
            cleanupOwned(gui, daemon, session, binary, environment, observed_identities)


def windowsFileVersion(path: Path) -> str:
    """Read one Windows product version directly from its version resource."""

    class FixedFileInfo(ctypes.Structure):
        _fields_ = [
            ("signature", ctypes.c_ulong),
            ("structure_version", ctypes.c_ulong),
            ("file_version_ms", ctypes.c_ulong),
            ("file_version_ls", ctypes.c_ulong),
            ("product_version_ms", ctypes.c_ulong),
            ("product_version_ls", ctypes.c_ulong),
            ("file_flags_mask", ctypes.c_ulong),
            ("file_flags", ctypes.c_ulong),
            ("file_os", ctypes.c_ulong),
            ("file_type", ctypes.c_ulong),
            ("file_subtype", ctypes.c_ulong),
            ("file_date_ms", ctypes.c_ulong),
            ("file_date_ls", ctypes.c_ulong),
        ]

    version_api = ctypes.WinDLL("version", use_last_error=True)
    version_api.GetFileVersionInfoSizeW.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_ulong)]
    version_api.GetFileVersionInfoSizeW.restype = ctypes.c_ulong
    version_api.GetFileVersionInfoW.argtypes = [
        ctypes.c_wchar_p,
        ctypes.c_ulong,
        ctypes.c_ulong,
        ctypes.c_void_p,
    ]
    version_api.VerQueryValueW.argtypes = [
        ctypes.c_void_p,
        ctypes.c_wchar_p,
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(ctypes.c_uint),
    ]
    ignored = ctypes.c_ulong()
    size = version_api.GetFileVersionInfoSizeW(str(path), ctypes.byref(ignored))
    if size == 0:
        raise Failed(f"could not size the WebView2 version resource at {path}")
    buffer = ctypes.create_string_buffer(size)
    if not version_api.GetFileVersionInfoW(str(path), 0, size, buffer):
        raise Failed(f"could not read the WebView2 version resource at {path}")
    raw = ctypes.c_void_p()
    length = ctypes.c_uint()
    if not version_api.VerQueryValueW(buffer, "\\", ctypes.byref(raw), ctypes.byref(length)):
        raise Failed(f"could not query the WebView2 fixed version resource at {path}")
    info = ctypes.cast(raw, ctypes.POINTER(FixedFileInfo)).contents
    return ".".join(
        str(part)
        for part in (
            info.product_version_ms >> 16,
            info.product_version_ms & 0xFFFF,
            info.product_version_ls >> 16,
            info.product_version_ls & 0xFFFF,
        )
    )


def webViewRuntime(samples: list[TreeMemory]) -> list[dict[str, str]]:
    """Return versioned provenance for every measured WebView2 runtime image."""
    paths = sorted(
        {
            path
            for sample in samples
            for path in sample.image_paths
            if Path(path).name.lower() == WEBVIEW_IMAGE
            and Path(path).is_file()
        }
    )
    if not paths:
        raise Failed("the measured tree exposed no WebView2 runtime image path")
    return [
        {
            "path": path,
            "version": windowsFileVersion(Path(path)),
            "sha256": binaryDigest(Path(path)),
        }
        for path in paths
    ]


def record(
    profile: str,
    binary: Path,
    fixture: Path,
    duration: float,
    settle: float,
    sample_seconds: float,
    churn_seconds: float,
    attestation_path: Path,
) -> dict[str, Any]:
    """Create one portable evidence record from the shared measurement core."""
    attestation = validatedAttestation(binary, fixture, attestation_path)
    attestation_sha256 = canonicalDigest(attestation)
    binary_sha256 = binaryDigest(binary)
    fixture_sha256 = binaryDigest(fixture)
    revision = sourceRevision()
    run = measure(
        binary,
        fixture,
        duration,
        settle,
        sample_seconds,
        churn_seconds,
    )
    if binaryDigest(binary) != binary_sha256:
        raise Failed("the production GUI binary changed during measurement")
    if binaryDigest(fixture) != fixture_sha256:
        raise Failed("the ACP workload fixture changed during measurement")
    if sourceRevision() != revision:
        raise Failed("the source revision or workspace state changed during measurement")
    ending_attestation = validatedAttestation(binary, fixture, attestation_path)
    if canonicalDigest(ending_attestation) != attestation_sha256:
        raise Failed("the production build attestation changed during measurement")
    summary = summarize(
        run.samples,
        run.sample_gaps,
        run.sample_lateness,
        run.churn_gaps,
        run.churn_lateness,
    )
    images = sorted({image for sample in run.samples for image in sample.images})
    return {
        "schema": SCHEMA,
        "kind": "runtrol-gui-memory-record",
        "profile": profile,
        "createdAt": datetime.now(UTC).isoformat(),
        "host": {
            "platform": platform.platform(),
            "python": platform.python_version(),
            "webView2Runtime": webViewRuntime(run.samples),
        },
        "binary": {
            "name": binary.name,
            "sha256": binary_sha256,
        },
        "fixture": {
            "name": fixture.name,
            "sha256": fixture_sha256,
        },
        "source": revision,
        "buildAttestation": {
            "name": attestation_path.name,
            "sha256": attestation_sha256,
            "sourceTreeSha256": attestation["sourceTreeSha256"],
            "distTreeSha256": attestation["distTreeSha256"],
        },
        "measurement": {
            "durationSeconds": run.elapsed,
            "settleSeconds": settle,
            "sampleSeconds": sample_seconds,
            "churnSeconds": churn_seconds,
            "fixtureTurns": run.turns,
            "appliedCheckpoints": run.applied_checkpoints,
            "paintedCheckpoints": run.painted_checkpoints,
            "completedCheckpoints": run.completed_checkpoints,
            "replyBytes": RENDERER_REPLY_BYTES,
            "images": images,
            "firstPaintTrace": next(
                (line for line in run.trace.splitlines() if TRACE_NEEDLE in line),
                "",
            ),
            **asdict(summary),
        },
    }


def loadJson(path: Path) -> dict[str, Any]:
    """Load one JSON object with an operator-readable failure."""
    try:
        loaded = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise Failed(f"cannot read {path}: {error}") from error
    if not isinstance(loaded, dict):
        raise Failed(f"{path} does not contain a JSON object")
    return loaded


def requireRecord(value: dict[str, Any], profile: str, source: str) -> dict[str, Any]:
    """Validate the fields used to seed or check a record."""
    if value.get("schema") != SCHEMA or value.get("kind") != "runtrol-gui-memory-record":
        raise Failed(f"{source} is not a schema {SCHEMA} GUI memory record")
    if value.get("profile") != profile:
        raise Failed(f"{source} is for profile {value.get('profile')!r}, not {profile!r}")
    host = value.get("host")
    runtime = host.get("webView2Runtime") if isinstance(host, dict) else None
    if (
        not isinstance(runtime, list)
        or not runtime
        or any(
            not isinstance(item, dict)
            or not isinstance(item.get("version"), str)
            or not item["version"]
            or not isSha256(item.get("sha256"))
            for item in runtime
        )
    ):
        raise Failed(f"{source} has no WebView2 runtime version provenance")
    attestation = value.get("buildAttestation")
    if (
        not isinstance(attestation, dict)
        or not isSha256(attestation.get("sha256"))
        or not isSha256(attestation.get("sourceTreeSha256"))
        or not isSha256(attestation.get("distTreeSha256"))
    ):
        raise Failed(f"{source} has no production build attestation provenance")
    measurement = value.get("measurement")
    if not isinstance(measurement, dict):
        raise Failed(f"{source} has no measurement object")
    required = (
        "durationSeconds",
        "settleSeconds",
        "sampleSeconds",
        "churnSeconds",
        "fixtureTurns",
        "appliedCheckpoints",
        "paintedCheckpoints",
        "completedCheckpoints",
        "replyBytes",
        "sample_count",
        *RATCHET_FIELDS,
        *CONTINUITY_FIELDS,
    )
    if any(not isinstance(measurement.get(name), (int, float)) for name in required):
        raise Failed(f"{source} has incomplete numeric memory evidence")
    if not all(math.isfinite(float(measurement[name])) for name in required):
        raise Failed(f"{source} contains non-finite memory evidence")
    if any(
        float(measurement[name]) < 0
        for name in (*RATCHET_FIELDS, *CONTINUITY_FIELDS)
        if "growth" not in name
    ):
        raise Failed(f"{source} contains negative continuity or process evidence")
    duration, settle, sample_seconds, churn_seconds, minimum_samples, minimum_turns = profileEvidence(
        profile
    )
    if float(measurement["durationSeconds"]) < duration:
        raise Failed(f"{source} ended before the {duration:.0f}-second {profile} duration")
    if float(measurement["settleSeconds"]) < settle:
        raise Failed(f"{source} did not complete the {profile} settle window")
    if float(measurement["sampleSeconds"]) > sample_seconds:
        raise Failed(f"{source} sampled less often than the {profile} cadence")
    if float(measurement["churnSeconds"]) > churn_seconds:
        raise Failed(f"{source} drove fewer hot-session turns than the {profile} cadence")
    if int(measurement["sample_count"]) < minimum_samples:
        raise Failed(f"{source} has fewer than {minimum_samples} process-tree samples")
    if int(measurement["fixtureTurns"]) < minimum_turns:
        raise Failed(f"{source} has fewer than {minimum_turns} real hot-session turns")
    expected_checkpoints = int(measurement["fixtureTurns"]) + 1
    if int(measurement["appliedCheckpoints"]) < expected_checkpoints:
        raise Failed(f"{source} has fewer worker-apply checkpoints than provider turns")
    if int(measurement["paintedCheckpoints"]) < expected_checkpoints:
        raise Failed(f"{source} has fewer React-paint checkpoints than provider turns")
    if int(measurement["completedCheckpoints"]) < expected_checkpoints:
        raise Failed(f"{source} has fewer ordered same-ID apply-to-paint checkpoints than provider turns")
    continuity_defects = continuityProblems(measurement, profile)
    if continuity_defects:
        raise Failed(f"{source} failed profile continuity: {'; '.join(continuity_defects)}")
    if int(measurement["replyBytes"]) < RENDERER_REPLY_BYTES:
        raise Failed(f"{source} never exceeded the renderer's retained-text bound")
    images = measurement.get("images")
    if not isinstance(images, list) or WEBVIEW_IMAGE not in images:
        raise Failed(f"{source} has no measured WebView2 image")
    first_paint = measurement.get("firstPaintTrace")
    if not isinstance(first_paint, str) or TRACE_NEEDLE not in first_paint:
        raise Failed(f"{source} has no production first-list paint trace")
    return measurement


def recordProvenance(source: str, value: dict[str, Any]) -> dict[str, str]:
    """Return enough immutable provenance to reproduce exactly which evidence seeded a ceiling."""
    binary = value.get("binary")
    fixture = value.get("fixture")
    host = value.get("host")
    revision = value.get("source")
    attestation = value.get("buildAttestation")
    created_at = value.get("createdAt")
    if (
        not isinstance(binary, dict)
        or not isSha256(binary.get("sha256"))
    ):
        raise Failed(f"{source} has no product binary digest")
    if (
        not isinstance(fixture, dict)
        or not isSha256(fixture.get("sha256"))
    ):
        raise Failed(f"{source} has no workload fixture digest")
    if not isinstance(host, dict) or not isinstance(host.get("platform"), str):
        raise Failed(f"{source} has no host platform")
    webview = host.get("webView2Runtime")
    if (
        not isinstance(webview, list)
        or not webview
        or any(
            not isinstance(item, dict)
            or not isinstance(item.get("path"), str)
            or not isinstance(item.get("version"), str)
            or not item["version"]
            or not isSha256(item.get("sha256"))
            for item in webview
        )
    ):
        raise Failed(f"{source} has no WebView2 runtime provenance")
    if (
        not isinstance(revision, dict)
        or not isGitCommit(revision.get("commit"))
        or not isSha256(revision.get("workspaceStateSha256"))
    ):
        raise Failed(f"{source} has no source revision")
    if revision.get("workspaceClean") is not True:
        raise Failed(f"{source} measured a dirty workspace and cannot seed a product contract")
    if (
        not isinstance(attestation, dict)
        or not isSha256(attestation.get("sha256"))
        or not isSha256(attestation.get("sourceTreeSha256"))
        or not isSha256(attestation.get("distTreeSha256"))
    ):
        raise Failed(f"{source} has no production build attestation")
    if not isinstance(created_at, str):
        raise Failed(f"{source} has no creation time")
    try:
        created = datetime.fromisoformat(created_at)
    except ValueError as error:
        raise Failed(f"{source} has an invalid creation time") from error
    if created.tzinfo is None:
        raise Failed(f"{source} creation time has no timezone")
    canonical = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return {
        "recordSha256": hashlib.sha256(canonical).hexdigest(),
        "binarySha256": binary["sha256"],
        "fixtureSha256": fixture["sha256"],
        "platform": host["platform"],
        "commit": revision["commit"],
        "workspaceStateSha256": revision["workspaceStateSha256"],
        "buildAttestationSha256": attestation["sha256"],
        "sourceTreeSha256": attestation["sourceTreeSha256"],
        "distTreeSha256": attestation["distTreeSha256"],
        "webView2RuntimeSha256": canonicalDigest({"runtime": webview}),
        "createdAt": created_at,
    }


def seed(profile: str, records: list[tuple[str, dict[str, Any]]]) -> dict[str, Any]:
    """Derive exact maxima from independent records without adding unmeasured headroom."""
    if profile == "smoke" and len(records) < SMOKE_SEED_RECORDS:
        raise Failed(f"a smoke ratchet seed needs at least {SMOKE_SEED_RECORDS} independent records")
    if profile == "campaign" and len(records) < 1:
        raise Failed("a campaign ratchet seed needs one complete 24-hour record")
    measurements = [requireRecord(value, profile, source) for source, value in records]
    provenance = [recordProvenance(source, value) for source, value in records]
    record_digests = {item["recordSha256"] for item in provenance}
    if len(record_digests) != len(provenance):
        raise Failed("ratchet seed records must be independent, not duplicate JSON evidence")
    created_at = {item["createdAt"] for item in provenance}
    if len(created_at) != len(provenance):
        raise Failed("ratchet seed records must come from independent measurement times")
    binary_digests = sorted({item["binarySha256"] for item in provenance})
    if len(binary_digests) != 1:
        raise Failed("all ratchet seed records must measure the same production binary digest")
    fixture_digests = sorted({item["fixtureSha256"] for item in provenance})
    if len(fixture_digests) != 1:
        raise Failed("all ratchet seed records must measure the same workload fixture digest")
    commits = sorted({item["commit"] for item in provenance})
    if len(commits) != 1:
        raise Failed("all ratchet seed records must measure the same clean source revision")
    workspace_states = sorted({item["workspaceStateSha256"] for item in provenance})
    if len(workspace_states) != 1:
        raise Failed("all ratchet seed records must measure the same clean workspace state")
    attestation_digests = sorted({item["buildAttestationSha256"] for item in provenance})
    if len(attestation_digests) != 1:
        raise Failed("all ratchet seed records must measure the same attested production build")
    source_tree_digests = sorted({item["sourceTreeSha256"] for item in provenance})
    if len(source_tree_digests) != 1:
        raise Failed("all ratchet seed records must measure the same relevant source tree")
    dist_tree_digests = sorted({item["distTreeSha256"] for item in provenance})
    if len(dist_tree_digests) != 1:
        raise Failed("all ratchet seed records must measure the same embedded frontend dist tree")
    minimum_duration = CAMPAIGN_SECONDS if profile == "campaign" else SMOKE_SECONDS
    if any(float(measurement["durationSeconds"]) < minimum_duration for measurement in measurements):
        raise Failed(f"every {profile} seed record must cover at least {minimum_duration:.0f} seconds")
    source_summary = [
        {
            "recordSha256": evidence["recordSha256"],
            "memory": {
                name: max(0, float(measurement[name])) if "growth" in name else measurement[name]
                for name in MEMORY_CEILING_FIELDS
            },
            "topology": {name: measurement[name] for name in TOPOLOGY_CEILING_FIELDS},
        }
        for evidence, measurement in zip(provenance, measurements)
    ]
    memory_ceilings = {
        name: max(float(item["memory"][name]) for item in source_summary)
        for name in MEMORY_CEILING_FIELDS
    }
    topology_ceilings = {
        name: max(float(item["topology"][name]) for item in source_summary)
        for name in TOPOLOGY_CEILING_FIELDS
    }
    return {
        "schema": SCHEMA,
        "kind": "runtrol-gui-memory-budget",
        "profile": profile,
        "minimumDurationSeconds": minimum_duration,
        "sourceRecords": len(records),
        "sourceBinarySha256": binary_digests,
        "sourceFixtureSha256": fixture_digests,
        "sourceCommit": commits[0],
        "sourceWorkspaceStateSha256": workspace_states[0],
        "sourceBuildAttestationSha256": attestation_digests[0],
        "sourceTreeSha256": source_tree_digests[0],
        "sourceDistTreeSha256": dist_tree_digests[0],
        "sourceEvidence": provenance,
        "sourceSummary": source_summary,
        "memoryCeilings": memory_ceilings,
        "topologyCeilings": topology_ceilings,
    }


def budgetProblems(budget: dict[str, Any], profile: str) -> list[str]:
    """Validate provenance and prove every ceiling is the maximum of its recorded source summary."""
    found: list[str] = []
    if budget.get("schema") != SCHEMA or budget.get("kind") != "runtrol-gui-memory-budget":
        return [f"the budget is not a schema {SCHEMA} GUI memory budget"]
    if budget.get("profile") != profile:
        return [f"the budget is for profile {budget.get('profile')!r}, not {profile!r}"]
    expected_duration, _settle, _sample, _churn, _samples, _turns = profileEvidence(profile)
    minimum_duration = budget.get("minimumDurationSeconds")
    source_records = budget.get("sourceRecords")
    evidence = budget.get("sourceEvidence")
    summary = budget.get("sourceSummary")
    memory_ceilings = budget.get("memoryCeilings")
    topology_ceilings = budget.get("topologyCeilings")
    minimum_records = SMOKE_SEED_RECORDS if profile == "smoke" else 1
    if not isinstance(minimum_duration, (int, float)) or float(minimum_duration) != expected_duration:
        found.append("the budget minimum duration does not match its profile")
    if not isinstance(source_records, int) or isinstance(source_records, bool) or source_records < minimum_records:
        found.append("the budget has too few source records")
    if not isinstance(evidence, list) or not isinstance(summary, list):
        found.append("the budget has no provenance evidence or source summary")
        return found
    if isinstance(source_records, int) and (len(evidence) != source_records or len(summary) != source_records):
        found.append("the budget source count disagrees with its provenance lists")

    singleton_fields = {
        "sourceBinarySha256": "binarySha256",
        "sourceFixtureSha256": "fixtureSha256",
    }
    for budget_name, evidence_name in singleton_fields.items():
        values = budget.get(budget_name)
        if not isinstance(values, list) or len(values) != 1 or not isSha256(values[0]):
            found.append(f"the budget has no single valid {budget_name}")
        elif any(not isinstance(item, dict) or item.get(evidence_name) != values[0] for item in evidence):
            found.append(f"the budget {budget_name} disagrees with source evidence")
    commit = budget.get("sourceCommit")
    state = budget.get("sourceWorkspaceStateSha256")
    if not isGitCommit(commit):
        found.append("the budget has no source commit")
    elif any(not isinstance(item, dict) or item.get("commit") != commit for item in evidence):
        found.append("the budget source commit disagrees with source evidence")
    if not isSha256(state):
        found.append("the budget has no workspace state digest")
    elif any(not isinstance(item, dict) or item.get("workspaceStateSha256") != state for item in evidence):
        found.append("the budget workspace state disagrees with source evidence")
    attestation_digest = budget.get("sourceBuildAttestationSha256")
    source_tree_digest = budget.get("sourceTreeSha256")
    dist_tree_digest = budget.get("sourceDistTreeSha256")
    if not isSha256(attestation_digest):
        found.append("the budget has no build attestation digest")
    elif any(
        not isinstance(item, dict) or item.get("buildAttestationSha256") != attestation_digest
        for item in evidence
    ):
        found.append("the budget build attestation disagrees with source evidence")
    if not isSha256(source_tree_digest):
        found.append("the budget has no relevant source tree digest")
    elif any(
        not isinstance(item, dict) or item.get("sourceTreeSha256") != source_tree_digest
        for item in evidence
    ):
        found.append("the budget relevant source tree disagrees with source evidence")
    if not isSha256(dist_tree_digest):
        found.append("the budget has no embedded frontend dist tree digest")
    elif any(
        not isinstance(item, dict) or item.get("distTreeSha256") != dist_tree_digest
        for item in evidence
    ):
        found.append("the budget embedded frontend dist tree disagrees with source evidence")

    evidence_fields = {
        "recordSha256",
        "binarySha256",
        "fixtureSha256",
        "platform",
        "commit",
        "workspaceStateSha256",
        "buildAttestationSha256",
        "sourceTreeSha256",
        "distTreeSha256",
        "webView2RuntimeSha256",
        "createdAt",
    }
    record_digests: list[str] = []
    creation_times: list[str] = []
    for index, item in enumerate(evidence):
        if not isinstance(item, dict) or set(item) != evidence_fields:
            found.append(f"source evidence {index} has an invalid structure")
            continue
        digest = item["recordSha256"]
        if not isSha256(digest):
            found.append(f"source evidence {index} has no record digest")
        else:
            record_digests.append(digest)
        if not isinstance(item["platform"], str) or not item["platform"]:
            found.append(f"source evidence {index} has no platform")
        for digest_name in (
            "binarySha256",
            "fixtureSha256",
            "workspaceStateSha256",
            "buildAttestationSha256",
            "sourceTreeSha256",
            "distTreeSha256",
            "webView2RuntimeSha256",
        ):
            if not isSha256(item[digest_name]):
                found.append(f"source evidence {index} has invalid {digest_name}")
        created_at = item["createdAt"]
        if not isinstance(created_at, str):
            found.append(f"source evidence {index} has no creation time")
        else:
            try:
                parsed = datetime.fromisoformat(created_at)
                if parsed.tzinfo is None:
                    found.append(f"source evidence {index} creation time has no timezone")
            except ValueError:
                found.append(f"source evidence {index} has an invalid creation time")
            creation_times.append(created_at)
    if len(set(record_digests)) != len(evidence):
        found.append("the budget contains duplicate source record digests")
    if len(set(creation_times)) != len(evidence):
        found.append("the budget contains duplicate source creation times")

    summary_fields = {"recordSha256", "memory", "topology"}
    summarized: dict[str, dict[str, Any]] = {}
    for index, item in enumerate(summary):
        if not isinstance(item, dict) or set(item) != summary_fields:
            found.append(f"source summary {index} has an invalid structure")
            continue
        digest = item["recordSha256"]
        if not isinstance(digest, str) or digest in summarized:
            found.append(f"source summary {index} has an invalid or duplicate record digest")
            continue
        valid_item = True
        for section_name, fields in (
            ("memory", MEMORY_CEILING_FIELDS),
            ("topology", TOPOLOGY_CEILING_FIELDS),
        ):
            section = item.get(section_name)
            if not isinstance(section, dict) or set(section) != set(fields):
                found.append(f"source summary {index} has invalid {section_name} structure")
                valid_item = False
                continue
            for name in fields:
                value = section[name]
                if (
                    not isinstance(value, (int, float))
                    or not math.isfinite(float(value))
                    or float(value) < 0
                ):
                    found.append(f"source summary {index} has invalid {name}")
                    valid_item = False
        if valid_item:
            summarized[digest] = item
    if set(summarized) != set(record_digests):
        found.append("source summaries do not map one-to-one to provenance records")
    for section_name, fields, ceilings in (
        ("memory", MEMORY_CEILING_FIELDS, memory_ceilings),
        ("topology", TOPOLOGY_CEILING_FIELDS, topology_ceilings),
    ):
        if not isinstance(ceilings, dict) or set(ceilings) != set(fields):
            found.append(f"the budget {section_name} ceiling table has an invalid structure")
            continue
        for name in fields:
            ceiling = ceilings[name]
            if (
                not isinstance(ceiling, (int, float))
                or not math.isfinite(float(ceiling))
                or float(ceiling) < 0
            ):
                found.append(f"the budget has no finite {name} ceiling")
                continue
            if summarized:
                expected = max(float(item[section_name][name]) for item in summarized.values())
                if float(ceiling) != expected:
                    found.append(f"the {name} ceiling is not the source-record maximum")
    return found


def problems(record_value: dict[str, Any], budget: dict[str, Any], profile: str) -> list[str]:
    """Return every ratchet violation rather than hiding later defects behind the first."""
    found = budgetProblems(budget, profile)
    if found:
        return found
    measurement = requireRecord(record_value, profile, "current measurement")
    minimum_duration = budget["minimumDurationSeconds"]
    if float(measurement["durationSeconds"]) < float(minimum_duration):
        found.append("the measurement ended before the budget's minimum duration")
    for fields, ceilings in (
        (MEMORY_CEILING_FIELDS, budget["memoryCeilings"]),
        (TOPOLOGY_CEILING_FIELDS, budget["topologyCeilings"]),
    ):
        for name in fields:
            observed = max(0, float(measurement[name])) if "growth" in name else float(measurement[name])
            if observed > float(ceilings[name]):
                found.append(
                    f"{name} observed {observed:.0f}, above ceiling {float(ceilings[name]):.0f}"
                )
    return found


def selftest() -> int:
    """Prove tree ownership, summarization, seeding, and every budget dimension can fail."""
    parents = {10: 1, 11: 10, 12: 11, 20: 1, 21: 20}
    if descendants(10, parents) != {10, 11, 12}:
        print("[guiMemoryContract --selftest] process ownership crossed trees.", file=sys.stderr)
        return 2
    old = ProcessIdentity(10, 100, "c:/runtrol.exe")
    child = ProcessIdentity(11, 110, "c:/msedgewebview2.exe")
    reused = ProcessIdentity(10, 101, "c:/other.exe")
    if survivingOwnedIdentities({old, child}, {10: reused, 11: child}) != {child}:
        print("[guiMemoryContract --selftest] cleanup survivor accounting crossed trees.", file=sys.stderr)
        return 2
    valid_trace = (
        "frame applied checkpoint=1:2:1:262144 view=1 seq=2 items=1 characters=262144\n"
        "feed painted checkpoint=1:2:1:262144 view=1 seq=2 items=1 characters=262144\n"
    )
    valid_completed = checkpointEvidence(valid_trace)[2]
    if len(qualifiedCheckpoints(valid_completed)) != 1:
        print("[guiMemoryContract --selftest] same-ID checkpoint completion was lost.", file=sys.stderr)
        return 2
    painted_zero = (
        "frame applied checkpoint=1:2:1:262144 view=1 seq=2 items=1 characters=262144\n"
        "feed painted checkpoint=1:2:1:262144 view=1 seq=2 items=0 characters=0\n"
    )
    status_only = (
        "frame applied checkpoint=1:3:1:3 view=1 seq=3 items=1 characters=3\n"
        "feed painted checkpoint=1:3:1:3 view=1 seq=3 items=1 characters=3\n"
    )
    paint_before_apply = (
        "feed painted checkpoint=1:2:1:262144 view=1 seq=2 items=1 characters=262144\n"
        "frame applied checkpoint=1:2:1:262144 view=1 seq=2 items=1 characters=262144\n"
    )
    for name, trace in (
        ("painted zero", painted_zero),
        ("status-only", status_only),
        ("paint before apply", paint_before_apply),
    ):
        if qualifiedCheckpoints(checkpointEvidence(trace)[2]):
            print(f"[guiMemoryContract --selftest] {name} checkpoint escaped.", file=sys.stderr)
            return 2
    unrelated_late = (
        "frame applied checkpoint=1:1:1:262144 view=1 seq=1 items=1 characters=262144\n"
        "feed painted checkpoint=1:1:1:262144 view=1 seq=1 items=1 characters=262144\n"
        "frame applied checkpoint=1:2:1:262144 view=1 seq=2 items=1 characters=262144\n"
    )
    if qualifiedCheckpoints(checkpointEvidence(unrelated_late, 1)[2]):
        print("[guiMemoryContract --selftest] unrelated late paint satisfied a new turn.", file=sys.stderr)
        return 2
    if nextCadence(5.0, 5.2, 1.0) != 6.0 or nextCadence(5.0, 7.0, 1.0) != 8.0:
        print("[guiMemoryContract --selftest] overdue cadence emitted catch-up work.", file=sys.stderr)
        return 2
    valid_actions = (
        ActionSelection(selftest=True),
        ActionSelection(build=True),
        ActionSelection(seed=True, records=1),
        ActionSelection(record=True),
        ActionSelection(budget=True),
        ActionSelection(record=True, budget=True),
    )
    invalid_actions = (
        ActionSelection(),
        ActionSelection(selftest=True, build=True),
        ActionSelection(seed=True),
        ActionSelection(record=True, record_auto=True),
        ActionSelection(record=True, records=1),
    )
    if any(actionProblems(action) for action in valid_actions) or any(
        not actionProblems(action) for action in invalid_actions
    ):
        print("[guiMemoryContract --selftest] CLI action ambiguity escaped.", file=sys.stderr)
        return 2
    samples = [
        TreeMemory(
            private_bytes=100 + index,
            working_set_bytes=200 + index,
            process_count=7,
            images=("runtrol.exe", WEBVIEW_IMAGE),
            pids=(10, 11),
            identities=(old, child),
            image_paths=(old.image_path, child.image_path),
        )
        for index in range(10)
    ]
    summary = summarize(samples, [1.0] * 9, [0.01] * 10, [2.0] * 4, [0.02] * 5)
    if summary.peak_private_bytes != 109 or summary.retained_private_growth_bytes != 9:
        print("[guiMemoryContract --selftest] memory windows lost their evidence.", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="gui-memory-attestation-selftest-") as raw_temp:
        test_binary = Path(raw_temp) / "runtrol.exe"
        test_fixture = Path(raw_temp) / "acpFixture.exe"
        test_binary.write_bytes(b"product")
        test_fixture.write_bytes(b"fixture")
        test_revision = {
            "commit": "c" * 40,
            "workspaceClean": True,
            "workspaceStateSha256": "d" * 64,
        }
        test_attestation = {
            "schema": SCHEMA,
            "kind": "runtrol-gui-production-build",
            "createdAt": "2026-08-02T00:00:00+00:00",
            "source": test_revision,
            "sourceTreeSha256": "1" * 64,
            "distTreeSha256": "2" * 64,
            "binary": {"name": test_binary.name, "sha256": binaryDigest(test_binary)},
            "fixture": {"name": test_fixture.name, "sha256": binaryDigest(test_fixture)},
        }
        if attestationProblems(
            test_attestation,
            test_binary,
            test_fixture,
            test_revision,
            "1" * 64,
            "2" * 64,
        ):
            print("[guiMemoryContract --selftest] current target attestation was rejected.", file=sys.stderr)
            return 2
        if not attestationProblems(
            test_attestation,
            test_binary,
            test_fixture,
            test_revision,
            "1" * 64,
            "3" * 64,
        ):
            print("[guiMemoryContract --selftest] stale frontend dist escaped.", file=sys.stderr)
            return 2
        test_binary.write_bytes(b"stale product")
        if not attestationProblems(
            test_attestation,
            test_binary,
            test_fixture,
            test_revision,
            "1" * 64,
            "2" * 64,
        ):
            print("[guiMemoryContract --selftest] stale production target escaped.", file=sys.stderr)
            return 2
    fixture = {
        "schema": SCHEMA,
        "kind": "runtrol-gui-memory-record",
        "profile": "smoke",
        "createdAt": "2026-08-02T00:00:00+00:00",
        "host": {
            "platform": "Windows-test",
            "python": "3.13",
            "webView2Runtime": [
                {"path": child.image_path, "version": "1.2.3.4", "sha256": "e" * 64}
            ],
        },
        "binary": {"name": "runtrol.exe", "sha256": "a" * 64},
        "fixture": {"name": "acpFixture.exe", "sha256": "f" * 64},
        "source": {
            "commit": "c" * 40,
            "workspaceClean": True,
            "workspaceStateSha256": "d" * 64,
        },
        "buildAttestation": {
            "name": ATTESTATION_NAME,
            "sha256": "b" * 64,
            "sourceTreeSha256": "1" * 64,
            "distTreeSha256": "2" * 64,
        },
        "measurement": {
            "durationSeconds": SMOKE_SECONDS,
            "settleSeconds": SMOKE_SETTLE_SECONDS,
            "sampleSeconds": SMOKE_SAMPLE_SECONDS,
            "churnSeconds": SMOKE_CHURN_SECONDS,
            "fixtureTurns": math.floor(SMOKE_SECONDS / SMOKE_CHURN_SECONDS),
            "appliedCheckpoints": math.floor(SMOKE_SECONDS / SMOKE_CHURN_SECONDS) + 1,
            "paintedCheckpoints": math.floor(SMOKE_SECONDS / SMOKE_CHURN_SECONDS) + 1,
            "completedCheckpoints": math.floor(SMOKE_SECONDS / SMOKE_CHURN_SECONDS) + 1,
            "replyBytes": RENDERER_REPLY_BYTES,
            "images": ["runtrol.exe", WEBVIEW_IMAGE],
            "firstPaintTrace": "first list at 10 ms with 1 rows",
            "peak_private_bytes": 100,
            "peak_working_set_bytes": 200,
            "retained_private_growth_bytes": 10,
            "retained_working_set_growth_bytes": 20,
            "maximum_process_count": 7,
            "sample_count": 60,
            "maximum_sample_gap_seconds": 1.01,
            "maximum_sample_lateness_seconds": 0.01,
            "maximum_churn_gap_seconds": 2.01,
            "maximum_churn_lateness_seconds": 0.01,
        },
    }
    records: list[tuple[str, dict[str, Any]]] = []
    for index in range(SMOKE_SEED_RECORDS):
        independent = json.loads(json.dumps(fixture))
        independent["createdAt"] = f"2026-08-02T00:00:0{index}+00:00"
        records.append((f"record-{index}", independent))
    budget = seed("smoke", records)
    budget_defects: list[tuple[str, dict[str, Any]]] = []
    missing_provenance = json.loads(json.dumps(budget))
    del missing_provenance["sourceEvidence"][0]["platform"]
    budget_defects.append(("provenance structure", missing_provenance))
    mismatched_binary = json.loads(json.dumps(budget))
    mismatched_binary["sourceBinarySha256"] = ["b" * 64]
    budget_defects.append(("binary provenance", mismatched_binary))
    missing_summary = json.loads(json.dumps(budget))
    missing_summary["sourceSummary"].pop()
    budget_defects.append(("source summary count", missing_summary))
    invented_ceiling = json.loads(json.dumps(budget))
    invented_ceiling["memoryCeilings"]["peak_private_bytes"] += 1
    budget_defects.append(("non-maximal ceiling", invented_ceiling))
    for name, broken_budget in budget_defects:
        if not budgetProblems(broken_budget, "smoke"):
            print(f"[guiMemoryContract --selftest] {name} budget defect escaped.", file=sys.stderr)
            return 2
    if problems(fixture, budget, "smoke"):
        print("[guiMemoryContract --selftest] green evidence was rejected.", file=sys.stderr)
        return 2
    for ceilings in (budget["memoryCeilings"], budget["topologyCeilings"]):
        for name in ceilings:
            broken = json.loads(json.dumps(fixture))
            broken["measurement"][name] = ceilings[name] + 1
            if not problems(broken, budget, "smoke"):
                print(f"[guiMemoryContract --selftest] {name} regression escaped.", file=sys.stderr)
                return 2
    too_short = json.loads(json.dumps(fixture))
    too_short["measurement"]["durationSeconds"] = SMOKE_SECONDS - 1
    rejected_short = False
    try:
        problems(too_short, budget, "smoke")
    except Failed:
        rejected_short = True
    if not rejected_short:
        print("[guiMemoryContract --selftest] a short measurement escaped.", file=sys.stderr)
        return 2
    rejected_small_seed = False
    try:
        seed("smoke", records[:-1])
    except Failed:
        rejected_small_seed = True
    if not rejected_small_seed:
        print("[guiMemoryContract --selftest] an unsupported ratchet seed escaped.", file=sys.stderr)
        return 2
    mixed = json.loads(json.dumps(fixture))
    mixed["createdAt"] = "2026-08-02T00:00:59+00:00"
    mixed["binary"]["sha256"] = "b" * 64
    rejected_mixed_binary = False
    try:
        seed("smoke", records[:-1] + [("mixed-binary", mixed)])
    except Failed:
        rejected_mixed_binary = True
    if not rejected_mixed_binary:
        print("[guiMemoryContract --selftest] mixed product binaries seeded one ratchet.", file=sys.stderr)
        return 2
    dirty = json.loads(json.dumps(fixture))
    dirty["createdAt"] = "2026-08-02T00:00:58+00:00"
    dirty["source"]["workspaceClean"] = False
    rejected_dirty_source = False
    try:
        seed("smoke", records[:-1] + [("dirty-source", dirty)])
    except Failed:
        rejected_dirty_source = True
    if not rejected_dirty_source:
        print("[guiMemoryContract --selftest] a dirty source seeded a product contract.", file=sys.stderr)
        return 2
    duplicate_record = records[:-1] + [records[0]]
    rejected_duplicate_record = False
    try:
        seed("smoke", duplicate_record)
    except Failed:
        rejected_duplicate_record = True
    if not rejected_duplicate_record:
        print("[guiMemoryContract --selftest] duplicate JSON evidence seeded a ratchet.", file=sys.stderr)
        return 2
    duplicate_time = json.loads(json.dumps(records[-1][1]))
    duplicate_time["createdAt"] = records[0][1]["createdAt"]
    duplicate_time["measurement"]["peak_private_bytes"] += 1
    rejected_duplicate_time = False
    try:
        seed("smoke", records[:-1] + [("duplicate-time", duplicate_time)])
    except Failed:
        rejected_duplicate_time = True
    if not rejected_duplicate_time:
        print("[guiMemoryContract --selftest] duplicate measurement time seeded a ratchet.", file=sys.stderr)
        return 2
    evidence_defects = {
        "sample count": ("sample_count", 59),
        "fixture turns": ("fixtureTurns", 29),
        "worker apply": ("appliedCheckpoints", 30),
        "React paint": ("paintedCheckpoints", 30),
        "ordered same-ID paint": ("completedCheckpoints", 30),
        "large sample stall": (
            "maximum_sample_gap_seconds",
            continuityLimits("smoke")["maximum_sample_gap_seconds"] + 0.000000001,
        ),
        "reply size": ("replyBytes", RENDERER_REPLY_BYTES - 1),
        "WebView image": ("images", ["runtrol.exe"]),
        "first paint": ("firstPaintTrace", ""),
    }
    for name, (field, broken_value) in evidence_defects.items():
        broken = json.loads(json.dumps(records[-1][1]))
        broken["measurement"][field] = broken_value
        rejected = False
        try:
            seed("smoke", records[:-1] + [(f"broken-{name}", broken)])
        except Failed:
            rejected = True
        if not rejected:
            print(f"[guiMemoryContract --selftest] {name} evidence defect escaped.", file=sys.stderr)
            return 2
    print("[guiMemoryContract --selftest] OK. tree, record, seed, and ratchet defects are red.")
    return 0


def parser() -> argparse.ArgumentParser:
    """Build the command surface without hiding campaign defaults in runner files."""
    found = argparse.ArgumentParser(description=__doc__)
    found.add_argument("--selftest", action="store_true")
    found.add_argument("--build", action="store_true")
    found.add_argument("--profile", choices=("smoke", "campaign"), default="smoke")
    found.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    found.add_argument("--fixture", type=Path, default=DEFAULT_FIXTURE)
    found.add_argument("--attestation", type=Path)
    found.add_argument("--duration-seconds", type=float)
    found.add_argument("--settle-seconds", type=float)
    found.add_argument("--sample-seconds", type=float)
    found.add_argument("--record", type=Path)
    found.add_argument("--record-auto", action="store_true")
    found.add_argument("--budget", type=Path)
    found.add_argument("--seed", choices=("smoke", "campaign"))
    found.add_argument("records", nargs="*", type=Path)
    return found


def main(argv: list[str]) -> int:
    """Run a pure selftest, seed a ratchet, or measure the production process tree."""
    arguments = parser().parse_args(argv)
    selection = ActionSelection(
        selftest=arguments.selftest,
        build=arguments.build,
        seed=arguments.seed is not None,
        record=arguments.record is not None,
        record_auto=arguments.record_auto,
        budget=arguments.budget is not None,
        records=len(arguments.records),
    )
    action_defects = actionProblems(selection)
    if action_defects:
        for defect in action_defects:
            print(f"[guiMemoryContract] FAIL. {defect}", file=sys.stderr)
        return 2
    if arguments.selftest:
        return selftest()
    try:
        if arguments.build:
            buildProduction()
            print("[guiMemoryContract] production GUI and workload fixture built.")
            return 0
        if arguments.seed:
            sources = [(str(path), loadJson(path)) for path in arguments.records]
            print(json.dumps(seed(arguments.seed, sources), indent=2, sort_keys=True))
            return 0
        if arguments.record_auto:
            arguments.record = Path(tempfile.gettempdir()) / f"runtrol-gui-memory-{arguments.profile}.json"
        duration, settle, sample_seconds = profileDefaults(arguments.profile)
        duration = arguments.duration_seconds if arguments.duration_seconds is not None else duration
        settle = arguments.settle_seconds if arguments.settle_seconds is not None else settle
        sample_seconds = arguments.sample_seconds if arguments.sample_seconds is not None else sample_seconds
        churn_seconds = CAMPAIGN_CHURN_SECONDS if arguments.profile == "campaign" else SMOKE_CHURN_SECONDS
        current = record(
            arguments.profile,
            arguments.binary.resolve(),
            arguments.fixture.resolve(),
            duration,
            settle,
            sample_seconds,
            churn_seconds,
            (
                arguments.attestation.resolve()
                if arguments.attestation is not None
                else attestationPath(arguments.binary.resolve())
            ),
        )
        if arguments.record is not None:
            arguments.record.parent.mkdir(parents=True, exist_ok=True)
            arguments.record.write_text(json.dumps(current, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            print(f"[guiMemoryContract] recorded {arguments.profile} evidence at {arguments.record}")
        if arguments.budget is not None:
            found = problems(current, loadJson(arguments.budget), arguments.profile)
            if found:
                print("[guiMemoryContract] FAIL. the production GUI memory ratchet increased.", file=sys.stderr)
                for problem in found:
                    print(f"  - {problem}", file=sys.stderr)
                return 2
            print("[guiMemoryContract] OK. the production GUI and WebView tree held its memory ratchet.")
        return 0
    except (Failed, acp.Failed, OSError, subprocess.SubprocessError, ValueError) as error:
        print(f"[guiMemoryContract] FAIL. {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
