"""Build and inspect reproducible standalone Runtrol Runtime archives."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import tomllib
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[3]
TARGETS_PATH = ROOT / "extensions" / "runtrol-vscode" / "release-targets.json"
SCHEMA_PATH = ROOT / "crates" / "runtrol-runtime-protocol" / "schema" / "runtime.schema.json"
STORE_SCHEMA_PATH = ROOT / "crates" / "runtrol-store" / "src" / "schema.rs"
ZIP_TIME = (1980, 1, 1, 0, 0, 0)
SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
HASH = re.compile(r"^[0-9a-f]{64}$")
PYTHON_WHEEL_GLOBS = {
    "darwin-arm64": "runtrol_runtime_client-*-cp311-abi3-*macosx*arm64.whl",
    "darwin-x64": "runtrol_runtime_client-*-cp311-abi3-*macosx*x86_64.whl",
    "linux-arm64": "runtrol_runtime_client-*-cp311-abi3-*manylinux*aarch64.whl",
    "linux-x64": "runtrol_runtime_client-*-cp311-abi3-*manylinux*x86_64.whl",
    "win32-arm64": "runtrol_runtime_client-*-cp311-abi3-*win_arm64.whl",
    "win32-x64": "runtrol_runtime_client-*-cp311-abi3-*win_amd64.whl",
}


def workspaceVersion() -> str:
    value = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    if not isinstance(value, str) or not SEMVER.fullmatch(value) or value == "0.0.0":
        raise ValueError("workspace package version is not publishable SemVer")
    return value


def targets() -> dict[str, dict[str, str]]:
    value = json.loads(TARGETS_PATH.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or len(value) != 6:
        raise ValueError("the shared native target manifest must contain six targets")
    return value


def protocolRevisions() -> list[str]:
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    revisions = schema.get("x-runtrol-finalized-revisions")
    if not isinstance(revisions, list) or not revisions or not all(isinstance(one, str) for one in revisions):
        raise ValueError("the public schema has no finalized revision inventory")
    return revisions


def storeSchema() -> int:
    source = STORE_SCHEMA_PATH.read_text(encoding="utf-8")
    matched = re.search(r"pub const SCHEMA_VERSION: u8 = (\d+);", source)
    if not matched:
        raise ValueError("the store schema version is not declared in its source of truth")
    return int(matched.group(1))


def sha256(body: bytes) -> str:
    return hashlib.sha256(body).hexdigest()


def archiveName(target: str) -> str:
    return f"runtrol-runtime-{workspaceVersion()}-{target}.zip"


def rootName(target: str) -> str:
    return f"runtrol-runtime-{workspaceVersion()}-{target}"


def packageEntries(target: str, binary: Path) -> dict[str, tuple[bytes, int]]:
    contract = targets().get(target)
    if not contract:
        raise ValueError(f"unsupported Runtime target {target}")
    expected = contract.get("executable")
    if not isinstance(expected, str) or binary.name != expected:
        raise ValueError(f"the selected binary must be named {expected}")
    binaryBytes = binary.read_bytes()
    if len(binaryBytes) < 1024 * 1024:
        raise ValueError("the selected Runtime binary is too small to be a release build")
    version = workspaceVersion()
    revisions = protocolRevisions()
    schemaVersion = storeSchema()
    executableHash = sha256(binaryBytes)
    manifest = {
        "schema": 1,
        "product": "runtrol-runtime",
        "version": version,
        "target": target,
        "executable": expected,
        "executableSha256": executableHash,
        "administrationExecutable": expected,
        "protocolRevisions": revisions,
        "rollbackSafeStoreSchema": schemaVersion,
        "perUser": True,
        "providerBinariesBundled": False,
        "providerCredentialsBundled": False,
        "consumerApplicationBundled": False,
    }
    entries: dict[str, tuple[bytes, int]] = {
        expected: (binaryBytes, 0o755),
        "LICENSE": ((ROOT / "LICENSE").read_bytes(), 0o644),
        "NOTICE": ((ROOT / "NOTICE").read_bytes(), 0o644),
        "runtime.schema.json": (SCHEMA_PATH.read_bytes(), 0o644),
        "manifest.json": (jsonBytes(manifest), 0o644),
        "README.md": (readme(target).encode(), 0o644),
    }
    if contract.get("family") == "windows":
        entries["install.ps1"] = (windowsInstall(version, target, executableHash).encode(), 0o644)
        entries["uninstall.ps1"] = (windowsUninstall().encode(), 0o644)
    else:
        entries["install.sh"] = (unixInstall(version, target, executableHash).encode(), 0o755)
        entries["uninstall.sh"] = (unixUninstall(contract.get("family") == "macos").encode(), 0o755)
    checksums = "".join(f"{sha256(body)} *{name}\n" for name, (body, _mode) in sorted(entries.items()))
    entries["SHA256SUMS"] = (checksums.encode(), 0o644)
    return entries


def jsonBytes(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")) + "\n").encode()


def readme(target: str) -> str:
    install = ".\\install.ps1" if target.startswith("win32-") else "./install.sh"
    uninstall = ".\\uninstall.ps1" if target.startswith("win32-") else "./uninstall.sh"
    return f"""# Runtrol Runtime

