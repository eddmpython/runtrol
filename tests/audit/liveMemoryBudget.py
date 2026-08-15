"""Gate: one hot session and four live watchers stay inside the daemon memory ceiling.

The gate uses the external ACP fixture through a manifest, starts the real daemon, opens four real watch
connections, and emits both an admitted 900 KiB provider event and three consecutive rejected 15 MiB events just
below the parser's 16 MiB input bound. Every admitted watcher
must receive the complete payload, every rejected watcher must receive an explicit lag boundary, and RSS is sampled
from outside the daemon through the operating system. The provider child and watch clients are deliberately excluded
from the daemon's budget.

Usage::

    python -X utf8 tests/audit/liveMemoryBudget.py --selftest
    python -X utf8 tests/audit/liveMemoryBudget.py
"""

from __future__ import annotations

import ctypes
import json
import math
import os
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterable
from dataclasses import dataclass, replace
from pathlib import Path

import genericAcpSmoke as acp

MIB = 1024 * 1024
# Hosted Linux idle measurement is 39.6 MiB, while the same executable idles below 20 MiB elsewhere. Linux therefore
# gets enough transient room for the 16 MiB provider input contract without weakening the 48 MiB ceiling elsewhere.
HARD_CEILING = (64 if sys.platform.startswith("linux") else 48) * MIB
HOT_INCREMENT = 10 * MIB
# Repeated hosted macOS journeys retained 5.02 to 5.30 MiB after every watch task and payload owner had exited. The
# 6 MiB ceiling leaves less than one measured mebibyte for allocator variation without weakening the 4 MiB ratchet
# elsewhere. The separate 10 MiB hot increment and 48 MiB hard ceiling remain unchanged.
RESIDUAL_INCREMENT = (6 if sys.platform == "darwin" else 4) * MIB
RESIDUAL_SETTLE_WINDOWS = 8
RESIDUAL_WINDOW_SECONDS = 0.25
REPLY_BYTES = 900 * 1024
REJECTED_REPLY_BYTES = 15 * MIB
WATCHERS = 4


class Failed(Exception):
    """The live memory journey did not hold."""


@dataclass(frozen=True)
class Evidence:
    """RSS measurements around one hot streamed event."""

    baseline: int
    peak: int
    residual: int


def problems(evidence: Evidence, enforce_hot_increment: bool = True) -> list[str]:
    """Return every memory contract violation."""
    found: list[str] = []
    if not all(
        math.isfinite(value) for value in (evidence.baseline, evidence.peak, evidence.residual)
    ):
        return ["a memory sample was not finite"]
    if evidence.peak < evidence.baseline:
        found.append("peak is below baseline")
    if evidence.peak > HARD_CEILING:
        found.append(f"peak exceeds the {HARD_CEILING // MIB} MiB hard ceiling")
    if enforce_hot_increment and evidence.peak - evidence.baseline > HOT_INCREMENT:
        found.append("one hot session and four watchers add more than 10 MiB")
    if evidence.residual - evidence.baseline > RESIDUAL_INCREMENT:
        found.append(
            f"released watch memory leaves more than {RESIDUAL_INCREMENT // MIB} MiB resident"
        )
    return found


def watchProblems(outputs: list[bytes], reply_bytes: int, admitted: bool) -> list[str]:
    """Return delivery defects from every real watch client."""
    found: list[str] = []
    if len(outputs) != WATCHERS:
        found.append(f"expected {WATCHERS} watcher outputs, got {len(outputs)}")
        return found
    for index, output in enumerate(outputs):
        if admitted:
            if output.count(b"x") < reply_bytes:
                found.append(f"watcher {index} did not receive the complete admitted payload")
        else:
            if b"watch lagged" not in output:
                found.append(f"watcher {index} received no explicit oversize lag boundary")
            if output.count(b"x") >= reply_bytes:
                found.append(f"watcher {index} received a rejected oversize payload")
    return found


