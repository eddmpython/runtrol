"""Gate: one user-facing first-run method is wired to every native package job."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

from vscodePackage import EXPECTED_TARGETS, releasePackageJourneyProblems, targetContractProblems


ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"


def ordinaryMatrixProblems(workflow: str) -> list[str]:
    """Require the live journey in the native three-OS hosted matrix."""
    jobMatch = re.search(
        r"(?ms)^  crossPlatform:\r?\n(?P<body>.*?)(?=^  [A-Za-z0-9_-]+:\r?\n|\Z)",
        workflow,
    )
    if jobMatch is None:
        return ["gates.yml has no crossPlatform job"]
    job = jobMatch.group("body")
    found: list[str] = []
    if "os: [windows-latest, macos-latest, ubuntu-latest]" not in job:
        found.append("the ordinary first-run matrix is not the native Windows, macOS, and Linux set")
    if "runs-on: ${{ matrix.os }}" not in job:
        found.append("the ordinary first-run matrix does not run on each selected native OS")
    stepMatch = re.search(
        r"(?ms)^      - name: 출하 VSIX 설치, 번들 Core, 새 대화 작성 탭, 닫기\r?\n"
        r"(?P<body>.*?)(?=^      - (?:name:|uses:)|\Z)",
        job,
    )
    if stepMatch is None:
        found.append("the ordinary matrix has no active first-run journey step")
        return found
    step = stepMatch.group("body")
    if re.search(r"(?m)^        if:", step):
        found.append("the ordinary first-run journey is conditional inside its native matrix")
    if re.search(r"(?m)^        run: python -X utf8 tests/audit/crossPlatformMatrix\.py\s*$", step) is None:
        found.append("the ordinary native matrix does not execute crossPlatformMatrix.py")
    return found


def contractProblems(
    package: dict[str, Any],
    targets: dict[str, object],
    verifier: str,
    gatesWorkflow: str,
    releaseWorkflow: str,
) -> list[str]:
    """Return drift in the shared command, automatic Core default, or native runner wiring."""
    found = targetContractProblems(targets)
    found.extend(ordinaryMatrixProblems(gatesWorkflow))
    found.extend(releasePackageJourneyProblems(releaseWorkflow))

    contributes = package.get("contributes")
    if not isinstance(contributes, dict):
        return [*found, "the package has no contribution table"]
    commands = contributes.get("commands")
    startCommands = [
        row for row in commands if isinstance(row, dict) and row.get("command") == "runtrol.startSession"
    ] if isinstance(commands, list) else []
    # The palette composes what a person reads from the category and the title, so that is what is checked. The
    # titles themselves dropped the prefix once the category carried it, and a gate matching the old composed
    # string went red on a rename that changed nothing anybody sees.
    started = startCommands[0] if len(startCommands) == 1 else {}
    if started.get("category") != "Runtrol" or started.get("title") != "New Conversation":
        found.append("the package has no single public Runtrol: New Conversation command")
    keybindings = contributes.get("keybindings")
    startBindings = [
        row for row in keybindings if isinstance(row, dict) and row.get("command") == "runtrol.startSession"
    ] if isinstance(keybindings, list) else []
    expectedBinding = {
        "command": "runtrol.startSession",
        "key": "ctrl+k ctrl+n",
        "mac": "cmd+k cmd+n",
        "when": "!terminalFocus",
    }
    if startBindings != [expectedBinding]:
        found.append("the public new-conversation shortcut differs by platform")
    configuration = contributes.get("configuration")
    properties = configuration.get("properties") if isinstance(configuration, dict) else None
    corePath = properties.get("runtrol.corePath") if isinstance(properties, dict) else None
    if not isinstance(corePath, dict) or corePath.get("default") != "":
        found.append("the shipped Core path is not automatic by default")

    # What the installed package has to prove on every desktop: the one public command runs, and a conversation
    # editor that was not there before is. The earlier draft-and-close dance was a surface this product no longer
    # has, and naming it here kept the gate red about something that had been deliberately removed.
    for token in (
        'executeCommand("runtrol.startSession"',
        "tabsBeforeCommand",
        "isConversationEditor",
        "newConversationTitle",
    ):
        if token not in verifier:
            found.append(f"the installed-package verifier is missing {token}")
    return found


def fixture() -> tuple[dict[str, Any], dict[str, object], str, str, str]:
    """Return one minimal valid shared first-run contract for mutation tests."""
    package = {
        "contributes": {
            "commands": [
                {"command": "runtrol.startSession", "title": "New Conversation", "category": "Runtrol"},
            ],
            "keybindings": [{
                "command": "runtrol.startSession",
                "key": "ctrl+k ctrl+n",
                "mac": "cmd+k cmd+n",
                "when": "!terminalFocus",
            }],
            "configuration": {"properties": {"runtrol.corePath": {"default": ""}}},
        },
    }
    targets = {name: dict(contract) for name, contract in EXPECTED_TARGETS.items()}
    verifier = (
        'executeCommand("runtrol.startSession", { interactive: false }) tabsBeforeCommand '
        "isConversationEditor newConversationTitle"
    )
    gates = """
  crossPlatform:
    strategy:
      matrix:
        os: [windows-latest, macos-latest, ubuntu-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - name: 출하 VSIX 설치, 번들 Core, 새 대화 작성 탭, 닫기
        run: python -X utf8 tests/audit/crossPlatformMatrix.py
      - name: next
        run: echo next
    """
    release = """
  package:
    strategy:
      matrix: ${{ fromJSON(needs.prepare.outputs.matrix) }}
    runs-on: ${{ matrix.runner }}
    steps:
      - name: Install the package and complete the shared first-run journey
        run: >-
          python -X utf8 tests/audit/crossPlatformMatrix.py
          --archive release/runtrol-studio-${{ needs.prepare.outputs.version }}-${{ matrix.target }}.vsix
      - name: next
        run: echo next
    """
    return package, targets, verifier, gates, release


def selftest() -> int:
    """Prove independent command, default, verifier, matrix, and runner drift turns red."""
    valid = fixture()
    if contractProblems(*valid):
        print("[crossPlatformContract:selftest] FAIL. valid contract was rejected.", file=sys.stderr)
        return 2
    package, targets, verifier, gates, release = valid
    wrongCommand = json.loads(json.dumps(package))
    wrongCommand["contributes"]["commands"][0]["title"] = "Platform setup"
    wrongDefault = json.loads(json.dumps(package))
    wrongDefault["contributes"]["configuration"]["properties"]["runtrol.corePath"]["default"] = "/core"
    wrongRunner = {name: dict(contract) for name, contract in targets.items()}
    wrongRunner["win32-arm64"]["runner"] = "windows-2025"
    defects = (
        (wrongCommand, targets, verifier, gates, release),
        (wrongDefault, targets, verifier, gates, release),
        (package, wrongRunner, verifier, gates, release),
        (package, targets, verifier.replace("tabsBeforeCommand", ""), gates, release),
        (package, targets, verifier, gates.replace("windows-latest", "windows-disabled"), release),
        (package, targets, verifier, gates.replace("runs-on: ${{ matrix.os }}", "runs-on: ubuntu-latest"), release),
        (package, targets, verifier, gates.replace("run: python", "if: false\n        run: python"), release),
        (package, targets, verifier, gates, release.replace("matrix.target", "matrix.family")),
        (package, targets, verifier, gates, release.replace("run: >-", "if: false\n        run: >-")),
    )
    for index, defect in enumerate(defects, start=1):
        if not contractProblems(*defect):
            print(f"[crossPlatformContract:selftest] FAIL. defect {index} escaped.", file=sys.stderr)
            return 2
    print(f"[crossPlatformContract:selftest] OK. all {len(defects)} contract defects make the gate red.")
    return 0


def run() -> int:
    """Read the repository-owned first-run surfaces and report contract drift."""
    try:
        package = json.loads((EXTENSION / "package.json").read_text(encoding="utf-8"))
        targets = json.loads((EXTENSION / "release-targets.json").read_text(encoding="utf-8"))
        verifier = (EXTENSION / "src" / "integration" / "installedPackage.test.ts").read_text(encoding="utf-8")
        gates = (ROOT / ".github" / "workflows" / "gates.yml").read_text(encoding="utf-8")
        release = (ROOT / ".github" / "workflows" / "vscode-release.yml").read_text(encoding="utf-8")
    except (OSError, json.JSONDecodeError) as error:
        print(f"[crossPlatformContract] FAIL. cannot read contract: {error}", file=sys.stderr)
        return 2
    found = contractProblems(package, targets, verifier, gates, release)
    if found:
        print("[crossPlatformContract] shared first-run contract drifted:", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[crossPlatformContract] OK. six native packages expose one automatic VS Code first-run method.")
    return 0


def main(argv: list[str]) -> int:
    """Select mutation proof or repository inspection."""
    if argv == ["--selftest"]:
        return selftest()
    if argv:
        print("usage: crossPlatformContract.py [--selftest]", file=sys.stderr)
        return 1
    return run()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