This archive contains the provider-neutral, per-user Runtrol Runtime for `{target}`. It contains one headless
`runtrol` executable, its local administration command surface, the finalized public protocol schema, and exact
install and uninstall metadata. It contains no provider CLI, provider credential, model, conversation, or consumer UI.

Verify the archive provenance with `gh attestation verify` and verify unpacked files against `SHA256SUMS` before
installation.

Run `{install}` from this directory to install the verified version for the current user. Installation does not add a
system service or require administrator rights. It asks the installed Runtime to discover provider command names,
which starts the shared user daemon if it is not already serving and publishes its owner-only locator. The installer
then creates provider-neutral command shims from those runtime-discovered manifests. A new shell that puts the shim
directory before the provider commands makes its original terminal the first viewer of the same daemon-owned PTY
every Runtrol window can attach to.

An already-running provider process that did not pass through a shim is never killed, migrated, or silently resumed.
Runtime may observe its provider-owned process identity, but joins its existing byte stream only when the provider or
terminal host publishes an official attach channel.

Run `{uninstall}` to remove the installed Runtime executable and Runtrol-owned locator, grant, cache, and session
pointer state. Uninstall refuses while a Runtime locator exists. It never reads or removes provider binaries,
provider authentication, or provider-owned conversation state.
"""


def windowsInstall(version: str, target: str, executableHash: str) -> str:
    return f"""param()
