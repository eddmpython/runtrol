"""Gate: every shipped component derives its release version from Cargo workspace metadata.

The checked-in VS Code manifest keeps the neutral ``0.0.0`` placeholder. Packaging replaces that value only in
temporary staging by reading ``[workspace.package].version``. Cargo members inherit the same value directly.
"""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")


def versionProblems(
    workspaceText: str,
    members: dict[str, str],
    extension: dict[str, object],
    lock: dict[str, object],
    derivation: str,
) -> list[str]:
    """Return every version ownership violation in the supplied source snapshot."""
    found: list[str] = []
    try:
        workspace = tomllib.loads(workspaceText)
        declaredMembers = workspace["workspace"]["members"]
        version = workspace["workspace"]["package"]["version"]
    except (KeyError, tomllib.TOMLDecodeError):
        return ["Cargo.toml has no readable workspace package version and member list"]

    if not isinstance(version, str) or not SEMVER.fullmatch(version) or version == "0.0.0":
        found.append("the workspace package version is not a publishable major.minor.patch")
    if not isinstance(declaredMembers, list) or any(not isinstance(member, str) for member in declaredMembers):
        found.append("the workspace member list is not a string list")
        return found
    if set(declaredMembers) != set(members):
        found.append("the inspected Cargo member set differs from the workspace member list")

    for member, text in sorted(members.items()):
        try:
            inherited = tomllib.loads(text)["package"]["version"]
        except (KeyError, tomllib.TOMLDecodeError):
            found.append(f"{member}/Cargo.toml has no package version")
            continue
        if inherited != {"workspace": True}:
            found.append(f"{member}/Cargo.toml does not inherit the workspace version")

    if extension.get("version") != "0.0.0":
        found.append("the checked-in VS Code manifest does not use the 0.0.0 derivation placeholder")
    rootLock = lock.get("packages")
    lockedExtension = rootLock.get("") if isinstance(rootLock, dict) else None
    if lock.get("version") != "0.0.0" or not isinstance(lockedExtension, dict) or lockedExtension.get("version") != "0.0.0":
        found.append("the VS Code lockfile does not use the 0.0.0 derivation placeholder")
    for token in (
        "workspace\\.package",
        'sourceManifest.version !== "0.0.0"',
        "version: workspaceVersion",
    ):
        if token not in derivation:
            found.append(f"the VS Code derivation module is missing {token}")
    return found


def selftest() -> int:
    """Prove every duplicated-version mutation makes the contract red."""
    workspace = '[workspace]\nmembers = ["one"]\n[workspace.package]\nversion = "1.2.3"\n'
    member = '[package]\nname = "one"\nversion.workspace = true\n'
    extension = {"version": "0.0.0"}
    lock = {"version": "0.0.0", "packages": {"": {"version": "0.0.0"}}}
    derivation = 'workspace\\.package sourceManifest.version !== "0.0.0" version: workspaceVersion'
    if versionProblems(workspace, {"one": member}, extension, lock, derivation):
        print("[versionSsot --selftest] FAIL. the valid fixture was rejected.", file=sys.stderr)
        return 2

    mutations = (
        (workspace.replace('version = "1.2.3"', 'version = "0.0.0"'), {"one": member}, extension, lock, derivation),
        (workspace, {"one": member.replace("version.workspace = true", 'version = "1.2.3"')}, extension, lock, derivation),
        (workspace, {"one": member}, {"version": "1.2.3"}, lock, derivation),
        (workspace, {"one": member}, extension, {"version": "1.2.3", "packages": {}}, derivation),
        (workspace, {"one": member}, extension, lock, derivation.replace("version: workspaceVersion", "")),
    )
    for index, mutation in enumerate(mutations, start=1):
        if not versionProblems(*mutation):
            print(f"[versionSsot --selftest] FAIL. mutation {index} escaped.", file=sys.stderr)
            return 2
    print("[versionSsot --selftest] OK. five duplicated-version mutations make the gate red.")
    return 0


def run() -> int:
    """Inspect the checked-in release version graph."""
    workspacePath = ROOT / "Cargo.toml"
    workspaceText = workspacePath.read_text(encoding="utf-8")
    workspace = tomllib.loads(workspaceText)
    members = {
        member: (ROOT / member / "Cargo.toml").read_text(encoding="utf-8")
        for member in workspace["workspace"]["members"]
    }
    extensionRoot = ROOT / "extensions" / "runtrol-vscode"
    found = versionProblems(
        workspaceText,
        members,
        json.loads((extensionRoot / "package.json").read_text(encoding="utf-8")),
        json.loads((extensionRoot / "package-lock.json").read_text(encoding="utf-8")),
        (extensionRoot / "tooling" / "extension-manifest.mjs").read_text(encoding="utf-8"),
    )
    if found:
        print("[versionSsot] FAIL. release version ownership violations:", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    version = workspace["workspace"]["package"]["version"]
    print(f"[versionSsot] OK. {len(members)} Cargo members and the VS Code package derive release {version}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest() if "--selftest" in sys.argv[1:] else run())
