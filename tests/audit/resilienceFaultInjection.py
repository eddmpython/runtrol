"""Gate: local IPC reconnect and daemon restart expose exact continuity or an explicit gap.

This drives a real daemon, real local endpoint, and the external ACP fixture. A disconnected watcher receives the
bounded in-memory replay exactly once. A hard daemon restart creates a new stream, reports the old cursor as a gap,
and continues only through the provider's native resume surface. It does not claim a phone, remote network, transcript
recovery, or lossless history.

Usage::

    python -X utf8 tests/audit/resilienceFaultInjection.py --selftest
    python -X utf8 tests/audit/resilienceFaultInjection.py
"""

from __future__ import annotations

import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path

import genericAcpSmoke as acp

ROOT = Path(__file__).resolve().parents[2]
NATIVE_SESSION = "fixture-session"
MAX_WATCH_BYTES = 1024 * 1024
CURSOR_RE = re.compile(r"^watching\s+([^\s]+:\d+:\d+)$", re.MULTILINE)
GAP_RE = re.compile(
    r"^watch gap\s+requested\s+([^\s]+:\d+:\d+)\s+live\s+([^\s]+:\d+:\d+)$",
    re.MULTILINE,
)
EVENT_CURSOR_RE = re.compile(
    r"^watch event\s+next\s+([^\s]+):(\d+):(\d+)$",
    re.MULTILINE,
)


class Failed(Exception):
    """The local resilience journey did not hold."""


@dataclass(frozen=True)
class Evidence:
    """Facts observed on both sides of the hard daemon restart."""

    live_text: str
    replay_text: str
    requested_cursor: str
    replay_live_cursor: str
    gap_requested: str
    resumed_cursor: str
    gap_live: str
    restart_text: str
    native_before: str
    native_after: str


@dataclass(frozen=True)
class ProcessRecord:
    """Kernel-backed identity and parentage for one process table row."""

    parent: int
    started: str
    executable: str


@dataclass
class BoundedWatcher:
    """One watch process whose captured output can never exceed the evidence bound."""

    process: subprocess.Popen[bytes]
    reader: threading.Thread
    overflow: threading.Event

    def wait(self, timeout: float) -> int:
        """Reap the process and its output reader."""
        code = self.process.wait(timeout=timeout)
        self.reader.join(timeout=2.0)
        if self.reader.is_alive():
            raise Failed("watcher output reader did not stop")
        return code

    def requireBound(self) -> None:
        """Reject a watcher that attempted to cross the fixed output bound."""
        if self.overflow.is_set():
            raise Failed("watcher output exceeded its 1 MiB evidence bound")


def cursorParts(cursor: str) -> tuple[str, int, int]:
    """Split one exact watch boundary without interpreting provider content."""
    match = re.fullmatch(r"([^\s:]+):(\d+):(\d+)", cursor)
    if match is None:
        raise Failed(f"invalid watch cursor: {cursor!r}")
    return match.group(1), int(match.group(2)), int(match.group(3))


def framedEvents(text: str, *, expect_gap: bool) -> list[tuple[str, int, int, str]]:
    """Split controlled one-line fixture payloads from their transport-owned boundaries."""
    lines = text.splitlines()
    if not lines or CURSOR_RE.fullmatch(lines[0]) is None:
        raise Failed("watch output did not begin with exactly one watching acknowledgement")
    body_at = 1
    if expect_gap:
        if len(lines) < 2 or GAP_RE.fullmatch(lines[1]) is None:
            raise Failed("watch output did not place its exact gap after the acknowledgement")
        body_at = 2
    if (len(lines) - body_at) % 2 != 0:
        raise Failed("watch output contained an unframed line")
    frames: list[tuple[str, int, int, str]] = []
    for index in range(body_at, len(lines), 2):
        marker = EVENT_CURSOR_RE.fullmatch(lines[index])
        if marker is None:
            raise Failed(f"watch output contained an unexpected transport line: {lines[index]!r}")
        frames.append(
            (
                marker.group(1),
                int(marker.group(2)),
                int(marker.group(3)),
                lines[index + 1],
            )
        )
    return frames


