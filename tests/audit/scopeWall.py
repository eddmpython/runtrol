"""Gate: every request has a rule about who may make it, and the wall is consulted before anything acts.

A remote caller is denied by default. That is the security posture's first line, and the shape it takes in the code
is a table saying what each request needs. The table has a wildcard arm, because the request vocabulary is open
ended and a build must not fall over when a newer one speaks to it, and that wildcard is exactly the hole this gate
exists to watch: the compiler cannot tell anybody that a request went unmapped, because as far as it is concerned
the wildcard handles it.

So three things are checked, and each one has already been the thing that went wrong somewhere:

1. **Every request the wire declares appears in the table.** A request that reaches the wildcard is refused rather
   than allowed, which is safe, but it is also a feature that silently does not work and an operator with no idea
   why. The compiler cannot catch this across a crate boundary.
2. **The wildcard refuses.** An arm that answered anything else would hand authority to whatever this build has
   never heard of.
3. **The wall is consulted before the dispatcher does anything else.** A check that runs after another branch has
   acted is a check on the way out, and what it was meant to prevent has already happened.

Usage::

    python -X utf8 tests/audit/scopeWall.py

Exit codes:
    0 every request has a rule and the wall is asked first
    2 something can be asked for with nobody deciding whether it may be
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

import rustSource

ROOT = Path(__file__).resolve().parents[2]
WIRE = ROOT / "crates" / "runtrol-ipc" / "src" / "wire.rs"
TABLE = ROOT / "crates" / "runtrol-daemon" / "src" / "scope.rs"
BOUNDARY = ROOT / "crates" / "runtrol-daemon" / "src" / "dispatch.rs"

# The enum whose variants are the requests a caller can make.
THE_REQUEST = "Request"

# What the table's wildcard has to answer. Anything else is authority handed to the unknown.
REFUSAL = "Needed::Unknown"

# The call that asks the wall, and the function it has to be first inside.
ASKS_THE_WALL = re.compile(
    r"crate::scope::allowed(?:_with_authority)?\(|scope::allowed(?:_with_authority)?\("
)
THE_BOUNDARY_NAME = "answer_prepared"
THE_BOUNDARY = re.compile(
    rf"^\s*pub\(crate\)\s+(?:async\s+)?fn\s+{THE_BOUNDARY_NAME}\s*\("
)


def variantsOf(path: Path, enum: str) -> list[str]:
    """Every variant name declared by one enum, in source order."""
    lines = path.read_text(encoding="utf-8").splitlines()
    regions = rustSource.testRegions(lines)

    start = None
    for index, line in enumerate(lines):
        if rustSource.inRegions(index, regions):
            continue
        if re.match(rf"^pub enum {re.escape(enum)}\b", line.strip()):
            start = index
            break
    if start is None:
        return []

    found: list[str] = []
    depth = 0
    for index in range(start, len(lines)):
        cleaned = rustSource.withoutNoise(lines[index])
        opened = cleaned.count("{")
        closed = cleaned.count("}")
        # A variant is declared at the enum's own level, which is depth one.
        if depth == 1:
            declared = re.match(r"^\s{4}([A-Z][A-Za-z0-9]*)\s*[,{(]", lines[index])
            if declared:
                found.append(declared.group(1))
        depth += opened - closed
        if index > start and depth <= 0:
            break
    return found


def mapped(path: Path, enum: str) -> set[str]:
    """Every variant the table names, outside its tests."""
    lines = path.read_text(encoding="utf-8").splitlines()
    regions = rustSource.testRegions(lines)
    pattern = re.compile(rf"{re.escape(enum)}::([A-Z][A-Za-z0-9]*)")

    found: set[str] = set()
    for index, line in enumerate(lines):
        if rustSource.inRegions(index, regions):
            continue
        found.update(pattern.findall(rustSource.withoutComments(line)))
    return found


def wildcardRefuses(path: Path) -> bool:
    """Whether the table's catch-all answers the refusal."""
    lines = path.read_text(encoding="utf-8").splitlines()
    regions = rustSource.testRegions(lines)
    for index, line in enumerate(lines):
        if rustSource.inRegions(index, regions):
            continue
        cleaned = rustSource.withoutComments(line)
        if re.match(r"^\s*_\s*=>", cleaned):
            return REFUSAL in cleaned
    return False


