"""Gate: an idle debug daemon stays inside its RSS budget and uses at most one percent of one CPU.

The existing Rust memoryBudget test remains the RSS number source. This gate runs that exact test, then starts a
separate real daemon and measures process CPU time from the operating system across a ten-second idle window.

Usage::

    python -X utf8 tests/audit/idleFootprintRatchet.py --selftest
    python -X utf8 tests/audit/idleFootprintRatchet.py
"""

from __future__ import annotations

import ctypes
import math
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MEMORY_GATE = [
    "cargo",
    "test",
    "-p",
    "runtrol-audit",
    "--test",
    "memoryBudget",
    "an_idle_daemon_stays_inside_its_budget",
    "--",
    "--exact",
]
BUILD_GATE = ["cargo", "build", "-p", "runtrol", "--bin", "runtrol"]
START_WITHIN = 20.0

# How long to wait for the daemon to finish the work starting costs before the idle sample begins.
#
# A fixed pause was wrong, not merely short. A daemon opening a home it has never seen asks every coding service
# on the machine what it is, and on this machine that ran past five seconds: the sample then measured provider
# discovery and called it idle, which is a different claim about a different thing (measured 2026-08-26: 125 to
# 172 ms with a five second pause, and exactly zero once the discovery had finished). So the daemon is watched
# until it goes quiet by itself, and only then is idle measured. A machine slower than this one waits longer
# rather than failing a budget it never broke.
QUIET_SLICE_SECONDS = 2.0
QUIET_SLICE_BUDGET_SECONDS = 0.010
# Four, because starting arrives in waves rather than as one push. Profiled 2026-08-26 on a fresh home: 1.7 s of
# work in the first four seconds, silence, then a second burst of about 150 ms between ten and fourteen seconds,
# and nothing at all after sixteen. Two quiet slices ended in the gap and sampled the second wave; four require
# eight seconds of silence, which the gap between waves does not offer.
QUIET_SLICES_REQUIRED = 4
SETTLE_CEILING_SECONDS = 90.0
SAMPLE_SECONDS = 10.0
MIN_SAMPLE_SECONDS = 9.5
CPU_BUDGET_SECONDS = 0.100


class Failed(Exception):
    """The idle footprint journey could not produce trustworthy evidence."""


def cargoTargetDir() -> Path:
    """Resolve Cargo's active output directory exactly as the build subprocess does."""
    configured = os.environ.get("CARGO_TARGET_DIR")
    if not configured:
        return ROOT / "target"
    target = Path(configured)
    return target if target.is_absolute() else ROOT / target


def productBinary() -> Path:
    """The daemon produced by this gate's Cargo build."""
    suffix = ".exe" if sys.platform == "win32" else ""
    return cargoTargetDir() / "debug" / f"runtrol{suffix}"


def problems(cpu_delta: float, elapsed: float) -> list[str]:
    """Return every CPU ratchet defect in one measurement."""
    found: list[str] = []
    if not math.isfinite(cpu_delta) or not math.isfinite(elapsed):
        found.append("the idle sample was not finite")
        return found
    if cpu_delta < 0:
        found.append("process CPU time moved backwards")
    if elapsed < MIN_SAMPLE_SECONDS:
        found.append("the idle sample window was too short")
    if cpu_delta > CPU_BUDGET_SECONDS:
        found.append("idle daemon CPU exceeded 100 ms in 10 seconds")
    return found


def selftest() -> int:
    """Prove each independent measurement defect makes the gate red."""
    if problems(0.0, SAMPLE_SECONDS):
        print("[idleFootprintRatchet --selftest] FAIL. green evidence was rejected.", file=sys.stderr)
        return 2
    defects = [
        (-0.001, SAMPLE_SECONDS),
        (0.0, MIN_SAMPLE_SECONDS - 0.001),
        (CPU_BUDGET_SECONDS + 0.000001, SAMPLE_SECONDS),
        (float("nan"), SAMPLE_SECONDS),
        (0.0, float("nan")),
    ]
    if any(not problems(cpu_delta, elapsed) for cpu_delta, elapsed in defects):
        print("[idleFootprintRatchet --selftest] FAIL. a CPU defect escaped.", file=sys.stderr)
        return 2
    print("[idleFootprintRatchet --selftest] OK. invalid and expensive idle samples are red.")
    return 0


def windowsCpuSeconds(pid: int) -> float:
    """Read one process's kernel and user time through the Windows process API."""

    class FileTime(ctypes.Structure):
        _fields_ = [("low", ctypes.c_uint32), ("high", ctypes.c_uint32)]

    query_limited_information = 0x1000
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
    kernel.OpenProcess.restype = ctypes.c_void_p
    kernel.GetProcessTimes.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(FileTime),
        ctypes.POINTER(FileTime),
        ctypes.POINTER(FileTime),
        ctypes.POINTER(FileTime),
    ]
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    handle = kernel.OpenProcess(query_limited_information, False, pid)
    if not handle:
        raise Failed(f"OpenProcess could not inspect daemon {pid}")
    try:
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
            raise Failed(f"GetProcessTimes could not inspect daemon {pid}")
        ticks = (
            (kernel_time.high << 32)
            + kernel_time.low
            + (user_time.high << 32)
            + user_time.low
        )
        return ticks / 10_000_000
    finally:
        kernel.CloseHandle(handle)