def requireExactReplayBoundaries(evidence: Evidence) -> None:
    """Require every dense replay position from the requested boundary to the live edge."""
    if len(CURSOR_RE.findall(evidence.replay_text)) != 1:
        raise Failed("bounded replay did not expose exactly one watching acknowledgement")
    if GAP_RE.search(evidence.replay_text) is not None or "watch lagged" in evidence.replay_text:
        raise Failed("bounded replay unexpectedly reported a gap or lag")
    requested_stream, requested_epoch, requested_seq = cursorParts(evidence.requested_cursor)
    live_stream, live_epoch, live_seq = cursorParts(evidence.replay_live_cursor)
    if (live_stream, live_epoch) != (requested_stream, requested_epoch):
        raise Failed("bounded replay acknowledgement changed stream or epoch")
    expected = [
        (requested_stream, requested_epoch, seq)
        for seq in range(requested_seq + 1, live_seq + 1)
    ]
    replay_frames = framedEvents(evidence.replay_text, expect_gap=False)
    live_frames = framedEvents(evidence.live_text, expect_gap=False)
    if replay_frames != live_frames:
        raise Failed("bounded replay transport headers or opaque payload bytes changed from live delivery")
    observed = [frame[:3] for frame in replay_frames]
    if len(observed) != 6:
        raise Failed(f"bounded replay exposed {len(observed)} event boundaries instead of 6")
    if observed != expected:
        raise Failed(f"bounded replay cursor sequence was not exact: {observed} != {expected}")


def requireDenseRestartBoundaries(evidence: Evidence) -> None:
    """Require the resumed live stream to begin at its acknowledged edge with no holes."""
    if len(CURSOR_RE.findall(evidence.restart_text)) != 1:
        raise Failed("provider-native continuation did not expose exactly one watching acknowledgement")
    if len(GAP_RE.findall(evidence.restart_text)) != 1 or "watch lagged" in evidence.restart_text:
        raise Failed("provider-native continuation did not expose exactly one gap and no lag")
    stream, epoch, seq = cursorParts(evidence.resumed_cursor)
    observed = [frame[:3] for frame in framedEvents(evidence.restart_text, expect_gap=True)]
    if len(observed) != 3:
        raise Failed(f"provider-native continuation exposed {len(observed)} event boundaries instead of 3")
    expected = [(stream, epoch, value) for value in range(seq + 1, seq + len(observed) + 1)]
    if observed != expected:
        raise Failed(f"provider-native continuation cursor sequence was not dense: {observed} != {expected}")


def verifyEvidence(evidence: Evidence) -> None:
    """Reject replay duplication, silent restart, stream reuse, and provider identity loss."""
    requireExactReplayBoundaries(evidence)
    replay_turns = [int(turn) for turn in re.findall(r"fixture reply (\d+)", evidence.replay_text)]
    if replay_turns != [2, 3]:
        raise Failed(f"bounded replay reply order was not exactly [2, 3]: {replay_turns}")
    if evidence.gap_requested != evidence.requested_cursor:
        raise Failed("restart gap did not name the requested old cursor")
    if evidence.gap_live != evidence.resumed_cursor:
        raise Failed("restart gap did not name the new live cursor")
    if evidence.requested_cursor.split(":", 1)[0] == evidence.resumed_cursor.split(":", 1)[0]:
        raise Failed("daemon restart reused the old stream identifier")
    if "watch gap" not in evidence.restart_text:
        raise Failed("daemon restart skipped old content without an explicit gap")
    restart_turns = [int(turn) for turn in re.findall(r"fixture reply (\d+)", evidence.restart_text)]
    if restart_turns != [4]:
        raise Failed(f"provider-native continuation reply order was not exactly [4]: {restart_turns}")
    if '"step":"ended"' not in evidence.restart_text:
        raise Failed("the resumed provider did not declare turn completion")
    requireDenseRestartBoundaries(evidence)
    if evidence.native_after != evidence.native_before:
        raise Failed("provider-native identity changed across daemon restart")
    if evidence.native_before != NATIVE_SESSION:
        raise Failed("the journey did not preserve the fixture's provider-native identity")