def selftest() -> int:
    """Prove memory and delivery defects each make the gate red."""
    green = Evidence(baseline=12 * MIB, peak=18 * MIB, residual=13 * MIB)
    defects = {
        "hard ceiling": replace(green, peak=HARD_CEILING + 1),
        "hot increment": replace(green, peak=green.baseline + HOT_INCREMENT + 1),
        "residual": replace(green, residual=green.baseline + RESIDUAL_INCREMENT + 1),
        "invalid sample": replace(green, peak=green.baseline - 1),
        "nonfinite sample": replace(green, residual=float("nan")),
    }
    if problems(green):
        print("[liveMemoryBudget --selftest] FAIL. green fixture was rejected.", file=sys.stderr)
        return 2
    for name, evidence in defects.items():
        if not problems(evidence):
            print(f"[liveMemoryBudget --selftest] FAIL. {name} escaped.", file=sys.stderr)
            return 2
    admitted = [b"x" * REPLY_BYTES for _ in range(WATCHERS)]
    rejected = [b"watch lagged  reconnect after cursor" for _ in range(WATCHERS)]
    if watchProblems(admitted, REPLY_BYTES, admitted=True):
        print("[liveMemoryBudget --selftest] FAIL. admitted fixture was rejected.", file=sys.stderr)
        return 2
    if watchProblems(rejected, REJECTED_REPLY_BYTES, admitted=False):
        print("[liveMemoryBudget --selftest] FAIL. rejected fixture was rejected.", file=sys.stderr)
        return 2
    if not watchProblems(admitted[:-1] + [b"short"], REPLY_BYTES, admitted=True):
        print("[liveMemoryBudget --selftest] FAIL. missing delivery escaped.", file=sys.stderr)
        return 2
    if not watchProblems(rejected[:-1] + [b"watching only"], REJECTED_REPLY_BYTES, admitted=False):
        print("[liveMemoryBudget --selftest] FAIL. silent oversize drop escaped.", file=sys.stderr)
        return 2
    settled = selectSettledResidual(
        [green.baseline + RESIDUAL_INCREMENT + 1, green.baseline + RESIDUAL_INCREMENT],
        green.baseline,
    )
    if settled != green.baseline + RESIDUAL_INCREMENT:
        print("[liveMemoryBudget --selftest] FAIL. bounded settling was rejected.", file=sys.stderr)
        return 2
    unsettled = selectSettledResidual(
        [green.baseline + RESIDUAL_INCREMENT + 2, green.baseline + RESIDUAL_INCREMENT + 1],
        green.baseline,
    )
    if unsettled != green.baseline + RESIDUAL_INCREMENT + 1:
        print("[liveMemoryBudget --selftest] FAIL. the lowest failed sample was lost.", file=sys.stderr)
        return 2
    print("[liveMemoryBudget --selftest] OK. memory and delivery defects make the gate red.")
    return 0


def windowsResident(pid: int) -> int:
    """Read one process working set through the Windows process API."""

    class Counters(ctypes.Structure):
        _fields_ = [
            ("cb", ctypes.c_ulong),
            ("page_fault_count", ctypes.c_ulong),
            ("peak_working_set_size", ctypes.c_size_t),
            ("working_set_size", ctypes.c_size_t),
            ("quota_peak_paged_pool_usage", ctypes.c_size_t),
            ("quota_paged_pool_usage", ctypes.c_size_t),
            ("quota_peak_non_paged_pool_usage", ctypes.c_size_t),
            ("quota_non_paged_pool_usage", ctypes.c_size_t),
            ("pagefile_usage", ctypes.c_size_t),
            ("peak_pagefile_usage", ctypes.c_size_t),
        ]

    query_information = 0x0400
    read_memory = 0x0010
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
    kernel.OpenProcess.restype = ctypes.c_void_p
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    psapi.GetProcessMemoryInfo.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(Counters),
        ctypes.c_ulong,
    ]
    handle = kernel.OpenProcess(query_information | read_memory, False, pid)
    if not handle:
        raise OSError(ctypes.get_last_error(), "OpenProcess failed")
    try:
        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            raise OSError(ctypes.get_last_error(), "GetProcessMemoryInfo failed")
        return int(counters.working_set_size)
    finally:
        kernel.CloseHandle(handle)


