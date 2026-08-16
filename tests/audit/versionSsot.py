"""Gate: shipped versions have one source and Studio stays on an exact patch-only release line.

The checked-in VS Code manifest keeps the neutral ``0.0.0`` placeholder. Packaging replaces that value only in
temporary staging by reading ``release-policy.json``. Cargo members independently inherit the Runtime and SDK version
from ``[workspace.package].version``. The extension policy fixes the 0.1 line and the changelog proves that no patch
release was skipped.
"""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
RELEASE_HEADING = re.compile(
    r"^## \[((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))\]",
    re.MULTILINE,
)
EXPECTED_EXTENSION_POLICY_FIELDS = {
    "major": 0,
    "minor": 1,
    "initialPatch": 0,
    "increment": 1,
    "tagPrefix": "vscode-v",
}


def versionProblems(
    workspaceText: str,
    members: dict[str, str],
    extension: dict[str, object],
    lock: dict[str, object],
    derivation: str,
    policy: dict[str, object],
    changelog: str,
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
    extensionVersion = policy.get("version")
    policyFields = {key: value for key, value in policy.items() if key != "version"}
    policyMatches = set(policyFields) == set(EXPECTED_EXTENSION_POLICY_FIELDS) and all(
        type(policyFields[key]) is type(expected) and policyFields[key] == expected
        for key, expected in EXPECTED_EXTENSION_POLICY_FIELDS.items()
    )
    if not policyMatches:
        found.append("the extension release policy must stay on 0.1.x with exact patch increments")
    if (
        not isinstance(extensionVersion, str)
        or not SEMVER.fullmatch(extensionVersion)
        or extensionVersion == "0.0.0"
    ):
        found.append("the extension release policy has no publishable version")
    elif policyMatches:
        major, minor, patch = (int(part) for part in extensionVersion.split("."))
        if (major, minor) != (policy["major"], policy["minor"]) or patch < policy["initialPatch"] + 1:
            found.append("the extension release version must be 0.1.1 or a later 0.1.x patch")
        else:
            expectedHistory = [
                f"{major}.{minor}.{candidate}"
                for candidate in range(patch, policy["initialPatch"] - 1, -policy["increment"])
            ]
            if RELEASE_HEADING.findall(changelog) != expectedHistory:
                found.append("the changelog release history must descend one 0.1.x patch at a time")
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
        'sourceManifest.version !== "0.0.0"',
        "version: extensionReleasePolicy.version",
        "release-policy.json",
        "previousExtensionReleaseTag",
    ):
        if token not in derivation:
            found.append(f"the VS Code derivation module is missing {token}")
    return found


def selftest() -> int:
    """Prove every duplicated-version mutation makes the contract red."""
    workspace = '[workspace]\nmembers = ["one"]\n[workspace.package]\nversion = "2.3.4"\n'
    member = '[package]\nname = "one"\nversion.workspace = true\n'
    extension = {"version": "0.0.0"}
    lock = {"version": "0.0.0", "packages": {"": {"version": "0.0.0"}}}
    derivation = (
        'sourceManifest.version !== "0.0.0" version: extensionReleasePolicy.version '
        "release-policy.json previousExtensionReleaseTag"
    )
    policy = {"version": "0.1.2", **EXPECTED_EXTENSION_POLICY_FIELDS}
    changelog = "## [Unreleased]\n\n## [0.1.2]\n\n## [0.1.1]\n\n## [0.1.0]\n"
    if versionProblems(workspace, {"one": member}, extension, lock, derivation, policy, changelog):
        print("[versionSsot --selftest] FAIL. the valid fixture was rejected.", file=sys.stderr)
        return 2

    members = {"one": member}
    mutations = (
        (
            workspace.replace('version = "2.3.4"', 'version = "0.0.0"'),
            members,
            extension,
            lock,
            derivation,
            policy,
            changelog,
        ),
        (
            workspace,
            {"one": member.replace("version.workspace = true", 'version = "2.3.4"')},
            extension,
            lock,
            derivation,
            policy,
            changelog,
        ),
        (workspace, members, {"version": "0.1.2"}, lock, derivation, policy, changelog),
        (
            workspace,
            members,
            extension,
            {"version": "0.1.2", "packages": {}},
            derivation,
            policy,
            changelog,
        ),
        (
            workspace,
            members,
            extension,
            lock,
            derivation.replace("previousExtensionReleaseTag", ""),
            policy,
            changelog,
        ),
        (workspace, members, extension, lock, derivation, {**policy, "version": "0.2.0"}, changelog),
        (workspace, members, extension, lock, derivation, {**policy, "minor": 2}, changelog),
        (workspace, members, extension, lock, derivation, {**policy, "increment": True}, changelog),
        (
            workspace,
            members,
            extension,
            lock,
            derivation,
            policy,
            changelog.replace("## [0.1.1]\n\n", ""),
        ),
    )
    for index, mutation in enumerate(mutations, start=1):
        if not versionProblems(*mutation):
            print(f"[versionSsot --selftest] FAIL. mutation {index} escaped.", file=sys.stderr)
            return 2
    print("[versionSsot --selftest] OK. nine source, series, and sequence mutations make the gate red.")
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
        json.loads((extensionRoot / "release-policy.json").read_text(encoding="utf-8")),
        (ROOT / "CHANGELOG.md").read_text(encoding="utf-8"),
    )
    if found:
        print("[versionSsot] FAIL. release version ownership violations:", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    version = workspace["workspace"]["package"]["version"]
    extensionVersion = json.loads((extensionRoot / "release-policy.json").read_text(encoding="utf-8"))["version"]
    print(
        f"[versionSsot] OK. {len(members)} Cargo members derive {version}; Studio derives {extensionVersion} "
        "and is locked to exact 0.1.x patch increments."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest() if "--selftest" in sys.argv[1:] else run())
