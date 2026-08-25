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
PUBLIC_EXTENSION_NAME = "runtrol-studio"
PUBLIC_PUBLISHER = "runtrol"
EXPECTED_TARGETS = {
    "darwin-arm64": {"executable": "runtrol", "family": "macos", "runner": "macos-15"},
    "darwin-x64": {"executable": "runtrol", "family": "macos", "runner": "macos-15-intel"},
    "linux-arm64": {"executable": "runtrol", "family": "linux", "runner": "ubuntu-24.04-arm"},
    "linux-x64": {"executable": "runtrol", "family": "linux", "runner": "ubuntu-24.04"},
    "win32-arm64": {
        "executable": "runtrol.exe",
        "family": "windows",
        "runner": "windows-11-vs2026-arm",
    },
    "win32-x64": {"executable": "runtrol.exe", "family": "windows", "runner": "windows-2025"},
}
EXTENSION_POLICY = json.loads((EXTENSION / "release-policy.json").read_text(encoding="utf-8"))
EXTENSION_VERSION = EXTENSION_POLICY["version"]


class ArchiveEntry(NamedTuple):
    """One uncompressed archive entry and its Unix mode."""

    body: bytes
    mode: int


def releasePackageJourneyProblems(releaseWorkflow: str) -> list[str]:
    """Require the first-run journey inside every job of the six-target package matrix."""
    jobMatch = re.search(
        r"(?ms)^  package:\r?\n(?P<body>.*?)(?=^  [A-Za-z0-9_-]+:\r?\n|\Z)",
        releaseWorkflow,
    )
    if jobMatch is None:
        return ["vscode-release.yml has no package matrix job"]
    job = jobMatch.group("body")
    found: list[str] = []
    for token in (
        "matrix: ${{ fromJSON(needs.prepare.outputs.matrix) }}",
        "runs-on: ${{ matrix.runner }}",
    ):
        if token not in job:
            found.append(f"the package matrix is missing {token}")

    stepMatch = re.search(
        r"(?ms)^      - name: Install the package and complete the shared first-run journey\r?\n"
        r"(?P<body>.*?)(?=^      - (?:name:|uses:)|\Z)",
        job,
    )
    if stepMatch is None:
        found.append("the package matrix has no active shared first-run step")
        return found
    step = stepMatch.group("body")
    if re.search(r"(?m)^        if:", step):
        found.append("the shared first-run step is conditional inside the package matrix")
    for pattern, label in (
        (
            r"(?m)^          python -X utf8 tests/audit/crossPlatformMatrix\.py\s*$",
            "the native first-run gate command",
        ),
        (
            r"(?m)^          --archive release/runtrol-studio-"
            r"\$\{\{ needs\.prepare\.outputs\.version \}\}-"
            r"\$\{\{ matrix\.target \}\}\.vsix\s*$",
            "the exact matrix-target archive",
        ),
    ):
        if re.search(pattern, step) is None:
            found.append(f"the shared first-run step is missing {label}")
    return found


def targetContractProblems(targets: dict[str, object]) -> list[str]:
    """Validate the one exact native executable, family, and hosted runner map."""
    found: list[str] = []
    if set(targets) != set(EXPECTED_TARGETS):
        found.append("release-targets.json does not name the six supported native Marketplace targets")
    for target, expected in EXPECTED_TARGETS.items():
        contract = targets.get(target)
        if not isinstance(contract, dict):
            continue
        if contract.get("executable") != expected["executable"]:
            found.append(f"{target} has the wrong Core executable name")
        if contract.get("family") != expected["family"]:
            found.append(f"{target} has the wrong runner family")
        if contract.get("runner") != expected["runner"]:
            found.append(f"{target} has the wrong native release runner")
    return found


