"""Gate: ``runtrol gui`` hides only a console private to the GUI process.

The Windows journey launches the real product twice. A new console owned only by ``runtrol gui`` must become
hidden, while a console shared with a parent command process must stay visible. The Tauri window must remain
visible in both cases.

Usage::

    python -X utf8 tests/audit/desktopConsolePolicy.py --selftest
    python -X utf8 tests/audit/desktopConsolePolicy.py
    python -X utf8 tests/audit/desktopConsolePolicy.py --built-product

The default command builds its own production artifacts. ``--built-product`` requires and reuses the release
product and release fixture prepared by the desktop product build gate.
"""

from __future__ import annotations

import ctypes
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any, Callable

import actualShellSmoke as production
import genericAcpSmoke as acp

ROOT = Path(__file__).resolve().parents[2]
WINDOW_WAIT_S = 20.0


class Failed(Exception):
    """The production console presentation policy did not hold."""


def problems(result: dict[str, Any]) -> list[str]:
    """Return every absent product behaviour."""
    expected = {
        "privateConsoleNotVisible": True,
        "privateTauriVisible": True,
        "sharedConsoleStartedVisible": True,
        "sharedConsoleStayedVisible": True,
        "sharedTauriVisible": True,
    }
    return [name for name, wanted in expected.items() if result.get(name) is not wanted]


def samplerProblem(stillRunning: bool, errors: list[BaseException]) -> str | None:
    """Turn an incomplete or failed visibility observation into a gate defect."""
    if stillRunning:
        return "the private-console visibility sampler did not stop"
    if errors:
        return f"the private-console visibility sampler failed: {errors[0]}"
    return None


def cleanupProblems(actions: list[tuple[str, Callable[[], None]]]) -> list[str]:
    """Run every cleanup action even when an earlier action fails."""
    found: list[str] = []
    for name, action in actions:
        try:
            action()
        except BaseException as error:
            found.append(f"{name}: {error}")
    return found


def selftest() -> None:
    """Prove every private and shared console regression makes the gate red."""
    names = (
        "privateConsoleNotVisible",
        "privateTauriVisible",
        "sharedConsoleStartedVisible",
        "sharedConsoleStayedVisible",
        "sharedTauriVisible",
    )
    green = {name: True for name in names}
    if problems(green):
        raise Failed("selftest defect: complete evidence was rejected")
    for name in names:
        broken = dict(green)
        broken[name] = False
        if problems(broken) != [name]:
            raise Failed(f"selftest defect: {name} could not make the gate red")
    if samplerProblem(True, []) is None:
        raise Failed("selftest defect: a sampler that never stopped was accepted")
    if samplerProblem(False, [RuntimeError("injected")]) is None:
        raise Failed("selftest defect: a sampler exception was accepted")
    if samplerProblem(False, []) is not None:
        raise Failed("selftest defect: a clean sampler was rejected")
    cleanupVisits: list[str] = []

    def brokenCleanup() -> None:
        cleanupVisits.append("owner")
        raise RuntimeError("injected")

    def finalCleanup() -> None:
        cleanupVisits.append("daemon")

    cleanupErrors = cleanupProblems([("owner", brokenCleanup), ("daemon", finalCleanup)])
    if cleanupVisits != ["owner", "daemon"] or len(cleanupErrors) != 1:
        raise Failed("selftest defect: one cleanup failure skipped a later cleanup action")
    print("[desktopConsolePolicy --selftest] OK. console and sampler regressions make the gate red.")


def visibleTauriWindow(pid: int) -> bool:
    """Whether one process owns a visible top-level window titled runtrol."""
    user32 = ctypes.windll.user32
    found = False
    callbackType = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    @callbackType
    def inspect(window: int, _argument: int) -> bool:
        nonlocal found
        owner = ctypes.c_ulong()
        user32.GetWindowThreadProcessId(window, ctypes.byref(owner))
        if owner.value != pid or not user32.IsWindowVisible(window):
            return True
        length = user32.GetWindowTextLengthW(window)
        title = ctypes.create_unicode_buffer(length + 1)
        user32.GetWindowTextW(window, title, len(title))
        if title.value == "runtrol":
            found = True
            return False
        return True

    user32.EnumWindows(inspect, 0)
    return found


