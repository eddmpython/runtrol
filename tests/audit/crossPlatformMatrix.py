"""Gate: one shipped VSIX first-run method works on the native host.

The live path installs an explicit native VSIX into a clean stable VS Code profile, lets the extension discover and
copy its bundled Core, opens Runtrol, opens a new-conversation draft through the public command, and closes that exact
draft. With no archive argument the gate builds a current-host package in an ignored temporary directory first.

Usage::

    python -X utf8 tests/audit/crossPlatformMatrix.py --selftest
    python -X utf8 tests/audit/crossPlatformMatrix.py
    python -X utf8 tests/audit/crossPlatformMatrix.py --archive release/current-platform.vsix
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"
INSTALLER = EXTENSION / "tooling" / "installed-package.mjs"
RELEASE_POLICY = EXTENSION / "release-policy.json"
MARKER = "RUNTROL_VSCODE_PACKAGE "
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")


class Failed(Exception):
    """The shipped-package journey did not hold."""


def nativeTarget() -> str:
    """Return the same native target spelling Node uses for release packages."""
    systems = {"Windows": "win32", "Darwin": "darwin", "Linux": "linux"}
    machines = {
        "AMD64": "x64",
        "x86_64": "x64",
        "arm64": "arm64",
        "ARM64": "arm64",
        "aarch64": "arm64",
    }
    system = systems.get(platform.system())
    machine = machines.get(platform.machine())
    if system is None or machine is None:
        raise Failed(f"unsupported native host {platform.system()} {platform.machine()}")
    return f"{system}-{machine}"


def evidenceProblems(evidence: dict[str, Any], expectedTarget: str) -> list[str]:
    """Return every missing or contradictory bounded first-run fact."""
    found: list[str] = []
    required = {
        "vscode",
        "extensionVersion",
        "target",
        "extensionPath",
        "bundledCore",
        "managedCore",
        "configuredCore",
        "draftOpened",
        "draftTitle",
        "draftClosed",
    }
    for name in sorted(required - evidence.keys()):
        found.append(f"evidence has no {name}")

    vscode = evidence.get("vscode")
    if not isinstance(vscode, str) or not vscode.strip():
        found.append("VS Code version is absent")
    version = evidence.get("extensionVersion")
    if not isinstance(version, str) or VERSION_RE.fullmatch(version) is None:
        found.append("extension version is not an exact semantic version")
    if evidence.get("target") != expectedTarget:
        found.append(f"package target is {evidence.get('target')}, expected {expectedTarget}")
    if evidence.get("configuredCore") != "":
        found.append("the clean profile used a manually configured Core path")
    if evidence.get("draftOpened") is not True:
        found.append("the public new-conversation command did not open a draft")
    if evidence.get("draftTitle") != "New chat":
        found.append("the opened draft did not have the shipped new-chat title")
    if evidence.get("draftClosed") is not True:
        found.append("the exact new-conversation draft did not close")

    extensionPath = absolutePath(evidence.get("extensionPath"), "installed extension", found)
    bundledCore = absolutePath(evidence.get("bundledCore"), "bundled Core", found)
    managedCore = absolutePath(evidence.get("managedCore"), "managed Core", found)
    executable = "runtrol.exe" if expectedTarget.startswith("win32-") else "runtrol"
    if bundledCore is not None:
        if bundledCore.name != executable:
            found.append(f"bundled Core is not named {executable}")
        if extensionPath is not None and not isWithin(bundledCore, extensionPath):
            found.append("bundled Core is outside the installed extension")
    if managedCore is not None:
        if managedCore.name != executable:
            found.append(f"managed Core is not named {executable}")
        if extensionPath is not None and isWithin(managedCore, extensionPath):
            found.append("managed Core was not copied out of the installed extension")
    return found


def absolutePath(value: object, label: str, found: list[str]) -> Path | None:
    """Parse one host-native absolute evidence path without consulting the removed temporary tree."""
    if not isinstance(value, str) or not value:
        found.append(f"{label} path is absent")
        return None
    path = Path(value)
    if not path.is_absolute():
        found.append(f"{label} path is not absolute")
        return None
    return path


def isWithin(candidate: Path, parent: Path) -> bool:
    """Compare already absolute native paths with Windows case folding where needed."""
    candidateText = os.path.normcase(os.path.normpath(str(candidate)))
    parentText = os.path.normcase(os.path.normpath(str(parent)))
    try:
        return os.path.commonpath((candidateText, parentText)) == parentText
    except ValueError:
        return False


def selftest() -> int:
    """Prove every required first-run fact can make the gate red."""
    target = nativeTarget()
    temporary = Path(tempfile.gettempdir()).resolve()
    extension = temporary / "runtrol-cross-platform-selftest" / "extensions" / "runtrol.runtrol-studio-0.1.9"
    executable = "runtrol.exe" if target.startswith("win32-") else "runtrol"
    valid: dict[str, Any] = {
        "vscode": "1.132.1",
        "extensionVersion": "0.1.9",
        "target": target,
        "extensionPath": str(extension),
        "bundledCore": str(extension / "resources" / "core" / executable),
        "managedCore": str(temporary / "runtrol-cross-platform-selftest" / "user-data" / executable),
        "configuredCore": "",
        "draftOpened": True,
        "draftTitle": "New chat",
        "draftClosed": True,
    }
    if evidenceProblems(valid, target):
        print("[crossPlatformMatrix:selftest] FAIL. valid evidence was rejected.", file=sys.stderr)
        return 2

    defects: list[dict[str, Any]] = []
    for name in valid:
        missing = dict(valid)
        del missing[name]
        defects.append(missing)
    for name, value in (
        ("extensionVersion", "next"),
        ("target", "unsupported-x64"),
        ("configuredCore", str(temporary / executable)),
        ("draftOpened", False),
        ("draftTitle", "Conversation"),
        ("draftClosed", False),
        ("bundledCore", str(temporary / executable)),
        ("managedCore", str(extension / executable)),
    ):
        changed = dict(valid)
        changed[name] = value
        defects.append(changed)
    for index, defect in enumerate(defects, start=1):
        if not evidenceProblems(defect, target):
            print(f"[crossPlatformMatrix:selftest] FAIL. defect {index} escaped.", file=sys.stderr)
            return 2
    print(f"[crossPlatformMatrix:selftest] OK. all {len(defects)} evidence defects make the gate red.")
    return 0


def command(program: list[str], environment: dict[str, str] | None = None, timeout: float = 300.0) -> str:
    """Run one bounded build or journey command and retain only its diagnostic output."""
    result = subprocess.run(
        program,
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        check=False,
    )
    output = f"{result.stdout}{result.stderr}"
    if result.returncode != 0:
        raise Failed(f"{' '.join(program[:3])} returned {result.returncode}:\n{output[-8000:]}")
    return output


def buildArchive(directory: Path, target: str) -> Path:
    """Build one current-host package without leaving a release artifact in the repository."""
    executable = "runtrol.exe" if target.startswith("win32-") else "runtrol"
    command(["cargo", "build", "-p", "runtrol", "--bin", "runtrol"])
    binary = ROOT / "target" / "debug" / executable
    if not binary.is_file():
        raise Failed(f"the current-host Core was not built at {binary}")
    environment = dict(os.environ)
    environment["RUNTROL_CORE_BINARY"] = str(binary)
    environment["RUNTROL_PACKAGE_OUTPUT_DIR"] = str(directory)
    npm = shutil.which("npm.cmd" if sys.platform == "win32" else "npm") or shutil.which("npm")
    if npm is None:
        raise Failed("npm is required to build the current-host VSIX")
    command([npm, "--prefix", str(EXTENSION), "run", "package:native"], environment)
    policy = json.loads(RELEASE_POLICY.read_text(encoding="utf-8"))
    version = policy.get("version")
    archive = directory / f"runtrol-studio-{version}-{target}.vsix"
    if not archive.is_file():
        raise Failed(f"the package build produced no archive at {archive}")
    return archive


def readEvidence(output: str) -> dict[str, Any]:
    """Read the installer's single bounded evidence record."""
    records = [line[len(MARKER):] for line in output.splitlines() if line.startswith(MARKER)]
    if len(records) != 1:
        raise Failed(f"expected one {MARKER.strip()} record, found {len(records)}")
    value = json.loads(records[0])
    if not isinstance(value, dict):
        raise Failed("the installed-package evidence is not an object")
    return value