def sourceProblems(
    package: dict[str, object],
    targets: dict[str, object],
    packageScript: str,
    extensionManifestScript: str,
    buildScript: str,
    releaseWorkflow: str,
    marketplaceScript: str,
    coreManifest: str,
    ignore: str,
    installedPackageScript: str,
    installedVerifierExists: bool,
    upgradeVerifierExists: bool,
) -> list[str]:
    """Return release-wiring defects that do not require a built binary."""
    found: list[str] = []
    version = package.get("version")
    if version != "0.0.0":
        found.append("the checked-in extension version must be the derived-version placeholder 0.0.0")
    if not isinstance(EXTENSION_VERSION, str) or not SEMVER.fullmatch(EXTENSION_VERSION):
        found.append("release-policy.json must own one publishable extension version")
    if package.get("publisher") != PUBLIC_PUBLISHER or package.get("name") != PUBLIC_EXTENSION_NAME:
        found.append("the public extension identity changed")
    if package.get("license") != "SEE LICENSE IN resources/LICENSE":
        found.append("the extension manifest does not point to the packaged repository license")
    dependencies = package.get("devDependencies")
    vsceVersion = dependencies.get("@vscode/vsce") if isinstance(dependencies, dict) else None
    if not isinstance(vsceVersion, str) or not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", vsceVersion):
        found.append("the Marketplace publisher CLI is not pinned to one exact version")
    codiconsVersion = dependencies.get("@vscode/codicons") if isinstance(dependencies, dict) else None
    if not isinstance(codiconsVersion, str) or not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", codiconsVersion):
        found.append("the provider glyph font is not pinned to one exact version")
    scripts = package.get("scripts")
    if not isinstance(scripts, dict) or scripts.get("package:native") != "node tooling/package.mjs":
        found.append("package:native is not the release packaging entry point")
    found.extend(targetContractProblems(targets))
    requiredPackageTokens = (
        "packageManifest.version",
        "JSON.stringify(packageManifest",
        "release-targets.json",
        "target !== nativeTarget",
        '"--no-dependencies"',
        "path.resolve(repositoryRoot, process.env.RUNTROL_CORE_BINARY)",
        'mkdtemp(path.join(os.tmpdir(), "runtrol-vsix-"))',
        "cp(source, path.join(stagedCore, targetContract.executable))",
        "await rm(staging",
    )
    for token in requiredPackageTokens:
        if token not in packageScript:
            found.append(f"package.mjs is missing release contract {token}")
    for token in (
        "sourceManifest.version !== \"0.0.0\"",
        "version: extensionReleasePolicy.version",
        "release-policy.json",
        "previousExtensionReleaseTag",
    ):
        if token not in extensionManifestScript:
            found.append(f"extension-manifest.mjs is missing version derivation contract {token}")
    # NOTICE is not decoration here. It carries the agreement for the CA root data the Core embeds,
    # and LICENSE cannot carry it: text beyond the license itself stops scanners from identifying
    # the license at all.
    for name in ("LICENSE", "NOTICE"):
        if f'path.join(repositoryRoot, "{name}")' not in buildScript:
            found.append(f"build.mjs does not copy the repository {name} into package resources")
    for token in (
        'path.join(codicons, "LICENSE")',
        'path.join(codicons, "dist", "codicon.css")',
        'path.join(codicons, "dist", "codicon.ttf")',
        'path.join(codicons, "dist", "codicon.svg")',
        'path.join(repositoryRoot, "crates", "runtrol-drivers", "manifests")',
        'path.join(providerIcons, `${name}.svg`)',
    ):
        if token not in buildScript:
            found.append(f"build.mjs does not package the pinned provider glyph asset {token}")
    requiredWorkflowTokens = (
        "push:",
        "extensions/runtrol-vscode/release-policy.json",
        "github.event_name == 'push' || inputs.release",
        "cargo build --release -p runtrol --bin runtrol --target-dir target/vscode-release",
        "--target-dir target/vscode-release",
        "RUNTROL_CORE_BINARY: target/vscode-release/release/${{ matrix.executable }}",
        "tests/audit/vscodeUpgradeRollback.py --archive",
        "fetch-depth: 0",
        "Verify patch-only extension release sequence",
        "extensionReleaseTag",
        "previousExtensionReleaseTag",
        "['show-ref', '--verify', '--quiet', `refs/tags/${tag}`]",
        "['merge-base', '--is-ancestor', tag, 'HEAD']",
        "gnome-keyring-daemon --components=secrets --daemonize --unlock",
        'echo "DBUS_SESSION_BUS_ADDRESS=$dbus_address" >> "$GITHUB_ENV"',
        "RUNTROL_TEST_MACOS_KEYCHAIN=$keychain",
        "if: inputs.publishExisting",
        "refs/heads/main",
        "Refuse an incomplete platform set",
        "VSCE_PAT: ${{ secrets.VSCE_PAT }}",
        "publish-marketplace.mjs",
        "--directory release",
        "Install and activate the public Marketplace release",
        "gh release download",
        "gh release create",
    )
    for token in requiredWorkflowTokens:
        if token not in releaseWorkflow:
            found.append(f"vscode-release.yml is missing release contract {token}")
    found.extend(releasePackageJourneyProblems(releaseWorkflow))
    for token in ("crates/runtrol-gui", "libwebkit2gtk-4.1-dev"):
        if token in releaseWorkflow:
            found.append(f"vscode-release.yml restores unused desktop release work {token}")
    for token in ("--no-default-features", "--features"):
        if token in releaseWorkflow:
            found.append(f"vscode-release.yml selects a removed Core feature surface: {token}")
    for token in ("--oidc", "--azure-credential", "id-token: write"):
        if token in releaseWorkflow:
            found.append(f"vscode-release.yml contains an unsupported Marketplace credential path: {token}")
    marketplaceSecrets = re.findall(r"secrets\.([A-Za-z0-9_]+)", releaseWorkflow)
    if not marketplaceSecrets or set(marketplaceSecrets) != {"VSCE_PAT"}:
        found.append("vscode-release.yml must use only the VSCE_PAT repository secret for Marketplace publishing")
    requiredMarketplaceTokens = (
        "const VERIFY_DEADLINE_MS = 15 * 60_000;",
        '"publish"',
        '"--skip-duplicate"',
        '"--packagePath"',
        '"show"',
        '"--json"',
        "GITHUB_ACTIONS",
        "GITHUB_REF",
        "GITHUB_REPOSITORY",
        "GITHUB_WORKFLOW_REF",
        "VSCE_PAT",
        "Microsoft.VisualStudio.Services.VsixSha256",
        "packageManifest.version",
        "release-targets.json",
    )
    for token in requiredMarketplaceTokens:
        if token not in marketplaceScript:
            found.append(f"publish-marketplace.mjs is missing automated publication contract {token}")
    for token in ('"--oidc"', '"--pat"', '"--azure-credential"'):
        if token in marketplaceScript:
            found.append(f"publish-marketplace.mjs contains an unsupported Marketplace credential path: {token}")
    for actionRevision in re.findall(r"uses:\s+[^@\s]+@([^\s#]+)", releaseWorkflow):
        if not re.fullmatch(r"[0-9a-f]{40}", actionRevision):
            found.append(f"vscode-release.yml uses an unpinned action revision: {actionRevision}")
    for token in ('default = ["desktop"]', 'desktop = ["dep:runtrol-gui"]', "runtrol-gui"):
        if token in coreManifest:
            found.append(f"runtrol Cargo.toml restores removed standalone GUI contract {token}")
    for token in ("tooling/**", "src/**", "node_modules/**", "performance-budget.json", "release-targets.json"):
        if token not in ignore:
            found.append(f".vscodeignore does not exclude {token}")
    for token in (
        "const MARKETPLACE_INSTALL_DEADLINE_MS = 15 * 60_000;",
        "const PACKAGE_JOURNEY_DEADLINE_MS = 3 * 60_000;",
        "`${extensionIdentifier}@${packageManifest.version}`",
        "findInstalledExtension(extensions, packageManifest.version)",
        "Marketplace did not install",
        '"installed package journey"',
        "await terminateExactProcesses(temporary, managedCore)",
    ):
        if token not in installedPackageScript:
            found.append(f"installed-package.mjs is missing public release contract {token}")
    if not installedVerifierExists:
        found.append("the installed-package verifier is missing")
    if not upgradeVerifierExists:
        found.append("the installed upgrade and rollback verifier is missing")
    return found