$ErrorActionPreference = 'Stop'
$source = Join-Path $PSScriptRoot 'runtrol.exe'
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash.ToLowerInvariant()
if ($actual -ne '{executableHash}') {{ throw 'Runtime executable checksum mismatch' }}
$productRoot = Join-Path $env:LOCALAPPDATA 'RuntrolRuntime'
$versionRoot = Join-Path $productRoot 'versions\\{version}'
$binRoot = Join-Path $productRoot 'bin'
$shimRoot = Join-Path $productRoot 'shims'
New-Item -ItemType Directory -Force -Path $versionRoot,$binRoot,$shimRoot | Out-Null
$installed = Join-Path $versionRoot 'runtrol.exe'
$temporary = "$installed.new-$PID"
Copy-Item -LiteralPath $source -Destination $temporary
Move-Item -Force -LiteralPath $temporary -Destination $installed
$launcher = Join-Path $binRoot 'runtrol.cmd'
$launcherTemporary = "$launcher.new-$PID"
Set-Content -LiteralPath $launcherTemporary -Encoding Ascii -Value ('@echo off' + [Environment]::NewLine + '"' + $installed + '" %*')
Move-Item -Force -LiteralPath $launcherTemporary -Destination $launcher
$stateRoot = Join-Path $env:LOCALAPPDATA 'runtrol'
New-Item -ItemType Directory -Force -Path $stateRoot | Out-Null
$record = [ordered]@{{schema=1;runtimeVersion='{version}';target='{target}';executable=$installed;sha256='{executableHash}'}} | ConvertTo-Json -Compress
$recordPath = Join-Path $stateRoot 'runtime.install.json'
$recordTemporary = "$recordPath.new-$PID"
Set-Content -LiteralPath $recordTemporary -Encoding UTF8 -NoNewline -Value $record
$acl = Get-Acl -LiteralPath $recordTemporary
$acl.SetAccessRuleProtection($true,$false)
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
$rule = New-Object Security.AccessControl.FileSystemAccessRule($identity,'FullControl','Allow')
$acl.SetAccessRule($rule)
Set-Acl -LiteralPath $recordTemporary -AclObject $acl
Move-Item -Force -LiteralPath $recordTemporary -Destination $recordPath
& $installed shims $shimRoot | Out-Null
if ($LASTEXITCODE -ne 0) {{ throw 'Provider command shim installation failed' }}
$profileLocal = [Environment]::GetFolderPath('LocalApplicationData')
$usesProfileLocal = [IO.Path]::GetFullPath($env:LOCALAPPDATA) -ieq [IO.Path]::GetFullPath($profileLocal)
if ($usesProfileLocal) {{
  $userPath = [Environment]::GetEnvironmentVariable('Path','User')
  if ($null -eq $userPath) {{ $userPath = '' }}
  $remaining = @($userPath -split ';' | Where-Object {{ $_ -and $_ -ine $shimRoot -and $_ -ine $binRoot }})
  $nextPath = (@($shimRoot,$binRoot) + $remaining) -join ';'
  [Environment]::SetEnvironmentVariable('Path',$nextPath,'User')
  [Environment]::SetEnvironmentVariable('RUNTROL_PROVIDER_SHIM_PATH',$shimRoot,'User')
  Write-Output "Installed Runtrol Runtime {version}. New terminals route declared provider commands through $shimRoot."
}} else {{
  Write-Output "Installed Runtrol Runtime {version} in a redirected local profile. Prepend $shimRoot and $binRoot to PATH for new terminals."
}}
"""


def windowsUninstall() -> str:
    return """param()
$ErrorActionPreference = 'Stop'
$productRoot = Join-Path $env:LOCALAPPDATA 'RuntrolRuntime'
$stateRoot = Join-Path $env:LOCALAPPDATA 'runtrol'
$locator = Join-Path $stateRoot 'runtime.locator.json'
if (Test-Path -LiteralPath $locator) { throw 'Runtime locator exists. Review active sessions and integrations, stop Runtime, and remove only a verified stale locator before uninstalling.' }
if ((Split-Path -Leaf $productRoot) -ne 'RuntrolRuntime' -or (Split-Path -Leaf $stateRoot) -ne 'runtrol') { throw 'Refusing an unexpected uninstall path' }
$binRoot = Join-Path $productRoot 'bin'
$shimRoot = Join-Path $productRoot 'shims'
$profileLocal = [Environment]::GetFolderPath('LocalApplicationData')
$usesProfileLocal = [IO.Path]::GetFullPath($env:LOCALAPPDATA) -ieq [IO.Path]::GetFullPath($profileLocal)
if ($usesProfileLocal) {
  $userPath = [Environment]::GetEnvironmentVariable('Path','User')
  if ($null -ne $userPath) {
    $remaining = @($userPath -split ';' | Where-Object { $_ -and $_ -ine $shimRoot -and $_ -ine $binRoot })
    [Environment]::SetEnvironmentVariable('Path',($remaining -join ';'),'User')
  }
  if ([Environment]::GetEnvironmentVariable('RUNTROL_PROVIDER_SHIM_PATH','User') -ieq $shimRoot) {
    [Environment]::SetEnvironmentVariable('RUNTROL_PROVIDER_SHIM_PATH',$null,'User')
  }
}
if (Test-Path -LiteralPath $productRoot) { Remove-Item -Recurse -Force -LiteralPath $productRoot }
if (Test-Path -LiteralPath $stateRoot) { Remove-Item -Recurse -Force -LiteralPath $stateRoot }
$result = [ordered]@{schema=1;status='removed';runtimeOwnedStateRemoved=$true;providerStateTouched=$false} | ConvertTo-Json -Compress
Write-Output $result
"""


def unixInstall(version: str, target: str, executableHash: str) -> str:
    return f"""#!/bin/sh
