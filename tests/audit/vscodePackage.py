"""Gate: a platform VSIX contains one matching Core and only release files.

The ordinary gate validates the release SSOT and packaging wiring without paying the release LTO cost. A release job
adds ``--archive`` and ``--core`` to inspect the actual VSIX bytes before installation or publication.

Usage::

    python -X utf8 tests/audit/vscodePackage.py --selftest
    python -X utf8 tests/audit/vscodePackage.py
    python -X utf8 tests/audit/vscodePackage.py --archive release/package.vsix --target win32-x64 --core target/release/runtrol.exe
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import zipfile
from pathlib import Path
from typing import NamedTuple
from xml.etree import ElementTree

ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"
PACKAGE_PATH = EXTENSION / "package.json"
TARGETS_PATH = EXTENSION / "release-targets.json"
SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
EXPECTED_TARGETS = {
    "darwin-arm64",
    "darwin-x64",
    "linux-arm64",
    "linux-x64",
    "win32-arm64",
    "win32-x64",
}


class ArchiveEntry(NamedTuple):
    """One uncompressed archive entry and its Unix mode."""

    body: bytes
    mode: int


def sourceProblems(
    package: dict[str, object],
    targets: dict[str, object],
    packageScript: str,
    buildScript: str,
    ignore: str,
    installedVerifierExists: bool,
) -> list[str]:
    """Return release-wiring defects that do not require a built binary."""
    found: list[str] = []
    version = package.get("version")
    if not isinstance(version, str) or not SEMVER.fullmatch(version) or version == "0.0.0":
        found.append("the extension version must be a publishable major.minor.patch other than 0.0.0")
    if package.get("publisher") != "eddmpython" or package.get("name") != "runtrol-studio":
        found.append("the public extension identity changed")
    if package.get("license") != "SEE LICENSE IN resources/LICENSE":
        found.append("the extension manifest does not point to the packaged repository license")
    scripts = package.get("scripts")
    if not isinstance(scripts, dict) or scripts.get("package:native") != "node tooling/package.mjs":
        found.append("package:native is not the release packaging entry point")
    if set(targets) != EXPECTED_TARGETS:
        found.append("release-targets.json does not name the six supported native Marketplace targets")
    for target, contract in targets.items():
        expected = "runtrol.exe" if target.startswith("win32-") else "runtrol"
        if not isinstance(contract, dict) or contract.get("executable") != expected:
            found.append(f"{target} has the wrong Core executable name")
            continue
        family = "windows" if target.startswith("win32-") else "macos" if target.startswith("darwin-") else "linux"
        if contract.get("family") != family:
            found.append(f"{target} has the wrong runner family")
        runner = contract.get("runner")
        if not isinstance(runner, str) or not runner:
            found.append(f"{target} has no native release runner")
    requiredPackageTokens = (
        "packageManifest.version",
        "release-targets.json",
        "target !== nativeTarget",
        '"--no-dependencies"',
        "await rm(coreDirectory",
    )
    for token in requiredPackageTokens:
        if token not in packageScript:
            found.append(f"package.mjs is missing release contract {token}")
    if 'path.join(repositoryRoot, "LICENSE")' not in buildScript:
        found.append("build.mjs does not copy the repository license into package resources")
    for token in ("tooling/**", "src/**", "node_modules/**", "performance-budget.json", "release-targets.json"):
        if token not in ignore:
            found.append(f".vscodeignore does not exclude {token}")
    if not installedVerifierExists:
        found.append("the installed-package verifier is missing")
    return found


def expectedEntries(target: str) -> set[str]:
    """Return the complete platform archive allowlist."""
    executable = "runtrol.exe" if target.startswith("win32-") else "runtrol"
    return {
        "[Content_Types].xml",
        "extension.vsixmanifest",
        "extension/package.json",
        "extension/readme.md",
        "extension/dist/extension.js",
        "extension/dist/webview.css",
        "extension/dist/webview.js",
        "extension/resources/LICENSE.txt",
        "extension/resources/icon.png",
        "extension/resources/symbol.svg",
        f"extension/resources/core/{executable}",
    }


def archiveProblems(
    entries: dict[str, ArchiveEntry],
    target: str,
    expectedVersion: str,
    licenseBytes: bytes,
    coreBytes: bytes | None,
) -> list[str]:
    """Return every content, identity, target, and binary mismatch in one VSIX."""
    found: list[str] = []
    names = set(entries)
    expected = expectedEntries(target)
    for name in sorted(expected - names):
        found.append(f"the VSIX is missing {name}")
    for name in sorted(names - expected):
        found.append(f"the VSIX contains forbidden file {name}")
    for name in names:
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or "\\" in name:
            found.append(f"the VSIX contains unsafe path {name}")

    packageEntry = entries.get("extension/package.json")
    if packageEntry:
        try:
            package = json.loads(packageEntry.body)
        except (UnicodeDecodeError, json.JSONDecodeError):
            found.append("extension/package.json is not valid UTF-8 JSON")
        else:
            if package.get("name") != "runtrol-studio" or package.get("publisher") != "eddmpython":
                found.append("the packaged extension identity is wrong")
            if package.get("version") != expectedVersion:
                found.append("the packaged extension version differs from the release SSOT")
            if package.get("license") != "SEE LICENSE IN resources/LICENSE":
                found.append("the packaged extension license pointer is wrong")

    manifestEntry = entries.get("extension.vsixmanifest")
    if manifestEntry:
        try:
            root = ElementTree.fromstring(manifestEntry.body)
        except ElementTree.ParseError:
            found.append("extension.vsixmanifest is not valid XML")
        else:
            namespace = {"v": "http://schemas.microsoft.com/developer/vsx-schema/2011"}
            identity = root.find("v:Metadata/v:Identity", namespace)
            if identity is None:
                found.append("the VSIX manifest has no identity")
            else:
                if identity.get("Id") != "runtrol-studio" or identity.get("Publisher") != "eddmpython":
                    found.append("the VSIX manifest identity is wrong")
                if identity.get("Version") != expectedVersion:
                    found.append("the VSIX manifest version differs from the release SSOT")
                if identity.get("TargetPlatform") != target:
                    found.append("the VSIX manifest target differs from its release target")

    licenseEntry = entries.get("extension/resources/LICENSE.txt")
    if licenseEntry and licenseEntry.body != licenseBytes:
        found.append("the packaged license differs from the repository license")
    executable = "runtrol.exe" if target.startswith("win32-") else "runtrol"
    coreEntry = entries.get(f"extension/resources/core/{executable}")
    if coreEntry:
        if len(coreEntry.body) < 1024 * 1024:
            found.append("the packaged Core is too small to be the product binary")
        if coreBytes is not None and hashlib.sha256(coreEntry.body).digest() != hashlib.sha256(coreBytes).digest():
            found.append("the packaged Core bytes differ from the verified release binary")
        if not target.startswith("win32-") and coreEntry.mode & 0o111 == 0:
            found.append("the packaged Unix Core is not executable")
    return found


def selftest() -> int:
    """Prove each archive and release-source defect can make the gate red."""
    version = "0.1.0"
    target = "win32-x64"
    licenseBytes = b"license\n"
    coreBytes = b"MZ" + b"x" * (1024 * 1024)
    manifest = (
        '<PackageManifest xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">'
        '<Metadata><Identity Id="runtrol-studio" Version="0.1.0" Publisher="eddmpython" '
        'TargetPlatform="win32-x64"/></Metadata></PackageManifest>'
    ).encode()
    package = json.dumps(
        {
            "name": "runtrol-studio",
            "publisher": "eddmpython",
            "version": version,
            "license": "SEE LICENSE IN resources/LICENSE",
        }
    ).encode()
    entries = {
        name: ArchiveEntry(b"content", 0o644)
        for name in expectedEntries(target)
    }
    entries["extension/package.json"] = ArchiveEntry(package, 0o644)
    entries["extension.vsixmanifest"] = ArchiveEntry(manifest, 0o644)
    entries["extension/resources/LICENSE.txt"] = ArchiveEntry(licenseBytes, 0o644)
    entries["extension/resources/core/runtrol.exe"] = ArchiveEntry(coreBytes, 0o644)
    if archiveProblems(entries, target, version, licenseBytes, coreBytes):
        print("[vscodePackage --selftest] FAIL. the green archive was rejected.", file=sys.stderr)
        return 2

    mutations: list[dict[str, ArchiveEntry]] = []
    missingCore = dict(entries)
    missingCore.pop("extension/resources/core/runtrol.exe")
    mutations.append(missingCore)
    withSource = dict(entries)
    withSource["extension/src/extension.ts"] = ArchiveEntry(b"source", 0o644)
    mutations.append(withSource)
    wrongPackage = dict(entries)
    wrongPackage["extension/package.json"] = ArchiveEntry(package.replace(b"0.1.0", b"0.2.0"), 0o644)
    mutations.append(wrongPackage)
    wrongManifest = dict(entries)
    wrongManifest["extension.vsixmanifest"] = ArchiveEntry(
        manifest.replace(b"win32-x64", b"linux-x64"), 0o644
    )
    mutations.append(wrongManifest)
    wrongLicense = dict(entries)
    wrongLicense["extension/resources/LICENSE.txt"] = ArchiveEntry(b"different", 0o644)
    mutations.append(wrongLicense)
    stubCore = dict(entries)
    stubCore["extension/resources/core/runtrol.exe"] = ArchiveEntry(b"MZ", 0o644)
    mutations.append(stubCore)
    unsafePath = dict(entries)
    unsafePath["../escape"] = ArchiveEntry(b"bad", 0o644)
    mutations.append(unsafePath)
    changedCore = dict(entries)
    changedCore["extension/resources/core/runtrol.exe"] = ArchiveEntry(coreBytes + b"changed", 0o644)
    mutations.append(changedCore)
    for index, mutation in enumerate(mutations, start=1):
        if not archiveProblems(mutation, target, version, licenseBytes, coreBytes):
            print(f"[vscodePackage --selftest] FAIL. archive mutation {index} escaped.", file=sys.stderr)
            return 2

    sourcePackage = {
        "name": "runtrol-studio",
        "publisher": "eddmpython",
        "version": version,
        "license": "SEE LICENSE IN resources/LICENSE",
        "scripts": {"package:native": "node tooling/package.mjs"},
    }
    targets = {}
    for name in EXPECTED_TARGETS:
        family = "windows" if name.startswith("win32-") else "macos" if name.startswith("darwin-") else "linux"
        targets[name] = {
            "executable": "runtrol.exe" if name.startswith("win32-") else "runtrol",
            "family": family,
            "runner": f"{family}-native",
        }
    packageScript = "packageManifest.version release-targets.json target !== nativeTarget \"--no-dependencies\" await rm(coreDirectory"
    buildScript = 'path.join(repositoryRoot, "LICENSE")'
    ignore = "tooling/** src/** node_modules/** performance-budget.json release-targets.json"
    if sourceProblems(sourcePackage, targets, packageScript, buildScript, ignore, True):
        print("[vscodePackage --selftest] FAIL. the green source contract was rejected.", file=sys.stderr)
        return 2
    brokenSource = dict(sourcePackage)
    brokenSource["version"] = "0.0.0"
    if not sourceProblems(brokenSource, targets, packageScript, buildScript, ignore, True):
        print("[vscodePackage --selftest] FAIL. development version escaped.", file=sys.stderr)
        return 2
    print("[vscodePackage --selftest] OK. eight archives and one source mutation make the gate red.")
    return 0


def readArchive(path: Path) -> tuple[dict[str, ArchiveEntry], list[str]]:
    """Read a VSIX once, rejecting duplicate names before a dictionary can hide them."""
    entries: dict[str, ArchiveEntry] = {}
    duplicates: list[str] = []
    with zipfile.ZipFile(path) as archive:
        for info in archive.infolist():
            if info.is_dir():
                continue
            if info.filename in entries:
                duplicates.append(info.filename)
            entries[info.filename] = ArchiveEntry(archive.read(info), info.external_attr >> 16)
    return entries, duplicates


def sourceRun() -> int:
    """Validate the fast, checked-in release wiring."""
    package = json.loads(PACKAGE_PATH.read_text(encoding="utf-8"))
    targets = json.loads(TARGETS_PATH.read_text(encoding="utf-8"))
    found = sourceProblems(
        package,
        targets,
        (EXTENSION / "tooling" / "package.mjs").read_text(encoding="utf-8"),
        (EXTENSION / "tooling" / "build.mjs").read_text(encoding="utf-8"),
        (EXTENSION / ".vscodeignore").read_text(encoding="utf-8"),
        (EXTENSION / "tooling" / "installed-package.mjs").is_file()
        and (EXTENSION / "src" / "integration" / "installedPackage.test.ts").is_file(),
    )
    if found:
        return report("vscodePackage", found)
    print(f"[vscodePackage] OK. release {package['version']} and six native targets are wired.")
    return 0


def archiveRun(archive: Path, target: str, core: Path | None) -> int:
    """Validate one built platform package against its exact release binary."""
    if target not in EXPECTED_TARGETS:
        print(f"[vscodePackage] FAIL. unsupported target {target}.", file=sys.stderr)
        return 2
    try:
        entries, duplicates = readArchive(archive)
        package = json.loads(PACKAGE_PATH.read_text(encoding="utf-8"))
        coreBytes = core.read_bytes() if core else None
        found = [f"the VSIX contains duplicate file {name}" for name in duplicates]
        found += archiveProblems(
            entries,
            target,
            package["version"],
            (ROOT / "LICENSE").read_bytes(),
            coreBytes,
        )
    except (OSError, KeyError, ValueError, zipfile.BadZipFile, json.JSONDecodeError) as error:
        print(f"[vscodePackage] FAIL. cannot inspect {archive}: {error}", file=sys.stderr)
        return 2
    if found:
        return report("vscodePackage", found)
    coreNote = " and exact Core bytes" if core else ""
    print(f"[vscodePackage] OK. {target} VSIX allowlist{coreNote} verified.")
    return 0


def report(name: str, found: list[str]) -> int:
    """Print one stable failure block."""
    print(f"[{name}] FAIL. package contract violations:", file=sys.stderr)
    for problem in found:
        print(f"  - {problem}", file=sys.stderr)
    return 2


def main(argv: list[str]) -> int:
    """Select selftest, fast source contract, or built archive inspection."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--core", type=Path)
    arguments = parser.parse_args(argv)
    if arguments.selftest:
        if arguments.archive or arguments.target or arguments.core:
            parser.error("--selftest cannot be combined with archive arguments")
        return selftest()
    if arguments.archive:
        if not arguments.target:
            parser.error("--archive requires --target")
        return archiveRun(arguments.archive.resolve(), arguments.target, arguments.core.resolve() if arguments.core else None)
    if arguments.target or arguments.core:
        parser.error("--target and --core require --archive")
    return sourceRun()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
