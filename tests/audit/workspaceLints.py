"""Gate: every crate inherits the workspace lint table, or says at the line why it cannot.

The workspace lint table is where `unsafe_code = "forbid"` lives, along with the clippy levels the whole
repository is held to. A crate that writes its own table is a crate the table no longer covers, and the failure is
silent in the worst way: nothing breaks, nothing warns, and a rule everybody believes is universal quietly applies
to one crate fewer than they think.

Three crates here have a real reason to opt out, each found by trying to do it the other way:

- `runtrol-childproc` contains the audited process-control FFI needed for containment.
- `runtrol-vault` contains the audited Windows DPAPI FFI needed to protect the machine identity.
In both platform crates, `forbid` cannot be relaxed from inside a crate that inherits it, and cargo hard-errors
  on mixing inheritance with an override, so the table is written out with `deny` instead.
- `runtrol-audit` holds gate helpers as free functions, which clippy's `allow-*-in-tests` escape does not cover.

Neither of those is a licence to write a weaker table. What this gate holds is that an opted-out crate still
forbids or denies `unsafe_code`, still denies the clippy `all` group, and still says why it opted out. An
exemption that stops explaining itself is the same as no exemption.

Usage::

    python -X utf8 tests/audit/workspaceLints.py

Exit codes:
    0 every crate is covered, by inheritance or by a table that is no weaker
    2 a crate escaped the table
"""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CRATES = ROOT / "crates"
AUDIT = ROOT / "tests" / "audit"

# The one line that means "held to whatever the workspace says".
INHERITS = {"workspace": True}

# Levels that are at least as strong as forbidding. Ordered weakest first for the message.
STRONG_ENOUGH = ("deny", "forbid")

# What a crate that writes its own table still has to say, and why each one is not negotiable.
REQUIRED: dict[tuple[str, str], str] = {
    ("rust", "unsafe_code"): "the whole repository is unsafe-free except where a safety argument is written",
    ("clippy", "all"): "the clippy group everything else is held to",
}

# A crate that writes its own table explains itself in the comment block directly above it. Prose is the only
# place a reason can live in TOML, and directly above the table is the only place somebody editing it will read.
TABLE = "[lints"


def levelOf(value: object) -> str | None:
    """The level a lint entry sets, whichever of the two spellings TOML used."""
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        level = value.get("level")
        return level if isinstance(level, str) else None
    return None


def explainsItself(text: str) -> bool:
    """Whether a comment block sits directly above the first lint table.

    The block is what stops at code or at a blank line, so a comment about something else further up does not
    count as an explanation of this. The same rule the safety-comment check uses, for the same reason: adjacency
    is what makes a comment about the thing below it.
    """
    lines = text.splitlines()
    first = next((index for index, line in enumerate(lines) if line.strip().startswith(TABLE)), None)
    if first is None:
        return False
    cursor = first - 1
    while cursor >= 0:
        stripped = lines[cursor].strip()
        if not stripped.startswith("#"):
            return False
        if len(stripped) > 1:
            return True
        cursor -= 1
    return False


def manifests() -> list[Path]:
    """Every workspace member's manifest, wherever it lives."""
    found = sorted(CRATES.glob("*/Cargo.toml"))
    audit = AUDIT / "Cargo.toml"
    if audit.is_file():
        found.append(audit)
    return found


def problemsWith(path: Path) -> list[str]:
    """What is wrong with one crate's lint declaration."""
    text = path.read_text(encoding="utf-8")
    declared = tomllib.loads(text)
    rel = path.relative_to(ROOT).as_posix()
    lints = declared.get("lints")

    if lints == INHERITS:
        return []

    if lints is None and "lints" not in declared:
        return [f"  - {rel} declares no lints at all. add `[lints]` with `workspace = true`"]

    problems: list[str] = []

    # Opting out is allowed and has to be argued for, in the file, where somebody editing it will read it.
    if not explainsItself(text):
        problems.append(f"  - {rel} writes its own lint table and does not say why, directly above it")

    for (tool, lint), why in REQUIRED.items():
        table = declared.get("lints", {}).get(tool, {})
        level = levelOf(table.get(lint))
        if level is None:
            problems.append(f"  - {rel} writes its own table and drops `{tool}.{lint}`: {why}")
        elif level not in STRONG_ENOUGH:
            problems.append(
                f"  - {rel} sets `{tool}.{lint} = {level!r}`, which is weaker than {' or '.join(STRONG_ENOUGH)}: {why}"
            )

    return problems


def main() -> int:
    found = manifests()
    if not found:
        print("[workspaceLints] no crate manifests found, so this gate would pass on nothing")
        return 2

    problems: list[str] = []
    inherited = 0
    ownTable = 0

    for path in found:
        theirs = problemsWith(path)
        problems.extend(theirs)
        if not theirs:
            text = path.read_text(encoding="utf-8")
            if tomllib.loads(text).get("lints") == INHERITS:
                inherited += 1
            else:
                ownTable += 1

    if problems:
        print("[workspaceLints] a crate escaped the workspace lint table:")
        print("\n".join(problems))
        return 2

    print(
        f"[workspaceLints] OK. {len(found)} crates: {inherited} inherit the table, "
        f"{ownTable} write their own and say why."
    )
    return 0


def selftest() -> int:
    """Check that this gate can still fail.

    Every shape below is one somebody could plausibly commit, which is the only reason to write a gate at all.
    """
    problems: list[str] = []
    scratch = ROOT / ".tmp"
    scratch.mkdir(exist_ok=True)
    probe = scratch / "workspaceLintsSelftest.toml"

    good = '[package]\nname = "x"\n\n[lints]\nworkspace = true\n'
    silent = '[package]\nname = "x"\n'
    unexplained = '[package]\nname = "x"\n\n[lints.rust]\nunsafe_code = "deny"\n\n[lints.clippy]\nall = "deny"\n'
    weakened = (
        '[package]\nname = "x"\n\n'
        "# Why this crate writes its own table: it has a reason.\n"
        '[lints.rust]\nunsafe_code = "allow"\n\n[lints.clippy]\nall = "deny"\n'
    )
    dropped = (
        '[package]\nname = "x"\n\n'
        "# Why this crate writes its own table: it has a reason.\n"
        '[lints.rust]\nunsafe_code = "deny"\n'
    )
    explained = (
        '[package]\nname = "x"\n\n'
        "# Why this crate writes its own table: it has a reason.\n"
        '[lints.rust]\nunsafe_code = "deny"\n\n'
        '[lints.clippy]\nall = { level = "deny", priority = -1 }\n'
    )

    try:
        for what, text, shouldFail in (
            ("a crate inheriting the table", good, False),
            ("a crate with no lints at all", silent, True),
            ("a crate opting out with no reason", unexplained, True),
            ("a crate allowing unsafe code", weakened, True),
            ("a crate dropping the clippy group", dropped, True),
            ("a crate opting out and saying why", explained, False),
        ):
            probe.write_text(text, encoding="utf-8", newline="\n")
            found = problemsWith(probe)
            if shouldFail and not found:
                problems.append(f"{what} was not caught")
            if not shouldFail and found:
                problems.append(f"{what} was reported: {found}")
    finally:
        probe.unlink(missing_ok=True)

    for one in problems:
        print(f"[workspaceLints] selftest: {one}")
    return 2 if problems else 0


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        raise SystemExit(selftest())
    raise SystemExit(main())