def selftest() -> int:
    """Prove every independent replay and restart defect makes the gate red."""
    old = "11111111-1111-1111-1111-111111111111:0:2"
    replay_live = "11111111-1111-1111-1111-111111111111:0:8"
    new = "22222222-2222-2222-2222-222222222222:0:0"
    valid = Evidence(
        live_text=(
            f"watching  {old}\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:3\n{\"step\":\"started\"}\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:4\nfixture reply 2\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:5\n{\"step\":\"ended\"}\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:6\n{\"step\":\"started\"}\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:7\nfixture reply 3\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:8\n{\"step\":\"ended\"}"
        ),
        replay_text=(
            f"watching  {replay_live}\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:3\n{\"step\":\"started\"}\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:4\nfixture reply 2\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:5\n{\"step\":\"ended\"}\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:6\n{\"step\":\"started\"}\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:7\nfixture reply 3\n"
            "watch event  next 11111111-1111-1111-1111-111111111111:0:8\n{\"step\":\"ended\"}"
        ),
        requested_cursor=old,
        replay_live_cursor=replay_live,
        gap_requested=old,
        resumed_cursor=new,
        gap_live=new,
        restart_text=(
            f"watching  {new}\nwatch gap  requested {old}  live {new}\n"
            "watch event  next 22222222-2222-2222-2222-222222222222:0:1\n{\"step\":\"started\"}\n"
            "watch event  next 22222222-2222-2222-2222-222222222222:0:2\nfixture reply 4\n"
            "watch event  next 22222222-2222-2222-2222-222222222222:0:3\n\"step\":\"ended\""
        ),
        native_before=NATIVE_SESSION,
        native_after=NATIVE_SESSION,
    )
    defects = [
        replace(valid, replay_text=valid.replay_text.replace("fixture reply 3", "")),
        replace(valid, replay_text=valid.replay_text.replace("fixture reply 3", "fixture reply 2\nfixture reply 3")),
        replace(valid, replay_text=valid.replay_text.replace("fixture reply 2", "fixture reply 1\nfixture reply 2")),
        replace(
            valid,
            replay_text=valid.replay_text.replace("fixture reply 2", "fixture reply temporary").replace(
                "fixture reply 3", "fixture reply 2"
            ).replace("fixture reply temporary", "fixture reply 3"),
        ),
        replace(valid, replay_text=valid.replay_text.replace("fixture reply 3", "fixture reply 3\nfixture reply 5")),
        replace(valid, replay_text=valid.replay_text.replace('{"step":"started"}', "corrupted-start", 1)),
        replace(valid, replay_text=valid.replay_text + "\nunexpected unframed output"),
        replace(valid, replay_text=valid.replay_text.replace(":0:3", ":0:2", 1)),
        replace(valid, replay_text=valid.replay_text.replace(":0:4", ":0:5", 1)),
        replace(
            valid,
            replay_text=valid.replay_text.replace(
                "watch event  next 11111111-1111-1111-1111-111111111111:0:4\n",
                "watch event  next 11111111-1111-1111-1111-111111111111:0:3\n"
                "watch event  next 11111111-1111-1111-1111-111111111111:0:4\n",
                1,
            ),
        ),
        replace(valid, gap_requested=new),
        replace(valid, gap_live=old),
        replace(valid, resumed_cursor=old, gap_live=old),
        replace(valid, restart_text="fixture reply 4\n\"step\":\"ended\""),
        replace(valid, restart_text=valid.restart_text + "\nfixture reply 2"),
        replace(valid, restart_text=valid.restart_text + "\nfixture reply 5"),
        replace(valid, restart_text=valid.restart_text.replace('\n"step":"ended"', "")),
        replace(valid, restart_text=valid.restart_text.replace(":0:1", ":0:0", 1)),
        replace(valid, native_after="different-native"),
        replace(valid, native_before="wrong-native", native_after="wrong-native"),
    ]
    try:
        verifyEvidence(valid)
    except Failed as error:
        print(f"[resilienceFaultInjection --selftest] FAIL. valid evidence was rejected: {error}", file=sys.stderr)
        return 2
    for evidence in defects:
        try:
            verifyEvidence(evidence)
        except Failed:
            # ok: each fixture is deliberately invalid; the loop fails if any one is accepted.
            continue
        print("[resilienceFaultInjection --selftest] FAIL. an injected defect escaped.", file=sys.stderr)
        return 2
    print(f"[resilienceFaultInjection --selftest] OK. {len(defects)} continuity defects are red.")
    return 0