def listingProblems(package: dict[str, object], readme: str) -> list[str]:
    """Return Marketplace presentation defects without contacting the Marketplace."""
    found: list[str] = []
    if package.get("displayName") != "Runtrol Studio":
        found.append("the Marketplace display name changed")
    if package.get("homepage") != "https://eddmpython.github.io/runtrol/":
        found.append("the Marketplace homepage is not the public product site")
    if package.get("bugs") != {"url": "https://github.com/eddmpython/runtrol/issues"}:
        found.append("the Marketplace issue link is not the public repository")
    if package.get("pricing") != "Free":
        found.append("the open source extension is not labelled Free")
    if package.get("galleryBanner") != {"color": "#0B0D0F", "theme": "dark"}:
        found.append("the Marketplace banner does not use the canonical graphite contract")
    if package.get("extensionKind") != ["ui"]:
        found.append("the local supervisor is not constrained to the UI extension host")
    expectedCapabilities = {
        "untrustedWorkspaces": {
            "supported": False,
            "description": (
                "Runtrol starts local coding-agent CLI processes that can change the selected repository. "
                "Trust the workspace before opening chats."
            ),
        },
        "virtualWorkspaces": {
            "supported": False,
            "description": (
                "Runtrol requires a local filesystem workspace for provider CLI processes, repositories, and worktrees."
            ),
        },
    }
    if package.get("capabilities") != expectedCapabilities:
        found.append("the Marketplace manifest does not declare exact workspace safety boundaries")
    keywords = package.get("keywords")
    requiredKeywords = {"agent manager", "ai agent", "chat", "cli", "coding agent", "session manager", "worktree"}
    if not isinstance(keywords, list) or not requiredKeywords.issubset(set(keywords)):
        found.append("the Marketplace listing is missing bounded discovery keywords")
    badges = package.get("badges")
    badgeUrls = {
        badge.get("url") for badge in badges if isinstance(badge, dict) and isinstance(badge.get("url"), str)
    } if isinstance(badges, list) else set()
    if badgeUrls != {
        "https://github.com/eddmpython/runtrol/actions/workflows/vscode-release.yml/badge.svg",
        "https://img.shields.io/visual-studio-marketplace/v/runtrol.runtrol-studio",
    }:
        found.append("the Marketplace listing does not expose exact release and version badges")
    requiredReadmeTokens = (
        "# Runtrol Studio for VS Code",
        "## Install",
        "Search for `Runtrol Studio`",
        "@id:runtrol.runtrol-studio",
        "No Core path is required for a Marketplace installation",
        "## Updates",
        "A manually installed VSIX has automatic updates disabled by VS Code",
        "## Requirements",
        "## Troubleshooting",
        "## Ownership and security",
        "https://eddmpython.github.io/runtrol/",
        "https://github.com/eddmpython/runtrol/blob/main/SECURITY.md",
    )
    for token in requiredReadmeTokens:
        if token not in readme:
            found.append(f"the Marketplace README is missing {token}")
    return found