def waitFor(predicate: Callable[[], bool], process: subprocess.Popen[bytes], what: str) -> None:
    """Wait for one observable window fact while refusing an exited process."""
    deadline = time.monotonic() + WINDOW_WAIT_S
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise Failed(f"the product exited before {what}")
        if predicate():
            return
        # A one-frame console flash is a product defect. Poll faster than a 60 Hz frame from process launch
        # through first list paint so a brief visible transition cannot hide between readiness checks.
        time.sleep(0.005)
    raise Failed(f"the product did not expose {what}")


def traceContains(path: Path, marker: str) -> bool:
    """Whether a live product trace contains one exact boundary marker."""
    try:
        return marker in path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return False


def stop(process: subprocess.Popen[bytes]) -> None:
    """Stop exactly one Windows process tree."""
    if process.poll() is not None:
        return
    subprocess.run(
        ["taskkill", "/PID", str(process.pid), "/T", "/F"],
        cwd=ROOT,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
        timeout=5.0,
    )
    try:
        process.wait(timeout=5.0)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5.0)


def consoleWindows() -> dict[int, bool]:
    """All native console windows and their visibility, without attaching and changing their state."""
    user32 = ctypes.windll.user32
    found: dict[int, bool] = {}
    callbackType = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    @callbackType
    def inspect(window: int, _argument: int) -> bool:
        name = ctypes.create_unicode_buffer(64)
        user32.GetClassNameW(window, name, len(name))
        if name.value == "ConsoleWindowClass":
            found[int(window)] = bool(user32.IsWindowVisible(window))
        return True

    user32.EnumWindows(inspect, 0)
    return found


def sharedOwner(binary: Path, resultPath: Path) -> int:
    """Own a visible console while the product inherits and shares it."""
    kernel32 = ctypes.windll.kernel32
    user32 = ctypes.windll.user32
    window = kernel32.GetConsoleWindow()
    startedVisible = bool(window and user32.IsWindowVisible(window))
    tracePath = resultPath.with_suffix(".trace")
    with tracePath.open("wb") as traceFile:
        gui = subprocess.Popen(
            [str(binary), "gui"],
            cwd=ROOT,
            env=os.environ,
            stdin=subprocess.DEVNULL,
            stdout=traceFile,
            stderr=subprocess.STDOUT,
        )
        try:
            waitFor(lambda: visibleTauriWindow(gui.pid), gui, "the shared-console Tauri window")
            waitFor(lambda: traceContains(tracePath, "first list at "), gui, "the embedded production page")
            result = {
                "startedVisible": startedVisible,
                "stayedVisible": bool(window and user32.IsWindowVisible(window)),
                "tauriVisible": visibleTauriWindow(gui.pid),
            }
            resultPath.write_text(json.dumps(result), encoding="utf-8")
            return 0
        finally:
            stop(gui)