def manifest(home: Path, fixture: Path, marker: Path) -> None:
    """Declare the fixture with provider-owned identity and turn-count state outside runtrol home."""
    providers = home / "providers"
    providers.mkdir(parents=True)
    text = f'''schema = 1
id = "{acp.PROVIDER}"
display_name = "ACP Resilience Fixture"
kind = "acp"

[bin]
names = [{json.dumps(fixture.name)}]

[probe]
version = {{ args = ["--version"], parse = "semver-anywhere" }}

[transport]
argv = ["--state", {json.dumps(str(marker))}]
listen = "stdio"
'''
    (providers / f"{acp.PROVIDER}.toml").write_text(text, encoding="utf-8")


def nativeFor(listing: str, session: str) -> str:
    """Read the provider-owned identifier from one product listing row."""
    for line in listing.splitlines():
        fields = line.split()
        if fields and fields[0] == session:
            if len(fields) < 5:
                raise Failed(f"session listing has no native identifier: {line!r}")
            return fields[4]
    raise Failed(f"session {session} is absent from the listing")


def waitIdle(binary: Path, environment: dict[str, str], session: str) -> None:
    """Wait until the provider has declared the current turn complete."""
    deadline = time.monotonic() + acp.TURN_WAIT_S
    while time.monotonic() < deadline:
        listing = acp.command(binary, environment, ["list"])
        row = next((line for line in listing.splitlines() if line.startswith(session)), "")
        if "  idle  " in row:
            return
        time.sleep(0.05)
    raise Failed(f"session {session} did not return to idle")


def completeTurn(binary: Path, environment: dict[str, str], session: str, turn: int) -> None:
    """Send one opaque prompt and wait for provider-declared completion."""
    acp.command(binary, environment, ["say", session, f"opaque fault turn {turn}"])
    waitIdle(binary, environment, session)