def resident(pid: int) -> int:
    """Read resident bytes through this platform's process surface."""
    if sys.platform == "win32":
        return windowsResident(pid)
    status = Path(f"/proc/{pid}/status")
    if status.is_file():
        for line in status.read_text(encoding="utf-8").splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) * 1024
        raise Failed("the daemon status has no VmRSS")
    measured = subprocess.run(
        ["ps", "-o", "rss=", "-p", str(pid)],
        capture_output=True,
        text=True,
        check=False,
        timeout=5.0,
    )
    if measured.returncode != 0 or not measured.stdout.strip().isdigit():
        raise Failed("the operating system returned no resident size")
    return int(measured.stdout.strip()) * 1024


def sample(pid: int, seconds: float) -> int:
    """Return the largest RSS observed during a fixed window."""
    deadline = time.monotonic() + seconds
    peak = 0
    while time.monotonic() < deadline:
        peak = max(peak, resident(pid))
        time.sleep(0.01)
    return peak


def settledResidual(pid: int, baseline: int) -> int:
    """Require one complete RSS window to settle within the residual ceiling."""
    observations = (
        sample(pid, RESIDUAL_WINDOW_SECONDS) for _ in range(RESIDUAL_SETTLE_WINDOWS)
    )
    return selectSettledResidual(observations, baseline)


def selectSettledResidual(observations: Iterable[int], baseline: int) -> int:
    """Return the first settled window or the lowest failed complete window."""
    lowest = math.inf
    for observed in observations:
        lowest = min(lowest, observed)
        if observed - baseline <= RESIDUAL_INCREMENT:
            return observed
    if not math.isfinite(lowest):
        raise Failed("the residual settling window produced no memory sample")
    return int(lowest)


def manifest(home: Path, fixture: Path, reply_bytes: int) -> None:
    """Declare the large-reply fixture without adding provider knowledge to the product."""
    providers = home / "providers"
    providers.mkdir(parents=True)
    text = f'''schema = 1
id = "{acp.PROVIDER}"
display_name = "ACP Memory Fixture"
kind = "acp"

[bin]
names = [{json.dumps(fixture.name)}]

[probe]
version = {{ args = ["--version"], parse = "semver-anywhere" }}

[transport]
argv = ["--reply-bytes", "{reply_bytes}"]
listen = "stdio"
'''
    (providers / f"{acp.PROVIDER}.toml").write_text(text, encoding="utf-8")


def stop(process: subprocess.Popen[str]) -> None:
    """Stop exactly one gate-owned client process."""
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=2.0)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=2.0)


def outputsReady(paths: list[Path], reply_bytes: int, admitted: bool) -> bool:
    """Whether every watcher has persisted enough evidence for its expected outcome."""
    if admitted:
        return all(path.exists() and path.stat().st_size >= reply_bytes for path in paths)
    return all(path.exists() and b"watch lagged" in path.read_bytes() for path in paths)


def waitWatcherReady(watcher: subprocess.Popen[str], output: Path) -> None:
    """Require one endpoint acknowledgement before opening the next concurrent watcher."""
    deadline = time.monotonic() + acp.TURN_WAIT_S
    while time.monotonic() < deadline:
        if watcher.poll() is not None:
            stdout, stderr = watcher.communicate(timeout=2.0)
            detail = (stderr or stdout or "watch client exited without diagnostics").strip()
            raise Failed(f"a watch client ended before its acknowledgement: {detail}")
        if output.exists() and b"watching" in output.read_bytes():
            return
        time.sleep(0.025)
    raise Failed("a watch client did not acknowledge its subscription")


