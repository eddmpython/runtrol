"""Gate: deleting runtrol's home leaves provider-owned session state usable.

The provider fixture keeps one marker outside ``RUNTROL_HOME``. The marker contains only its native session
identifier and completed-turn count, never prompt or response content. This gate completes a turn, stops the
daemon, deletes the whole runtrol home, resumes the native session directly through the provider executable while
runtrol is absent, then proves optional reinstallation can load the same native session and complete another turn.

Usage::

    python -X utf8 tests/audit/uninstallLeavesNoTrace.py
    python -X utf8 tests/audit/uninstallLeavesNoTrace.py --selftest

Exit codes:
    0 the journey completed, or every injected defect was rejected
    2 the provider-owned session did not survive independently of runtrol
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, replace
from pathlib import Path

import genericAcpSmoke as acp

ROOT = Path(__file__).resolve().parents[2]
NATIVE_SESSION = "fixture-session"


class Failed(Exception):
    """The uninstall journey did not hold."""


@dataclass(frozen=True)
class Evidence:
    """Facts observed across the destructive boundary."""

    home_deleted: bool
    direct_provider_resume: bool
    daemon_restarted: bool
    native_before: str
    native_after: str
    completed_before: int
    completed_after: int


def verifyEvidence(evidence: Evidence) -> None:
    """Reject any journey that did not cross every required boundary."""
    if not evidence.home_deleted:
        raise Failed("RUNTROL_HOME was not deleted before the resumed daemon started")
    if not evidence.direct_provider_resume:
        raise Failed("the native session was not resumed directly while runtrol was absent")
    if not evidence.daemon_restarted:
        raise Failed("the load ran without stopping one daemon and starting another")
    if evidence.native_after != evidence.native_before:
        raise Failed(
            f"the provider loaded {evidence.native_after!r}, not {evidence.native_before!r}"
        )
    if evidence.completed_before < 1 or evidence.completed_after <= evidence.completed_before:
        raise Failed("the provider-owned completed-turn marker did not survive and advance")


def selftest() -> int:
    """Prove deletion, restart, identity, and provider-state defects each make the gate red."""
    valid = Evidence(
        home_deleted=True,
        direct_provider_resume=True,
        daemon_restarted=True,
        native_before=NATIVE_SESSION,
        native_after=NATIVE_SESSION,
        completed_before=1,
        completed_after=2,
    )
    defects = {
        "home was not deleted": replace(valid, home_deleted=False),
        "direct provider resume failed": replace(valid, direct_provider_resume=False),
        "daemon was not restarted": replace(valid, daemon_restarted=False),
        "native id changed": replace(valid, native_after="different-session"),
        "provider state was lost": replace(valid, completed_after=0),
    }
    problems: list[str] = []
    try:
        verifyEvidence(valid)
    except Failed as error:
        problems.append(f"the valid journey was rejected: {error}")

    for name, evidence in defects.items():
        rejected = False
        try:
            verifyEvidence(evidence)
        except Failed:
            rejected = True
        if not rejected:
            problems.append(f"{name} was accepted")

    if problems:
        print("[uninstallLeavesNoTrace --selftest] the gate cannot detect what it claims to.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print(
        f"[uninstallLeavesNoTrace --selftest] OK. {len(defects)} injected defects all made the gate red."
    )
    return 0


def manifest(home: Path, fixture: Path, marker: Path) -> None:
    """Declare the same external provider in whichever runtrol home currently exists."""
    providers = home / "providers"
    providers.mkdir(parents=True)
    executable = json.dumps(fixture.name)
    state = json.dumps(str(marker))
    text = f'''schema = 1
id = "{acp.PROVIDER}"
display_name = "ACP Fixture"
kind = "acp"

[bin]
names = [{executable}]

[probe]
version = {{ args = ["--version"], parse = "semver-anywhere" }}

[transport]
argv = ["--state", {state}]
listen = "stdio"
'''
    (providers / f"{acp.PROVIDER}.toml").write_text(text, encoding="utf-8")


def markerTurns(marker: Path) -> int:
    """Read only the fixture's provider metadata, refusing transcript-shaped or malformed state."""
    try:
        value = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise Failed(f"the provider marker is unreadable: {error}") from error
    if not isinstance(value, dict) or set(value) != {"native", "completed_turns"}:
        raise Failed("the provider marker contains fields other than identity and completed-turn count")
    if value.get("native") != NATIVE_SESSION:
        raise Failed(f"the provider marker names {value.get('native')!r}, not {NATIVE_SESSION!r}")
    turns = value.get("completed_turns")
    if not isinstance(turns, int) or isinstance(turns, bool) or turns < 0:
        raise Failed("the provider marker has no usable completed-turn count")
    return turns


def nativeFor(listing: str, session: str) -> str:
    """Read the provider-owned identifier from one product listing row."""
    for line in listing.splitlines():
        fields = line.split()
        if fields and fields[0] == session:
            if len(fields) < 5:
                raise Failed(f"the session listing is missing its native identifier: {line!r}")
            return fields[4]
    raise Failed(f"session {session} is absent from the listing")


