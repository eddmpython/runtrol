"""Gate: an installed VSIX upgrades and rolls back without stopping an active session.

The live gate installs a baseline package, starts one external ACP session through the package's managed Core,
installs the current package, then reinstalls the baseline package. Every phase runs in the exact tested VS Code
Extension Host with one isolated profile and runtrol home. The original daemon and provider process identifiers must
remain alive, the exact selected session must restore, and the stable managed Core path must move old to current to
old by digest.

Usage::

    python -X utf8 tests/audit/vscodeUpgradeRollback.py --selftest
    python -X utf8 tests/audit/vscodeUpgradeRollback.py
    python -X utf8 tests/audit/vscodeUpgradeRollback.py --archive release/current-platform.vsix
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from typing import Any
from xml.etree import ElementTree

ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"
RELEASE_POLICY = EXTENSION / "release-policy.json"
BASELINE_VERSION = "0.0.1"
RESULT_MARKER = "RUNTROL_VSCODE_UPGRADE "
SESSION_PATTERN = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


def evidenceProblems(
    value: dict[str, Any],
    currentVersion: str,
    baselineDigest: str,
    currentDigest: str,
) -> list[str]:
    """Return every missing lifecycle guarantee in one result record."""
    found: list[str] = []
    if value.get("baselineVersion") != BASELINE_VERSION:
        found.append("the baseline extension version is wrong")
    if value.get("currentVersion") != currentVersion:
        found.append("the current extension version is wrong")
    if not isinstance(value.get("session"), str) or SESSION_PATTERN.fullmatch(value["session"]) is None:
        found.append("the exact runtrol session identifier is missing")
    daemon = value.get("daemonPid")
    provider = value.get("providerPid")
    if not isinstance(daemon, int) or daemon <= 0:
        found.append("the original daemon process identifier is missing")
    if not isinstance(provider, int) or provider <= 0 or provider == daemon:
        found.append("the original provider process identifier is missing")
    if value.get("baselineDigest") != baselineDigest:
        found.append("the baseline managed Core digest is wrong")
    if value.get("upgradeDigest") != currentDigest or currentDigest == baselineDigest:
        found.append("the upgrade did not select the distinct current Core")
    if value.get("rollbackDigest") != baselineDigest:
        found.append("the rollback did not restore the baseline Core")
    workspace = value.get("workspace")
    if not isinstance(workspace, str) or not workspace:
        found.append("the selected workspace evidence is missing")
    baselineDirectory = value.get("baselineDirectory")
    currentDirectory = value.get("currentDirectory")
    rollbackDirectory = value.get("rollbackDirectory")
    if not all(isinstance(item, str) and item for item in (baselineDirectory, currentDirectory, rollbackDirectory)):
        found.append("the installed extension directory evidence is incomplete")
    elif baselineDirectory == currentDirectory or rollbackDirectory != baselineDirectory:
        found.append("the installed extension did not move baseline to current to baseline")
    return found


def selftest() -> int:
    """Prove every continuity field can independently make the gate red."""
    baseline = "1" * 64
    current = "2" * 64
    green: dict[str, Any] = {
        "baselineVersion": BASELINE_VERSION,
        "currentVersion": "0.1.0",
        "session": "019ff27d-e4fe-7fe1-b900-8bdf903628f4",
        "daemonPid": 101,
        "providerPid": 202,
        "baselineDigest": baseline,
        "upgradeDigest": current,
        "rollbackDigest": baseline,
        "workspace": "/isolated/workspace",
        "baselineDirectory": "/extensions/runtrol-0.0.1",
        "currentDirectory": "/extensions/runtrol-0.1.0",
        "rollbackDirectory": "/extensions/runtrol-0.0.1",
    }
    if evidenceProblems(green, "0.1.0", baseline, current):
        print("[vscodeUpgradeRollback --selftest] FAIL. the green fixture was rejected.", file=sys.stderr)
        return 2
    mutations = (
        {**green, "baselineVersion": "0.0.2"},
        {**green, "currentVersion": "0.2.0"},
        {**green, "session": ""},
        {**green, "daemonPid": 0},
        {**green, "providerPid": 101},
        {**green, "baselineDigest": current},
        {**green, "upgradeDigest": baseline},
        {**green, "rollbackDigest": current},
        {**green, "workspace": ""},
        {**green, "currentDirectory": green["baselineDirectory"]},
    )
    for index, mutation in enumerate(mutations, start=1):
        if not evidenceProblems(mutation, "0.1.0", baseline, current):
            print(f"[vscodeUpgradeRollback --selftest] FAIL. mutation {index} escaped.", file=sys.stderr)
            return 2
    print(
        "[vscodeUpgradeRollback --selftest] OK. "
        f"all {len(mutations)} injected continuity defects make the gate red."
    )
    return 0


def runCommand(
    command: list[str],
    cwd: Path,
    environment: dict[str, str] | None = None,
    timeout: int = 600,
) -> subprocess.CompletedProcess[str]:
    """Run one gate command and preserve its output for marker parsing."""
    result = subprocess.run(
        command,
        cwd=cwd,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        timeout=timeout,
    )
    if result.stdout:
        print(result.stdout, end="" if result.stdout.endswith("\n") else "\n")
    if result.stderr:
        print(result.stderr, file=sys.stderr, end="" if result.stderr.endswith("\n") else "\n")
    return result


def npmCommand() -> list[str]:
    """Return npm without asking a shell to interpret product data."""
    npm = shutil.which("npm.cmd" if sys.platform == "win32" else "npm") or shutil.which("npm")
    if npm is None:
        raise RuntimeError("npm is required")
    if sys.platform == "win32":
        command = os.environ.get("ComSpec", r"C:\Windows\System32\cmd.exe")
        return [command, "/d", "/c", npm]
    return [npm]


def buildArtifacts() -> tuple[Path, Path]:
    """Build a native Core, ACP process, and current native package in isolated build output."""
    target = ROOT / "target" / "vscode-upgrade"
    command = [
        "cargo", "build", "-p", "runtrol", "--bin", "runtrol",
        "--target-dir", str(target),
    ]
    built = runCommand(command, ROOT)
    if built.returncode != 0:
        raise RuntimeError(f"{' '.join(command[:3])} returned {built.returncode}")
    suffix = ".exe" if sys.platform == "win32" else ""
    core = target / "debug" / f"runtrol{suffix}"
    fixture = buildFixture()
    environment = dict(os.environ)
    environment["RUNTROL_CORE_BINARY"] = str(core)
    # A rehearsal package belongs in the ignored build tree, not in the release CI's artifact
    # directory: a stray repo-root VSIX blocks every later commit at the hygiene gate.
    packageOutput = target / "release"
    environment["RUNTROL_PACKAGE_OUTPUT_DIR"] = str(packageOutput)
    packaged = runCommand([*npmCommand(), "run", "package:native"], EXTENSION, environment)
    if packaged.returncode != 0:
        raise RuntimeError(f"native package build returned {packaged.returncode}")
    version = currentVersion()
    targetName = f"{sys.platform}-{normalizeArchitecture()}"
    archive = packageOutput / f"runtrol-studio-{version}-{targetName}.vsix"
    for expected in (core, fixture, archive):
        if not expected.is_file():
            raise RuntimeError(f"upgrade rehearsal artifact is missing at {expected}")
    # The exact-archive contract, on the one really built VSIX a local run has. Without this, a packaging
    # drift (2026-08-20: a NOTICE staged without the .txt the contract pins) stays invisible locally and
    # fails six release jobs instead.
    inspected = runCommand(
        [
            sys.executable, "-X", "utf8", str(ROOT / "tests" / "audit" / "vscodePackage.py"),
            "--archive", str(archive), "--target", targetName, "--core", str(core),
        ],
        ROOT,
    )
    if inspected.returncode != 0:
        raise RuntimeError("the rehearsal VSIX violates the exact package contract")
    return archive, fixture


def buildFixture() -> Path:
    """Build only the external ACP process when a release job already supplied its package."""
    target = ROOT / "target" / "vscode-upgrade"
    command = [
        "cargo", "build", "-p", "runtrol-drivers", "--example", "acpFixture",
        "--target-dir", str(target),
    ]
    built = runCommand(command, ROOT)
    if built.returncode != 0:
        raise RuntimeError(f"{' '.join(command[:3])} returned {built.returncode}")
    suffix = ".exe" if sys.platform == "win32" else ""
    fixture = target / "debug" / "examples" / f"acpFixture{suffix}"
    if not fixture.is_file():
        raise RuntimeError(f"upgrade rehearsal ACP fixture is missing at {fixture}")
    return fixture


def normalizeArchitecture() -> str:
    """Use the architecture vocabulary owned by release-targets.json."""
    machine = os.environ.get("PROCESSOR_ARCHITECTURE", "") if sys.platform == "win32" else os.uname().machine
    lowered = machine.lower()
    if lowered in {"amd64", "x86_64"}:
        return "x64"
    if lowered in {"arm64", "aarch64"}:
        return "arm64"
    raise RuntimeError(f"unsupported upgrade rehearsal architecture {machine}")


def currentVersion() -> str:
    """Read the extension release version from its independent source of truth."""
    policy = json.loads(RELEASE_POLICY.read_text(encoding="utf-8"))
    value = policy.get("version")
    if not isinstance(value, str) or re.fullmatch(r"\d+\.\d+\.\d+", value) is None:
        raise RuntimeError("the extension release policy has no stable semantic version")
    if value == BASELINE_VERSION:
        raise RuntimeError(f"the release version must be newer than the rehearsal baseline {BASELINE_VERSION}")
    return value


def coreEntry(entries: list[zipfile.ZipInfo]) -> str:
    """Find the one packaged Core without deriving a provider or operating-system contract."""
    matches = [entry.filename for entry in entries if entry.filename.startswith("extension/resources/core/")]
    if len(matches) != 1:
        raise RuntimeError(f"expected one packaged Core, found {len(matches)}")
    return matches[0]


def baselineArchive(current: Path, output: Path) -> tuple[str, str]:
    """Create an executable older package with distinct Core bytes from the verified current archive."""
    with zipfile.ZipFile(current) as source:
        entries = source.infolist()
        core = coreEntry(entries)
        bodies = {entry.filename: source.read(entry) for entry in entries}
    package = json.loads(bodies["extension/package.json"])
    package["version"] = BASELINE_VERSION
    bodies["extension/package.json"] = json.dumps(package, separators=(",", ":")).encode("utf-8")
    manifest = ElementTree.fromstring(bodies["extension.vsixmanifest"])
    namespace = {"v": "http://schemas.microsoft.com/developer/vsx-schema/2011"}
    identity = manifest.find("v:Metadata/v:Identity", namespace)
    if identity is None:
        raise RuntimeError("the current VSIX has no extension identity")
    identity.set("Version", BASELINE_VERSION)
    bodies["extension.vsixmanifest"] = ElementTree.tostring(manifest, encoding="utf-8", xml_declaration=True)
    bodies[core] += b"\0RUNTROL_BASELINE_CORE_IMAGE\0"

    with zipfile.ZipFile(output, "w") as target:
        for original in entries:
            cloned = copy.copy(original)
            target.writestr(cloned, bodies[original.filename])
    return hashlib.sha256(bodies[core]).hexdigest(), core


def archiveCoreDigest(archive: Path, core: str) -> str:
    """Hash the exact current Core carried by the native package."""
    with zipfile.ZipFile(archive) as packaged:
        return hashlib.sha256(packaged.read(core)).hexdigest()


def nodeCommand() -> list[str]:
    """Wrap the Extension Host journey in a virtual display only where required."""
    command = [
        shutil.which("node") or "node",
        str(EXTENSION / "tooling" / "upgrade-rollback.mjs"),
    ]
    if sys.platform.startswith("linux") and not os.environ.get("DISPLAY"):
        xvfb = shutil.which("xvfb-run")
        if xvfb is None:
            raise RuntimeError("xvfb-run is required to test VS Code without a Linux display")
        return [xvfb, "-a", *command]
    return command


def exercise(archive: Path | None) -> dict[str, Any]:
    """Run the installed baseline to current to baseline lifecycle."""
    current = archive.resolve() if archive else None
    if current is None:
        current, fixture = buildArtifacts()
    else:
        if not current.is_file():
            raise RuntimeError(f"current VSIX is missing at {current}")
        fixture = buildFixture()
    version = currentVersion()
    with tempfile.TemporaryDirectory(prefix="runtrol-vscode-baseline-") as raw:
        baseline = Path(raw) / f"runtrol-studio-{BASELINE_VERSION}.vsix"
        baselineDigest, core = baselineArchive(current, baseline)
        currentDigest = archiveCoreDigest(current, core)
        command = [
            *nodeCommand(),
            str(baseline),
            str(current),
            BASELINE_VERSION,
            version,
            str(fixture),
        ]
        result = runCommand(command, ROOT, timeout=600)
        if result.returncode != 0:
            raise RuntimeError(f"installed upgrade and rollback journey returned {result.returncode}")
        records = [
            line[len(RESULT_MARKER):]
            for line in result.stdout.splitlines()
            if line.startswith(RESULT_MARKER)
        ]
        if len(records) != 1:
            raise RuntimeError(f"expected one {RESULT_MARKER.strip()} record, found {len(records)}")
        value = json.loads(records[0])
        if not isinstance(value, dict):
            raise RuntimeError("the upgrade journey result is not an object")
        found = evidenceProblems(value, version, baselineDigest, currentDigest)
        if found:
            raise RuntimeError("; ".join(found))
        return value


def run(archive: Path | None) -> int:
    """Execute the live product journey and report its exact continuity evidence."""
    try:
        value = exercise(archive)
    except (OSError, RuntimeError, ValueError, json.JSONDecodeError, zipfile.BadZipFile) as error:
        print(f"[vscodeUpgradeRollback] FAIL. {error}", file=sys.stderr)
        return 2
    print(
        "[vscodeUpgradeRollback] OK. installed VSIX upgrade and rollback preserved session "
        f"{value['session']}, daemon {value['daemonPid']}, provider {value['providerPid']}, and exact Core digests."
    )
    return 0


def main(argv: list[str]) -> int:
    """Select selftest, a supplied package, or a locally built native package."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--archive", type=Path)
    arguments = parser.parse_args(argv)
    if arguments.selftest:
        if arguments.archive:
            parser.error("--selftest cannot be combined with --archive")
        return selftest()
    return run(arguments.archive)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