def exercise(reuseBuiltProduct: bool = False) -> None:
    """Measure private hiding and shared preservation on the actual product executable."""
    if sys.platform != "win32":
        print("[desktopConsolePolicy] SKIP. Windows owns the console-window policy.")
        return
    try:
        binary, fixture = production.builtProduct() if reuseBuiltProduct else production.build()
        initialEvidence = production.productEvidence(binary, fixture)
    except production.Failed as error:
        raise Failed(str(error)) from error
    with tempfile.TemporaryDirectory(prefix="runtrol-console-policy-") as rawHome:
        home = Path(rawHome)
        environment = dict(os.environ)
        environment["RUNTROL_HOME"] = str(home)
        environment["RUNTROL_GUI_TRACE"] = "1"
        daemon = acp.startDaemon(binary, environment, home)
        private: subprocess.Popen[bytes] | None = None
        owner: subprocess.Popen[bytes] | None = None
        try:
            consolesBefore = consoleWindows()
            privateConsoleBecameVisible = False
            privateSamplingStop = threading.Event()
            privateSamplingErrors: list[BaseException] = []

            def samplePrivateConsole() -> None:
                """Observe the whole launch interval, including time spent inside process creation."""
                nonlocal privateConsoleBecameVisible
                try:
                    while not privateSamplingStop.is_set():
                        current = consoleWindows()
                        if any(visible for window, visible in current.items() if window not in consolesBefore):
                            privateConsoleBecameVisible = True
                        privateSamplingStop.wait(0.005)
                except BaseException as error:
                    privateSamplingErrors.append(error)

            privateSampler = threading.Thread(
                target=samplePrivateConsole,
                name="runtrol-console-visibility-sampler",
                daemon=True,
            )
            privateSampler.start()
            privateTrace = home / "private.trace"
            try:
                with privateTrace.open("wb") as traceFile:
                    private = subprocess.Popen(
                        [str(binary), "gui"],
                        cwd=ROOT,
                        env=environment,
                        stdin=subprocess.DEVNULL,
                        stdout=traceFile,
                        stderr=subprocess.STDOUT,
                        creationflags=subprocess.CREATE_NEW_CONSOLE,
                    )
                    waitFor(lambda: visibleTauriWindow(private.pid), private, "the private-console Tauri window")
                    waitFor(
                        lambda: traceContains(privateTrace, "first list at "),
                        private,
                        "the embedded production page",
                    )
                    privateTauri = visibleTauriWindow(private.pid)
            finally:
                privateSamplingStop.set()
                privateSampler.join(timeout=2.0)
            samplingDefect = samplerProblem(privateSampler.is_alive(), privateSamplingErrors)
            if samplingDefect is not None:
                raise Failed(samplingDefect)
            stop(private)
            private = None

            sharedResult = home / "shared.json"
            owner = subprocess.Popen(
                [
                    sys.executable,
                    "-X",
                    "utf8",
                    __file__,
                    "--shared-owner",
                    str(binary),
                    str(sharedResult),
                ],
                cwd=ROOT,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                creationflags=subprocess.CREATE_NEW_CONSOLE,
            )
            owner.wait(timeout=WINDOW_WAIT_S + 10.0)
            if owner.returncode != 0 or not sharedResult.is_file():
                raise Failed("the shared-console owner produced no result")
            shared = json.loads(sharedResult.read_text(encoding="utf-8"))
            result = {
                # A pseudoconsole may have no enumerable top-level HWND. What the operator can observe is whether
                # any new console became visible between process launch and the first production list paint.
                "privateConsoleNotVisible": not privateConsoleBecameVisible,
                "privateTauriVisible": privateTauri,
                "sharedConsoleStartedVisible": shared["startedVisible"],
                "sharedConsoleStayedVisible": shared["stayedVisible"],
                "sharedTauriVisible": shared["tauriVisible"],
            }
            found = problems(result)
            if found:
                raise Failed(f"missing product behaviours: {found}; measured {result}")
        finally:
            cleanupActions: list[tuple[str, Callable[[], None]]] = []
            if owner is not None and owner.poll() is None:
                cleanupActions.append(("shared owner", lambda: stop(owner)))
            if private is not None:
                cleanupActions.append(("private GUI", lambda: stop(private)))
            cleanupActions.append(("daemon", lambda: acp.stopDaemon(daemon)))
            cleanupErrors = cleanupProblems(cleanupActions)
            if cleanupErrors:
                raise Failed(f"cleanup failures: {cleanupErrors!r}")
    try:
        endingEvidence = production.productEvidence(binary, fixture)
    except production.Failed as error:
        raise Failed(str(error)) from error
    changed = production.changedEvidence(initialEvidence, endingEvidence)
    if changed:
        raise Failed(f"production evidence changed during the console journey: {changed!r}")
    print(
        "[desktopConsolePolicy] OK. private console hidden, shared console preserved. "
        f"product sha256={initialEvidence[0]} fixture sha256={initialEvidence[1]} "
        f"attestation sha256={initialEvidence[2]}"
    )


def main(argv: list[str]) -> int:
    """Dispatch helper modes, the selftest, or the product journey."""
    try:
        if len(argv) == 3 and argv[0] == "--shared-owner":
            return sharedOwner(Path(argv[1]), Path(argv[2]))
        known = {"--selftest", "--built-product"}
        unknown = sorted(set(argv).difference(known))
        if unknown:
            raise Failed(f"unknown argument: {unknown[0]}")
        if "--selftest" in argv and "--built-product" in argv:
            raise Failed("--selftest and --built-product are separate actions")
        if "--selftest" in argv:
            selftest()
        else:
            exercise(reuseBuiltProduct="--built-product" in argv)
    except (Failed, acp.Failed, OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"[desktopConsolePolicy] FAIL. {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
