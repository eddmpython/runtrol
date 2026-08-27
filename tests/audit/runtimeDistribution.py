"""Gate: standalone Runtime release wiring and archive allowlist."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGE_TOOL = ROOT / ".github" / "scripts" / "release" / "runtimePackage.py"
WORKFLOW = ROOT / ".github" / "workflows" / "runtime-release.yml"


def packageModule():
    specification = importlib.util.spec_from_file_location("runtimePackage", PACKAGE_TOOL)
    if specification is None or specification.loader is None:
        raise RuntimeError("cannot load Runtime package tool")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def sourceProblems(tool: str, workflow: str) -> list[str]:
    found: list[str] = []
    requiredTool = (
        "release-targets.json",
        "runtime.schema.json",
        "SCHEMA_VERSION",
        "providerBinariesBundled",
        "providerCredentialsBundled",
        "consumerApplicationBundled",
        "GitHub Sigstore artifact attestation",
        "SHA256SUMS",
        "ZIP_TIME",
        "os.replace(temporary, output)",
        "runtime.install.json",
        "Runtime locator exists",
        "runtimeOwnedStateRemoved",
        "providerStateTouched",
        "sdkArtifacts",
        "pythonArtifacts",
        "PYTHON_WHEEL_GLOBS",
    )
    for token in requiredTool:
        if token not in tool:
            found.append(f"runtimePackage.py is missing release contract {token}")
    for forbidden in ("runtrol-gui", "provider credential file", "provider transcript"):
        if forbidden in tool:
            found.append(f"runtimePackage.py contains forbidden package surface {forbidden}")

    requiredWorkflow = (
        "name: runtime-release",
        "cargo build --release -p runtrol --bin runtrol --target-dir target/runtime-release",
        "Refuse a runner that differs from the release SSOT",
        "runtimePackage.py package",
        "runtimePackage.py inspect",
        "runtimePackage.py manifest",
        "actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6",
        "subject-path:",
        "attestations: write",
        "id-token: write",
        "Refuse an incomplete Runtime platform set",
        "runtimeRustClientSdk.py",
        "runtimeClientSdk.py",
        "runtimePythonClientSdk.py",
        "maturin==1.12.6",
        "cp311-abi3",
        "runtrol-runtime-protocol-${{ needs.prepare.outputs.version }}.crate",
        "runtrol-runtime-client-${{ needs.prepare.outputs.version }}.tgz",
        "Refuse an incomplete SDK artifact set",
        "Refuse an incomplete Python wheel set or any sdist",
        "needs: [prepare, package, sdk, python_wheel]",
        "gh release create runtime-v",
        "Publish Python client to PyPI with Trusted Publishing",
        "pypa/gh-action-pypi-publish@dc37677b2e1c63e2034f94d8a5b11f265b73ba33",
        "name: pypi",
    )
    for token in requiredWorkflow:
        if token not in workflow:
            found.append(f"runtime-release.yml is missing release contract {token}")
    for revision in re.findall(r"uses:\s+[^@\s]+@([^\s#]+)", workflow):
        if not re.fullmatch(r"[0-9a-f]{40}", revision):
            found.append(f"runtime-release.yml uses an unpinned action revision {revision}")
    if "inputs.release" not in workflow:
        found.append("runtime-release.yml cannot separate verification from release publication")
    return found


def selftest() -> int:
    package = packageModule()
    with tempfile.TemporaryDirectory(prefix="runtrol-runtime-distribution-") as scratchText:
        scratch = Path(scratchText)
        binary = scratch / "runtrol.exe"
        binary.write_bytes(b"MZ" + b"r" * (1024 * 1024))
        archive = scratch / package.archiveName("win32-x64")
        package.writeArchive("win32-x64", binary, archive)
        if package.archiveProblems(archive, "win32-x64", binary):
            print("[runtimeDistribution --selftest] green archive was rejected", file=sys.stderr)
            return 2

        mutations: list[Path] = []
        mutations.append(mutate(archive, scratch / "missing.zip", remove="LICENSE"))
        mutations.append(mutate(archive, scratch / "extra.zip", add=("provider-cli.exe", b"bad")))
        mutations.append(mutate(archive, scratch / "stub.zip", replace=("runtrol.exe", b"MZ")))
        mutations.append(mutate(archive, scratch / "unsafe.zip", add=("../escape", b"bad")))
        manifest = archiveEntry(archive, "manifest.json")
        changed = json.loads(manifest)
        changed["providerCredentialsBundled"] = True
        mutations.append(
            mutate(
                archive,
                scratch / "credentials.zip",
                replace=("manifest.json", json.dumps(changed).encode()),
            )
        )
        for index, changedArchive in enumerate(mutations, start=1):
            try:
                problems = package.archiveProblems(changedArchive, "win32-x64")
            except ValueError:
                problems = ["unsafe archive"]
            if not problems:
                print(
                    f"[runtimeDistribution --selftest] mutation {index} escaped",
                    file=sys.stderr,
                )
                return 2
        installedJourney(package, scratch)
        releaseManifestJourney(package, scratch)

    cleanTool = " ".join(
        (
            "release-targets.json runtime.schema.json SCHEMA_VERSION",
            "providerBinariesBundled providerCredentialsBundled consumerApplicationBundled",
            "GitHub Sigstore artifact attestation SHA256SUMS ZIP_TIME",
            "os.replace(temporary, output) runtime.install.json Runtime locator exists",
            "runtimeOwnedStateRemoved providerStateTouched",
            "sdkArtifacts pythonArtifacts PYTHON_WHEEL_GLOBS",
        )
    )
    cleanWorkflow = """