def expectedEntries(target: str) -> set[str]:
    """Return the complete platform archive allowlist."""
    executable = "runtrol.exe" if target.startswith("win32-") else "runtrol"
    provider_icons = {"sparkle"}
    manifests = ROOT / "crates" / "runtrol-drivers" / "manifests"
    for manifest in manifests.glob("*.toml"):
        match = re.search(r'^icon\s*=\s*"([a-z0-9-]{1,64})"\s*$', manifest.read_text(encoding="utf-8"), re.MULTILINE)
        if match:
            provider_icons.add(match.group(1))
    return {
        "[Content_Types].xml",
        "extension.vsixmanifest",
        "extension/package.json",
        "extension/readme.md",
        "extension/dist/codicon.css",
        "extension/dist/codicon.ttf",
        "extension/dist/extension.js",
        "extension/dist/pairingQrVendor.js",
        "extension/dist/usageView.css",
        "extension/dist/usageView.js",
        "extension/resources/CODICONS_LICENSE.txt",
        "extension/resources/LICENSE.txt",
        "extension/resources/NOTICE.txt",
        "extension/resources/icon.png",
        "extension/resources/symbol.svg",
        f"extension/resources/core/{executable}",
    } | {f"extension/resources/provider-icons/{icon}.svg" for icon in provider_icons}