def startWatcher(
    binary: Path,
    environment: dict[str, str],
    session: str,
    output: Path,
    after: str | None = None,
) -> BoundedWatcher:
    """Start one real endpoint watcher with output in a bounded temporary file."""
    words = [str(binary), "watch", session]
    if after is not None:
        words.extend(["--after", after])
    process = subprocess.Popen(
        words,
        cwd=ROOT,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    overflow = threading.Event()

    def capture() -> None:
        kept = 0
        with output.open("wb") as destination:
            source = process.stdout
            if source is None:
                overflow.set()
                if process.poll() is None:
                    process.kill()
                return
            while chunk := source.read1(64 * 1024):
                remaining = MAX_WATCH_BYTES - kept
                if len(chunk) > remaining:
                    destination.write(chunk[:remaining])
                    destination.flush()
                    overflow.set()
                    if process.poll() is None:
                        process.kill()
                    return
                destination.write(chunk)
                destination.flush()
                kept += len(chunk)

    reader = threading.Thread(target=capture, name="bounded-watch-output")
    reader.start()
    return BoundedWatcher(process=process, reader=reader, overflow=overflow)


def readOutput(path: Path, watcher: BoundedWatcher | None = None) -> str:
    """Read watcher output without assuming every frame is valid UTF-8."""
    if watcher is not None and watcher.overflow.is_set():
        raise Failed("watcher output exceeded its 1 MiB evidence bound")
    if path.exists() and path.stat().st_size > MAX_WATCH_BYTES:
        raise Failed("watcher output exceeded its 1 MiB evidence bound")
    return path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""


def waitFor(
    path: Path,
    predicate: Callable[[str], bool],
    description: str,
    watcher: BoundedWatcher,
) -> str:
    """Wait until a watcher output predicate accepts its accumulated text."""
    deadline = time.monotonic() + acp.TURN_WAIT_S
    while time.monotonic() < deadline:
        text = readOutput(path, watcher)
        if predicate(text):
            return text
        time.sleep(0.025)
    raise Failed(f"watcher did not report {description}: {readOutput(path, watcher)!r}")


def stopWatcher(watcher: BoundedWatcher) -> None:
    """Stop and reap exactly one gate-owned watcher."""
    if watcher.process.poll() is None:
        watcher.process.terminate()
        try:
            watcher.wait(timeout=2.0)
        except subprocess.TimeoutExpired:
            watcher.process.kill()
            watcher.wait(timeout=2.0)
    else:
        watcher.wait(timeout=2.0)
    watcher.requireBound()


def cursorFrom(text: str) -> str:
    """Read the live boundary acknowledged by a real watch request."""
    match = CURSOR_RE.search(text)
    if match is None:
        raise Failed(f"watch acknowledgement has no cursor: {text!r}")
    return match.group(1)


def hardStop(daemon: subprocess.Popen[str]) -> None:
    """Kill the daemon without cleanup and require its process to exit."""
    daemon.kill()
    daemon.wait(timeout=5.0)


def processTable() -> dict[int, ProcessRecord]:
    """Read PID, parent, kernel start identity, and executable without command arguments."""
    if sys.platform == "win32":
        script = (
            "Get-CimInstance Win32_Process | ForEach-Object { "
            "[PSCustomObject]@{ pid=$_.ProcessId; parent=$_.ParentProcessId; "
            "started=$_.CreationDate.ToUniversalTime().Ticks; executable=$_.ExecutablePath } } | "
            "ConvertTo-Json -Compress"
        )
        listed = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
            capture_output=True,
            text=True,
            timeout=15.0,
            check=False,
        )
        if listed.returncode != 0 or not listed.stdout.strip():
            raise Failed("the gate could not inspect Windows process identities")
        decoded = json.loads(listed.stdout)
        rows = decoded if isinstance(decoded, list) else [decoded]
        return {
            int(row["pid"]): ProcessRecord(
                parent=int(row["parent"]),
                started=str(row["started"]),
                executable=str(row.get("executable") or ""),
            )
            for row in rows
        }
    if sys.platform.startswith("linux"):
        table: dict[int, ProcessRecord] = {}
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                stat = (entry / "stat").read_text(encoding="ascii")
                closing = stat.rfind(")")
                fields = stat[closing + 2 :].split()
                executable = str((entry / "exe").resolve())
                table[int(entry.name)] = ProcessRecord(
                    parent=int(fields[1]),
                    started=fields[19],
                    executable=executable,
                )
            except (FileNotFoundError, PermissionError, OSError, ValueError, IndexError):
                # ok: process-table rows may disappear during inspection; exact identities are rechecked before action.
                continue
        return table
    listed = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,lstart=,comm="],
        capture_output=True,
        text=True,
        timeout=15.0,
        check=False,
    )
    if listed.returncode != 0:
        raise Failed("the gate could not inspect Unix process identities")
    table = {}
    for line in listed.stdout.splitlines():
        fields = line.split(maxsplit=7)
        if len(fields) != 8:
            continue
        try:
            table[int(fields[0])] = ProcessRecord(
                parent=int(fields[1]),
                started=" ".join(fields[2:7]),
                executable=fields[7],
            )
        except ValueError:
            # ok: one malformed process-table row cannot identify a target and is therefore excluded.
            continue
    return table


