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
    matrixTokens = (
        "matrix: ${{ fromJSON(needs.prepare.outputs.packageMatrix) }}",
        "matrix: ${{ fromJSON(needs.prepare.outputs.matrix) }}",
    )
    if not any(token in job for token in matrixTokens):
        found.append("the package matrix is missing a prepare-owned target matrix")
    for token in ("runs-on: ${{ matrix.runner }}",):
        if token not in job:
            found.append(f"the package matrix is missing {token}")

    stepMatch = re.search(
        r"(?ms)^      - name: Install the package and complete the shared first-run journey\r?\n"
        r"(?P<body>.*?)(?=^      - |\Z)",
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


def workflowJobBody(workflow: str, jobName: str) -> str:
    """Return one exact top-level workflow job body, or an empty string."""
    match = re.search(
        rf"(?ms)^  {re.escape(jobName)}:\r?\n(?P<body>.*?)(?=^  [A-Za-z0-9_-]+:\r?\n|\Z)",
        workflow,
    )
    return "" if match is None else match.group("body")


def workflowStepBody(jobBody: str, stepName: str) -> str:
    """Return one named workflow step body, or an empty string."""
    match = re.search(
        rf"(?ms)^      - name: {re.escape(stepName)}\r?\n"
        rf"(?P<body>.*?)(?=^      - |\Z)",
        jobBody,
    )
    return "" if match is None else match.group("body")


def activeWorkflowCommandPosition(stepBody: str, command: str) -> int:
    """Return one exact active run-command position, or -1 for missing or duplicate lines."""
    matches = tuple(
        re.finditer(rf"(?m)^          {re.escape(command)}\r?$", stepBody)
    )
    return matches[0].start() if len(matches) == 1 else -1


def replaceWorkflowStepToken(
    workflow: str,
    jobName: str,
    stepName: str,
    old: str,
    new: str = "",
    occurrence: int = 1,
) -> str:
    """Replace one token only inside one named workflow step for fault injection."""
    jobMatch = re.search(
        rf"(?ms)^  {re.escape(jobName)}:\r?\n(?P<body>.*?)(?=^  [A-Za-z0-9_-]+:\r?\n|\Z)",
        workflow,
    )
    if jobMatch is None:
        raise AssertionError(f"missing workflow job {jobName}")
    jobBody = jobMatch.group("body")
    stepMatch = re.search(
        rf"(?ms)^      - name: {re.escape(stepName)}\r?\n"
        rf"(?P<body>.*?)(?=^      - |\Z)",
        jobBody,
    )
    if stepMatch is None:
        raise AssertionError(f"missing workflow step {jobName}/{stepName}")
    stepBody = stepMatch.group("body")
    matches = tuple(re.finditer(re.escape(old), stepBody))
    if occurrence < 1 or len(matches) < occurrence:
        raise AssertionError(
            f"missing mutation token occurrence {occurrence} in {jobName}/{stepName}: {old}"
        )
    tokenMatch = matches[occurrence - 1]
    mutatedStep = f"{stepBody[:tokenMatch.start()]}{new}{stepBody[tokenMatch.end():]}"
    absoluteStart = jobMatch.start("body") + stepMatch.start("body")
    absoluteEnd = jobMatch.start("body") + stepMatch.end("body")
    return f"{workflow[:absoluteStart]}{mutatedStep}{workflow[absoluteEnd:]}"


def moveWorkflowStepCommandBefore(
    workflow: str,
    jobName: str,
    stepName: str,
    command: str,
    before: str,
) -> str:
    """Move one exact active command before another command for fault injection."""
    jobBody = workflowJobBody(workflow, jobName)
    stepBody = workflowStepBody(jobBody, stepName)
    commandLine = f"          {command}\n"
    beforeLine = f"          {before}\n"
    if stepBody.count(commandLine) != 1 or stepBody.count(beforeLine) != 1:
        raise AssertionError(f"cannot move command in {jobName}/{stepName}")
    mutatedStep = stepBody.replace(commandLine, "", 1).replace(
        beforeLine,
        f"{commandLine}{beforeLine}",
        1,
    )
    stepStart = workflow.index(stepBody, workflow.index(f"  {jobName}:"))
    return f"{workflow[:stepStart]}{mutatedStep}{workflow[stepStart + len(stepBody):]}"


def swapWorkflowStepTokens(
    workflow: str,
    jobName: str,
    stepName: str,
    left: str,
    right: str,
) -> str:
    """Swap two unique step tokens to inject an ordering defect."""
    jobBody = workflowJobBody(workflow, jobName)
    stepBody = workflowStepBody(jobBody, stepName)
    sentinel = "__VSCODE_PACKAGE_AUDIT_SWAP__"
    if (
        stepBody.count(left) != 1
        or stepBody.count(right) != 1
        or sentinel in stepBody
    ):
        raise AssertionError(f"cannot swap tokens in {jobName}/{stepName}")
    mutatedStep = stepBody.replace(left, sentinel).replace(right, left).replace(sentinel, right)
    stepStart = workflow.index(stepBody, workflow.index(f"  {jobName}:"))
    return f"{workflow[:stepStart]}{mutatedStep}{workflow[stepStart + len(stepBody):]}"


def replaceWorkflowJobToken(
    workflow: str,
    jobName: str,
    token: str,
    replacement: str,
) -> str:
    """Replace one unique token inside one workflow job."""
    jobBody = workflowJobBody(workflow, jobName)
    if jobBody.count(token) != 1:
        raise AssertionError(f"cannot replace unique token in {jobName}")
    mutatedJob = jobBody.replace(token, replacement, 1)
    jobStart = workflow.index(jobBody, workflow.index(f"  {jobName}:"))
    return f"{workflow[:jobStart]}{mutatedJob}{workflow[jobStart + len(jobBody):]}"


def releaseGovernanceProblems(releaseWorkflow: str) -> list[str]:
    """Require one exact gated SHA and durable, repairable release staging."""
    found: list[str] = []
    if re.search(r"(?m)^  push:\s*$", releaseWorkflow):
        found.append("vscode-release.yml publishes directly from push instead of completed Gates")
    for token in (
        "github.run_attempt",
        "actions/upload-artifact",
        "actions/download-artifact",
        "gh release create",
    ):
        if token in releaseWorkflow:
            found.append(f"vscode-release.yml uses non-recoverable release transport {token}")
    requiredTokens = (
        "workflow_run:",
        "workflows: [gates]",
        "types: [completed]",
        "queue: max",
        "branches: [main]",
        "actions: read",
        "github.event.workflow_run.conclusion == 'success'",
        "github.event.workflow_run.event == 'push'",
        "github.event.workflow_run.head_branch == 'main'",
        "github.event.workflow_run.path == '.github/workflows/gates.yml'",
        "github.event.workflow_run.head_repository.full_name == github.repository",
        "github.event.workflow_run.head_sha",
        "Select only an exact gated main release commit",
        "['diff-tree', '--root', '--no-commit-id', '--name-only', '-r', releaseSha]",
        "new Set(['CHANGELOG.md', releasePolicy])",
        "actions/workflows/gates.yml/runs",
        "latestRun.conclusion !== 'success'",
        "['merge-base', '--is-ancestor', releaseSha, 'FETCH_HEAD']",
        "Generate and validate the governed release message",
        ".github/scripts/release/releaseNotes.py --selftest",
        "Verify patch-only extension release sequence and tag identity",
        "compareVersions(candidateVersion, currentVersion) > 0",
        "newer extension release tags already exist",
        "runCommand('git', ['cat-file', '-t', tag])",
        "process.env.CREATE_RELEASE === 'true' && tagType !== 'tag'",
        "process.env.CREATE_RELEASE === 'true' && releaseCommit !== releaseSha",
        "tagMessage !== releaseMessage",
        "tagObjectSha: ${{ steps.identity.outputs.tagObjectSha }}",
        "tagObjectSha=${remoteTagObject}",
        "remoteTagObject !== localTagObject",
        "remoteTagCommit !== releaseCommit",
        "else if (process.env.PUBLISH_EXISTING === 'true')",
        "'--cleanup=verbatim'",
        "runCommand('git', ['push', 'origin', `refs/tags/${tag}`])",
        "Create or repair durable draft release staging",
        "id: staging\n        if: needs.route.outputs.createRelease == 'true'",
        "target_commitish: process.env.RELEASE_SHA",
        "name: title,\n                  body: releaseMessage,\n                  draft: true",
        "draft: true",
        "body: releaseMessage",
        "method: 'PATCH'",
        "release.name !== title || release.body !== releaseMessage || release.prerelease",
        "`/repos/${repository}/releases?per_page=100&page=${pageNumber}`",
        "`/repos/${repository}/releases/assets/${asset.id}`",
        "{ method: 'DELETE' }",
        "packageCount=${packageEntries.length}",
        "packageMatrix=${JSON.stringify({ include: packageEntries })}",
        "let archiveIsValid = false",
        "matrix: ${{ fromJSON(needs.prepare.outputs.packageMatrix) }}",
        "needs.prepare.outputs.packageCount != '0'",
        "persist-credentials: false",
        "Reconcile the exact VSIX into durable release staging\n        shell: bash",
        "releaseState.upload_url.replace(/\\{.*\\}$/, '')",
        "const assetLabel = `${assetName} @ ${releaseSha}`",
        "&label=${encodeURIComponent(assetLabel)}",
        "assets[0].label !== assetLabel",
        "'Content-Type': 'application/octet-stream'",
        "body: localBytes",
        "let assetIsReady = false",
        "existingBytes.equals(localBytes)",
        "recoveredBytes.equals(localBytes)",
        "discarding unreadable staging asset",
        "discarding incomplete upload",
        "durable staging bytes differ",
        "needs: [route, prepare, package]",
        "always() &&",
        "needs.prepare.outputs.packageCount == '0'",
        "needs.package.result == 'success'",
        "needs.publish.result == 'success'",
        "needs.marketplace.result == 'success'",
        "Download the exact durable staging asset set",
        "Inspect every staged VSIX before Marketplace publication",
        "Refuse a release commit outside the current main history",
        'git merge-base --is-ancestor "${{ needs.prepare.outputs.releaseCommit }}" FETCH_HEAD',
        "needs.route.outputs.createRelease == 'true'",
        "needs.route.outputs.publishExisting == 'true'",
        "Regenerate the exact tagged release message",
        "Verify the exact tagged release message and asset set",
        'gh release view "$TAG" --json assets,body,isDraft,isPrerelease,name,tagName',
        'state["body"] != expectedMessage',
        'state["isDraft"] or state["isPrerelease"]',
        "actualAssets != expectedAssets",
        "Regenerate the governed message and verify the exact annotated tag",
        "Reconcile and download the exact release before publication",
        "release ${tag} does not have the exact complete asset set",
        "Inspect every release VSIX before making the release public",
        "Publish and verify the exact repaired GitHub release",
        "draft: false",
        "make_latest: 'true'",
        "release.draft ||",
        "release.assets.some((asset) => asset.label !== `${asset.name} @ ${releaseSha}`)",
        "published GitHub release ${tag} did not converge to the exact state",
        "verifiedBytes.equals(localBytes)",
    )
    for token in requiredTokens:
        if token not in releaseWorkflow:
            found.append(f"vscode-release.yml is missing release governance contract {token}")
    publishJob = workflowJobBody(releaseWorkflow, "publish")
    if re.search(r"(?m)^    permissions:\r?$\n^      contents: write\r?$", publishJob) is None:
        found.append("the publish job cannot read the private durable draft release")

    localizedContracts = (
        (
            "prepare",
            "Verify patch-only extension release sequence and tag identity",
            (
                "remoteTagObject !== localTagObject",
                "tagObjectSha=${remoteTagObject}",
            ),
        ),
        (
            "prepare",
            "Create or repair durable draft release staging",
            (
                "target_commitish: process.env.RELEASE_SHA",
                "draft: true",
                "method: 'PATCH'",
                "if (assets.length !== 1 || assets[0].label !== assetLabel)",
                "if (audit.status === 0)",
                "if (!archiveIsValid)",
                "for (const [assetName, assets] of assetsByName)",
                "if (!expectedNames.has(assetName))",
                "packageEntries.push({ target, ...contract })",
                "packageMatrix=${JSON.stringify({ include: packageEntries })}",
            ),
        ),
        (
            "package",
            "Reconcile the exact VSIX into durable release staging",
            (
                ").replace(/\\r\\n/g, '\\n')",
                "matchingAssets[0].label === assetLabel",
                "for (const asset of matchingAssets)",
                "existingBytes.equals(localBytes)",
                "for (const asset of recoveredAssets)",
                "discarding incomplete upload",
                "retrying interrupted upload",
                "verifiedAssets[0].label !== assetLabel",
                "verifiedBytes.equals(localBytes)",
            ),
        ),
        (
            "publish",
            "Download the exact durable staging asset set",
            (
                "tagMessage !== releaseMessage",
                'test "$(git rev-parse "${TAG}^{commit}")" = "$RELEASE_SHA"',
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)" = "$RELEASE_SHA"',
                "release.assets.some((asset) => asset.label !== `${asset.name} @ ${releaseSha}`)",
                "JSON.stringify(actualNames) !== JSON.stringify(expectedNames)",
                "Accept: 'application/octet-stream'",
                "Buffer.from(await response.arrayBuffer())",
            ),
        ),
        (
            "publish",
            "Inspect every staged VSIX before Marketplace publication",
            ('python -X utf8 tests/audit/vscodePackage.py --archive "$archive" --target "$target"',),
        ),
        (
            "publish",
            "Publish and verify all Marketplace platform packages",
            (
                "TAG_OBJECT_SHA: ${{ needs.prepare.outputs.tagObjectSha }}",
                'git fetch --no-tags origin main',
                'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
                'test "$(git rev-parse "$TAG")" = "$TAG_OBJECT_SHA"',
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}" | cut -f1)" = "$TAG_OBJECT_SHA"',
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)" = "$RELEASE_SHA"',
                "node extensions/runtrol-vscode/tooling/publish-marketplace.mjs --directory release",
            ),
        ),
        (
            "release",
            "Regenerate the governed message and verify the exact annotated tag",
            (
                'test "$(git rev-parse "${TAG}^{commit}")" = "$RELEASE_SHA"',
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)" = "$RELEASE_SHA"',
                "if tagMessage != releaseMessage:",
            ),
        ),
        (
            "release",
            "Reconcile and download the exact release before publication",
            (
                "method: 'PATCH'",
                "if (!expectedNameSet.has(asset.name))",
                "{ method: 'DELETE' }",
                "release.assets.some((asset) => asset.label !== `${asset.name} @ ${releaseSha}`)",
                "JSON.stringify(actualNames) !== JSON.stringify(expectedNames)",
                "Buffer.from(await response.arrayBuffer())",
            ),
        ),
        (
            "release",
            "Publish and verify the exact repaired GitHub release",
            (
                "TAG_OBJECT_SHA: ${{ needs.prepare.outputs.tagObjectSha }}",
                'git fetch --no-tags origin main',
                'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
                'test "$(git rev-parse "$TAG")" = "$TAG_OBJECT_SHA"',
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}" | cut -f1)" = "$TAG_OBJECT_SHA"',
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)" = "$RELEASE_SHA"',
                "release.body !== releaseMessage",
                "draft: false",
                "make_latest: 'true'",
                "release.draft ||",
                "JSON.stringify(verifiedNames) !== JSON.stringify(expectedNames)",
                "stagedBytes.equals(localBytes)",
                "Microsoft.VisualStudio.Services.VsixSha256",
                "marketplaceDigest !== localDigest",
                "verifiedBytes.equals(localBytes)",
            ),
        ),
        (
            "release",
            "Inspect every release VSIX before making the release public",
            ('python -X utf8 tests/audit/vscodePackage.py --archive "$archive" --target "$target"',),
        ),
        (
            "publishExisting",
            "Check out the exact tagged release history",
            (
                "fetch-depth: 0",
                "ref: ${{ needs.prepare.outputs.releaseCommit }}",
            ),
        ),
        (
            "publishExisting",
            "Verify the exact tagged release message and asset set",
            (
                'state["body"] != expectedMessage',
                'state["isDraft"] or state["isPrerelease"]',
                "actualAssets != expectedAssets",
            ),
        ),
        (
            "publishExisting",
            "Inspect every downloaded VSIX before publication",
            ('python -X utf8 tests/audit/vscodePackage.py --archive "$archive" --target "$target"',),
        ),
        (
            "publishExisting",
            "Publish and verify the tagged Marketplace platform packages",
            (
                "TAG_OBJECT_SHA: ${{ needs.prepare.outputs.tagObjectSha }}",
                'git fetch --no-tags origin main',
                'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
                'remote_tag_object="$(git ls-remote --tags origin "refs/tags/${TAG}" | cut -f1)"',
                'test "$(git rev-parse "$TAG")" = "$TAG_OBJECT_SHA"',
                'test "$remote_tag_object" = "$TAG_OBJECT_SHA"',
                'test "$remote_tag_commit" = "$RELEASE_SHA"',
                "release.body !== releaseMessage",
                "JSON.stringify(actualNames) !== JSON.stringify(expectedNames)",
                "manualRemoteBytes.equals(localBytes)",
                "node extensions/runtrol-vscode/tooling/publish-marketplace.mjs --directory release",
            ),
        ),
    )
    for jobName, stepName, tokens in localizedContracts:
        jobBody = workflowJobBody(releaseWorkflow, jobName)
        stepBody = workflowStepBody(jobBody, stepName)
        if not stepBody:
            found.append(f"the {jobName} job is missing governed step {stepName}")
            continue
        for token in tokens:
            if token not in stepBody:
                found.append(f"the {jobName} step {stepName} is missing localized contract {token}")

    prepareStaging = workflowStepBody(
        workflowJobBody(releaseWorkflow, "prepare"),
        "Create or repair durable draft release staging",
    )
    invalidIdentityBranch = re.search(
        r"(?ms)^            if \(assets\.length !== 1 \|\| assets\[0\]\.label !== assetLabel\) \{\r?\n"
        r"(?P<body>.*?)(?=^            \})",
        prepareStaging,
    )
    invalidIdentityBody = "" if invalidIdentityBranch is None else invalidIdentityBranch.group("body")
    for token in (
        "await deleteAsset(asset);",
        "packageEntries.push({ target, ...contract });",
        "continue;",
    ):
        if token not in invalidIdentityBody:
            found.append(f"the invalid staging asset identity repair branch is missing {token}")
    invalidArchiveBranch = re.search(
        r"(?ms)^            if \(!archiveIsValid\) \{\r?\n"
        r"(?P<body>.*?)(?=^            \})",
        prepareStaging,
    )
    invalidArchiveBody = "" if invalidArchiveBranch is None else invalidArchiveBranch.group("body")
    for token in (
        "await deleteAsset(assets[0]);",
        "fs.rmSync(archivePath);",
        "packageEntries.push({ target, ...contract });",
    ):
        if token not in invalidArchiveBody:
            found.append(f"the invalid staging archive repair branch is missing {token}")

    packageReconcile = workflowStepBody(
        workflowJobBody(releaseWorkflow, "package"),
        "Reconcile the exact VSIX into durable release staging",
    )
    if packageReconcile.count("await uploadAsset(release)") != 2:
        found.append("the package reconcile step must upload once and retry one interrupted upload")
    if packageReconcile.count("await deleteAsset(asset)") < 2:
        found.append("the package reconcile step must delete duplicate and interrupted assets")

    fetchCommand = "git fetch --no-tags origin main"
    ancestryCommand = 'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD'
    localTagCommand = 'test "$(git rev-parse "$TAG")" = "$TAG_OBJECT_SHA"'
    remoteTagCommand = (
        'test "$(git ls-remote --tags origin "refs/tags/${TAG}" | cut -f1)" = "$TAG_OBJECT_SHA"'
    )
    peeledTagCommand = (
        'test "$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)" = "$RELEASE_SHA"'
    )
    latestTagCommand = (
        r'''latest_release_tag="$(git ls-remote --tags origin 'refs/tags/vscode-v*' | cut -f2 | sed -E 's#^refs/tags/##; s/\^\{\}$//' | grep -E '^vscode-v[0-9]+\.[0-9]+\.[0-9]+$' | sort -uV | tail -n1)"'''
    )
    latestTagGuard = 'test "$latest_release_tag" = "$TAG"'
    marketplacePublisher = (
        "node extensions/runtrol-vscode/tooling/publish-marketplace.mjs --directory release"
    )
    inlineNode = "node --input-type=module <<'NODE'"

    def requireActiveOrder(stepBody: str, commands: tuple[str, ...], label: str) -> list[int]:
        positions = [activeWorkflowCommandPosition(stepBody, command) for command in commands]
        if any(position < 0 for position in positions):
            found.append(f"the {label} boundary is missing one exact active command")
        elif positions != sorted(positions):
            found.append(f"the {label} boundary runs after its external mutation")
        return positions

    automaticPublish = workflowStepBody(
        workflowJobBody(releaseWorkflow, "publish"),
        "Publish and verify all Marketplace platform packages",
    )
    requireActiveOrder(
        automaticPublish,
        (
            fetchCommand,
            ancestryCommand,
            localTagCommand,
            remoteTagCommand,
            peeledTagCommand,
            latestTagCommand,
            latestTagGuard,
            marketplacePublisher,
        ),
        "automatic Marketplace publication",
    )

    manualPublish = workflowStepBody(
        workflowJobBody(releaseWorkflow, "publishExisting"),
        "Publish and verify the tagged Marketplace platform packages",
    )
    manualPositions = requireActiveOrder(
        manualPublish,
        (
            fetchCommand,
            ancestryCommand,
            'remote_tag_object="$(git ls-remote --tags origin "refs/tags/${TAG}" | cut -f1)"',
            localTagCommand,
            'test "$remote_tag_object" = "$TAG_OBJECT_SHA"',
            'remote_tag_commit="$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)"',
            'test "$remote_tag_commit" = "$RELEASE_SHA"',
            latestTagCommand,
            latestTagGuard,
            inlineNode,
            marketplacePublisher,
        ),
        "manual Marketplace publication",
    )
    manualIdentityCheck = manualPublish.find("release.body !== releaseMessage")
    manualByteCheck = manualPublish.find("manualRemoteBytes.equals(localBytes)")
    if not (
        len(manualPositions) == 11
        and 0 <= manualPositions[9] < manualIdentityCheck < manualByteCheck < manualPositions[10]
    ):
        found.append("the manual release must reverify remote identity and bytes before publication")

    finalPublish = workflowStepBody(
        workflowJobBody(releaseWorkflow, "release"),
        "Publish and verify the exact repaired GitHub release",
    )
    finalPositions = requireActiveOrder(
        finalPublish,
        (
            fetchCommand,
            ancestryCommand,
            localTagCommand,
            remoteTagCommand,
            peeledTagCommand,
            latestTagCommand,
            latestTagGuard,
            inlineNode,
        ),
        "GitHub release exposure",
    )
    stagedByteCheck = finalPublish.find("stagedBytes.equals(localBytes)")
    marketplaceDigestCheck = finalPublish.find("marketplaceDigest !== localDigest")
    publicTransition = finalPublish.find("draft: false")
    publishedByteCheck = finalPublish.find("verifiedBytes.equals(localBytes)")
    if not (
        len(finalPositions) == 8
        and 0 <= finalPositions[7] < stagedByteCheck < marketplaceDigestCheck
        < publicTransition < publishedByteCheck
    ):
        found.append(
            "the final release must verify remote and Marketplace bytes before public exposure"
        )

    marketplaceJob = workflowJobBody(releaseWorkflow, "marketplace")
    if "needs: [route, prepare, publish]" not in marketplaceJob:
        found.append("the public Marketplace install gate must depend on publication verification")

    exactCheckout = "ref: ${{ github.event_name == 'workflow_run' && github.event.workflow_run.head_sha || github.sha }}"
    if releaseWorkflow.count(exactCheckout) != 1:
        found.append("the release router must check out the triggering Gates SHA exactly once")
    if releaseWorkflow.find("Generate and validate the governed release message") > releaseWorkflow.find("  publish:"):
        found.append("the governed release message is not prepared before Marketplace publication")
    if releaseWorkflow.count("draft: true") != 1:
        found.append("the exact release must have one durable draft staging creation point")
    if releaseWorkflow.count("draft: false") != 1:
        found.append("the exact release must have one final publication transition")
    if releaseWorkflow.count("'--cleanup=verbatim'") != 1:
        found.append("the annotated tag must consume the governed message exactly once")
    if releaseWorkflow.count("tagMessage !== releaseMessage") != 2:
        found.append("the tag message must be rechecked before Marketplace publication")
    if releaseWorkflow.count("      contents: write") != 4:
        found.append(
            "only prepare, package, staging publish, and final release may access draft release state"
        )
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
        "extensions/runtrol-vscode/release-policy.json",
        "cargo build --release -p runtrol --bin runtrol --target-dir target/vscode-release",
        "--target-dir target/vscode-release",
        "RUNTROL_CORE_BINARY: target/vscode-release/release/${{ matrix.executable }}",
        "tests/audit/vscodeUpgradeRollback.py --archive",
        "fetch-depth: 0",
        "Verify patch-only extension release sequence",
        "extensionReleaseTag",
        "previousExtensionReleaseTag",
        "['show-ref', '--verify', '--quiet', `refs/tags/${tag}`]",
        "['merge-base', '--is-ancestor', previousExtensionReleaseTag, releaseSha]",
        "gnome-keyring-daemon --components=secrets --daemonize --unlock",
        'echo "DBUS_SESSION_BUS_ADDRESS=$dbus_address" >> "$GITHUB_ENV"',
        "RUNTROL_TEST_MACOS_KEYCHAIN=$keychain",
        "refs/heads/main",
        "Refuse an incomplete platform set",
        "VSCE_PAT: ${{ secrets.VSCE_PAT }}",
        "publish-marketplace.mjs",
        "--directory release",
        "Install and activate the public Marketplace release",
        "gh release download",
    )
    for token in requiredWorkflowTokens:
        if token not in releaseWorkflow:
            found.append(f"vscode-release.yml is missing release contract {token}")
    found.extend(releaseGovernanceProblems(releaseWorkflow))
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
        "extension/resources/CODICONS_LICENSE.txt",
        "extension/resources/LICENSE.txt",
        "extension/resources/NOTICE.txt",
        "extension/resources/icon.png",
        "extension/resources/symbol.svg",
        # Deleting a conversation is the one row action that does not come back, so its control is drawn in
        # the editor's error colour. A menu icon cannot be tinted through a theme token the way a tree item
        # can, so the colour is baked into a file the build generates from the pinned glyph set.
        "extension/resources/action-icons/trash.svg",
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
            containers = package.get("contributes", {}).get("viewsContainers", {}).get("activitybar", [])
            runtrolContainer = next(
                (container for container in containers if container.get("id") == "runtrol"),
                None,
            )
            if not isinstance(runtrolContainer, dict) or runtrolContainer.get("title") != f"Runtrol {expectedVersion}":
                found.append("the packaged sidebar header does not carry the exact release version")
            runtrolViews = package.get("contributes", {}).get("views", {}).get("runtrol", [])
            runtrolView = next(
                (view for view in runtrolViews if view.get("id") == "runtrol.sidebar"),
                None,
            )
            if not isinstance(runtrolView, dict) or runtrolView.get("name") != f"Runtrol {expectedVersion}":
                found.append("the packaged sidebar view does not merge into its versioned header")

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
            "contributes": {
                "viewsContainers": {
                    "activitybar": [{"id": "runtrol", "title": f"Runtrol {version}"}],
                },
                "views": {
                    "runtrol": [{"id": "runtrol.sidebar", "name": f"Runtrol {version}"}],
                },
            },
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
    wrongHeader = dict(entries)
    wrongHeader["extension/package.json"] = ArchiveEntry(package.replace(b"Runtrol 0.1.0", b"Runtrol"), 0o644)
    mutations.append(wrongHeader)
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
    releaseWorkflow = (ROOT / ".github" / "workflows" / "vscode-release.yml").read_text(encoding="utf-8")
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
        (sourcePackage, releaseWorkflow.replace("workflow_run:", ""), marketplaceScript, coreManifest),
        (sourcePackage, releaseWorkflow.replace("queue: max", ""), marketplaceScript, coreManifest),
        (
            sourcePackage,
            releaseWorkflow.replace(
                "tagObjectSha: ${{ steps.identity.outputs.tagObjectSha }}",
                "",
            ),
            marketplaceScript,
            coreManifest,
        ),
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
            releaseWorkflow.replace("compareVersions(candidateVersion, currentVersion) > 0", "false"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("process.env.CREATE_RELEASE === 'true' && tagType !== 'tag'", "false"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace(
                "process.env.CREATE_RELEASE === 'true' && releaseCommit !== releaseSha",
                "false",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("remoteTagCommit !== releaseCommit", "false"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("else if (process.env.PUBLISH_EXISTING === 'true')", "else if (false)"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("github.event.workflow_run.conclusion == 'success'", "true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("github.event.workflow_run.head_sha", "github.sha"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("github.event.workflow_run.path == '.github/workflows/gates.yml'", "true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("latestRun.conclusion !== 'success'", "false"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("Generate and validate the governed release message", "Generate release message"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            f"{releaseWorkflow}\ngithub.run_attempt",
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            f"{releaseWorkflow}\nuses: actions/upload-artifact@0000000000000000000000000000000000000000",
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("      contents: write", "      contents: read", 1),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowJobToken(
                releaseWorkflow,
                "publish",
                "      contents: write",
                "      contents: read",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("persist-credentials: false", "persist-credentials: true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("Create or repair durable draft release staging", "Create release"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace(
                "id: staging\n        if: needs.route.outputs.createRelease == 'true'",
                "id: staging\n        if: true",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("'--cleanup=verbatim'", "'--cleanup=strip'"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("tagMessage !== releaseMessage", "false"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("target_commitish: process.env.RELEASE_SHA", "target_commitish: 'main'"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("draft: true", "draft: false"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("body: releaseMessage,", "body: '',", 1),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("method: 'PATCH'", "method: 'GET'"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("`/repos/${repository}/releases/assets/${asset.id}`", "asset.url"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace(
                "matrix: ${{ fromJSON(needs.prepare.outputs.packageMatrix) }}",
                "matrix: ${{ fromJSON(needs.prepare.outputs.matrix) }}",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("releaseState.upload_url.replace(/\\{.*\\}$/, '')", "apiBase"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("&label=${encodeURIComponent(assetLabel)}", ""),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("assets[0].label !== assetLabel", "false"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("body: localBytes", "body: Buffer.alloc(0)"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("let archiveIsValid = false", "let archiveIsValid = true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("let assetIsReady = false", "let assetIsReady = true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("existingBytes.equals(localBytes)", "true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("recoveredBytes.equals(localBytes)", "true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("always() &&", "true &&"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("needs.package.result == 'success'", "true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("needs.publish.result == 'success'", "true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("needs.marketplace.result == 'success'", "true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace(
                "Refuse a release commit outside the current main history",
                "Check release commit",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace(
                'git merge-base --is-ancestor "${{ needs.prepare.outputs.releaseCommit }}" FETCH_HEAD',
                "true",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace('state["body"] != expectedMessage', "False"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace('state["isDraft"] or state["isPrerelease"]', "False"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("actualAssets != expectedAssets", "False"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("draft: false", "draft: true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("make_latest: 'true'", "make_latest: 'legacy'"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("release.draft ||", "false ||"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace(
                "release.assets.some((asset) => asset.label !== `${asset.name} @ ${releaseSha}`)",
                "false",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace("verifiedBytes.equals(localBytes)", "true"),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "prepare",
                "Verify patch-only extension release sequence and tag identity",
                "tagObjectSha=${remoteTagObject}",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "prepare",
                "Create or repair durable draft release staging",
                "for (const [assetName, assets] of assetsByName)",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "prepare",
                "Create or repair durable draft release staging",
                "packageEntries.push({ target, ...contract });",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "prepare",
                "Create or repair durable draft release staging",
                "packageEntries.push({ target, ...contract });",
                occurrence=2,
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "prepare",
                "Create or repair durable draft release staging",
                "if (audit.status === 0)",
                "if (true)",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "prepare",
                "Create or repair durable draft release staging",
                "if (!archiveIsValid)",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Download the exact durable staging asset set",
                'test "$(git rev-parse "${TAG}^{commit}")" = "$RELEASE_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Download the exact durable staging asset set",
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)" = "$RELEASE_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Inspect every staged VSIX before Marketplace publication",
                'python -X utf8 tests/audit/vscodePackage.py --archive "$archive" --target "$target"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Publish and verify all Marketplace platform packages",
                "git fetch --no-tags origin main",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Publish and verify all Marketplace platform packages",
                'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Publish and verify all Marketplace platform packages",
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}" | cut -f1)" = "$TAG_OBJECT_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Publish and verify all Marketplace platform packages",
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)" = "$RELEASE_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            releaseWorkflow.replace(
                "needs: [route, prepare, publish]",
                "needs: [route, prepare]",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Regenerate the governed message and verify the exact annotated tag",
                'test "$(git rev-parse "${TAG}^{commit}")" = "$RELEASE_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Regenerate the governed message and verify the exact annotated tag",
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)" = "$RELEASE_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "package",
                "Reconcile the exact VSIX into durable release staging",
                "for (const asset of matchingAssets)",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Inspect every release VSIX before making the release public",
                'python -X utf8 tests/audit/vscodePackage.py --archive "$archive" --target "$target"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                "git fetch --no-tags origin main",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}" | cut -f1)" = "$TAG_OBJECT_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                'test "$(git ls-remote --tags origin "refs/tags/${TAG}^{}" | cut -f1)" = "$RELEASE_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                "stagedBytes.equals(localBytes)",
                "true",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Check out the exact tagged release history",
                "fetch-depth: 0",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Publish and verify all Marketplace platform packages",
                "TAG_OBJECT_SHA: ${{ needs.prepare.outputs.tagObjectSha }}",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Publish and verify the tagged Marketplace platform packages",
                "TAG_OBJECT_SHA: ${{ needs.prepare.outputs.tagObjectSha }}",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                "TAG_OBJECT_SHA: ${{ needs.prepare.outputs.tagObjectSha }}",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Publish and verify all Marketplace platform packages",
                "node extensions/runtrol-vscode/tooling/publish-marketplace.mjs --directory release",
                "true\n      - run: node extensions/runtrol-vscode/tooling/publish-marketplace.mjs --directory release",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            moveWorkflowStepCommandBefore(
                releaseWorkflow,
                "publish",
                "Publish and verify all Marketplace platform packages",
                "node extensions/runtrol-vscode/tooling/publish-marketplace.mjs --directory release",
                "git fetch --no-tags origin main",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Publish and verify all Marketplace platform packages",
                'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
                '# git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Publish and verify all Marketplace platform packages",
                'test "$latest_release_tag" = "$TAG"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            moveWorkflowStepCommandBefore(
                releaseWorkflow,
                "publishExisting",
                "Publish and verify the tagged Marketplace platform packages",
                "node extensions/runtrol-vscode/tooling/publish-marketplace.mjs --directory release",
                "git fetch --no-tags origin main",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Publish and verify the tagged Marketplace platform packages",
                'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
                '# git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Publish and verify the tagged Marketplace platform packages",
                'test "$latest_release_tag" = "$TAG"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Publish and verify the tagged Marketplace platform packages",
                "manualRemoteBytes.equals(localBytes)",
                "true",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
                '# git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                'test "$latest_release_tag" = "$TAG"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                "marketplaceDigest !== localDigest",
                "false",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            swapWorkflowStepTokens(
                releaseWorkflow,
                "release",
                "Publish and verify the exact repaired GitHub release",
                "stagedBytes.equals(localBytes)",
                "draft: false",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "package",
                "Install the package and complete the shared first-run journey",
                """python -X utf8 tests/audit/crossPlatformMatrix.py
          --archive release/runtrol-studio-${{ needs.prepare.outputs.version }}-${{ matrix.target }}.vsix""",
                """true
      - run: >-
          python -X utf8 tests/audit/crossPlatformMatrix.py
          --archive release/runtrol-studio-${{ needs.prepare.outputs.version }}-${{ matrix.target }}.vsix""",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Inspect every downloaded VSIX before publication",
                'python -X utf8 tests/audit/vscodePackage.py --archive "$archive" --target "$target"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Publish and verify the tagged Marketplace platform packages",
                "git fetch --no-tags origin main",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Publish and verify the tagged Marketplace platform packages",
                'git merge-base --is-ancestor "$RELEASE_SHA" FETCH_HEAD',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Publish and verify the tagged Marketplace platform packages",
                'test "$remote_tag_object" = "$TAG_OBJECT_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publishExisting",
                "Publish and verify the tagged Marketplace platform packages",
                'test "$remote_tag_commit" = "$RELEASE_SHA"',
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "package",
                "Reconcile the exact VSIX into durable release staging",
                """if (!assetIsReady) {
                process.stderr.write(`retrying interrupted upload ${assetName}: ${error.message}\\n`);
                release = await findRelease();
                await uploadAsset(release);
              }""",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "publish",
                "Download the exact durable staging asset set",
                "release.assets.some((asset) => asset.label !== `${asset.name} @ ${releaseSha}`)",
                "false",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "release",
                "Reconcile and download the exact release before publication",
                "if (!expectedNameSet.has(asset.name))",
            ),
            marketplaceScript,
            coreManifest,
        ),
        (
            sourcePackage,
            replaceWorkflowStepToken(
                releaseWorkflow,
                "package",
                "Reconcile the exact VSIX into durable release staging",
                ").replace(/\\r\\n/g, '\\n')",
            ),
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
