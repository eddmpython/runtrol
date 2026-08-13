"""Gate: a provider declared only by TOML completes a real ACP child-process turn.

The fixture is deliberately a separate executable. This drives the same manifest loader, binary resolver,
daemon, process containment, JSON-RPC transport, session manager, CLI command surface, and event watcher an
operator uses. It needs no provider credential and spends no token.

This gate exercises fresh and loaded sessions under one runtrol home. Daemon restart, deletion of that home,
and provider-owned state survival belong to ``uninstallLeavesNoTrace.py``.

Usage::

    python -X utf8 tests/audit/genericAcpSmoke.py
    python -X utf8 tests/audit/genericAcpSmoke.py --selftest

Exit codes:
    0 the manifest-only journey completed, or the injected failure was detected
    2 the journey did not hold
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TIMEOUT_S = 90.0
TURN_WAIT_S = 10.0
PROVIDER = "fixture-acp"
SESSION_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
)


class Failed(Exception):
    """The manifest-only ACP journey did not hold."""


def verifyWatch(text: str) -> None:
    """Require both provider content and provider-declared completion."""
    if "fixture reply" not in text:
        raise Failed("the ACP message chunk did not reach the command watcher")
    if '"step":"ended"' not in text or '"stop":"endTurn"' not in text:
        raise Failed("the ACP prompt response did not end the turn as the provider's word")


def selftest() -> None:
    """Prove the gate rejects a stream with content but no completion."""
    broken = '{"event":"agentMessageChunk","text":"fixture reply"}'
    try:
        verifyWatch(broken)
    except Failed:
        print("[genericAcpSmoke:selftest] OK. a missing provider completion is red.")
        return
    raise Failed("selftest defect: a stream with no completion passed")


def build() -> tuple[Path, Path]:
    """Build the product and the separate protocol process from this tree."""
    for words in (
        ["cargo", "build", "-p", "runtrol", "--bin", "runtrol"],
        ["cargo", "build", "-p", "runtrol-drivers", "--example", "acpFixture"],
    ):
        built = subprocess.run(
            words,
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_S,
        )
        if built.returncode != 0:
            detail = (built.stderr or built.stdout or "cargo build failed without output").strip()
            raise Failed(detail)
    suffix = ".exe" if sys.platform == "win32" else ""
    runtrol = ROOT / "target" / "debug" / f"runtrol{suffix}"
    fixture = ROOT / "target" / "debug" / "examples" / f"acpFixture{suffix}"
    for binary in (runtrol, fixture):
        if not binary.is_file():
            raise Failed(f"cargo succeeded but {binary.relative_to(ROOT)} is missing")
    return runtrol, fixture


def buildNativeProbe() -> Path:
    """Build the separate consumer that drives only the official ACP catalogue SPI."""
    built = subprocess.run(
        ["cargo", "build", "-p", "runtrol-drivers", "--example", "nativeCatalogueProbe"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        timeout=TIMEOUT_S,
    )
    if built.returncode != 0:
        detail = (built.stderr or built.stdout or "native catalogue probe build failed").strip()
        raise Failed(detail)
    suffix = ".exe" if sys.platform == "win32" else ""
    probe = ROOT / "target" / "debug" / "examples" / f"nativeCatalogueProbe{suffix}"
    if not probe.is_file():
        raise Failed(f"cargo succeeded but {probe.relative_to(ROOT)} is missing")
    return probe


def manifest(home: Path, fixture: Path) -> None:
    """Declare the fixture exactly where an operator declares a third provider."""
    providers = home / "providers"
    providers.mkdir(parents=True)
    text = f'''schema = 1
id = "{PROVIDER}"
display_name = "ACP Fixture"
kind = "acp"

[bin]
names = ["{fixture.name}"]

[probe]
version = {{ args = ["--version"], parse = "semver-anywhere" }}

[transport]
argv = []
listen = "stdio"
'''
    (providers / f"{PROVIDER}.toml").write_text(text, encoding="utf-8")


def environment(home: Path, fixture: Path) -> dict[str, str]:
    """Isolate the daemon and put only the fixture directory in front of PATH."""
    result = dict(os.environ)
    result["RUNTROL_HOME"] = str(home)
    result["PATH"] = f"{fixture.parent}{os.pathsep}{result.get('PATH', '')}"
    return result


def command(binary: Path, env: dict[str, str], words: list[str]) -> str:
    """Run one product command and require an answer."""
    proc = subprocess.run(
        [str(binary), *words],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=TIMEOUT_S,
        check=False,
    )
    said = (proc.stdout or "").strip() or (proc.stderr or "").strip()
    if proc.returncode != 0:
        raise Failed(f"runtrol {' '.join(words)} failed: {said}")
    return said


def startDaemon(binary: Path, env: dict[str, str], home: Path) -> subprocess.Popen[str]:
    """Start this gate's daemon explicitly and wait until its endpoint is ready."""
    daemon = subprocess.Popen(
        [str(binary), "daemon"],
        cwd=ROOT,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    ready = home / ("runtrol.redb" if sys.platform == "win32" else "runtrol.sock")
    deadline = time.monotonic() + TURN_WAIT_S
    while time.monotonic() < deadline:
        if daemon.poll() is not None:
            stdout, stderr = daemon.communicate()
            detail = (stderr or stdout or "daemon exited without output").strip()
            raise Failed(f"the isolated daemon exited before it was ready: {detail}")
        if ready.exists():
            # The database opens just before the named pipe is bound on Windows. Give that final bind one scheduling
            # turn so the first command never races the daemon and starts a second copy.
            if sys.platform == "win32":
                time.sleep(0.1)
            return daemon
        time.sleep(0.025)
    stopDaemon(daemon)
    raise Failed("the isolated daemon did not become ready")


def stopDaemon(daemon: subprocess.Popen[str]) -> None:
    """Stop exactly the daemon this gate started, on every platform."""
    if daemon.poll() is not None:
        return
    daemon.terminate()
    try:
        daemon.wait(timeout=2.0)
    except subprocess.TimeoutExpired:
        daemon.kill()
        daemon.wait(timeout=2.0)


def exercise() -> None:
    """Drive discovery, start, prompt, streamed output, completion, and cleanup."""
    binary, fixture = build()
    native_probe = buildNativeProbe()
    with tempfile.TemporaryDirectory(prefix="runtrol-acp-") as raw_home:
        home = Path(raw_home)
        workspace = home / "workspace"
        workspace.mkdir()
        probed = subprocess.run(
            [str(native_probe), str(fixture), str(workspace)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=TIMEOUT_S,
            check=False,
        )
        if probed.returncode != 0:
            detail = (probed.stderr or probed.stdout or "native catalogue probe failed").strip()
            raise Failed(detail)
        manifest(home, fixture)
        env = environment(home, fixture)
        daemon = startDaemon(binary, env, home)
        watcher: subprocess.Popen[str] | None = None
        try:
            catalogue = command(binary, env, ["models", PROVIDER])
            if "catalogue unknown" not in catalogue:
                raise Failed(f"the external manifest was not discovered as usable: {catalogue}")

            session = command(binary, env, ["start", PROVIDER, str(workspace)])
            if SESSION_RE.fullmatch(session) is None:
                raise Failed(f"start returned no session identifier: {session!r}")

            watcher = subprocess.Popen(
                [str(binary), "watch", session],
                cwd=ROOT,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            time.sleep(0.25)
            command(binary, env, ["say", session, "hello from the gate"])

            deadline = time.monotonic() + TURN_WAIT_S
            while time.monotonic() < deadline:
                listing = command(binary, env, ["list"])
                row = next((line for line in listing.splitlines() if line.startswith(session)), "")
                if "  idle  " in row:
                    break
                time.sleep(0.05)
            else:
                raise Failed("the ACP turn did not return to idle")

            watcher.terminate()
            stdout, stderr = watcher.communicate(timeout=5.0)
            watched = (stdout or "") + "\n" + (stderr or "")
            verifyWatch(watched)

            command(binary, env, ["close", session, "--now"])
            resumed = command(
                binary,
                env,
                ["resume", PROVIDER, "fixture-session", str(workspace)],
            )
            if SESSION_RE.fullmatch(resumed) is None or resumed == session:
                raise Failed(f"resume returned no fresh runtrol session identifier: {resumed!r}")
            command(binary, env, ["close", resumed, "--now"])
            print(
                "[genericAcpSmoke] OK. official ACP catalogue pages -> external TOML -> "
                "ACP child -> streamed turn -> completion -> load."
            )
        finally:
            if watcher is not None and watcher.poll() is None:
                watcher.terminate()
                try:
                    watcher.wait(timeout=2.0)
                except subprocess.TimeoutExpired:
                    watcher.kill()
                    watcher.wait(timeout=2.0)
            stopDaemon(daemon)


def main(argv: list[str]) -> int:
    """Run the selftest or the real child-process gate."""
    try:
        if "--selftest" in argv:
            selftest()
        else:
            exercise()
        return 0
    except (Failed, subprocess.SubprocessError) as error:
        print(f"[genericAcpSmoke] FAIL: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