set -eu
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
source_binary="$source_dir/runtrol"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$source_binary" | awk '{{print $1}}')
else
  actual=$(shasum -a 256 "$source_binary" | awk '{{print $1}}')
fi
[ "$actual" = "{executableHash}" ] || {{ echo 'Runtime executable checksum mismatch' >&2; exit 1; }}
product_root="${{RUNTROL_INSTALL_ROOT:-$HOME/.local/share/runtrol}}"
bin_root="${{RUNTROL_BIN_DIR:-$HOME/.local/bin}}"
shim_root="$product_root/shims"
version_root="$product_root/versions/{version}"
mkdir -p "$version_root" "$bin_root" "$shim_root"
temporary="$version_root/runtrol.new-$$"
cp "$source_binary" "$temporary"
chmod 755 "$temporary"
mv -f "$temporary" "$version_root/runtrol"
launcher_temporary="$bin_root/.runtrol.new-$$"
printf '%s\\n' '#!/bin/sh' 'exec "{chr(36)}{{RUNTROL_INSTALL_ROOT:-{chr(36)}HOME/.local/share/runtrol}}/versions/{version}/runtrol" "{chr(36)}@"' > "$launcher_temporary"
chmod 755 "$launcher_temporary"
mv -f "$launcher_temporary" "$bin_root/runtrol"
case "{target}" in
  darwin-*) state_root="$HOME/Library/Application Support/runtrol" ;;
  *) state_root="${{XDG_STATE_HOME:-$HOME/.local/state}}/runtrol" ;;
esac
mkdir -p "$state_root"
record_temporary="$state_root/runtime.install.json.new-$$"
printf '%s' '{{"executable":"'"$version_root/runtrol"'","runtimeVersion":"{version}","schema":1,"sha256":"{executableHash}","target":"{target}"}}' > "$record_temporary"
chmod 600 "$record_temporary"
mv -f "$record_temporary" "$state_root/runtime.install.json"
"$version_root/runtrol" shims "$shim_root" >/dev/null
printf '%s\\n' "Installed Runtrol Runtime {version}. Add these lines to your shell startup file:"
printf '%s\\n' "export RUNTROL_PROVIDER_SHIM_PATH=\"$shim_root\""
printf '%s\\n' "export PATH=\"$shim_root:$bin_root:\\$PATH\""
"""


def unixUninstall(macos: bool) -> str:
    state = 'state_root="$HOME/Library/Application Support/runtrol"' if macos else 'state_root="${XDG_STATE_HOME:-$HOME/.local/state}/runtrol"'
    return f"""#!/bin/sh
