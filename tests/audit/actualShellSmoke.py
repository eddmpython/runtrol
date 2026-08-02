"""Gate: the production Tauri window crosses the real command, IPC, and event seams.

The browser behavior gates replace ``window.__TAURI__``. This gate does not. It launches the product binary's
``gui`` personality against an isolated real daemon and the deterministic external ACP fixture. The page must
obtain its session row through a Rust command, install a watch through local IPC, and receive a provider event
serialized by Tauri.

Windows is the first product-window runner because that is the operator's current desktop and because an actual
top-level window can be identified there without adding an automation dependency.

Usage::

    python -X utf8 tests/audit/actualShellSmoke.py --selftest
    python -X utf8 tests/audit/actualShellSmoke.py
    python -X utf8 tests/audit/actualShellSmoke.py --built-product

The default command builds its own production artifacts. ``--built-product`` requires and reuses the release
product and release fixture prepared by the desktop product build gate.
"""

from __future__ import annotations

import ctypes
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Callable

import genericAcpSmoke as acp
import guiMemoryContract as production

ROOT = Path(__file__).resolve().parents[2]
WINDOW_WAIT_S = 20.0
TRACE_WAIT_S = 20.0
EVIDENCE_NAMES = ("product", "fixture", "attestation")


class Failed(Exception):
    """The actual product window journey did not hold."""


def requiredTrace(session: str) -> tuple[str, ...]:
    """The independent crossings a successful product-window journey must report."""
    return (
        "first list at ",
        f"row {acp.PROVIDER} {session} ",
        f"watching {session} view=",
        "relayed a frame to the page: taken",
        "frame queued",
    )


def missingTrace(trace: str, session: str) -> list[str]:
    """Return every product boundary whose evidence is absent."""
    return [marker for marker in requiredTrace(session) if marker not in trace]


def changedEvidence(before: tuple[str, ...], after: tuple[str, ...]) -> list[str]:
    """Name every production artifact whose identity changed during a journey."""
    return [name for name, first, last in zip(EVIDENCE_NAMES, before, after) if first != last]


def selftest() -> None:
    """Prove every independent boundary can make the gate red."""
    session = "11111111-1111-1111-1111-111111111111"
    markers = requiredTrace(session)
    green = "\n".join(markers)
    if missingTrace(green, session):
        raise Failed("selftest defect: complete evidence was rejected")
    for absent in markers:
        broken = "\n".join(marker for marker in markers if marker != absent)
        if missingTrace(broken, session) != [absent]:
            raise Failed(f"selftest defect: {absent!r} could not make the gate red")
    evidence = ("1" * 64, "2" * 64, "3" * 64)
    if changedEvidence(evidence, evidence):
        raise Failed("selftest defect: stable production evidence was rejected")
    for index, name in enumerate(EVIDENCE_NAMES):
        changed = list(evidence)
        changed[index] = "4" * 64
        if changedEvidence(evidence, tuple(changed)) != [name]:
            raise Failed(f"selftest defect: {name} identity changes could not make the gate red")
    print(
        "[actualShellSmoke --selftest] OK. all five product boundaries and three evidence identities "
        "can make the gate red."
    )


def build() -> tuple[Path, Path]:
    """Build the production page, release product, and release workload fixture once."""
    try:
        production.buildProduction()
    except production.Failed as error:
        raise Failed(str(error)) from error
    return builtProduct()


def builtProduct() -> tuple[Path, Path]:
    """Require release artifacts with current canonical production build evidence."""
    product = production.DEFAULT_BINARY.resolve()
    fixture = production.DEFAULT_FIXTURE.resolve()
    for binary in (product, fixture):
        if not binary.is_file():
            raise Failed(f"the built production artifact is missing: {binary.relative_to(ROOT)}")
    productEvidence(product, fixture)
    return product, fixture


def digest(path: Path) -> str:
    """Identify the exact release bits used by this product journey."""
    return production.binaryDigest(path)


def productEvidence(product: Path, fixture: Path) -> tuple[str, str, str]:
    """Validate and identify the product, fixture, and canonical build attestation."""
    try:
        attestation = production.validatedAttestation(
            product,
            fixture,
            production.attestationPath(product),
        )
    except production.Failed as error:
        raise Failed(str(error)) from error
    return (
        digest(product),
        digest(fixture),
        production.canonicalDigest(attestation),
    )


def visibleWindow(pid: int) -> bool:
    """Whether this process owns a visible top-level window titled runtrol."""
    if sys.platform != "win32":
        return False

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


def readTrace(path: Path) -> str:
    """Read the append-only process trace while its writer is still alive."""
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""


