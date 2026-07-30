"""Gate: runtrol keeps no copy of anybody's conversation.

This is the product's one absolute rule expressed as something a machine can check. runtrol supervises coding
CLIs; the conversation belongs to the CLI, is read live from the CLI's own store, and is never duplicated here.
Everything else in the design follows from it: it is why the database is roughly 200 bytes a session, why a
subscriber that falls behind is served from the provider's file, and why this product is allowed to sit alongside
the tools it supervises at all.

The rule dies by convenience, not by decision. Nobody sets out to keep transcripts. Somebody adds a preview so a
list looks nicer, or a last-message column so sorting works, and each step is small. So the check is structural
rather than a matter of judgement:

**No type that carries a payload may appear anywhere in the storage crate.** A payload is where a conversation
lives, so a storage row that can hold one is a storage row that will.

The payload-carrying types are discovered from the vocabulary, not listed here. Any type in the event vocabulary
with an `Opaque` field is one, plus `Opaque` itself. A type added tomorrow is covered the day it is written, and a
list written here would go stale in exactly the direction that matters.

Usage::

    python -X utf8 tests/audit/noTranscriptCopy.py

Exit codes:
    0 nothing that can hold a conversation reaches storage
    2 something can
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

import rustSource

ROOT = Path(__file__).resolve().parents[2]
CRATES = ROOT / "crates"

# Where the conversation-carrying types are defined.
VOCABULARY = CRATES / "runtrol-provider" / "src" / "event"

# The crate that writes to disk. What must never reach it is the whole of this gate.
STORAGE = "runtrol-store"

# The type every payload is made of. Everything that holds one of these holds a conversation.
THE_PAYLOAD = "Opaque"

# A declaration, and the name it declares.
DECLARES = re.compile(r"^\s*pub (?:struct|enum) (?P<name>[A-Z][A-Za-z0-9]*)")

# A field or variant whose type is the payload, however it is wrapped.
CARRIES = re.compile(rf"(?<![A-Za-z0-9_]){THE_PAYLOAD}(?![A-Za-z0-9_])")

# A name only counts when it stands on its own.
WORD = "[A-Za-z0-9_]"


def carriers() -> set[str]:
    """Every vocabulary type that can hold a conversation, discovered from how it is declared.

    A type counts when the payload appears anywhere between its declaration and the next one at the same level.
    That is deliberately generous: a type holding a type holding a payload is still a type that holds a
    conversation, and being generous in this direction costs a gate nothing.
    """
    found = {THE_PAYLOAD}
    if not VOCABULARY.is_dir():
        return found

    for path in sorted(VOCABULARY.rglob("*.rs")):
        lines = path.read_text(encoding="utf-8").splitlines()
        regions = rustSource.testRegions(lines)
        current: str | None = None
        for index, line in enumerate(lines):
            if rustSource.inRegions(index, regions):
                continue
            declared = DECLARES.match(line)
            if declared:
                current = declared.group("name")
                continue
            if current and CARRIES.search(rustSource.withoutComments(line)):
                found.add(current)
    return found


def offences(path: Path, names: set[str]) -> list[str]:
    """Every place this file names something that can hold a conversation."""
    lines = path.read_text(encoding="utf-8").splitlines()
    regions = rustSource.testRegions(lines)
    rel = path.relative_to(ROOT).as_posix()

    patterns = {name: re.compile(rf"(?<!{WORD}){re.escape(name)}(?!{WORD})") for name in names}

    found: list[str] = []
    for index, line in enumerate(lines):
        # Test code is checked too. A fixture that stores a conversation is a conversation on somebody's disk,
        # and a test is where the first one would arrive.
        cleaned = rustSource.withoutComments(line)
        if not cleaned.strip():
            continue
        for name, pattern in patterns.items():
            if pattern.search(cleaned):
                where = " (in a test, which still writes to a disk)" if rustSource.inRegions(index, regions) else ""
                found.append(f"  - {rel}:{index + 1} names `{name}`{where}")
    return found


def main() -> int:
    names = carriers()
    if names == {THE_PAYLOAD}:
        print("[noTranscriptCopy] found no payload-carrying types beyond the payload itself, which cannot be right")
        return 2

    source = CRATES / STORAGE / "src"
    if not source.is_dir():
        print(f"[noTranscriptCopy] the storage crate `{STORAGE}` has no source directory")
        return 2

    problems: list[str] = []
    checked = 0
    for path in sorted(source.rglob("*.rs")):
        checked += 1
        problems.extend(offences(path, names))

    if problems:
        print(f"[noTranscriptCopy] something that can hold a conversation reached `{STORAGE}`:")
        print("\n".join(problems))
        print("[noTranscriptCopy] runtrol stores the pointer to a session and never what was said in it.")
        return 2

    print(
        f"[noTranscriptCopy] OK. {checked} files in `{STORAGE}`, none names any of the "
        f"{len(names)} types that can hold a conversation."
    )
    return 0


def selftest() -> int:
    """Check that this gate can still fail, and that it still knows what a carrier is."""
    problems: list[str] = []

    names = carriers()
    for expected in ("Opaque", "Chunk", "Attached"):
        if expected not in names:
            problems.append(f"`{expected}` holds a payload and was not discovered as a carrier")
    for absent in ("SessionId", "TurnId"):
        if absent in names:
            problems.append(f"`{absent}` holds no payload and was discovered as a carrier")

    scratch = ROOT / ".tmp"
    scratch.mkdir(exist_ok=True)
    probe = scratch / "noTranscriptCopySelftest.rs"

    # The way this rule actually dies: a column somebody added so a list would look nicer.
    preview = 'pub struct SessionRow {\n    pub preview: Opaque,\n}\n'
    inATest = '#[cfg(test)]\nmod tests {\n    fn row() -> Chunk {\n        todo!()\n    }\n}\n'
    clean = 'pub struct SessionRow {\n    pub session: SessionId,\n    pub last_seen: WallMs,\n}\n'

    try:
        for what, source, shouldFail in (
            ("a payload stored as a preview", preview, True),
            ("a conversation type used only in a test", inATest, True),
            ("a row of identifiers and times", clean, False),
        ):
            probe.write_text(source, encoding="utf-8", newline="\n")
            found = offences(probe, names)
            if shouldFail and not found:
                problems.append(f"{what} was not caught")
            if not shouldFail and found:
                problems.append(f"{what} was reported: {found}")
    finally:
        probe.unlink(missing_ok=True)

    for one in problems:
        print(f"[noTranscriptCopy] selftest: {one}")
    return 2 if problems else 0


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        raise SystemExit(max(selftest(), rustSource.selftest()))
    raise SystemExit(main())