def descendantIdentities(parent: int) -> dict[int, ProcessRecord]:
    """Capture every current descendant of one exact gate-owned daemon."""
    table = processTable()
    found: dict[int, ProcessRecord] = {}
    frontier = {parent}
    while frontier:
        children = {
            pid: record
            for pid, record in table.items()
            if record.parent in frontier and pid not in found
        }
        found.update(children)
        frontier = set(children)
    return found


def emergencyCleanup(owned: dict[int, ProcessRecord]) -> None:
    """Kill only still-matching gate-owned identities and require their absence."""
    if not owned:
        return
    current = processTable()
    targets = {pid for pid, identity in owned.items() if current.get(pid) == identity}
    termination = signal.SIGTERM if sys.platform == "win32" else signal.SIGKILL
    for pid in sorted(targets, reverse=True):
        try:
            os.kill(pid, termination)
        except ProcessLookupError:
            # ok: an already absent exact PID is the requested cleanup outcome and survivor verification follows.
            pass
    deadline = time.monotonic() + 5.0
    survivors = targets
    while survivors and time.monotonic() < deadline:
        current = processTable()
        survivors = {pid for pid in targets if current.get(pid) == owned[pid]}
        if survivors:
            time.sleep(0.05)
    if survivors:
        raise Failed(f"gate-owned provider processes survived cleanup: {sorted(survivors)}")


