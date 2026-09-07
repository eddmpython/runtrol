"""One exact Runtime resource cohort, including its completion keeper on Windows."""

from __future__ import annotations

import ctypes
import json
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class Failed(Exception):
    """The resource cohort or its positive completion could not be proved."""


class FileTime(ctypes.Structure):
    _fields_ = [("low", ctypes.c_uint32), ("high", ctypes.c_uint32)]

    def ticks(self) -> int:
        return (self.high << 32) | self.low


class Counters(ctypes.Structure):
    _fields_ = [
        ("cb", ctypes.c_ulong), ("pageFaultCount", ctypes.c_ulong),
        ("peakWorkingSet", ctypes.c_size_t), ("workingSet", ctypes.c_size_t),
        ("peakPagedPool", ctypes.c_size_t), ("pagedPool", ctypes.c_size_t),
        ("peakNonPagedPool", ctypes.c_size_t), ("nonPagedPool", ctypes.c_size_t),
        ("pagefile", ctypes.c_size_t), ("peakPagefile", ctypes.c_size_t),
    ]


class WindowsProcess:
    """Retain the exact process object, so later samples cannot follow PID reuse."""

    def __init__(self, pid: int, started: int | None = None):
        self.pid = pid
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.psapi = ctypes.WinDLL("psapi", use_last_error=True)
        self.kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        self.kernel.OpenProcess.restype = ctypes.c_void_p
        self.kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        self.kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        self.kernel.GetProcessTimes.argtypes = [ctypes.c_void_p] + [ctypes.POINTER(FileTime)] * 4
        self.psapi.GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.POINTER(Counters), ctypes.c_ulong]
        self.handle = self.kernel.OpenProcess(0x1000 | 0x0010 | 0x00100000, False, pid)
        if not self.handle:
            raise OSError(ctypes.get_last_error(), f"cannot inspect process {pid}")
        try:
            actual, _ = self.times()
            if started is not None and actual != started:
                raise Failed(f"process {pid} has a different birth identity")
            self.started = actual
        except BaseException:
            self.close()
            raise

    def times(self) -> tuple[int, float]:
        if self.kernel.WaitForSingleObject(self.handle, 0) != 258:
            raise Failed(f"measured process {self.pid} ended or could not be inspected")
        created, exited, kernel, user = FileTime(), FileTime(), FileTime(), FileTime()
        if not self.kernel.GetProcessTimes(self.handle, ctypes.byref(created), ctypes.byref(exited),
                                          ctypes.byref(kernel), ctypes.byref(user)):
            raise OSError(ctypes.get_last_error(), f"cannot read process times for {self.pid}")
        return created.ticks(), (kernel.ticks() + user.ticks()) / 10_000_000

    def resident(self) -> int:
        self.times()
        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        if not self.psapi.GetProcessMemoryInfo(self.handle, ctypes.byref(counters), counters.cb):
            raise OSError(ctypes.get_last_error(), f"cannot read process RSS for {self.pid}")
        return int(counters.workingSet)

    def close(self) -> None:
        if self.handle:
            if not self.kernel.CloseHandle(self.handle):
                raise OSError(ctypes.get_last_error(), f"cannot close process handle {self.pid}")
            self.handle = None


def build() -> None:
    """Build the private wire consumer; it has no provider or transcript access."""
    built = subprocess.run(["cargo", "build", "-p", "runtrol-audit", "--example", "runtimeFootprint"],
                           cwd=ROOT, check=False)
    if built.returncode:
        raise Failed("the exact Runtime completion observer did not build")


def observerBinary() -> Path:
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    return target / "debug" / "examples" / ("runtimeFootprint.exe" if sys.platform == "win32" else "runtimeFootprint")


class Home:
    """A fixture home is disposable only after its exact cohort positively completes."""

    def __init__(self, prefix: str):
        self.path = Path(tempfile.mkdtemp(prefix=prefix))
        self.completed = False

    def __enter__(self) -> Path:
        return self.path

    def __exit__(self, *_error) -> None:
        if self.completed:
            shutil.rmtree(self.path)
        else:
            print(f"[runtimeFootprint] completion unconfirmed; retaining {self.path}", file=sys.stderr)