set -eu
product_root="${{RUNTROL_INSTALL_ROOT:-$HOME/.local/share/runtrol}}"
bin_root="${{RUNTROL_BIN_DIR:-$HOME/.local/bin}}"
{state}
locator="$state_root/runtime.locator.json"
[ ! -e "$locator" ] || {{ echo 'Runtime locator exists. Review active sessions and integrations, stop Runtime, and remove only a verified stale locator before uninstalling.' >&2; exit 1; }}
[ "${{product_root##*/}}" = 'runtrol' ] || {{ echo 'Refusing an unexpected install path' >&2; exit 1; }}
[ "${{state_root##*/}}" = 'runtrol' ] || {{ echo 'Refusing an unexpected state path' >&2; exit 1; }}
[ ! -L "$product_root" ] || {{ echo 'Refusing a symlinked install root' >&2; exit 1; }}
[ ! -L "$state_root" ] || {{ echo 'Refusing a symlinked state root' >&2; exit 1; }}
rm -f -- "$bin_root/runtrol"
[ ! -d "$product_root" ] || rm -rf -- "$product_root"
[ ! -d "$state_root" ] || rm -rf -- "$state_root"
printf '%s\\n' '{{"providerStateTouched":false,"runtimeOwnedStateRemoved":true,"schema":1,"status":"removed"}}'
"""


def writeArchive(target: str, binary: Path, output: Path) -> None:
    entries = packageEntries(target, binary)
    prefix = rootName(target)
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f"{output.name}.new-{os.getpid()}")
    try:
        with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for name, (body, mode) in sorted(entries.items()):
                information = zipfile.ZipInfo(f"{prefix}/{name}", ZIP_TIME)
                information.create_system = 3
                information.external_attr = (stat.S_IFREG | mode) << 16
                information.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(information, body)
        os.replace(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)


def readArchive(path: Path) -> tuple[str, dict[str, tuple[bytes, int]], list[str]]:
    entries: dict[str, tuple[bytes, int]] = {}
    duplicates: list[str] = []
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        roots = {PurePosixPath(name).parts[0] for name in names if PurePosixPath(name).parts}
        if len(roots) != 1:
            raise ValueError("archive does not have one package root")
        prefix = next(iter(roots))
        for item in archive.infolist():
            parts = PurePosixPath(item.filename).parts
            if len(parts) != 2 or parts[0] != prefix or parts[1] in {"", ".", ".."}:
                raise ValueError(f"unsafe archive path {item.filename}")
            name = parts[1]
            if name in entries:
                duplicates.append(name)
            entries[name] = (archive.read(item), (item.external_attr >> 16) & 0o777)
    return prefix, entries, duplicates


def archiveProblems(path: Path, target: str, binary: Path | None = None) -> list[str]:
    expected = packageEntries(target, binary) if binary else None
    prefix, entries, duplicates = readArchive(path)
    found = [f"duplicate archive entry {name}" for name in duplicates]
    if prefix != rootName(target):
        found.append("archive root differs from the release identity")
    contract = targets()[target]
    names = {
        contract["executable"], "LICENSE", "NOTICE", "runtime.schema.json", "manifest.json",
        "README.md", "SHA256SUMS",
        *( ["install.ps1", "uninstall.ps1"] if contract["family"] == "windows" else ["install.sh", "uninstall.sh"] ),
    }
    found.extend(f"missing archive entry {name}" for name in sorted(names - entries.keys()))
    found.extend(f"forbidden archive entry {name}" for name in sorted(entries.keys() - names))
    try:
        manifest = json.loads(entries["manifest.json"][0])
    except (KeyError, UnicodeDecodeError, json.JSONDecodeError):
        found.append("manifest.json is not valid UTF-8 JSON")
        manifest = {}
    revisions = protocolRevisions()
    expectedManifest = {
        "schema": 1,
        "product": "runtrol-runtime",
        "version": workspaceVersion(),
        "target": target,
        "executable": contract["executable"],
        "administrationExecutable": contract["executable"],
        "protocolRevisions": revisions,
        "rollbackSafeStoreSchema": storeSchema(),
        "perUser": True,
        "providerBinariesBundled": False,
        "providerCredentialsBundled": False,
        "consumerApplicationBundled": False,
    }
    for key, value in expectedManifest.items():
        if manifest.get(key) != value:
            found.append(f"manifest field {key} differs from the release contract")
    executable = entries.get(contract["executable"])
    if executable:
        actualHash = sha256(executable[0])
        if manifest.get("executableSha256") != actualHash:
            found.append("manifest executable checksum differs from archive bytes")
        if len(executable[0]) < 1024 * 1024:
            found.append("archive Runtime executable is a stub")
        if contract["family"] != "windows" and executable[1] & 0o111 == 0:
            found.append("archive Runtime executable is not executable")
    checksumLines = entries.get("SHA256SUMS", (b"", 0))[0].decode("utf-8", errors="replace").splitlines()
    expectedChecksums = {
        f"{sha256(body)} *{name}"
        for name, (body, _mode) in entries.items()
        if name != "SHA256SUMS"
    }
    if set(checksumLines) != expectedChecksums:
        found.append("SHA256SUMS does not cover every other exact archive entry")
    for name in ("LICENSE", "NOTICE"):
        if entries.get(name, (b"", 0))[0] != (ROOT / name).read_bytes():
            found.append(f"archive {name} differs from the repository copy")
    if entries.get("runtime.schema.json", (b"", 0))[0] != SCHEMA_PATH.read_bytes():
        found.append("archive public schema differs from the protocol package")
    if expected is not None:
        for name, expectedEntry in expected.items():
            if entries.get(name) != expectedEntry:
                found.append(f"archive entry {name} differs from deterministic assembly")
    return found


def releaseManifest(directory: Path) -> dict[str, object]:
    version = workspaceVersion()
    artifacts = []
    for target in sorted(targets()):
        path = directory / archiveName(target)
        body = path.read_bytes()
        artifacts.append({"target": target, "file": path.name, "sha256": sha256(body), "bytes": len(body)})
    sdkFiles = (
        ("rust-protocol", f"runtrol-runtime-protocol-{version}.crate"),
        ("rust-client", f"runtrol-runtime-client-{version}.crate"),
        ("typescript-client", f"runtrol-runtime-client-{version}.tgz"),
    )
    sdkArtifacts = []
    for package, name in sdkFiles:
        path = directory / name
        body = path.read_bytes()
        sdkArtifacts.append({"package": package, "file": name, "sha256": sha256(body), "bytes": len(body)})
    pythonArtifacts = []
    selectedWheels: set[Path] = set()
    expectedPrefix = f"runtrol_runtime_client-{version}-cp311-abi3-"
    for target, pattern in sorted(PYTHON_WHEEL_GLOBS.items()):
        matched = list(directory.glob(pattern))
        if len(matched) != 1:
            raise FileNotFoundError(
                f"Python wheel target {target} matched {len(matched)} files instead of one"
            )
        path = matched[0]
        if not path.name.startswith(expectedPrefix):
            raise ValueError(f"Python wheel {path.name} differs from Runtime version {version}")
        body = path.read_bytes()
        selectedWheels.add(path)
        pythonArtifacts.append(
            {
                "package": "python-client",
                "target": target,
                "file": path.name,
                "sha256": sha256(body),
                "bytes": len(body),
            }
        )
    allWheels = set(directory.glob("runtrol_runtime_client-*.whl"))
    if allWheels != selectedWheels:
        extras = sorted(path.name for path in allWheels - selectedWheels)
        raise ValueError(f"release contains unclassified Python wheels: {extras}")
    if any(directory.glob("runtrol_runtime_client-*.tar.gz")):
        raise ValueError("release contains a forbidden Python source distribution")
    return {
        "schema": 1,
        "product": "runtrol-runtime",
        "version": version,
        "protocolRevisions": protocolRevisions(),
        "rollbackSafeStoreSchema": storeSchema(),
        "artifacts": artifacts,
        "sdkArtifacts": sdkArtifacts,
        "pythonArtifacts": pythonArtifacts,
        "signature": "GitHub Sigstore artifact attestation",
    }


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    package = commands.add_parser("package")
    package.add_argument("--target", required=True)
    package.add_argument("--binary", required=True, type=Path)
    package.add_argument("--output", required=True, type=Path)
    inspect = commands.add_parser("inspect")
    inspect.add_argument("--target", required=True)
    inspect.add_argument("--archive", required=True, type=Path)
    inspect.add_argument("--binary", type=Path)
    manifest = commands.add_parser("manifest")
    manifest.add_argument("--directory", required=True, type=Path)
    manifest.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args(argv)
    try:
        if arguments.command == "package":
            writeArchive(arguments.target, arguments.binary.resolve(), arguments.output.resolve())
        elif arguments.command == "inspect":
            found = archiveProblems(
                arguments.archive.resolve(),
                arguments.target,
                arguments.binary.resolve() if arguments.binary else None,
            )
            if found:
                for problem in found:
                    print(f"  - {problem}", file=sys.stderr)
                return 2
        else:
            value = releaseManifest(arguments.directory.resolve())
            arguments.output.write_bytes(jsonBytes(value))
    except (OSError, ValueError, KeyError, zipfile.BadZipFile) as error:
        print(f"[runtimePackage] FAIL. {error}", file=sys.stderr)
        return 2
    print(f"[runtimePackage] OK. {arguments.command} completed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
