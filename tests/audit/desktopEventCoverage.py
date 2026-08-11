"""Gate: every product surface consumes one complete event-presentation SSOT.

The wire vocabulary belongs to ``EventBody::wire_name``. Presentation kind, message side, and localization keys belong
to ``assets/event-presentation.json``. Desktop, VS Code, and a future phone surface may localize those keys differently,
but they may not invent another event-name table or inspect opaque provider content through this contract.

Usage::

    python -X utf8 tests/audit/desktopEventCoverage.py
    python -X utf8 tests/audit/desktopEventCoverage.py --selftest
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
DESKTOP = ROOT / "crates" / "runtrol-gui" / "ui" / "src" / "frames.ts"
VSCODE = (
    ROOT / "extensions" / "runtrol-vscode" / "src" / "webview" / "main.ts",
    ROOT / "extensions" / "runtrol-vscode" / "src" / "webview" / "presentation.ts",
)

WIRE_NAME_ARM = re.compile(r'Self::\w+[^=]*=>\s*"([A-Za-z][A-Za-z0-9]*)"')
WIRE_NAME_FN = re.compile(r"fn\s+wire_name\s*\(")
ALLOWED_KINDS = {"message", "status", "turn", "notice", "usage", "rateLimit", "approval", "discard"}
ALLOWED_SIDES = {"mine", "theirs", "thought"}


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
        extras = sorted(set(contract) - expected)
        if extras:
            found.append(f"{name} presentation carries forbidden fields: {', '.join(extras)}")
    return found


def surfaceProblems(texts: dict[str, tuple[str, str]], kinds: set[str]) -> list[str]:
    """Return surfaces that ignore the SSOT or restore a local event-name map."""
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


def localizationProblems(data: Any, desktop: str, vscode: str) -> list[str]:
    """Return localization keys that a consuming surface would expose raw."""
    if not isinstance(data, dict) or not isinstance(data.get("events"), dict):
        return []
    desktopKeys: set[str] = set()
    vscodeKeys: set[str] = set()
    for contract in data["events"].values():
        if not isinstance(contract, dict):
            continue
        labelKey = contract.get("labelKey")
        textKey = contract.get("textKey")
        if nonempty(labelKey):
            desktopKeys.add(labelKey)
        if nonempty(textKey):
            desktopKeys.add(textKey)
            if contract.get("kind") == "status":
                vscodeKeys.add(textKey)
    found = [
        f"desktop has no localized text for {key}"
        for key in sorted(desktopKeys)
        if f'"{key}"' not in desktop
    ]
    found += [
        f"VS Code has no localized text for {key}"
        for key in sorted(vscodeKeys)
        if f'"{key}"' not in vscode
    ]
    return found


def nonempty(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


def report(problems: list[str]) -> int:
    print("[desktopEventCoverage] shared event presentation violations:", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    return 2


def main() -> int:
    """Compare the Rust vocabulary, shared contract, and both shipped consumers."""
    paths = (VOCABULARY, PRESENTATION, DESKTOP, *VSCODE)
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
        "desktop frames.ts": (DESKTOP.read_text(encoding="utf-8"), "event-presentation.json"),
        "VS Code main.ts": (VSCODE[0].read_text(encoding="utf-8"), "presentationOf"),
        "VS Code presentation.ts": (VSCODE[1].read_text(encoding="utf-8"), "event-presentation.json"),
    }
    found = (
        contractProblems(data, kinds)
        + surfaceProblems(texts, kinds)
        + localizationProblems(data, texts["desktop frames.ts"][0], texts["VS Code main.ts"][0])
    )
    if found:
        return report(found)
    print(f"[desktopEventCoverage] OK. {len(kinds)} events use one shared contract across desktop and VS Code.")
    return 0


def selftest() -> int:
    """Prove vocabulary, schema, meaning, and consumer drift all make the gate red."""
    rustFixture = """
    pub const fn wire_name(&self) -> &'static str {
        match self {
            Self::Attached(_) => "attached",
            Self::AgentMessageChunk(_) => "agentMessageChunk",
        }
    }
    pub const fn other(&self) -> &'static str { "notAnEvent" }
    """
    kinds = vocabulary(rustFixture)
    if kinds != {"attached", "agentMessageChunk"}:
        print("[desktopEventCoverage --selftest] FAIL. wire_name was not isolated.", file=sys.stderr)
        return 2
    green = {
        "schema": 1,
        "events": {
            "attached": {"kind": "status", "textKey": "session.attached"},
            "agentMessageChunk": {"kind": "message", "side": "theirs", "labelKey": "message.agent"},
        },
    }
    consumer = 'import presentation from "event-presentation.json";\nconst use = presentation.events;'
    if contractProblems(green, kinds) or surfaceProblems({"surface": (consumer, "event-presentation.json")}, kinds):
        print("[desktopEventCoverage --selftest] FAIL. the green fixture was rejected.", file=sys.stderr)
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
    ]
    for index, mutation in enumerate(mutations, start=1):
        if not contractProblems(mutation, kinds):
            print(f"[desktopEventCoverage --selftest] FAIL. contract mutation {index} escaped.", file=sys.stderr)
            return 2
    consumerMutations = ("const local = 1;", f'{consumer}\nif (event === "attached") return;')
    for index, mutation in enumerate(consumerMutations, start=1):
        if not surfaceProblems({"surface": (mutation, "event-presentation.json")}, kinds):
            print(f"[desktopEventCoverage --selftest] FAIL. consumer mutation {index} escaped.", file=sys.stderr)
            return 2
    desktop = 'const text = { "session.attached": "Attached", "message.agent": "Agent" };'
    vscode = 'const text = { "session.attached": "Attached" };'
    if localizationProblems(green, desktop, vscode):
        print("[desktopEventCoverage --selftest] FAIL. green localization was rejected.", file=sys.stderr)
        return 2
    if not localizationProblems(green, desktop.replace('"message.agent"', '"missing"'), vscode):
        print("[desktopEventCoverage --selftest] FAIL. missing localization escaped.", file=sys.stderr)
        return 2
    print("[desktopEventCoverage --selftest] OK. nine injected defects make the gate red.")
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest() if "--selftest" in sys.argv else main())