def linuxCpuSeconds(pid: int) -> float:
    """Read one process's user and kernel ticks from procfs."""
    text = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    closing = text.rfind(")")
    if closing < 0:
        raise Failed("the daemon stat record has no command boundary")
    fields = text[closing + 2 :].split()
    if len(fields) <= 12:
        raise Failed("the daemon stat record has no CPU fields")
    ticks_per_second = os.sysconf("SC_CLK_TCK")
    return (int(fields[11]) + int(fields[12])) / ticks_per_second


def parsePsTime(value: str) -> float:
    """Parse the portable [[days-]hours:]minutes:seconds process time shape."""
    text = value.strip()
    days = 0
    if "-" in text:
        day_text, text = text.split("-", 1)
        days = int(day_text)
    pieces = text.split(":")
    if len(pieces) == 2:
        hours = 0
        minutes, seconds = pieces
    elif len(pieces) == 3:
        hours, minutes, seconds = pieces
    else:
        raise Failed(f"ps returned an unreadable CPU time: {value!r}")
    return days * 86_400 + int(hours) * 3_600 + int(minutes) * 60 + float(seconds)


def unixPsCpuSeconds(pid: int) -> float:
    """Read cumulative CPU time through the system ps tool."""
    measured = subprocess.run(
        ["ps", "-o", "time=", "-p", str(pid)],
        capture_output=True,
        text=True,
        check=False,
        timeout=5.0,
    )
    if measured.returncode != 0 or not measured.stdout.strip():
        raise Failed(f"ps could not inspect daemon {pid}")
    return parsePsTime(measured.stdout)


def cpuSeconds(pid: int) -> float:
    """Read cumulative CPU seconds from this operating system."""
    if sys.platform == "win32":
        return windowsCpuSeconds(pid)
    if sys.platform.startswith("linux"):
        return linuxCpuSeconds(pid)
    return unixPsCpuSeconds(pid)


def stop(process: subprocess.Popen[bytes]) -> None:
    """Stop and reap exactly one gate-owned daemon."""
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=2.0)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=2.0)


def measureCpu() -> tuple[float, float]:
    """Start one idle daemon and return its CPU delta and wall-clock sample length."""
    binary = productBinary()
    if not binary.is_file():
        raise Failed(f"product binary is missing: {binary}")
    with tempfile.TemporaryDirectory(prefix="runtrol-idle-ratchet-") as raw_home:
        home = Path(raw_home)
        environment = os.environ.copy()
        environment["RUNTROL_HOME"] = str(home)
        creation_flags = subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0
        daemon = subprocess.Popen(
            [str(binary), "daemon"],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=creation_flags,
        )
        try:
            deadline = time.monotonic() + START_WITHIN
            while time.monotonic() < deadline:
                if daemon.poll() is not None:
                    raise Failed(f"daemon exited during startup with code {daemon.returncode}")
                if (home / "providers").is_dir():
                    break
                time.sleep(0.025)
            else:
                raise Failed("daemon did not open its home within 20 seconds")

            quiet = 0
            settle_deadline = time.monotonic() + SETTLE_CEILING_SECONDS
            while quiet < QUIET_SLICES_REQUIRED:
                slice_before = cpuSeconds(daemon.pid)
                time.sleep(QUIET_SLICE_SECONDS)
                if daemon.poll() is not None:
                    raise Failed(f"daemon exited while settling with code {daemon.returncode}")
                if cpuSeconds(daemon.pid) - slice_before <= QUIET_SLICE_BUDGET_SECONDS:
                    quiet += 1
                else:
                    quiet = 0
                if time.monotonic() >= settle_deadline:
                    raise Failed(
                        "the daemon never went quiet: startup work was still running after "
                        f"{SETTLE_CEILING_SECONDS:.0f} seconds"
                    )
            cpu_before = cpuSeconds(daemon.pid)
            started = time.monotonic()
            deadline = started + SAMPLE_SECONDS
            while time.monotonic() < deadline:
                if daemon.poll() is not None:
                    raise Failed(f"daemon exited during the idle sample with code {daemon.returncode}")
                time.sleep(0.050)
            elapsed = time.monotonic() - started
            cpu_after = cpuSeconds(daemon.pid)
            return cpu_after - cpu_before, elapsed
        finally:
            stop(daemon)


def main(argv: list[str]) -> int:
    """Run the selftest or both real idle footprint measurements."""
    if "--selftest" in argv:
        return selftest()
    built = subprocess.run(BUILD_GATE, cwd=ROOT, check=False)
    if built.returncode != 0:
        print("[idleFootprintRatchet] FAIL. the product daemon build failed.", file=sys.stderr)
        return built.returncode
    memory = subprocess.run(MEMORY_GATE, cwd=ROOT, check=False)
    if memory.returncode != 0:
        print("[idleFootprintRatchet] FAIL. the idle RSS contract failed.", file=sys.stderr)
        return memory.returncode
    try:
        cpu_delta, elapsed = measureCpu()
    except (Failed, OSError, ValueError) as error:
        print(f"[idleFootprintRatchet] FAIL. {error}", file=sys.stderr)
        return 2
    found = problems(cpu_delta, elapsed)
    if found:
        print(
            f"[idleFootprintRatchet] FAIL. idle CPU contract regressed: "
            f"CPU {cpu_delta:.6f}s over {elapsed:.3f}s.",
            file=sys.stderr,
        )
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print(
        f"[idleFootprintRatchet] OK. RSS held; CPU {cpu_delta:.6f}s over {elapsed:.3f}s."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
