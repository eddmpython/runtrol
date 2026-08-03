"""Gate: the window presents every event kind runtrol's own vocabulary can produce.

The vocabulary is `runtrol_provider::EventBody::wire_name`, which is the one place an event's name on the
wire is written. The window has a presentation table beside it. Nothing held the two together, and the
result shipped: of nineteen kinds the page presented seven, and the other twelve fell through to a
fallback that printed the wire name. A Korean window showed `attached`, `toolCall` and `approvalRequested`
as bare English machine words in the middle of a conversation, and a tool call, which is one of the things
an operator most wants to read, was a single untranslated token.

The fallback itself is correct and stays: a name the vocabulary does not have means the two ends of
runtrol disagree about their own vocabulary, and an operator has to see which name it was. What this gate
refuses is that path being reached by names runtrol itself emits.

Why the comparison runs on text
-------------------------------

The two sides are a Rust match and a TypeScript object, and no build step crosses them. Reading both as
text is what makes the check possible at all, and it is why each side has one obvious place to read: a
`wire_name` match arm, and an exported list the module builds from its own tables.

Usage::

    python -X utf8 tests/audit/desktopEventCoverage.py
    python -X utf8 tests/audit/desktopEventCoverage.py --selftest

Exit codes:
    0 every kind the vocabulary emits has a presentation in the window
    2 a kind would reach the window as its raw wire name
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VOCABULARY = ROOT / "crates" / "runtrol-provider" / "src" / "event" / "mod.rs"
PRESENTATION = ROOT / "crates" / "runtrol-gui" / "ui" / "src" / "frames.ts"

# One arm of the `wire_name` match: `Self::Attached(_) => "attached",`
WIRE_NAME_ARM = re.compile(r"Self::\w+[^=]*=>\s*\"([A-Za-z][A-Za-z0-9]*)\"")

# The function whose arms are the vocabulary, so an unrelated match elsewhere is not read as one.
WIRE_NAME_FN = re.compile(r"fn\s+wire_name\s*\(")

# The list the page exports, and the names inside it.
PRESENTED_LIST = re.compile(r"PRESENTED_EVENTS[^=]*=\s*\[(.*?)\]", re.DOTALL)
QUOTED = re.compile(r"\"([A-Za-z][A-Za-z0-9]*)\"")

# Names the exported list pulls in from the tables it is built from, which are objects rather than
# literals inside the list. Read from their own declarations for the same reason.
TABLE_KEYS = re.compile(r"^\s{2}([A-Za-z][A-Za-z0-9]*):", re.MULTILINE)
TABLE_BLOCK = re.compile(r"const (?:PRESENTATION|STATUS_TEXT)[^{]*\{(.*?)\n\};", re.DOTALL)


def vocabulary(text: str) -> set[str]:
    """Every wire name the provider vocabulary can produce."""
    start = WIRE_NAME_FN.search(text)
    if start is None:
        return set()
    # From the function to the end of its match block. Bounded by the next `}` at the function's own
    # indentation, which is what closes it.
    tail = text[start.end():]
    end = tail.find("\n    }")
    body = tail if end < 0 else tail[:end]
    return set(WIRE_NAME_ARM.findall(body))


def presented(text: str) -> set[str]:
    """Every event kind the window has a presentation for."""
    names: set[str] = set()
    listed = PRESENTED_LIST.search(text)
    if listed:
        names.update(QUOTED.findall(listed.group(1)))
        # The list spreads its own tables in, so their keys count as presented too.
        if "UNREAD_EVENT" in listed.group(1):
            unread = re.search(r"UNREAD_EVENT\s*=\s*\"([A-Za-z][A-Za-z0-9]*)\"", text)
            if unread:
                names.add(unread.group(1))
    for block in TABLE_BLOCK.findall(text):
        names.update(TABLE_KEYS.findall(block))
    return names


def main() -> int:
    """Compare the two tables and name anything that would arrive as a raw wire name."""
    for path in (VOCABULARY, PRESENTATION):
        if not path.is_file():
            print(f"[desktopEventCoverage] {path.relative_to(ROOT)} is missing, so this gate watches nothing")
            return 2

    kinds = vocabulary(VOCABULARY.read_text(encoding="utf-8"))
    shown = presented(PRESENTATION.read_text(encoding="utf-8"))

    if not kinds:
        print("[desktopEventCoverage] found no wire names, so the comparison would pass on nothing")
        return 2
    if not shown:
        print("[desktopEventCoverage] found no presentations, so every kind would look uncovered")
        return 2

    missing = sorted(kinds - shown)
    if missing:
        print("[desktopEventCoverage] the window would show these as raw English wire names:")
        for name in missing:
            print(f"  - `{name}` is in the vocabulary and {PRESENTATION.name} has no presentation for it")
        print("  add it to STATUS_TEXT (or PRESENTATION) in that file, in the language the window speaks")
        return 2

    stale = sorted(shown - kinds)
    if stale:
        print("[desktopEventCoverage] the window presents kinds the vocabulary cannot produce:")
        for name in stale:
            print(f"  - `{name}` is presented and no `wire_name` arm emits it. it is dead presentation")
        return 2

    print(f"[desktopEventCoverage] OK. all {len(kinds)} event kinds have a presentation in the window.")
    return 0


def selftest() -> int:
    """Prove the reading of both sides can fail."""
    problems: list[str] = []

    rustFixture = """
    pub const fn wire_name(&self) -> &'static str {
        match self {
            Self::Attached(_) => "attached",
            Self::ToolCall(_) => "toolCall",
        }
    }

    pub const fn other(&self) -> &'static str {
        match self {
            Self::Nothing => "notAnEvent",
        }
    }
"""
    read = vocabulary(rustFixture)
    if read != {"attached", "toolCall"}:
        problems.append(f"the vocabulary was not read from wire_name alone: {sorted(read)}")

    tsFixture = """
export const UNREAD_EVENT = "unmapped";
const PRESENTATION: Record<string, X> = {
  agentMessageChunk: { side: "theirs", label: "에이전트" },
};
const STATUS_TEXT: Record<string, string> = {
  attached: "세션에 연결됐다",
};
export const PRESENTED_EVENTS: readonly string[] = [
  ...Object.keys(PRESENTATION),
  ...Object.keys(STATUS_TEXT),
  "turn",
  UNREAD_EVENT,
];
"""
    shown = presented(tsFixture)
    for name in ("agentMessageChunk", "attached", "turn", "unmapped"):
        if name not in shown:
            problems.append(f"`{name}` was not read as presented: {sorted(shown)}")
    if "toolCall" in shown:
        problems.append("a kind with no presentation was read as presented")

    if problems:
        print("[desktopEventCoverage --selftest] the gate cannot catch what it claims to.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[desktopEventCoverage --selftest] OK. 6 injected defects all caught.")
    return 0


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        raise SystemExit(selftest())
    raise SystemExit(main())
