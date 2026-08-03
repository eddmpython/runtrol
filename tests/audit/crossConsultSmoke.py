"""Gate: the consult toggle wires two real CLIs through their own commands, and undoes itself exactly.

This is the north star axis `crossConsult` as something a machine checks. The product claim is one toggle:
flip it on and one CLI can ask the other's opinion mid-turn, flip it off and both configurations are exactly
what they were. Everything here drives the real `runtrol consult` surface against the real installed CLIs,
in isolated provider homes, and judges the outcome with the CLIs' own answers.

What is asserted, and why each line matters
-------------------------------------------

- The supported direction reports honestly, wires, reads back as wired from the registering CLI's own `get`,
  and unwires back to exactly the configuration that was there before.
- The reverse direction reports **unsupported with a measured reason** rather than wiring something that
  fails mid-turn: the served CLI's own MCP server offers no consultation tool today.
- The registration the toggle writes is an executable name and its serve words, nothing else. A conversation
  or a path in that entry would be runtrol putting its own content into another program's configuration.
- Flipping to a state a direction is already in succeeds, so the toggle is never order-sensitive.

Why it costs nothing to run
---------------------------

Registration, verification, and removal are configuration work. The one server that is started answers
`tools/list` and exits; no model turn ever begins, so no tokens and no rate limit are spent.

The real mid-turn reception (a prompt through the wired registration answering back) was measured by hand on
2026-08-03 and recorded in the initiative's ledger; it costs a real turn on both CLIs, so this gate stops
where a turn would begin, and says so.

Why it cannot run on hosted CI
------------------------------

It drives the real installed CLIs, which authenticate as a person. Declared local-only in `gateCoverage.py`.

Usage::

    python -X utf8 tests/audit/crossConsultSmoke.py
    python -X utf8 tests/audit/crossConsultSmoke.py --selftest

Exit codes:
    0 the toggle held on this machine, or the CLIs are absent and this said so
    2 the toggle did not hold
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# The one name the product registers a counterpart under, and the direction that is wireable today.
# Must match `runtrol_daemon::consult::consult_name` and the driver declarations in `bound.rs`; the wire
# journey below fails loudly if either drifts.
CONSULT_NAME = "codexConsult"
FROM_PROVIDER = "claude"
TO_PROVIDER = "codex"

# What the wired registration may contain: the counterpart's executable name and its serve words. Anything
# beyond these keys would be runtrol writing its own content into another program's configuration.
ENTRY_KEYS_ALLOWED = {"type", "command", "args", "env"}
ENTRY_COMMAND = "codex"
ENTRY_ARGS = ["mcp-server"]

# One consult line: `consult  <from>  <to>  <state>` with an optional parenthesised reason after it.
CONSULT_LINE = re.compile(r"^consult\s+(\S+)\s+(\S+)\s+(wired|unwired|unsupported)(?:\s+\((.*)\))?$")

COMMAND_TIMEOUT_S = 120.0


class Failed(Exception):
    """The toggle did not hold. The message is what an operator reads."""


def parseConsult(text: str) -> dict[tuple[str, str], tuple[str, str | None]]:
    """Read a consult status into directions.

    Refuses a line it cannot read rather than skipping it: a skipped line would report a missing direction
    as "unwired and fine", which is the judgement this gate exists to make.
    """
    directions: dict[tuple[str, str], tuple[str, str | None]] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        matched = CONSULT_LINE.match(line)
        if matched is None:
            raise Failed(f"a consult line does not read as one: {line!r}")
        source, target, state, why = matched.groups()
        directions[(source, target)] = (state, why)
    if not directions:
        raise Failed("the consult answer named no direction at all")
    return directions


def assertSupported(directions: dict[tuple[str, str], tuple[str, str | None]], state: str) -> None:
    """Require the wireable direction to be present and in `state`."""
    found = directions.get((FROM_PROVIDER, TO_PROVIDER))
    if found is None:
        raise Failed(f"the {FROM_PROVIDER} -> {TO_PROVIDER} direction is not in the answer: {directions}")
    if found[0] != state:
        raise Failed(f"{FROM_PROVIDER} -> {TO_PROVIDER} is {found[0]} and this step needs {state}")


def assertReverseUnsupported(directions: dict[tuple[str, str], tuple[str, str | None]]) -> None:
    """Require the reverse direction to refuse honestly, with a sentence saying what was measured."""
    found = directions.get((TO_PROVIDER, FROM_PROVIDER))
    if found is None:
        raise Failed(f"the {TO_PROVIDER} -> {FROM_PROVIDER} direction is not in the answer: {directions}")
    state, why = found
    if state != "unsupported":
        raise Failed(
            f"{TO_PROVIDER} -> {FROM_PROVIDER} reports {state}. the served CLI offers no consult tool, so "
            f"anything but an honest unsupported would wire a direction that fails mid-turn"
        )
    if not why or len(why) < 20:
        raise Failed(f"the unsupported direction carries no measured reason: {why!r}")


def assertEntry(entry: object) -> None:
    """Require the written registration to be the counterpart's command and nothing of runtrol's own."""
    if not isinstance(entry, dict):
        raise Failed(f"the registration is not an object: {entry!r}")
    extras = set(entry) - ENTRY_KEYS_ALLOWED
    if extras:
        raise Failed(
            f"the registration carries keys beyond a server command: {sorted(extras)}. runtrol must not "
            f"put content of its own into another program's configuration"
        )
    if entry.get("command") != ENTRY_COMMAND or entry.get("args") != ENTRY_ARGS:
        raise Failed(
            f"the registration is not the counterpart's own serve command: "
            f"command={entry.get('command')!r} args={entry.get('args')!r}"
        )


def registrationIn(claudeHome: Path) -> object | None:
    """The registration entry in the isolated claude configuration, or None when it is absent."""
    config = claudeHome / ".claude.json"
    if not config.is_file():
        return None
    data = json.loads(config.read_text(encoding="utf-8"))
    servers = data.get("mcpServers")
    if not isinstance(servers, dict):
        return None
    return servers.get(CONSULT_NAME)


def buildBinary() -> Path:
    """Build the executable this gate drives, and answer where it is."""
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


def run(environment: dict[str, str], words: list[str]) -> str:
    """Run one command in this gate's own environment, and answer what it printed.

    # Errors

    [`Failed`] when the command fails, times out, or answers with a refusal.
    """
    try:
        proc = subprocess.run(
            words,
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
        raise Failed(f"`{' '.join(words)}` did not answer in {COMMAND_TIMEOUT_S:.0f} s") from expired
    said = (proc.stdout or "").strip() or (proc.stderr or "").strip()
    if proc.returncode != 0:
        raise Failed(f"`{' '.join(words)}` failed: {said}")
    return said


def cliSays(environment: dict[str, str], words: list[str]) -> bool:
    """Whether a CLI's own command succeeds, for judging state with the CLI's answer rather than runtrol's."""
    try:
        proc = subprocess.run(
            words,
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
        raise Failed(f"`{' '.join(words)}` did not answer in {COMMAND_TIMEOUT_S:.0f} s") from expired
    return proc.returncode == 0


def exercise(binary: Path, environment: dict[str, str], claudeHome: Path) -> None:
    """The whole toggle journey, judged at every step by the CLIs' own answers."""
    runtrol = [str(binary)]
    # Resolved rather than bare: on Windows the CLI is a launcher script, and CreateProcess only runs a file
    # it is given a path to.
    claude = shutil.which(FROM_PROVIDER)
    if claude is None:
        raise Failed(f"{FROM_PROVIDER} resolved when this gate began and does not any more")

    directions = parseConsult(run(environment, [*runtrol, "consult"]))
    assertSupported(directions, "unwired")
    assertReverseUnsupported(directions)
    print("  status: the wireable direction is unwired and the reverse refuses with its measured reason")

    directions = parseConsult(run(environment, [*runtrol, "consult", "wire", FROM_PROVIDER, TO_PROVIDER]))
    assertSupported(directions, "wired")
    if not cliSays(environment, [claude, "mcp", "get", CONSULT_NAME]):
        raise Failed(f"runtrol says wired and `claude mcp get {CONSULT_NAME}` says it is not there")
    assertEntry(registrationIn(claudeHome))
    print("  wire: registered, confirmed by the registering CLI itself, and the entry is only a command")

    directions = parseConsult(run(environment, [*runtrol, "consult", "wire", FROM_PROVIDER, TO_PROVIDER]))
    assertSupported(directions, "wired")
    print("  wire again: flipping to the state it is already in succeeds")

    directions = parseConsult(run(environment, [*runtrol, "consult", "unwire", FROM_PROVIDER, TO_PROVIDER]))
    assertSupported(directions, "unwired")
    if cliSays(environment, [claude, "mcp", "get", CONSULT_NAME]):
        raise Failed(f"runtrol says unwired and `claude mcp get {CONSULT_NAME}` still finds it")
    leftover = registrationIn(claudeHome)
    if leftover is not None:
        raise Failed(f"the configuration still carries the registration after unwire: {leftover!r}")
    print("  unwire: removed, confirmed by the registering CLI, configuration restored")

    directions = parseConsult(run(environment, [*runtrol, "consult", "unwire", FROM_PROVIDER, TO_PROVIDER]))
    assertSupported(directions, "unwired")
    print("  unwire again: also not order-sensitive")


def main(argv: list[str]) -> int:
    """Drive the toggle against the real CLIs, or say why nothing was driven."""
    if "--selftest" in argv:
        return selftest()

    if shutil.which("cargo") is None:
        print("[crossConsultSmoke] SKIP: cargo is not here, so there is nothing to build or drive.")
        return 0
    missing = [name for name in (FROM_PROVIDER, TO_PROVIDER) if shutil.which(name) is None]
    if missing:
        # Loud rather than green: a machine without the CLIs proves nothing about wiring them.
        print(
            f"[crossConsultSmoke] SKIP: {', '.join(missing)} is not installed on this machine. "
            f"this axis is unverified here."
        )
        return 0

    binary = buildBinary()
    home = Path(tempfile.mkdtemp(prefix="runtrolConsultHome"))
    claudeHome = Path(tempfile.mkdtemp(prefix="runtrolConsultClaude"))
    codexHome = Path(tempfile.mkdtemp(prefix="runtrolConsultCodex"))
    environment = dict(os.environ)
    # The gate's own daemon and both CLIs' own configuration, all isolated: the daemon inherits these, so
    # every registration this gate makes lands in homes it deletes afterwards. The operator's configuration
    # is never touched.
    environment["RUNTROL_HOME"] = str(home)
    environment["CLAUDE_CONFIG_DIR"] = str(claudeHome)
    environment["CODEX_HOME"] = str(codexHome)
    print(f"[crossConsultSmoke] driving the toggle under {home}")

    try:
        exercise(binary, environment, claudeHome)
    except Failed as failure:
        print(f"[crossConsultSmoke] the toggle did not hold: {failure}", file=sys.stderr)
        crashLog = home / "daemon-crash.log"
        if crashLog.is_file():
            words = crashLog.read_text(encoding="utf-8", errors="replace")[-4096:]
            print(f"[crossConsultSmoke] the daemon's crash file said:\n{words}", file=sys.stderr)
        return 2
    finally:
        try:
            run(environment, [str(binary), "panic"])
        except Failed as stopping:
            print(f"[crossConsultSmoke] could not stop its own daemon: {stopping}", file=sys.stderr)
        for scratch in (home, claudeHome, codexHome):
            shutil.rmtree(scratch, ignore_errors=True)

    print(
        "[crossConsultSmoke] OK. the toggle wired, verified, and restored through the CLIs' own commands. "
        "mid-turn reception costs a real turn and is measured by hand, not here."
    )
    return 0


def selftest() -> int:
    """Prove every judgement can fail before trusting it when it passes."""
    problems: list[str] = []

    def refused(what: str, work) -> None:
        try:
            work()
        except Failed:
            return
        problems.append(f"{what} was accepted")

    refused("a consult answer with no direction", lambda: parseConsult(""))
    refused("a line that is not a consult line", lambda: parseConsult("no provider called claude"))
    refused(
        "a missing wireable direction",
        lambda: assertSupported(parseConsult("consult  codex  claude  unsupported  (why)"), "unwired"),
    )
    refused(
        "a wired direction where unwired was required",
        lambda: assertSupported(parseConsult("consult  claude  codex  wired"), "unwired"),
    )
    refused(
        "a reverse direction that claims to be wireable",
        lambda: assertReverseUnsupported(parseConsult("consult  codex  claude  unwired")),
    )
    refused(
        "an unsupported reverse with no measured reason",
        lambda: assertReverseUnsupported(parseConsult("consult  codex  claude  unsupported")),
    )
    refused(
        "a registration carrying content beyond a command",
        lambda: assertEntry(
            {"type": "stdio", "command": "codex", "args": ["mcp-server"], "history": ["hello"]}
        ),
    )
    refused(
        "a registration naming a different executable",
        lambda: assertEntry({"type": "stdio", "command": "elsewhere", "args": ["mcp-server"]}),
    )
    refused(
        "a registration with the wrong serve words",
        lambda: assertEntry({"type": "stdio", "command": "codex", "args": ["serve", "--all"]}),
    )

    good = parseConsult(
        "consult  claude  codex  unwired\n"
        "consult  codex  claude  unsupported  (measured: the served CLI offers no consult tool)"
    )
    try:
        assertSupported(good, "unwired")
        assertReverseUnsupported(good)
        assertEntry({"type": "stdio", "command": "codex", "args": ["mcp-server"], "env": {}})
    except Failed as wrong:
        problems.append(f"the honest surface was refused: {wrong}")

    if problems:
        print("[crossConsultSmoke --selftest] the gate cannot catch what it claims to.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[crossConsultSmoke --selftest] OK. 9 injected defects all caught.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