def exerciseCase(
    binary: Path, fixture: Path, reply_bytes: int, admitted: bool
) -> Evidence:
    """Measure one admitted event or three rejected events across four real watch connections."""
    with tempfile.TemporaryDirectory(prefix="runtrol-live-memory-") as raw_home:
        home = Path(raw_home)
        workspace = home / "workspace"
        workspace.mkdir()
        manifest(home, fixture, reply_bytes)
        environment = acp.environment(home, fixture)
        daemon = acp.startDaemon(binary, environment, home)
        watchers: list[subprocess.Popen[str]] = []
        outputPaths: list[Path] = []
        try:
            baseline = sample(daemon.pid, 0.5)
            peak = baseline
            cases = 1 if admitted else 3
            for case in range(cases):
                session = acp.command(binary, environment, ["start", acp.PROVIDER, str(workspace)])
                if acp.SESSION_RE.fullmatch(session) is None:
                    raise Failed(f"start returned no session identifier: {session!r}")
                outputPaths = []
                for index in range(WATCHERS):
                    outputPath = home / f"watch-{case}-{index}.out"
                    outputPaths.append(outputPath)
                    with outputPath.open("wb") as output:
                        watcher = subprocess.Popen(
                            [str(binary), "watch", session],
                            cwd=acp.ROOT,
                            env=environment,
                            stdout=output,
                            stderr=subprocess.PIPE,
                            text=True,
                            encoding="utf-8",
                            errors="replace",
                        )
                    watchers.append(watcher)
                    waitWatcherReady(watcher, outputPath)
                prompt = subprocess.Popen(
                    [str(binary), "say", session, f"large memory gate event {case + 1}"],
                    cwd=acp.ROOT,
                    env=environment,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                )
                deadline = time.monotonic() + acp.TURN_WAIT_S
                while time.monotonic() < deadline:
                    peak = max(peak, resident(daemon.pid))
                    if prompt.poll() is not None and outputsReady(outputPaths, reply_bytes, admitted):
                        break
                    time.sleep(0.005)
                if prompt.poll() is None:
                    stop(prompt)
                    raise Failed("the large provider event did not finish")
                stdout, stderr = prompt.communicate(timeout=2.0)
                if prompt.returncode != 0:
                    raise Failed((stderr or stdout or "the prompt failed without output").strip())
                if not outputsReady(outputPaths, reply_bytes, admitted):
                    expected = "complete payload" if admitted else "explicit lag boundary"
                    raise Failed(f"not every watcher received the {expected}")
                peak = max(peak, sample(daemon.pid, 0.5))

                for watcher in watchers:
                    stop(watcher)
                watchers.clear()
                outputs = [path.read_bytes() for path in outputPaths]
                deliveryProblems = watchProblems(outputs, reply_bytes, admitted)
                if deliveryProblems:
                    raise Failed("; ".join(deliveryProblems))
                acp.command(binary, environment, ["close", session, "--now"])
                time.sleep(0.25)
            residual = settledResidual(daemon.pid, baseline)
            evidence = Evidence(baseline=baseline, peak=peak, residual=residual)
            found = problems(evidence, enforce_hot_increment=admitted)
            if found:
                raise Failed(
                    "; ".join(found)
                    + f" (baseline={baseline}, peak={peak}, residual={residual})"
                )
            return evidence
        except (Failed, acp.Failed, OSError, subprocess.SubprocessError) as error:
            if daemon.poll() is None:
                time.sleep(0.1)
            if daemon.poll() is not None:
                stdout, stderr = daemon.communicate(timeout=2.0)
                detail = (stderr or stdout or "daemon exited without diagnostics").strip()
                raise Failed(f"{error}; daemon exited: {detail}") from error
            raise
        finally:
            for watcher in watchers:
                stop(watcher)
            acp.stopDaemon(daemon)


def exercise() -> tuple[Evidence, Evidence]:
    """Measure admitted delivery and rejected oversize handling in isolated daemons."""
    binary, fixture = acp.build()
    admitted = exerciseCase(binary, fixture, REPLY_BYTES, admitted=True)
    rejected = exerciseCase(binary, fixture, REJECTED_REPLY_BYTES, admitted=False)
    return admitted, rejected


def main(argv: list[str]) -> int:
    """Run the selftest or the live process measurement."""
    if "--selftest" in argv:
        return selftest()
    try:
        admitted, rejected = exercise()
    except (Failed, acp.Failed, OSError, subprocess.SubprocessError) as error:
        print(f"[liveMemoryBudget] FAIL. {error}", file=sys.stderr)
        return 2
    print(
        "[liveMemoryBudget] OK. "
        f"admitted baseline {admitted.baseline / MIB:.1f} MiB, peak {admitted.peak / MIB:.1f} MiB, "
        f"residual {admitted.residual / MIB:.1f} MiB; "
        f"rejected baseline {rejected.baseline / MIB:.1f} MiB, peak {rejected.peak / MIB:.1f} MiB, "
        f"residual {rejected.residual / MIB:.1f} MiB."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