def waitFor(
    predicate: Callable[[], bool],
    process: subprocess.Popen[bytes],
    deadline: float,
    what: str,
) -> None:
    """Wait for one observable product fact, refusing an exited window."""
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise Failed(f"the product window exited before {what}")
        if predicate():
            return
        time.sleep(0.05)
    raise Failed(f"the product window did not expose {what}")


def waitForTrace(
    path: Path,
    process: subprocess.Popen[bytes],
    markers: tuple[str, ...],
    deadline: float,
    what: str,
) -> None:
    """Wait for exact trace markers and report the evidence that did arrive."""
    waitFor(
        lambda: all(marker in readTrace(path) for marker in markers),
        process,
        deadline,
        what,
    )


def stopProcessTree(process: subprocess.Popen[bytes] | subprocess.Popen[str]) -> None:
    """Stop the exact process tree started by this gate."""
    if process.poll() is not None:
        return
    if sys.platform == "win32":
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            cwd=ROOT,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=5.0,
        )
    else:
        process.terminate()
    try:
        process.wait(timeout=5.0)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5.0)


def exercise(reuseBuiltProduct: bool = False) -> None:
    """Launch the actual window and drive one real event across every desktop seam."""
    if sys.platform != "win32":
        print("[actualShellSmoke] SKIP. the first product-window runner is Windows.")
        return

    binary, fixture = builtProduct() if reuseBuiltProduct else build()
    initialEvidence = productEvidence(binary, fixture)
    with tempfile.TemporaryDirectory(prefix="runtrol-actual-shell-") as raw_home:
        home = Path(raw_home)
        workspace = home / "workspace"
        workspace.mkdir()
        acp.manifest(home, fixture)
        environment = acp.environment(home, fixture)
        environment["RUNTROL_GUI_TRACE"] = "1"
        daemon = acp.startDaemon(binary, environment, home)
        gui: subprocess.Popen[bytes] | None = None
        session: str | None = None
        tracePath = home / "gui-trace.txt"
        try:
            session = acp.command(binary, environment, ["start", acp.PROVIDER, str(workspace)])
            if acp.SESSION_RE.fullmatch(session) is None:
                raise Failed(f"start returned no session identifier: {session!r}")

            with tracePath.open("wb") as traceFile:
                gui = subprocess.Popen(
                    [str(binary), "gui"],
                    cwd=ROOT,
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=traceFile,
                    stderr=subprocess.STDOUT,
                )
                waitFor(
                    lambda: visibleWindow(gui.pid),
                    gui,
                    time.monotonic() + WINDOW_WAIT_S,
                    "a visible top-level Tauri window",
                )
                watching = requiredTrace(session)[:3]
                try:
                    waitForTrace(
                        tracePath,
                        gui,
                        watching,
                        time.monotonic() + TRACE_WAIT_S,
                        "the command and local IPC watch evidence",
                    )
                except Failed as error:
                    arrived = readTrace(tracePath)
                    absent = [marker for marker in watching if marker not in arrived]
                    raise Failed(f"{error}; missing {absent!r}; trace was {arrived!r}") from error

                acp.command(binary, environment, ["say", session, "actual shell smoke"])
                try:
                    waitForTrace(
                        tracePath,
                        gui,
                        requiredTrace(session),
                        time.monotonic() + TRACE_WAIT_S,
                        "the serialized Tauri event evidence",
                    )
                except Failed as error:
                    arrived = readTrace(tracePath)
                    absent = missingTrace(arrived, session)
                    raise Failed(f"{error}; missing {absent!r}; trace was {arrived!r}") from error

        finally:
            try:
                if gui is not None:
                    stopProcessTree(gui)
            finally:
                try:
                    if session is not None:
                        acp.command(binary, environment, ["close", session, "--now"])
                finally:
                    acp.stopDaemon(daemon)
    endingEvidence = productEvidence(binary, fixture)
    changed = changedEvidence(initialEvidence, endingEvidence)
    if changed:
        raise Failed(f"production evidence changed during the actual shell journey: {changed!r}")
    print(
        "[actualShellSmoke] OK. product gui personality with embedded production bundle, Rust commands, "
        f"local IPC, and Tauri events crossed. product sha256={initialEvidence[0]} "
        f"fixture sha256={initialEvidence[1]} attestation sha256={initialEvidence[2]}"
    )


def main(argv: list[str]) -> int:
    """Run the failure proof or the actual product-window journey."""
    try:
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
    except (Failed, acp.Failed, OSError, subprocess.SubprocessError) as error:
        print(f"[actualShellSmoke] FAIL. {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
