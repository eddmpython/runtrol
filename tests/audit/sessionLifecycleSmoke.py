"""Gate: sessions start, survive a daemon restart, and are picked back up. Against the real CLIs.

This is the north star axis `oneSessionList` as something a machine checks. Everything else about that axis
is a claim until this runs: that a session appears where the operator looks, that a daemon restart leaves its
minimal pointer intact, that the listing carries what a resume needs, and that resuming reaches the same
conversation rather than a new one.

Why it costs nothing to run
---------------------------

No prompt is ever sent. Starting a conversation, listing it, letting go of it and picking it back up are all
protocol work, so this gate spends no tokens and no rate limit. That is what makes it cheap enough to run on
every preflight rather than nightly, and it is why the assertions below stop where a turn would begin.

Why it cannot run on hosted CI
------------------------------

Both CLIs authenticate as a person, with a subscription login that cannot be carried in a secret, and the
whole point is to drive the real binaries. So this runs where those logins live: the operator's own machine.
It is declared local-only in `gateCoverage.py` for exactly that reason.

Why it cannot disturb the operator
----------------------------------

Everything happens under a temporary `RUNTROL_HOME`, so the daemon this gate starts, the sessions it creates
and the store it writes are its own. The operator's daemon is never contacted and never stopped.

Usage::

    python -X utf8 tests/audit/sessionLifecycleSmoke.py
    python -X utf8 tests/audit/sessionLifecycleSmoke.py --selftest

Exit codes:
    0 every installed provider completed the lifecycle, or none is installed and this said so
    2 a provider is installed and the lifecycle did not hold
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANIFESTS = ROOT / "crates" / "runtrol-drivers" / "manifests"

# Long enough for a cold start on a loaded machine. Measured on this one: the slowest of these commands starts
# a conversation on the daemon-backed CLI in about six seconds. This is an order of magnitude above that,
# because its job is to turn a hang into a named failure rather than to police latency (that belongs to the
# bench gate on the `instantResponse` axis).
COMMAND_TIMEOUT_S = 120.0

# What a listing prints where a conversation's own name would go, before the provider has said one. Must match
# `runtrol_cli::lines::NOT_NAMED_YET`; a listing that prints this for a started session is the defect this gate
# exists to catch, so it is named rather than written as a bare string in an assertion.
NOT_NAMED_YET = "-"

# A session line is `<session>  <provider>  <tier>  <doing>  <native>` and may carry a stuck marker after it.
FIELDS_PER_LINE = 5

# How long a started session is watched for a conversation name before concluding it has none yet.
#
# The two supported CLIs differ here, by design rather than by accident, and this gate discovers which is
# which rather than being told:
#
#   - One creates the conversation in the answer that starts it, so it is named before `start` returns.
#   - The other has no conversation until its first turn. It announces a name on its stream when one begins,
#     and until then there is genuinely nothing in its own store to resume. A listing that printed a name
#     there would be inviting a resume that fails, which is why an unnamed session prints a placeholder.
#
# Measured on this machine: the first is named at zero seconds, and the second is still unnamed after twenty.
# Five seconds sits between those with room on both sides, and nothing waits on it in the ordinary case.
NAMING_GRACE_S = 5.0

# How often to look while waiting. Short enough that a session already named costs no wait at all.
LOOK_EVERY_S = 0.25


class Failed(Exception):
    """The lifecycle did not hold. The message is what an operator reads."""


@dataclass(frozen=True)
class Listed:
    """One line of a listing, as this gate reads it."""

    session: str
    provider: str
    tier: str
    doing: str
    native: str


def parseListing(text: str) -> list[Listed]:
    """Read a listing into rows.

    Refuses a line it cannot read rather than skipping it. A parser that skipped would report an empty listing
    as a passing "the session is gone", which is the assertion this gate most depends on.
    """
    rows: list[Listed] = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line == "no sessions":
            continue
        fields = line.split()
        if len(fields) < FIELDS_PER_LINE:
            raise Failed(
                f"a listing line has {len(fields)} fields and the surface prints {FIELDS_PER_LINE}: {line!r}"
            )
        rows.append(
            Listed(
                session=fields[0],
                provider=fields[1],
                tier=fields[2],
                doing=fields[3],
                native=fields[4],
            )
        )
    return rows


def rowFor(rows: list[Listed], session: str) -> Listed | None:
    """The row for one session, or None when the listing does not have it."""
    for row in rows:
        if row.session == session:
            return row
    return None


def shippedProviders() -> dict[str, list[str]]:
    """Provider id to the executable names its manifest says to look for.

    Read from the manifests this build compiles in, so a provider added tomorrow is exercised by this gate
    without anybody remembering to add it here.
    """
    shipped: dict[str, list[str]] = {}
    for path in sorted(MANIFESTS.glob("*.toml")):
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
        identifier = manifest.get("id")
        names = (manifest.get("bin") or {}).get("names") or []
        if identifier and names:
            shipped[identifier] = list(names)
    return shipped


def installed(names: list[str]) -> bool:
    """Whether any of a provider's executable names resolves on this machine."""
    return any(shutil.which(name) is not None for name in names)


