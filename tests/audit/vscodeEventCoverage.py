"""Gate: the VS Code surface consumes the complete event-presentation SSOT.

The wire vocabulary belongs to ``EventBody::wire_name``. Presentation kind, message side, and localization keys belong
to ``assets/event-presentation.json``. VS Code and future surfaces may localize those keys differently, but they may not
invent another event-name table or inspect opaque provider content through this contract.

Usage::

    python -X utf8 tests/audit/vscodeEventCoverage.py
    python -X utf8 tests/audit/vscodeEventCoverage.py --selftest
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
VOCABULARY = ROOT / "crates" / "runtrol-provider" / "src" / "event" / "mod.rs"
PRESENTATION = ROOT / "assets" / "event-presentation.json"
# The conversation surface is the service's own terminal, so the extension presents provider events only
# in the sidebar (activity, approvals): one file carries that vocabulary now.
VSCODE = (
    ROOT / "extensions" / "runtrol-vscode" / "src" / "events" / "presentation.ts",
)

WIRE_NAME_ARM = re.compile(r'Self::\w+[^=]*=>\s*"([A-Za-z][A-Za-z0-9]*)"')
WIRE_NAME_FN = re.compile(r"fn\s+wire_name\s*\(")
# `tool` renders the provider's own classification, label and status as one line that updates in place. It is a
# kind of its own because a fixed status sentence cannot say which tool ran against what, which is most of what a
# coding agent does.
ALLOWED_KINDS = {
    "message",
    "status",
    "turn",
    "notice",
    "usage",
    "rateLimit",
    "approval",
    "tool",
    "discard",
}
ALLOWED_SIDES = {"mine", "theirs", "thought"}
# A tool frame is either the call or what answered it. A surface needs to tell them apart to show both at
# once, and the event name is the only thing that says which: the payload that would answer belongs to the
# provider, and arrival order freezes the first partial result of a service that streams them.
ALLOWED_PARTS = {"call", "result"}


def vocabulary(text: str) -> set[str]:
    """Return every wire name the provider vocabulary can produce."""
    start = WIRE_NAME_FN.search(text)
    if start is None:
        return set()
    tail = text[start.end():]
    end = tail.find("\n    }")
    body = tail if end < 0 else tail[:end]
    return set(WIRE_NAME_ARM.findall(body))


def contractProblems(data: Any, kinds: set[str]) -> list[str]:
    """Return completeness, schema, and bounded-meaning defects in presentation data."""
    if not isinstance(data, dict) or data.get("schema") != 1 or not isinstance(data.get("events"), dict):
        return ["the shared presentation must have schema 1 and an events object"]
    events = data["events"]
    found: list[str] = []
    missing = sorted(kinds - set(events))
    stale = sorted(set(events) - kinds)
    if missing:
        found.append(f"wire events have no shared presentation: {', '.join(missing)}")
    if stale:
        found.append(f"shared presentations have no wire event: {', '.join(stale)}")
    for name, contract in events.items():
        if not isinstance(contract, dict):
            found.append(f"{name} presentation is not an object")
            continue
        kind = contract.get("kind")
        if kind not in ALLOWED_KINDS:
            found.append(f"{name} has unsupported presentation kind {kind!r}")
            continue
        expected = {"kind"}
        if kind == "message":
            expected.update(("side", "labelKey"))
            if contract.get("side") not in ALLOWED_SIDES or not nonempty(contract.get("labelKey")):
                found.append(f"{name} message presentation has no valid side and labelKey")
        elif kind in {"status", "approval"}:
            expected.add("textKey")
            if not nonempty(contract.get("textKey")):
                found.append(f"{name} {kind} presentation has no textKey")
        elif kind == "tool":
            expected.add("part")
            if contract.get("part") not in ALLOWED_PARTS:
                found.append(f"{name} tool presentation has no valid part")
        extras = sorted(set(contract) - expected)
        if extras:
            found.append(f"{name} presentation carries forbidden fields: {', '.join(extras)}")
    return found


def surfaceProblems(texts: dict[str, tuple[str, str]], kinds: set[str]) -> list[str]:
    """Return consumers that ignore the SSOT or restore a local event-name map."""
    found: list[str] = []
    for name, (text, requiredToken) in texts.items():
        if requiredToken not in text:
            found.append(f"{name} does not consume the shared presentation through {requiredToken}")
        duplicates = sorted(
            kind
            for kind in kinds
            if re.search(rf"\bevent\s*===\s*['\"]{re.escape(kind)}['\"]", text)
            or re.search(rf"^\s+{re.escape(kind)}\s*:", text, re.MULTILINE)
        )
        if duplicates:
            found.append(f"{name} redeclares wire event names: {', '.join(duplicates)}")
    return found


def localizationProblems(data: Any, vscode: str) -> list[str]:
    """Return localization keys that VS Code would expose raw."""
    if not isinstance(data, dict) or not isinstance(data.get("events"), dict):
        return []
    vscodeKeys = {
        contract["textKey"]
        for contract in data["events"].values()
        if isinstance(contract, dict)
        and contract.get("kind") == "status"
        and nonempty(contract.get("textKey"))
    }
    return [
        f"VS Code has no localized text for {key}"
        for key in sorted(vscodeKeys)
        if f'"{key}"' not in vscode
    ]


def nonempty(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


def report(problems: list[str]) -> int:
    print("[vscodeEventCoverage] shared event presentation violations:", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    return 2


def main() -> int:
    """Compare the Rust vocabulary, shared contract, and shipped VS Code consumers."""
    paths = (VOCABULARY, PRESENTATION, *VSCODE)
    missingPaths = [str(path.relative_to(ROOT)) for path in paths if not path.is_file()]
    if missingPaths:
        return report([f"required file is missing: {name}" for name in missingPaths])
    kinds = vocabulary(VOCABULARY.read_text(encoding="utf-8"))
    if not kinds:
        return report(["no wire names were found, so the gate would pass on nothing"])
    try:
        data = json.loads(PRESENTATION.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return report([f"the shared presentation is not readable JSON: {error}"])
    texts = {
        "VS Code main.ts": (VSCODE[0].read_text(encoding="utf-8"), "presentationOf"),
        "VS Code presentation.ts": (VSCODE[1].read_text(encoding="utf-8"), "event-presentation.json"),
    }
    found = (
        contractProblems(data, kinds)
        + surfaceProblems(texts, kinds)
        + localizationProblems(data, texts["VS Code main.ts"][0])
    )
    if found:
        return report(found)
    print(f"[vscodeEventCoverage] OK. {len(kinds)} events use one shared contract in VS Code.")
    return 0


def selftest() -> int:
    """Prove vocabulary, schema, meaning, consumer, and localization drift all make the gate red."""
    rustFixture = """
    pub const fn wire_name(&self) -> &'static str {
        match self {
            Self::Attached(_) => "attached",
            Self::AgentMessageChunk(_) => "agentMessageChunk",
            Self::ToolCall(_) => "toolCall",
        }
    }
    pub const fn other(&self) -> &'static str { "notAnEvent" }
    """
    kinds = vocabulary(rustFixture)
    if kinds != {"attached", "agentMessageChunk", "toolCall"}:
        print("[vscodeEventCoverage --selftest] FAIL. wire_name was not isolated.", file=sys.stderr)
        return 2
    green = {
        "schema": 1,
        "events": {
            "attached": {"kind": "status", "textKey": "session.attached"},
            "agentMessageChunk": {"kind": "message", "side": "theirs", "labelKey": "message.agent"},
            "toolCall": {"kind": "tool", "part": "call"},
        },
    }
    consumer = 'import presentation from "event-presentation.json";\nconst use = presentation.events;'
    if contractProblems(green, kinds) or surfaceProblems({"surface": (consumer, "event-presentation.json")}, kinds):
        print("[vscodeEventCoverage --selftest] FAIL. the green fixture was rejected.", file=sys.stderr)
        return 2
    mutations = [
        {"schema": 2, "events": green["events"]},
        {"schema": 1, "events": {"attached": green["events"]["attached"]}},
        {"schema": 1, "events": {**green["events"], "stale": {"kind": "discard"}}},
        {"schema": 1, "events": {**green["events"], "attached": {"kind": "chat"}}},
        {
            "schema": 1,
            "events": {**green["events"], "agentMessageChunk": {"kind": "message", "side": "other"}},
        },
        {
            "schema": 1,
            "events": {**green["events"], "attached": {"kind": "status", "textKey": "x", "payload": "x"}},
        },
        {"schema": 1, "events": {**green["events"], "toolCall": {"kind": "tool"}}},
        {"schema": 1, "events": {**green["events"], "toolCall": {"kind": "tool", "part": "both"}}},
    ]
    for index, mutation in enumerate(mutations, start=1):
        if not contractProblems(mutation, kinds):
            print(f"[vscodeEventCoverage --selftest] FAIL. contract mutation {index} escaped.", file=sys.stderr)
            return 2
    consumerMutations = ("const local = 1;", f'{consumer}\nif (event === "attached") return;')
    for index, mutation in enumerate(consumerMutations, start=1):
        if not surfaceProblems({"surface": (mutation, "event-presentation.json")}, kinds):
            print(f"[vscodeEventCoverage --selftest] FAIL. consumer mutation {index} escaped.", file=sys.stderr)
            return 2
    vscode = 'const text = { "session.attached": "Attached" };'
    if localizationProblems(green, vscode):
        print("[vscodeEventCoverage --selftest] FAIL. green localization was rejected.", file=sys.stderr)
        return 2
    if not localizationProblems(green, vscode.replace('"session.attached"', '"missing"')):
        print("[vscodeEventCoverage --selftest] FAIL. missing localization escaped.", file=sys.stderr)
        return 2
    print("[vscodeEventCoverage --selftest] OK. eleven injected defects make the gate red.")
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest() if "--selftest" in sys.argv else main())