def exercise(archive: Path) -> None:
    """Install and drive one explicit package on its matching native host."""
    node = shutil.which("node.exe" if sys.platform == "win32" else "node") or shutil.which("node")
    if node is None:
        raise Failed("Node.js is required to launch the installed-package journey")
    program = [node, str(INSTALLER), str(archive)]
    if sys.platform.startswith("linux") and not os.environ.get("DISPLAY"):
        xvfb = shutil.which("xvfb-run")
        if xvfb is None:
            raise Failed("xvfb-run is required to test the installed VSIX without a Linux display")
        program = [xvfb, "-a", *program]
    output = command(program, timeout=240.0)
    target = nativeTarget()
    evidence = readEvidence(output)
    problems = evidenceProblems(evidence, target)
    if problems:
        raise Failed("installed-package evidence is incomplete:\n  - " + "\n  - ".join(problems))
    print(
        f"[crossPlatformMatrix] OK. {target} installed the exact VSIX, discovered bundled Core, "
        "opened Runtrol and a new-conversation draft, then closed it."
    )


def run(archive: Path | None) -> int:
    """Use an explicit release package or build one in a temporary directory."""
    try:
        if archive is not None:
            if not archive.is_file():
                raise Failed(f"archive is absent: {archive}")
            exercise(archive)
            return 0
        with tempfile.TemporaryDirectory(prefix="runtrol-cross-platform-") as raw:
            exercise(buildArchive(Path(raw), nativeTarget()))
    except (Failed, OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"[crossPlatformMatrix] FAIL: {error}", file=sys.stderr)
        return 2
    return 0


def main(argv: list[str]) -> int:
    """Select defect injection or the live native package journey."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--archive", type=Path)
    arguments = parser.parse_args(argv)
    if arguments.selftest:
        if arguments.archive:
            parser.error("--selftest cannot be combined with --archive")
        return selftest()
    return run(arguments.archive.resolve() if arguments.archive else None)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