def archiveProblems(
    entries: dict[str, ArchiveEntry],
    target: str,
    expectedVersion: str,
    verbatim: dict[str, bytes],
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
            if package.get("name") != PUBLIC_EXTENSION_NAME or package.get("publisher") != PUBLIC_PUBLISHER:
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
                if identity.get("Id") != PUBLIC_EXTENSION_NAME or identity.get("Publisher") != PUBLIC_PUBLISHER:
                    found.append("the VSIX manifest identity is wrong")
                if identity.get("Version") != expectedVersion:
                    found.append("the VSIX manifest version differs from the release SSOT")
                if identity.get("TargetPlatform") != target:
                    found.append("the VSIX manifest target differs from its release target")

    for archivePath, body in verbatim.items():
        entry = entries.get(archivePath)
        if entry and entry.body != body:
            name = archivePath.rsplit("/", 1)[-1]
            found.append(f"the packaged {name} differs from the repository copy")
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
    verbatim = {
        "extension/resources/LICENSE.txt": b"license\n",
        "extension/resources/NOTICE.txt": b"notice\n",
    }
    coreBytes = b"MZ" + b"x" * (1024 * 1024)
    manifest = (
        '<PackageManifest xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">'
        f'<Metadata><Identity Id="{PUBLIC_EXTENSION_NAME}" Version="0.1.0" Publisher="{PUBLIC_PUBLISHER}" '
        'TargetPlatform="win32-x64"/></Metadata></PackageManifest>'
    ).encode()
    package = json.dumps(
        {
            "name": PUBLIC_EXTENSION_NAME,
            "publisher": PUBLIC_PUBLISHER,
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
    for archivePath, body in verbatim.items():
        entries[archivePath] = ArchiveEntry(body, 0o644)
    entries["extension/resources/core/runtrol.exe"] = ArchiveEntry(coreBytes, 0o644)
    if archiveProblems(entries, target, version, verbatim, coreBytes):
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
    for archivePath in verbatim:
        swapped = dict(entries)
        swapped[archivePath] = ArchiveEntry(b"different", 0o644)
        mutations.append(swapped)
        dropped = dict(entries)
        dropped.pop(archivePath)
        mutations.append(dropped)
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
        if not archiveProblems(mutation, target, version, verbatim, coreBytes):
            print(f"[vscodePackage --selftest] FAIL. archive mutation {index} escaped.", file=sys.stderr)
            return 2

    sourcePackage = {
        "name": PUBLIC_EXTENSION_NAME,
        "displayName": "Runtrol Studio",
        "publisher": PUBLIC_PUBLISHER,
        "version": "0.0.0",
        "license": "SEE LICENSE IN resources/LICENSE",
        "homepage": "https://eddmpython.github.io/runtrol/",
        "bugs": {"url": "https://github.com/eddmpython/runtrol/issues"},
        "pricing": "Free",
        "galleryBanner": {"color": "#0B0D0F", "theme": "dark"},
        "badges": [
            {
                "url": "https://github.com/eddmpython/runtrol/actions/workflows/vscode-release.yml/badge.svg",
            },
            {
                "url": "https://img.shields.io/visual-studio-marketplace/v/runtrol.runtrol-studio",
            },
        ],
        "keywords": [
            "agent manager",
            "ai agent",
            "chat",
            "cli",
            "coding agent",
            "session manager",
            "worktree",
        ],
        "extensionKind": ["ui"],
        "capabilities": {
            "untrustedWorkspaces": {
                "supported": False,
                "description": (
                    "Runtrol starts local coding-agent CLI processes that can change the selected repository. "
                    "Trust the workspace before opening chats."
                ),
            },
            "virtualWorkspaces": {
                "supported": False,
                "description": (
                    "Runtrol requires a local filesystem workspace for provider CLI processes, repositories, and worktrees."
                ),
            },
        },
        "scripts": {"package:native": "node tooling/package.mjs"},
        "devDependencies": {
            "@vscode/codicons": "0.0.46-24",
            "@vscode/vsce": "3.9.3-5",
        },
    }
    listingReadme = """
    # Runtrol Studio for VS Code
    ## Install
    Search for `Runtrol Studio`
    @id:runtrol.runtrol-studio
    No Core path is required for a Marketplace installation
    ## Updates
    A manually installed VSIX has automatic updates disabled by VS Code
    ## Requirements
    ## Troubleshooting
    ## Ownership and security
    https://eddmpython.github.io/runtrol/
    https://github.com/eddmpython/runtrol/blob/main/SECURITY.md
    """
    if listingProblems(sourcePackage, listingReadme):
        print("[vscodePackage --selftest] FAIL. the green Marketplace listing was rejected.", file=sys.stderr)
        return 2
    listingMutations = (
        ({**sourcePackage, "homepage": "https://example.invalid/"}, listingReadme),
        (sourcePackage, listingReadme.replace("## Install", "## Setup")),
        ({**sourcePackage, "extensionKind": ["workspace"]}, listingReadme),
        ({**sourcePackage, "capabilities": {}}, listingReadme),
        ({**sourcePackage, "keywords": ["coding agent"]}, listingReadme),
        (sourcePackage, listingReadme.replace("## Updates", "## Delivery")),
    )
    for index, (mutatedPackage, mutatedReadme) in enumerate(listingMutations, start=1):
        if not listingProblems(mutatedPackage, mutatedReadme):
            print(f"[vscodePackage --selftest] FAIL. listing mutation {index} escaped.", file=sys.stderr)
            return 2
    targets = {name: dict(contract) for name, contract in EXPECTED_TARGETS.items()}
    packageScript = (
        "packageManifest.version JSON.stringify(packageManifest release-targets.json target !== nativeTarget \"--no-dependencies\" "
        "path.resolve(repositoryRoot, process.env.RUNTROL_CORE_BINARY) "
        "path.resolve(repositoryRoot, process.env.RUNTROL_PACKAGE_OUTPUT_DIR) "
        "mkdtemp(path.join(os.tmpdir(), \"runtrol-vsix-\")) "
        "cp(source, path.join(stagedCore, targetContract.executable)) "
        "cp(path.join(extensionRoot, \"resources/provider-icons\"), stagedProviderIcons, { recursive: true }) "
        "cp(path.join(extensionRoot, \"resources/CODICONS_LICENSE.txt\"), "
        "path.join(stagedResources, \"CODICONS_LICENSE.txt\")) await rm(staging"
    )
    extensionManifestScript = (
        'sourceManifest.version !== "0.0.0" version: extensionReleasePolicy.version '
        "release-policy.json previousExtensionReleaseTag"
    )
    buildScript = (
        'path.join(repositoryRoot, "LICENSE") path.join(repositoryRoot, "NOTICE") '
        'path.join(codicons, "LICENSE") path.join(codicons, "dist", "codicon.css") '
        'path.join(codicons, "dist", "codicon.ttf") path.join(codicons, "dist", "codicon.svg") '
        'path.join(repositoryRoot, "crates", "runtrol-drivers", "manifests") '
        'path.join(providerIcons, `${name}.svg`)'
    )
    releaseWorkflow = """
    push:
    extensions/runtrol-vscode/release-policy.json
    github.event_name == 'push' || inputs.release
    cargo build --release -p runtrol --bin runtrol --target-dir target/vscode-release
    RUNTROL_CORE_BINARY: target/vscode-release/release/${{ matrix.executable }}
    tests/audit/vscodeUpgradeRollback.py --archive
    fetch-depth: 0
    Verify patch-only extension release sequence
    extensionReleaseTag
    previousExtensionReleaseTag
    ['show-ref', '--verify', '--quiet', `refs/tags/${tag}`]
    ['merge-base', '--is-ancestor', tag, 'HEAD']
    gnome-keyring-daemon --components=secrets --daemonize --unlock
    echo "DBUS_SESSION_BUS_ADDRESS=$dbus_address" >> "$GITHUB_ENV"
    RUNTROL_TEST_MACOS_KEYCHAIN=$keychain
    if: inputs.publishExisting
    refs/heads/main
    Refuse an incomplete platform set
    VSCE_PAT: ${{ secrets.VSCE_PAT }}
    publish-marketplace.mjs
    --directory release
    Install and activate the public Marketplace release
    gh release download
    gh release create
    uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
  package:
    strategy:
      matrix: ${{ fromJSON(needs.prepare.outputs.matrix) }}
    runs-on: ${{ matrix.runner }}
    steps:
      - name: Install the package and complete the shared first-run journey
        env:
          TMPDIR: ${{ runner.temp }}
        run: >-
          python -X utf8 tests/audit/crossPlatformMatrix.py
          --archive release/runtrol-studio-${{ needs.prepare.outputs.version }}-${{ matrix.target }}.vsix
      - name: Continue release checks
        run: echo checked
    """
    marketplaceScript = """
    const VERIFY_DEADLINE_MS = 15 * 60_000;
    "publish" "--skip-duplicate" "--packagePath"
    "show" "--json"
    GITHUB_ACTIONS GITHUB_REF GITHUB_REPOSITORY GITHUB_WORKFLOW_REF VSCE_PAT
    Microsoft.VisualStudio.Services.VsixSha256
    packageManifest.version release-targets.json
    """
    coreManifest = '[package]\nname = "runtrol"'
    ignore = "tooling/** src/** node_modules/** performance-budget.json release-targets.json"
    installedPackageScript = """
    const MARKETPLACE_INSTALL_DEADLINE_MS = 15 * 60_000;
    const PACKAGE_JOURNEY_DEADLINE_MS = 3 * 60_000;
    `${extensionIdentifier}@${packageManifest.version}`
    findInstalledExtension(extensions, packageManifest.version)
    Marketplace did not install
    "installed package journey"
    await terminateExactProcesses(temporary, managedCore)
    """
    if sourceProblems(
        sourcePackage,
        targets,
        packageScript,
        extensionManifestScript,
        buildScript,
        releaseWorkflow,
        marketplaceScript,
        coreManifest,
        ignore,
        installedPackageScript,
        True,
        True,
    ):
        print("[vscodePackage --selftest] FAIL. the green source contract was rejected.", file=sys.stderr)
        return 2
    brokenSource = dict(sourcePackage)
    brokenSource["version"] = "0.1.0"
    floatingPublisher = {
        **sourcePackage,
        "devDependencies": {
            "@vscode/codicons": "0.0.46-24",
            "@vscode/vsce": "^3.9.3",
        },
    }
    wrongRunnerTargets = {name: dict(contract) for name, contract in targets.items()}
    wrongRunnerTargets["win32-arm64"]["runner"] = "windows-2025"
    targetMutations = (
        {name: contract for name, contract in targets.items() if name != "linux-arm64"},
        wrongRunnerTargets,
    )
    for index, mutatedTargets in enumerate(targetMutations, start=1):
        if not sourceProblems(
            sourcePackage,
            mutatedTargets,
            packageScript,
            extensionManifestScript,
            buildScript,
            releaseWorkflow,
            marketplaceScript,
            coreManifest,
            ignore,
            installedPackageScript,
            True,
            True,
        ):
            print(f"[vscodePackage --selftest] FAIL. target mutation {index} escaped.", file=sys.stderr)
            return 2
    sourceMutations = (
        (sourcePackage, releaseWorkflow.replace("push:", ""), marketplaceScript, coreManifest),
        (
            sourcePackage,
            releaseWorkflow.replace("extensions/runtrol-vscode/release-policy.json", ""),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("Install and activate the public Marketplace release", ""),
            marketplaceScript,
            coreManifest,
        ),
        (brokenSource, releaseWorkflow, marketplaceScript, coreManifest),
        (floatingPublisher, releaseWorkflow, marketplaceScript, coreManifest),
        (
            sourcePackage,
            releaseWorkflow.replace("--bin runtrol", "--bin runtrol --features desktop"),
            marketplaceScript,
            coreManifest,
        ),
        (sourcePackage, releaseWorkflow, marketplaceScript, f'{coreManifest}\ndefault = ["desktop"]'),
        (sourcePackage, f"{releaseWorkflow}\ncrates/runtrol-gui", marketplaceScript, coreManifest),
        (
            sourcePackage,
            releaseWorkflow.replace("tests/audit/vscodeUpgradeRollback.py --archive", ""),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace(
                "python -X utf8 tests/audit/crossPlatformMatrix.py",
                "python -X utf8 tests/audit/vscodePackage.py",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace(
                "      - name: Install the package and complete the shared first-run journey",
                "      - name: Install the package and complete the shared first-run journey\n        if: false",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("previousExtensionReleaseTag", ""),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("gh release create", ""),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("gnome-keyring-daemon --components=secrets --daemonize --unlock", ""),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("RUNTROL_TEST_MACOS_KEYCHAIN=$keychain", ""),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("VSCE_PAT: ${{ secrets.VSCE_PAT }}", ""),
            marketplaceScript,
            coreManifest,
        ),
        (sourcePackage, f"{releaseWorkflow}\nid-token: write", marketplaceScript, coreManifest),
        (sourcePackage, releaseWorkflow, marketplaceScript.replace("VSCE_PAT", ""), coreManifest),
        (
            sourcePackage,
            releaseWorkflow,
            marketplaceScript.replace("Microsoft.VisualStudio.Services.VsixSha256", ""),
            coreManifest,
        ),
        (sourcePackage, releaseWorkflow, f'{marketplaceScript}\n"--oidc"', coreManifest),
        (
            sourcePackage,
            f"{releaseWorkflow}\nuses: actions/setup-node@v7",
            marketplaceScript,
            coreManifest,
        ),
    )
    for index, (mutatedPackage, mutatedWorkflow, mutatedMarketplace, mutatedManifest) in enumerate(
        sourceMutations, start=1
    ):
        if not sourceProblems(
            mutatedPackage,
            targets,
            packageScript,
            extensionManifestScript,
            buildScript,
            mutatedWorkflow,
            mutatedMarketplace,
            mutatedManifest,
            ignore,
            installedPackageScript,
            True,
            True,
        ):
            print(f"[vscodePackage --selftest] FAIL. source mutation {index} escaped.", file=sys.stderr)
            return 2
    installedMutations = (
        installedPackageScript.replace("const MARKETPLACE_INSTALL_DEADLINE_MS = 15 * 60_000;", ""),
        installedPackageScript.replace("const PACKAGE_JOURNEY_DEADLINE_MS = 3 * 60_000;", ""),
        installedPackageScript.replace("`${extensionIdentifier}@${packageManifest.version}`", "extensionIdentifier"),
        installedPackageScript.replace("findInstalledExtension(extensions, packageManifest.version)", ""),
    )
    for index, mutatedInstalledPackage in enumerate(installedMutations, start=1):
        if not sourceProblems(
            sourcePackage,
            targets,
            packageScript,
            extensionManifestScript,
            buildScript,
            releaseWorkflow,
            marketplaceScript,
            coreManifest,
            ignore,
            mutatedInstalledPackage,
            True,
            True,
        ):
            print(f"[vscodePackage --selftest] FAIL. installer mutation {index} escaped.", file=sys.stderr)
            return 2
    print(
        "[vscodePackage --selftest] OK. "
        f"{len(mutations)} archive, {len(targetMutations)} target, {len(sourceMutations)} source, "
        f"{len(installedMutations)} installer, "
        f"and {len(listingMutations)} listing mutations "
        "make the gate red."
    )
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
        (EXTENSION / "tooling" / "extension-manifest.mjs").read_text(encoding="utf-8"),
        (EXTENSION / "tooling" / "build.mjs").read_text(encoding="utf-8"),
        (ROOT / ".github" / "workflows" / "vscode-release.yml").read_text(encoding="utf-8"),
        (EXTENSION / "tooling" / "publish-marketplace.mjs").read_text(encoding="utf-8"),
        (ROOT / "crates" / "runtrol" / "Cargo.toml").read_text(encoding="utf-8"),
        (EXTENSION / ".vscodeignore").read_text(encoding="utf-8"),
        (EXTENSION / "tooling" / "installed-package.mjs").read_text(encoding="utf-8"),
        (EXTENSION / "tooling" / "installed-package.mjs").is_file()
        and (EXTENSION / "src" / "integration" / "installedPackage.test.ts").is_file(),
        (EXTENSION / "tooling" / "upgrade-rollback.mjs").is_file()
        and (EXTENSION / "src" / "integration" / "upgradeRollback.test.ts").is_file(),
    )
    found += listingProblems(package, (EXTENSION / "README.md").read_text(encoding="utf-8"))
    if found:
        return report("vscodePackage", found)
    print(f"[vscodePackage] OK. Studio release {EXTENSION_VERSION} and six native targets are wired.")
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
            EXTENSION_VERSION,
            {
                "extension/resources/LICENSE.txt": (ROOT / "LICENSE").read_bytes(),
                "extension/resources/NOTICE.txt": (ROOT / "NOTICE").read_bytes(),
            },
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