name: runtime-release
inputs.release
cargo build --release -p runtrol --bin runtrol --target-dir target/runtime-release
Refuse a runner that differs from the release SSOT
runtimePackage.py package
runtimePackage.py inspect
runtimePackage.py manifest
uses: actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6
subject-path:
attestations: write
id-token: write
Refuse an incomplete Runtime platform set
runtimeRustClientSdk.py
runtimeClientSdk.py
runtimePythonClientSdk.py
maturin==1.12.6
cp311-abi3
runtrol-runtime-protocol-${{ needs.prepare.outputs.version }}.crate
runtrol-runtime-client-${{ needs.prepare.outputs.version }}.tgz
Refuse an incomplete SDK artifact set
Refuse an incomplete Python wheel set or any sdist
needs: [prepare, package, sdk, python_wheel]
gh release create runtime-v
Publish Python client to PyPI with Trusted Publishing
uses: pypa/gh-action-pypi-publish@dc37677b2e1c63e2034f94d8a5b11f265b73ba33
name: pypi
"""
    if sourceProblems(cleanTool, cleanWorkflow):
        print("[runtimeDistribution --selftest] green source fixture was rejected", file=sys.stderr)
        return 2
    if not sourceProblems(cleanTool, cleanWorkflow.replace("subject-path:", "subject:")):
        print("[runtimeDistribution --selftest] unsigned source mutation escaped", file=sys.stderr)
        return 2
    print("[runtimeDistribution --selftest] OK. archive and signing mutations fail closed.")
    return 0


def releaseManifestJourney(package, scratch: Path) -> None:
    release = scratch / "release"
    release.mkdir()
    for target in package.targets():
        (release / package.archiveName(target)).write_bytes(f"runtime:{target}".encode())
    version = package.workspaceVersion()
    for name in (
        f"runtrol-runtime-protocol-{version}.crate",
        f"runtrol-runtime-client-{version}.crate",
        f"runtrol-runtime-client-{version}.tgz",
    ):
        (release / name).write_bytes(f"sdk:{name}".encode())
    wheelNames = {
        "darwin-arm64": f"runtrol_runtime_client-{version}-cp311-abi3-macosx_11_0_arm64.whl",
        "darwin-x64": f"runtrol_runtime_client-{version}-cp311-abi3-macosx_10_12_x86_64.whl",
        "linux-arm64": f"runtrol_runtime_client-{version}-cp311-abi3-manylinux_2_17_aarch64.manylinux2014_aarch64.whl",
        "linux-x64": f"runtrol_runtime_client-{version}-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl",
        "win32-arm64": f"runtrol_runtime_client-{version}-cp311-abi3-win_arm64.whl",
        "win32-x64": f"runtrol_runtime_client-{version}-cp311-abi3-win_amd64.whl",
    }
    for target, name in wheelNames.items():
        (release / name).write_bytes(f"python:{target}".encode())
    manifest = package.releaseManifest(release)
    if (
        len(manifest.get("artifacts", [])) != 6
        or len(manifest.get("sdkArtifacts", [])) != 3
        or len(manifest.get("pythonArtifacts", [])) != 6
    ):
        raise RuntimeError("release manifest does not cover every Runtime and SDK artifact")
    (release / wheelNames["win32-x64"]).unlink()
    missingWheelRejected = False
    try:
        package.releaseManifest(release)
    except FileNotFoundError:
        missingWheelRejected = True
    if not missingWheelRejected:
        raise RuntimeError("release manifest accepted a missing Python wheel")
    (release / wheelNames["win32-x64"]).write_bytes(b"python:win32-x64")
    (release / f"runtrol-runtime-client-{version}.tgz").unlink()
    try:
        package.releaseManifest(release)
    except FileNotFoundError:
        return
    raise RuntimeError("release manifest accepted a missing SDK artifact")


def installedJourney(package, scratch: Path) -> None:
    if sys.platform == "win32":
        target = "win32-x64"
        executable = "runtrol.exe"
    elif sys.platform == "darwin":
        target = "darwin-x64"
        executable = "runtrol"
    else:
        target = "linux-x64"
        executable = "runtrol"
    binary = scratch / executable
    binary.write_bytes((b"MZ" if sys.platform == "win32" else b"\x7fELF") + b"i" * (1024 * 1024))
    binary.chmod(0o755)
    archive = scratch / package.archiveName(target)
    package.writeArchive(target, binary, archive)
    unpacked = scratch / "unpacked"
    with zipfile.ZipFile(archive) as opened:
        opened.extractall(unpacked)
    root = unpacked / package.rootName(target)
    environment = os.environ.copy()
    if sys.platform == "win32":
        local = scratch / "local"
        environment["LOCALAPPDATA"] = str(local)
        runChecked(
            [
                shutil.which("powershell") or "powershell",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-File",
                str(root / "install.ps1"),
            ],
            environment,
        )
        installed = local / "RuntrolRuntime" / "versions" / package.workspaceVersion() / executable
        if installed.read_bytes() != binary.read_bytes():
            raise RuntimeError("the Windows installer changed Runtime bytes")
        locator = local / "runtrol" / "runtime.locator.json"
        locator.write_text("{}", encoding="utf-8")
        runExpectFailure(
            [
                shutil.which("powershell") or "powershell",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-File",
                str(root / "uninstall.ps1"),
            ],
            environment,
        )
        if not installed.is_file():
            raise RuntimeError("the Windows uninstaller mutated a running installation")
        locator.unlink()
        result = runChecked(
            [
                shutil.which("powershell") or "powershell",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-File",
                str(root / "uninstall.ps1"),
            ],
            environment,
        )
        if (local / "RuntrolRuntime").exists():
            raise RuntimeError("the Windows uninstaller left Runtrol-owned state")
        verifyUninstallResult(result)
        return

    home = scratch / "home"
    installRoot = scratch / "install" / "runtrol"
    binRoot = scratch / "bin"
    stateRoot = scratch / "state"
    home.mkdir()
    environment.update(
        {
            "HOME": str(home),
            "XDG_STATE_HOME": str(stateRoot),
            "RUNTROL_INSTALL_ROOT": str(installRoot),
            "RUNTROL_BIN_DIR": str(binRoot),
        }
    )
    runChecked(["sh", str(root / "install.sh")], environment)
    installed = installRoot / "versions" / package.workspaceVersion() / executable
    if installed.read_bytes() != binary.read_bytes():
        raise RuntimeError("the Unix installer changed Runtime bytes")
    expectedState = home / "Library" / "Application Support" / "runtrol" if sys.platform == "darwin" else stateRoot / "runtrol"
    locator = expectedState / "runtime.locator.json"
    locator.write_text("{}", encoding="utf-8")
    runExpectFailure(["sh", str(root / "uninstall.sh")], environment)
    if not installed.is_file():
        raise RuntimeError("the Unix uninstaller mutated a running installation")
    locator.unlink()
    result = runChecked(["sh", str(root / "uninstall.sh")], environment)
    if installRoot.exists() or expectedState.exists() or (binRoot / "runtrol").exists():
        raise RuntimeError("the Unix uninstaller left Runtrol-owned state")
    verifyUninstallResult(result)


def runChecked(command: list[str], environment: dict[str, str]) -> str:
    completed = subprocess.run(
        command,
        check=False,
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=30,
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()
        raise RuntimeError(f"isolated installer command failed: {detail}")
    return completed.stdout


def runExpectFailure(command: list[str], environment: dict[str, str]) -> None:
    completed = subprocess.run(
        command,
        check=False,
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=30,
    )
    if completed.returncode == 0 or "Runtime locator exists" not in (completed.stderr + completed.stdout):
        raise RuntimeError("uninstall did not fail closed around a running Runtime locator")


def verifyUninstallResult(output: str) -> None:
    lines = [line for line in output.splitlines() if line.strip()]
    try:
        result = json.loads(lines[-1])
    except (IndexError, json.JSONDecodeError) as error:
        raise RuntimeError("uninstall emitted no machine-verifiable result") from error
    if result != {
        "schema": 1,
        "status": "removed",
        "runtimeOwnedStateRemoved": True,
        "providerStateTouched": False,
    }:
        raise RuntimeError(f"uninstall emitted an unexpected result: {result}")


def mutate(
    source: Path,
    output: Path,
    *,
    remove: str | None = None,
    add: tuple[str, bytes] | None = None,
    replace: tuple[str, bytes] | None = None,
) -> Path:
    with zipfile.ZipFile(source) as original, zipfile.ZipFile(output, "w") as changed:
        for item in original.infolist():
            name = Path(item.filename).name
            if name == remove:
                continue
            body = replace[1] if replace and name == replace[0] else original.read(item)
            changed.writestr(item, body)
        if add:
            prefix = original.namelist()[0].split("/", maxsplit=1)[0]
            changed.writestr(f"{prefix}/{add[0]}", add[1])
    return output


def archiveEntry(archive: Path, name: str) -> bytes:
    with zipfile.ZipFile(archive) as opened:
        matched = next(item for item in opened.namelist() if Path(item).name == name)
        return opened.read(matched)


def run(archive: Path | None, target: str | None, binary: Path | None) -> int:
    try:
        tool = PACKAGE_TOOL.read_text(encoding="utf-8")
        workflow = WORKFLOW.read_text(encoding="utf-8")
        found = sourceProblems(tool, workflow)
        package = packageModule()
        releaseTargets = package.targets()
        if set(releaseTargets) != {
            "darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64", "win32-arm64", "win32-x64"
        }:
            found.append("the standalone Runtime does not share the exact six native target classes")
        if archive:
            if not target:
                found.append("archive inspection requires one exact target")
            else:
                found += package.archiveProblems(archive, target, binary)
    except (OSError, RuntimeError, ValueError, KeyError, zipfile.BadZipFile) as error:
        found = [f"cannot inspect Runtime distribution: {error}"]
    if found:
        print("[runtimeDistribution] FAIL. standalone Runtime release defects:", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[runtimeDistribution] OK. six native packages, allowlist, checksums, and provenance are wired.")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--binary", type=Path)
    arguments = parser.parse_args(argv)
    if arguments.selftest:
        if arguments.archive or arguments.target or arguments.binary:
            parser.error("--selftest cannot be combined with archive arguments")
        return selftest()
    if arguments.target and not arguments.archive:
        parser.error("--target requires --archive")
    if arguments.binary and not arguments.archive:
        parser.error("--binary requires --archive")
    return run(
        arguments.archive.resolve() if arguments.archive else None,
        arguments.target,
        arguments.binary.resolve() if arguments.binary else None,
    )


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