class Cohort:
    """Read only Runtime and keeper HANDLEs; the retained observer owns stop completion."""

    def __init__(self, daemon: subprocess.Popen, home: Home):
        self.daemon = daemon
        self.home = home
        print("[runtimeFootprint] " + json.dumps({"runtimePid": daemon.pid, "home": str(home.path)}), flush=True)
        self.processes: list[WindowsProcess] = []
        self.messages: queue.Queue[str] = queue.Queue(maxsize=3)
        self.observer: subprocess.Popen | None = None
        deadline = time.monotonic() + 20.0
        while not (home.path / "runtime.locator.json").is_file():
            if daemon.poll() is not None or time.monotonic() >= deadline:
                raise Failed("the owned Runtime did not publish its locator")
            time.sleep(0.025)
        self.observer = subprocess.Popen([str(observerBinary()), str(home.path), str(daemon.pid)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        text=True, encoding="utf-8", creationflags=(subprocess.CREATE_NO_WINDOW
                                                                                 if sys.platform == "win32" else 0))
        def receive() -> None:
            assert self.observer is not None and self.observer.stdout is not None
            for line in self.observer.stdout:
                self.messages.put(line)
            self.messages.put("")
        self.reader = threading.Thread(target=receive, daemon=True)
        self.reader.start()
        try:
            proof = self.message()
            members = proof.get("members")
            if not isinstance(members, list) or len(members) != (2 if sys.platform == "win32" else 1):
                raise Failed("the observer supplied the wrong resource membership")
            if members[0].get("pid") != daemon.pid or len({member["pid"] for member in members}) != len(members):
                raise Failed("the observer supplied an unrelated or repeated process")
            if sys.platform == "win32":
                for member in members:
                    self.processes.append(WindowsProcess(member["pid"], int(member["started"])))
            print("[runtimeFootprint] " + json.dumps({"members": members}), flush=True)
        except BaseException:
            self.stop()
            raise

    def message(self) -> dict:
        try:
            line = self.messages.get(timeout=35.0)
        except queue.Empty as error:
            raise Failed("the exact completion observer did not answer") from error
        if not line:
            assert self.observer is not None
            detail = self.observer.stderr.read(4096) if self.observer.stderr else ""
            raise Failed(f"the exact completion observer ended: {detail}")
        return json.loads(line)

    def resident(self, single) -> int:
        return sum(process.resident() for process in self.processes) if self.processes else single(self.daemon.pid)

    def cpu(self, single) -> float:
        return sum(process.times()[1] for process in self.processes) if self.processes else single(self.daemon.pid)

    def stop(self) -> None:
        try:
            assert self.observer is not None
            if self.observer.stdin and not self.observer.stdin.closed:
                self.observer.stdin.write("stop\n")
                self.observer.stdin.close()
            if self.message() != {"completed": True}:
                raise Failed("the observer did not positively confirm completion")
            if self.observer.wait(timeout=5.0) != 0:
                raise Failed("the completion observer failed after its response")
            self.daemon.wait(timeout=5.0)
            self.home.completed = True
        finally:
            cleanupErrors: list[Exception] = []
            for process in self.processes:
                try:
                    process.close()
                except OSError as error:
                    cleanupErrors.append(error)
            for process in (self.observer, self.daemon):
                if process is not None and process.poll() is None:
                    try:
                        # Exact Popen objects belong to this fixture. Root-only teardown never authorizes removal.
                        process.terminate()
                        process.wait(timeout=5.0)
                    except (OSError, subprocess.SubprocessError) as error:
                        cleanupErrors.append(error)
            if cleanupErrors:
                raise Failed("resource handle cleanup failed: " + "; ".join(map(str, cleanupErrors)))


def selftest() -> None:
    """Inject excluded cost and failed completion into the actual cohort methods."""
    from io import StringIO
    from types import SimpleNamespace
    from unittest.mock import Mock, patch

    cohort = object.__new__(Cohort)
    cohort.processes = [SimpleNamespace(resident=lambda: 11, times=lambda: (1, 0.025)),
                        SimpleNamespace(resident=lambda: 7, times=lambda: (2, 0.050))]
    excluded = Mock(side_effect=AssertionError("provider or watch cost was requested"))
    assert cohort.resident(excluded) == 18
    assert abs(cohort.cpu(excluded) - 0.075) < 0.000001
    excluded.assert_not_called()

    for reply, code, completed in [({"completed": True}, 0, True),
                                   ({"completed": False}, 0, False),
                                   ({"completed": True}, 1, False)]:
        fixture = object.__new__(Cohort)
        fixture.processes = []
        fixture.home = SimpleNamespace(completed=False)
        fixture.observer = SimpleNamespace(stdin=StringIO(), wait=lambda **_: code, poll=lambda: code)
        fixture.daemon = SimpleNamespace(wait=lambda **_: 0, poll=lambda: 0)
        fixture.message = lambda: reply
        try:
            fixture.stop()
        except Failed:
            assert not completed
        assert fixture.home.completed is completed

    directory = object.__new__(Home)
    directory.path = Path("exact-gate-owned-home")
    directory.completed = False
    with patch.object(shutil, "rmtree") as remove, patch("sys.stderr", StringIO()):
        directory.__exit__()
        remove.assert_not_called()
        directory.completed = True
        directory.__exit__()
        remove.assert_called_once_with(directory.path)

    if sys.platform == "win32":
        current = WindowsProcess(os.getpid())
        try:
            assert current.resident() > 0
            try:
                WindowsProcess(os.getpid(), current.started + 1)
            except Failed:
                # ok: the changed birth must be refused; the else branch rejects accidental acceptance.
                pass
            else:
                raise AssertionError("a recycled process identity was accepted")
        finally:
            current.close()