def completeTurn(
    binary: Path,
    environment: dict[str, str],
    session: str,
    expected_reply: str,
) -> None:
    """Complete one real fixture turn through the product command and watcher surfaces."""
    watcher = subprocess.Popen(
        [str(binary), "watch", session],
        cwd=ROOT,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    try:
        time.sleep(0.25)
        acp.command(binary, environment, ["say", session, "opaque gate prompt"])
        deadline = time.monotonic() + acp.TURN_WAIT_S
        while time.monotonic() < deadline:
            row = next(
                (
                    line
                    for line in acp.command(binary, environment, ["list"]).splitlines()
                    if line.startswith(session)
                ),
                "",
            )
            if "  idle  " in row:
                break
            time.sleep(0.05)
        else:
            raise Failed("the provider did not declare the turn complete")

        watcher.terminate()
        stdout, stderr = watcher.communicate(timeout=5.0)
        watched = (stdout or "") + "\n" + (stderr or "")
        if expected_reply not in watched or '"step":"ended"' not in watched:
            raise Failed(f"the watcher did not receive {expected_reply!r} and provider completion")
    finally:
        if watcher.poll() is None:
            watcher.terminate()
            try:
                watcher.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                watcher.kill()
                watcher.wait(timeout=2.0)


def stopDaemon(daemon: subprocess.Popen[str]) -> None:
    """Terminate exactly the isolated daemon and require the process to exit."""
    acp.stopDaemon(daemon)
    if daemon.poll() is None:
        raise Failed("the first isolated daemon remained alive after termination")


def deleteHome(home: Path, root: Path) -> None:
    """Delete exactly this gate's runtrol home and no broader path."""
    resolved = home.resolve()
    if resolved.parent != root.resolve() or resolved.name != "runtrol-home":
        raise Failed(f"refusing to delete unexpected path {resolved}")
    shutil.rmtree(resolved)
    if resolved.exists():
        raise Failed(f"{resolved} still exists after deletion")


def directProviderResume(fixture: Path, marker: Path, native: str) -> bool:
    """Resume through the provider executable with no runtrol home or runtrol process present."""
    environment = dict(acp.os.environ)
    environment.pop("RUNTROL_HOME", None)
    completed = subprocess.run(
        [str(fixture), "--state", str(marker), "--resume", native],
        cwd=marker.parent,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=10.0,
        check=False,
    )
    if completed.returncode != 0:
        raise Failed(f"direct provider resume failed: {completed.stderr.strip()}")
    try:
        result = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise Failed("direct provider resume returned no JSON evidence") from error
    return result == {"native": native, "completedTurns": markerTurns(marker)}


def exercise() -> None:
    """Cross the uninstall boundary and prove the provider marker remains authoritative."""
    binary, fixture = acp.build()
    with tempfile.TemporaryDirectory(prefix="runtrol-uninstall-") as raw_root:
        root = Path(raw_root)
        home = root / "runtrol-home"
        workspace = root / "workspace"
        provider_state = root / "provider-state"
        marker = provider_state / "session.json"
        workspace.mkdir()
        provider_state.mkdir()

        manifest(home, fixture, marker)
        environment = acp.environment(home, fixture)
        first = acp.startDaemon(binary, environment, home)
        second: subprocess.Popen[str] | None = None
        try:
            started = acp.command(binary, environment, ["start", acp.PROVIDER, str(workspace)])
            if acp.SESSION_RE.fullmatch(started) is None:
                raise Failed(f"start returned no runtrol session identifier: {started!r}")
            native_before = nativeFor(acp.command(binary, environment, ["list"]), started)
            completeTurn(binary, environment, started, "fixture reply 1")
            completed_before = markerTurns(marker)

            stopDaemon(first)
            deleteHome(home, root)
            home_deleted = not home.exists()
            direct_provider_resume = directProviderResume(fixture, marker, native_before)

            manifest(home, fixture, marker)
            second = acp.startDaemon(binary, environment, home)
            restarted = first.poll() is not None and second.poll() is None

            resumed = acp.command(
                binary,
                environment,
                ["resume", acp.PROVIDER, native_before, str(workspace)],
            )
            if acp.SESSION_RE.fullmatch(resumed) is None or resumed == started:
                raise Failed(f"load returned no fresh runtrol session identifier: {resumed!r}")
            native_after = nativeFor(acp.command(binary, environment, ["list"]), resumed)
            completeTurn(binary, environment, resumed, "fixture reply 2")
            completed_after = markerTurns(marker)

            verifyEvidence(
                Evidence(
                    home_deleted=home_deleted,
                    direct_provider_resume=direct_provider_resume,
                    daemon_restarted=restarted,
                    native_before=native_before,
                    native_after=native_after,
                    completed_before=completed_before,
                    completed_after=completed_after,
                )
            )
            acp.command(binary, environment, ["close", resumed, "--now"])
            print(
                "[uninstallLeavesNoTrace] OK. provider state survived daemon stop and home deletion, "
                "resumed directly while runtrol was absent, then survived reinstallation and a second turn."
            )
        finally:
            if first.poll() is None:
                acp.stopDaemon(first)
            if second is not None:
                acp.stopDaemon(second)


def main(argv: list[str]) -> int:
    """Run the selftest or the real provider-owned-state journey."""
    if "--selftest" in argv:
        return selftest()
    try:
        exercise()
        return 0
    except (Failed, acp.Failed, OSError, subprocess.SubprocessError) as error:
        print(f"[uninstallLeavesNoTrace] FAIL: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