def buildBinary() -> Path:
    """Build the executable this gate drives, and answer where it is.

    Built here rather than assumed, so the gate exercises the tree as it stands rather than whatever was left
    in `target/` by an earlier run.
    """
    subprocess.run(
        ["cargo", "build", "-p", "runtrol"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    name = "runtrol.exe" if sys.platform == "win32" else "runtrol"
    binary = ROOT / "target" / "debug" / name
    if not binary.is_file():
        raise Failed(f"cargo built without error and {binary.relative_to(ROOT)} is not there")
    return binary


def run(binary: Path, home: Path, words: list[str]) -> str:
    """Run one command against this gate's own home, and answer what it printed.

    # Errors

    [`Failed`] when the command fails, times out, or says something the surface reports as a failure.
    """
    ok, said = attempt(binary, home, words)
    if not ok:
        raise Failed(f"`runtrol {' '.join(words)}` failed: {said}")
    return said


def attempt(binary: Path, home: Path, words: list[str]) -> tuple[bool, str]:
    """Run one command and answer whether it worked, along with what it said.

    For the places where a refusal is a legitimate outcome to be checked rather than a failure to report.

    # Errors

    [`Failed`] only when the command does not answer at all, which is never an outcome.
    """
    environment = dict(os.environ)
    environment["RUNTROL_HOME"] = str(home)
    try:
        proc = subprocess.run(
            [str(binary), *words],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=COMMAND_TIMEOUT_S,
            check=False,
        )
    except subprocess.TimeoutExpired as expired:
        raise Failed(
            f"`runtrol {' '.join(words)}` did not answer in {COMMAND_TIMEOUT_S:.0f} s"
        ) from expired

    said = (proc.stdout or "").strip() or (proc.stderr or "").strip()
    return proc.returncode == 0, said


def nameOf(binary: Path, home: Path, session: str, provider: str) -> str | None:
    """The conversation name a session is listed with, or None when this provider has not named one yet.

    Discovered rather than assumed. Which providers name a conversation at start is a fact about them, and a
    gate that hard-coded the answer would be a gate that names a CLI, which is the thing the whole design
    keeps out of everything except a driver.

    # Errors

    [`Failed`] when the session leaves the listing while it is being looked at, which means it stopped rather
    than that it has no name.
    """
    deadline = time.monotonic() + NAMING_GRACE_S
    while True:
        row = rowFor(parseListing(run(binary, home, ["list"])), session)
        if row is None:
            raise Failed(f"{provider} session {session} left the listing while it was being read")
        if row.native != NOT_NAMED_YET:
            return row.native
        if time.monotonic() >= deadline:
            return None
        time.sleep(LOOK_EVERY_S)


def isSessionId(text: str) -> bool:
    """Whether something reads as the identifier runtrol mints."""
    return re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", text) is not None


def assertRestarted(
    started: dict[str, str], names: dict[str, str | None], restarted: list[Listed]
) -> None:
    """Require every storable pointer, and only a storable pointer, to survive a daemon restart."""
    for provider, session in started.items():
        native = names[provider]
        row = rowFor(restarted, session)
        if native is None:
            if row is not None:
                raise Failed(
                    f"{provider} had no provider conversation name before restart and came back as resumable"
                )
            continue
        if row is None:
            raise Failed(f"{provider} session {session} disappeared across the daemon restart")
        if row.native != native:
            raise Failed(
                f"{provider} session {session} was {native} before restart and {row.native} after it"
            )
        if row.tier != "idle" or row.doing != "detached":
            raise Failed(
                f"{provider} session {session} came back as {row.tier}/{row.doing}, not idle/detached"
            )


def exercise(binary: Path, home: Path, workspace: Path, providers: list[str]) -> None:
    """Drive the whole lifecycle for every provider, and hold the surface to it.

    # Errors

    [`Failed`] at the first thing that does not hold, naming what was expected and what was there.
    """
    started: dict[str, str] = {}
    workspaces: dict[str, Path] = {}
    for provider in providers:
        provider_workspace = workspace / provider
        provider_workspace.mkdir()
        workspaces[provider] = provider_workspace
        session = run(binary, home, ["start", provider, str(provider_workspace)])
        if not isSessionId(session):
            raise Failed(f"starting {provider} answered {session!r}, which is not a session identifier")
        started[provider] = session
        print(f"  {provider}: started {session}")

    # The axis, in one assertion: every provider's session, in one listing, from one command.
    rows = parseListing(run(binary, home, ["list"]))
    for provider, session in started.items():
        row = rowFor(rows, session)
        if row is None:
            raise Failed(f"{provider} started {session} and the listing does not have it")
        if row.provider != provider:
            raise Failed(f"{session} was started on {provider} and the listing says {row.provider}")
    print(f"  one listing holds all {len(started)} of them")

    names: dict[str, str | None] = {}
    for provider, session in started.items():
        names[provider] = nameOf(binary, home, session, provider)

    # Stop the daemon and every child it contains, then let `list` start a fresh daemon over the same home. This is
    # intentionally between discovery and resume: a memory-only list would now be empty, while a transcript copy
    # would violate the product's thinness. The only acceptable survivor is the provider/native/workspace pointer.
    run(binary, home, ["panic"])
    restarted = parseListing(run(binary, home, ["list"]))
    assertRestarted(started, names, restarted)
    print("  named session pointers survived a full daemon restart and came back detached")

    unreached: list[str] = []
    for provider, session in started.items():
        native = names[provider]

        if native is None:
            # This provider has no conversation until its first turn, so there is nothing to pick back up and
            # the listing said so. Recorded rather than passed over: the tempting "fix" is to print the
            # identifier runtrol issued, which would invite a resume of something that does not exist.
            unreached.append(provider)
            print(f"  {provider}: started and listed. names a conversation only once one exists")
            continue

        ok, said = attempt(binary, home, ["resume", provider, native, str(workspaces[provider])])

        if not ok:
            # Measured, and it is the same truth the other provider states by staying unnamed: a conversation
            # that never had a turn has nothing on disk to continue. What this gate holds is that runtrol says
            # so, naming the provider and carrying its words, instead of quietly starting a fresh conversation
            # and reporting it as the one that was asked for. That silent substitution is the failure a
            # wrapper actually ships.
            if isSessionId(said):
                raise Failed(f"{provider} refused to resume {native} and answered with a session identifier")
            if provider not in said:
                raise Failed(
                    f"{provider} refused to resume {native} and the refusal does not name the provider: "
                    f"{said!r}"
                )
            unreached.append(provider)
            run(binary, home, ["close", session])
            print(
                f"  {provider}: persisted across restart, then refused to continue a conversation with no turns"
            )
            continue

        resumed = said
        if not isSessionId(resumed):
            raise Failed(f"resuming {provider} answered {resumed!r}, which is not a session identifier")

        back = rowFor(parseListing(run(binary, home, ["list"])), resumed)
        if back is None:
            raise Failed(f"{provider} was resumed as {resumed} and the listing does not have it")
        # The assertion that makes this a resume rather than a fresh start. Without it the gate would pass on a
        # provider that quietly began a new conversation every time somebody asked to continue one.
        if back.native != native:
            raise Failed(
                f"{provider} was asked to continue {native} and came back naming {back.native}, which is a "
                f"different conversation"
            )
        print(f"  {provider}: survived restart, resumed as {resumed}, still {native}")

        run(binary, home, ["close", resumed])

    if unreached:
        # Said out loud every run. A resume that succeeds needs a conversation with a turn in it, and a turn
        # costs money and rate limit on both providers, so this gate stops short of one on purpose. Staying
        # quiet about that would be reporting coverage this does not have.
        print(
            f"  a successful resume is unverified for: {', '.join(unreached)}. it needs a conversation with a "
            f"turn in it, and this gate never spends one"
        )


def main(argv: list[str]) -> int:
    """Drive the lifecycle against every installed provider, or say why nothing was driven."""
    if "--selftest" in argv:
        return selftest()

    if shutil.which("cargo") is None:
        print("[sessionLifecycleSmoke] SKIP: cargo is not here, so there is nothing to build or drive.")
        return 0

    shipped = shippedProviders()
    present = [provider for provider, names in shipped.items() if installed(names)]
    absent = [provider for provider in shipped if provider not in present]

    if not present:
        # Loud rather than green. A machine without the CLIs proves nothing about them, and a gate that said
        # OK here would be reporting coverage it does not have.
        print(
            f"[sessionLifecycleSmoke] SKIP: none of {', '.join(shipped)} is installed on this machine. "
            f"this axis is unverified here."
        )
        return 0

    binary = buildBinary()
    home = Path(tempfile.mkdtemp(prefix="runtrolGateHome"))
    workspace = Path(tempfile.mkdtemp(prefix="runtrolGateWork"))
    print(f"[sessionLifecycleSmoke] driving {', '.join(present)} under {home}")

    try:
        exercise(binary, home, workspace, present)
    except Failed as failure:
        print(f"[sessionLifecycleSmoke] the lifecycle did not hold: {failure}", file=sys.stderr)
        crash_log = home / "daemon-crash.log"
        if crash_log.is_file():
            # The daemon's own last words, read before the finally below deletes the home. Without
            # this, a daemon that died mid-journey leaves only "stopped without answering".
            words = crash_log.read_text(encoding="utf-8", errors="replace")[-4096:]
            print(f"[sessionLifecycleSmoke] the daemon's crash file said:\n{words}", file=sys.stderr)
        return 2
    finally:
        # This gate's own daemon, and only ever this one: it is the daemon serving the temporary home above.
        try:
            run(binary, home, ["panic"])
        except Failed as stopping:
            # Reported rather than swallowed. A gate that left a daemon running would leave the next run to
            # inherit its sessions, and the operator to wonder what started it.
            print(f"[sessionLifecycleSmoke] could not stop its own daemon: {stopping}", file=sys.stderr)
        shutil.rmtree(home, ignore_errors=True)
        shutil.rmtree(workspace, ignore_errors=True)

    if absent:
        print(f"  not exercised (not installed): {', '.join(absent)}")
    print(f"[sessionLifecycleSmoke] OK. {len(present)} provider(s) started and survived a daemon restart.")
    return 0


def selftest() -> int:
    """Prove the assertions can fail before trusting them when they pass.

    A smoke gate's reading of the surface is the part that can silently stop checking: a parser that skips a
    line it cannot read, or a comparison that treats a missing row as a pass, goes green forever. Each defect
    below is injected into the text the gate reads, and each has to be caught.
    """
    cases: list[tuple[str, str]] = [
        (
            "a listing line missing the conversation name",
            "019fb610-7a0f-7ea2-91d1-f459220692cb  codex  running  idle",
        ),
        (
            "a listing line with fewer fields than the surface prints",
            "019fb610-7a0f-7ea2-91d1-f459220692cb  codex",
        ),
    ]
    problems: list[str] = []
    for what, text in cases:
        refused = False
        try:
            parseListing(text)
        except Failed:
            # ok: being refused is the assertion. The parser is supposed to reject this text, so catching it
            # here is the passing outcome, and the check below is what reports the other one.
            refused = True
        if not refused:
            problems.append(f"{what} was accepted")

    # A session that did not disappear has to be seen as still there, and one that did as gone. Getting this
    # backwards is how a close that does nothing passes.
    rows = parseListing("019fb610-7a0f-7ea2-91d1-f459220692cb  codex  running  idle  thread_abc")
    if rowFor(rows, "019fb610-7a0f-7ea2-91d1-f459220692cb") is None:
        problems.append("a session that is listed was read as gone")
    if rowFor(rows, "019fb610-0000-0000-0000-000000000000") is not None:
        problems.append("a session that is not listed was read as present")

    # A conversation name that came back different is a fresh conversation wearing a resume's clothes.
    if rows[0].native != "thread_abc":
        problems.append("the conversation name was not read out of the line it is on")

    # Only what runtrol mints counts as a session identifier, or `start` answering with an error message would
    # read as success.
    if isSessionId("done") or not isSessionId("019fb610-7a0f-7ea2-91d1-f459220692cb"):
        problems.append("a session identifier is not told apart from any other word")

    session = "019fb610-7a0f-7ea2-91d1-f459220692cb"
    started = {"driver": session}
    restart_defects = [
        ("a named session disappeared across restart", {"driver": "thread_abc"}, []),
        (
            "a different native conversation came back",
            {"driver": "thread_abc"},
            parseListing(f"{session}  driver  idle  detached  thread_other"),
        ),
        (
            "a restarted session claimed to be running",
            {"driver": "thread_abc"},
            parseListing(f"{session}  driver  running  idle  thread_abc"),
        ),
        (
            "an unnamed session came back as resumable",
            {"driver": None},
            parseListing(f"{session}  driver  idle  detached  thread_invented"),
        ),
    ]
    for what, names, restarted in restart_defects:
        caught = False
        try:
            assertRestarted(started, names, restarted)
        except Failed:
            caught = True
        if not caught:
            problems.append(f"{what} was accepted")

    if problems:
        print("[sessionLifecycleSmoke --selftest] the gate cannot catch what it claims to.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print(
        f"[sessionLifecycleSmoke --selftest] OK. "
        f"{len(cases) + len(restart_defects) + 4} injected defects all caught."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