def exercise() -> None:
    """Drive bounded replay, hard restart, explicit stream gap, and native continuation."""
    binary, fixture = acp.build()
    with tempfile.TemporaryDirectory(prefix="runtrol-resilience-") as raw_root:
        root = Path(raw_root)
        home = root / "home"
        workspace = root / "workspace"
        marker = root / "provider-state.json"
        workspace.mkdir()
        manifest(home, fixture, marker)
        environment = acp.environment(home, fixture)
        first = acp.startDaemon(binary, environment, home)
        second: subprocess.Popen[str] | None = None
        watcher: BoundedWatcher | None = None
        owned_processes: dict[int, ProcessRecord] = {}
        try:
            session = acp.command(binary, environment, ["start", acp.PROVIDER, str(workspace)])
            if acp.SESSION_RE.fullmatch(session) is None:
                raise Failed(f"start returned no session identifier: {session!r}")
            completeTurn(binary, environment, session, 1)
            native_before = nativeFor(acp.command(binary, environment, ["list"]), session)

            cursor_path = root / "cursor-watch.out"
            watcher = startWatcher(binary, environment, session, cursor_path)
            acknowledged = waitFor(
                cursor_path,
                lambda text: CURSOR_RE.search(text) is not None,
                "a cursor",
                watcher,
            )
            requested_cursor = cursorFrom(acknowledged)
            stopWatcher(watcher)
            watcher = None

            live_path = root / "live-watch.out"
            watcher = startWatcher(binary, environment, session, live_path, requested_cursor)
            waitFor(
                live_path,
                lambda text: (match := CURSOR_RE.search(text)) is not None
                and match.group(1) == requested_cursor,
                "the exact live reference boundary",
                watcher,
            )

            completeTurn(binary, environment, session, 2)
            completeTurn(binary, environment, session, 3)
            waitFor(
                live_path,
                lambda text: "fixture reply 2" in text
                and "fixture reply 3" in text
                and len(EVENT_CURSOR_RE.findall(text)) >= 6,
                "both live turns",
                watcher,
            )
            stopWatcher(watcher)
            live_text = readOutput(live_path, watcher)
            watcher = None

            replay_path = root / "replay-watch.out"
            watcher = startWatcher(binary, environment, session, replay_path, requested_cursor)
            waitFor(
                replay_path,
                lambda text: "fixture reply 2" in text
                and "fixture reply 3" in text
                and len(EVENT_CURSOR_RE.findall(text)) >= 6,
                "both retained turns",
                watcher,
            )

            first_descendants = descendantIdentities(first.pid)
            if not first_descendants:
                raise Failed("the hard-stop journey observed no provider descendant")
            owned_processes.update(first_descendants)
            hardStop(first)
            watcher.wait(timeout=5.0)
            watcher.requireBound()
            replay_text = readOutput(replay_path, watcher)
            replay_live_cursor = cursorFrom(replay_text)
            watcher = None

            second = acp.startDaemon(binary, environment, home)
            time.sleep(0.5)
            listing = acp.command(binary, environment, ["list"])
            native_after_crash = nativeFor(listing, session)
            if native_after_crash != native_before:
                raise Failed("stored session lost its provider-native identity after restart")

            resumed = acp.command(
                binary,
                environment,
                ["resume", acp.PROVIDER, native_before, str(workspace)],
            )
            if acp.SESSION_RE.fullmatch(resumed) is None or resumed == session:
                raise Failed(f"resume returned no fresh session identifier: {resumed!r}")
            native_after = nativeFor(acp.command(binary, environment, ["list"]), resumed)

            restart_path = root / "restart-watch.out"
            watcher = startWatcher(binary, environment, resumed, restart_path, requested_cursor)
            restart_header = waitFor(
                restart_path,
                lambda text: GAP_RE.search(text) is not None,
                "an explicit restart gap",
                watcher,
            )
            resumed_cursor = cursorFrom(restart_header)
            gap = GAP_RE.search(restart_header)
            if gap is None:
                raise Failed("restart acknowledgement has no gap")
            completeTurn(binary, environment, resumed, 4)
            waitFor(
                restart_path,
                lambda text: "fixture reply 4" in text
                and '"step":"ended"' in text
                and len(EVENT_CURSOR_RE.findall(text)) >= 3,
                "the resumed turn and completion",
                watcher,
            )
            stopWatcher(watcher)
            restart_text = readOutput(restart_path, watcher)
            watcher = None

            verifyEvidence(
                Evidence(
                    live_text=live_text,
                    replay_text=replay_text,
                    requested_cursor=requested_cursor,
                    replay_live_cursor=replay_live_cursor,
                    gap_requested=gap.group(1),
                    resumed_cursor=resumed_cursor,
                    gap_live=gap.group(2),
                    restart_text=restart_text,
                    native_before=native_before,
                    native_after=native_after,
                )
            )
            acp.command(binary, environment, ["close", resumed, "--now"])
            print(
                "[resilienceFaultInjection] OK. local replay was exact; hard restart produced an explicit "
                "stream gap and provider-native continuation."
            )
        finally:
            cleanup_errors: list[str] = []
            if watcher is not None:
                try:
                    stopWatcher(watcher)
                except (Failed, OSError, subprocess.SubprocessError) as error:
                    cleanup_errors.append(str(error))
            if first.poll() is None:
                try:
                    acp.stopDaemon(first)
                except (OSError, subprocess.SubprocessError) as error:
                    cleanup_errors.append(str(error))
            if second is not None:
                try:
                    if second.poll() is None:
                        owned_processes.update(descendantIdentities(second.pid))
                except (Failed, OSError, ValueError, subprocess.SubprocessError) as error:
                    cleanup_errors.append(str(error))
                finally:
                    try:
                        acp.stopDaemon(second)
                    except (OSError, subprocess.SubprocessError) as error:
                        cleanup_errors.append(str(error))
            try:
                emergencyCleanup(owned_processes)
            except (Failed, OSError, ValueError, subprocess.SubprocessError) as error:
                cleanup_errors.append(str(error))
            if cleanup_errors:
                raise Failed("; ".join(cleanup_errors))


def main(argv: list[str]) -> int:
    """Run the selftest or the real local resilience journey."""
    if "--selftest" in argv:
        return selftest()
    try:
        exercise()
        return 0
    except (Failed, acp.Failed, OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"[resilienceFaultInjection] FAIL. {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