def askedFirst(path: Path) -> str | None:
    """What the boundary does before it asks the wall, when it does anything."""
    lines = path.read_text(encoding="utf-8").splitlines()
    start = next((index for index, line in enumerate(lines) if THE_BOUNDARY.search(line)), None)
    if start is None:
        return f"`{THE_BOUNDARY_NAME}` is not in this file, so this gate is watching the wrong function"

    for index in range(start + 1, len(lines)):
        cleaned = rustSource.withoutComments(lines[index]).strip()
        if not cleaned or cleaned.startswith(")") or cleaned in ("{", "}"):
            continue
        if ASKS_THE_WALL.search(cleaned):
            return None
        # Anything else that runs first. A signature spread over several lines is still the signature, so the
        # opening brace is what says the body has started.
        if any(cleaned.startswith(word) for word in ("if ", "match ", "let ", "return ", "for ")):
            return f"line {index + 1} runs before the wall is asked: {cleaned[:70]}"
    return "the wall is never asked in this function"


def main() -> int:
    declared = variantsOf(WIRE, THE_REQUEST)
    if not declared:
        print(f"[scopeWall] found no `{THE_REQUEST}` variants in {WIRE.name}, so this gate would pass on nothing")
        return 2

    problems: list[str] = []

    unmapped = [name for name in declared if name not in mapped(TABLE, THE_REQUEST)]
    for name in unmapped:
        problems.append(
            f"  - `{THE_REQUEST}::{name}` has no rule in {TABLE.name}. it would fall through to the wildcard, "
            f"be refused, and look to an operator like a feature that silently does nothing"
        )

    if not wildcardRefuses(TABLE):
        problems.append(
            f"  - the wildcard in {TABLE.name} does not answer `{REFUSAL}`. anything this build has never heard "
            f"of would be allowed"
        )

    ranFirst = askedFirst(BOUNDARY)
    if ranFirst:
        problems.append(
            f"  - {BOUNDARY.name}: {ranFirst}. a wall consulted after something has acted is a wall on the way out"
        )

    if problems:
        print("[scopeWall] something can be asked for with nobody deciding whether it may be:")
        print("\n".join(problems))
        return 2

    print(
        f"[scopeWall] OK. {len(declared)} requests all have a rule, the wildcard refuses, "
        f"and the wall is asked before anything acts."
    )
    return 0


def selftest() -> int:
    """Check that this gate can still fail."""
    problems: list[str] = []

    if "List" not in variantsOf(WIRE, THE_REQUEST):
        problems.append("the request variants were not found, so the comparison would pass on nothing")
    if "List" not in mapped(TABLE, THE_REQUEST):
        problems.append("the table's rules were not found, so every request would look unmapped")

    scratch = ROOT / ".tmp"
    scratch.mkdir(exist_ok=True)
    probe = scratch / "scopeWallSelftest.rs"

    allows = "pub fn needed(request: &Request) -> Needed {\n    match request {\n        _ => Needed::Anyone(\"\"),\n    }\n}\n"
    refuses = "pub fn needed(request: &Request) -> Needed {\n    match request {\n        _ => Needed::Unknown,\n    }\n}\n"
    late = (
        "pub(crate) fn answer_prepared(\n"
        "    conversation: &mut Conversation,\n"
        ") -> Reply {\n"
        "    if let Request::Hello { wire } = request {\n"
        "        return greet(wire);\n"
        "    }\n"
        "    if let Err(refusal) = crate::scope::allowed(&conversation.caller, &request, &ledger) {\n"
        "        return refuse();\n"
        "    }\n"
        "}\n"
    )
    early = (
        "pub(crate) async fn answer_prepared(\n"
        "    conversation: &mut Conversation,\n"
        ") -> Reply {\n"
        "    if let Err(refusal) = crate::scope::allowed(&conversation.caller, &request, &ledger) {\n"
        "        return refuse();\n"
        "    }\n"
        "}\n"
    )

    try:
        probe.write_text(allows, encoding="utf-8", newline="\n")
        if wildcardRefuses(probe):
            problems.append("a wildcard that allows everything was read as refusing")
        probe.write_text(refuses, encoding="utf-8", newline="\n")
        if not wildcardRefuses(probe):
            problems.append("a wildcard that refuses was read as allowing")

        probe.write_text(late, encoding="utf-8", newline="\n")
        if askedFirst(probe) is None:
            problems.append("a wall asked after the greeting was read as asked first")
        probe.write_text(early, encoding="utf-8", newline="\n")
        if askedFirst(probe) is not None:
            problems.append(f"a wall asked first was reported: {askedFirst(probe)}")
    finally:
        probe.unlink(missing_ok=True)

    for one in problems:
        print(f"[scopeWall] selftest: {one}")
    return 2 if problems else 0


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        raise SystemExit(max(selftest(), rustSource.selftest()))
    raise SystemExit(main())
