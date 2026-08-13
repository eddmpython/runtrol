"""Gate: packed public Rust Runtime crates build for a repository-external consumer."""

from __future__ import annotations

import subprocess
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROTOCOL = ROOT / "crates" / "runtrol-runtime-protocol"
CLIENT = ROOT / "crates" / "runtrol-runtime-client"


def boundaryProblems(workspace: dict[str, object], protocol: dict[str, object], client: dict[str, object], source: str) -> list[str]:
    """Return publishability and private-authority defects."""
    found: list[str] = []
    try:
        dependency = workspace["workspace"]["dependencies"]["runtrol-runtime-protocol"]  # type: ignore[index]
    except (KeyError, TypeError):
        dependency = None
    if not isinstance(dependency, dict) or not isinstance(dependency.get("version"), str):
        found.append("the published protocol dependency has no registry version requirement")
    for name, manifest in (("protocol", protocol), ("client", client)):
        package = manifest.get("package")
        if not isinstance(package, dict) or package.get("publish") is not True:
            found.append(f"the public {name} crate is not publishable")
        if not isinstance(package, dict) or not isinstance(package.get("description"), str):
            found.append(f"the public {name} crate has no package description")
        if not isinstance(package, dict) or package.get("readme") != "README.md":
            found.append(f"the public {name} crate does not package its README")
    forbidden = (
        "runtrol_core",
        "runtrol_daemon",
        "runtrol_drivers",
        "runtrol_ipc",
        "runtrol_store",
        "extensions/runtrol-vscode",
    )
    for token in forbidden:
        if token in source:
            found.append(f"the public Rust crates reach private authority `{token}`")
    return found


def selftest() -> int:
    """Prove each publishability defect makes the gate red."""
    workspace = {"workspace": {"dependencies": {"runtrol-runtime-protocol": {"path": "one", "version": "1.0.0"}}}}
    package = {"package": {"publish": True, "description": "public", "readme": "README.md"}}
    if boundaryProblems(workspace, package, package, "pub fn public() {}"):
        print("[runtimeRustClientSdk --selftest] FAIL. the green fixture was rejected.", file=sys.stderr)
        return 2
    mutations = (
        ({"workspace": {"dependencies": {"runtrol-runtime-protocol": {"path": "one"}}}}, package, package, ""),
        (workspace, {"package": {"publish": False, "description": "public", "readme": "README.md"}}, package, ""),
        (workspace, package, {"package": {"publish": True, "readme": "README.md"}}, ""),
        (workspace, package, {"package": {"publish": True, "description": "public"}}, ""),
        (workspace, package, package, "use runtrol_core::Core;"),
    )
    for index, mutation in enumerate(mutations, start=1):
        if not boundaryProblems(*mutation):
            print(f"[runtimeRustClientSdk --selftest] FAIL. mutation {index} escaped.", file=sys.stderr)
            return 2
    print(f"[runtimeRustClientSdk --selftest] OK. all {len(mutations)} defects make the gate red.")
    return 0


def runCommand(command: list[str], *, cwd: Path, timeout: int = 300) -> None:
    """Run one bounded command and retain actionable failure output."""
    result = subprocess.run(command, cwd=cwd, check=False, text=True, capture_output=True, timeout=timeout)
    if result.returncode != 0:
        raise RuntimeError(f"{' '.join(command)} returned {result.returncode}\n{result.stdout}\n{result.stderr}")


def extractPackage(archive: Path, destination: Path) -> Path:
    """Extract one Cargo package while rejecting links and path traversal."""
    with tarfile.open(archive, mode="r:gz") as package:
        members = package.getmembers()
        roots = {Path(member.name).parts[0] for member in members if Path(member.name).parts}
        if len(roots) != 1:
            raise RuntimeError(f"{archive.name} has no single package root")
        for member in members:
            target = (destination / member.name).resolve()
            if not target.is_relative_to(destination.resolve()) or member.issym() or member.islnk():
                raise RuntimeError(f"{archive.name} contains unsafe entry {member.name}")
        package.extractall(destination, members=members, filter="data")
    root = destination / next(iter(roots))
    if not root.is_dir():
        raise RuntimeError(f"{archive.name} did not extract its package root")
    return root


def run() -> int:
    """Package both crates and compile a consumer using only their extracted artifacts."""
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    protocol = tomllib.loads((PROTOCOL / "Cargo.toml").read_text(encoding="utf-8"))
    client = tomllib.loads((CLIENT / "Cargo.toml").read_text(encoding="utf-8"))
    sources = "\n".join(path.read_text(encoding="utf-8") for crate in (PROTOCOL, CLIENT) for path in (crate / "src").rglob("*.rs"))
    failures = boundaryProblems(workspace, protocol, client, sources)
    if failures:
        print("[runtimeRustClientSdk] FAIL. public Rust package boundary violations:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 2
    version = workspace["workspace"]["package"]["version"]
    try:
        runCommand(
            ["cargo", "package", "-p", "runtrol-runtime-protocol", "--allow-dirty", "--no-verify"],
            cwd=ROOT,
        )
        patch = f"patch.crates-io.runtrol-runtime-protocol.path='{PROTOCOL.as_posix()}'"
        runCommand(
            [
                "cargo",
                "package",
                "-p",
                "runtrol-runtime-client",
                "--allow-dirty",
                "--no-verify",
                "--config",
                patch,
            ],
            cwd=ROOT,
        )
        packageRoot = ROOT / "target" / "package"
        protocolArchive = packageRoot / f"runtrol-runtime-protocol-{version}.crate"
        clientArchive = packageRoot / f"runtrol-runtime-client-{version}.crate"
        if not protocolArchive.is_file() or not clientArchive.is_file():
            raise RuntimeError("Cargo did not produce both expected public crate archives")
        with tempfile.TemporaryDirectory(prefix="runtrol-rust-sdk-consumer-") as scratchText:
            scratch = Path(scratchText)
            protocolPackage = extractPackage(protocolArchive, scratch / "protocol")
            clientPackage = extractPackage(clientArchive, scratch / "client")
            for package in (protocolPackage, clientPackage):
                for required in ("README.md", "CHANGELOG.md", "LICENSE"):
                    if not (package / required).is_file():
                        raise RuntimeError(f"{package.name} does not contain {required}")
            if not (protocolPackage / "schema" / "runtime.schema.json").is_file():
                raise RuntimeError("the packed protocol crate does not contain the checked schema")
            normalized = tomllib.loads((clientPackage / "Cargo.toml").read_text(encoding="utf-8"))
            dependency = normalized.get("dependencies", {}).get("runtrol-runtime-protocol", {})
            if "path" in dependency or not isinstance(dependency.get("version"), str):
                raise RuntimeError("the packed client does not contain a registry protocol dependency")
            consumer = scratch / "consumer"
            (consumer / "src").mkdir(parents=True)
            (consumer / "Cargo.toml").write_text(
                "\n".join(
                    (
                        "[package]",
                        'name = "outside-runtime-consumer"',
                        'version = "0.0.0"',
                        'edition = "2024"',
                        "",
                        "[dependencies]",
                        f'runtrol-runtime-client = {{ path = "{clientPackage.as_posix()}" }}',
                        "",
                        "[patch.crates-io]",
                        f'runtrol-runtime-protocol = {{ path = "{protocolPackage.as_posix()}" }}',
                        "",
                    )
                ),
                encoding="utf-8",
            )
            (consumer / "src" / "main.rs").write_text(
                "use runtrol_runtime_client::{ClientOptions, RuntimeLocator};\n\n"
                "fn main() -> Result<(), Box<dyn std::error::Error>> {\n"
                "    let _options = ClientOptions::new(\"outside consumer\", \"1.0.0\");\n"
                "    let _locator = RuntimeLocator::system()?;\n"
                "    Ok(())\n"
                "}\n",
                encoding="utf-8",
            )
            runCommand(["cargo", "generate-lockfile"], cwd=consumer)
            runCommand(["cargo", "check", "--locked"], cwd=consumer)
    except (OSError, RuntimeError, tarfile.TarError, subprocess.TimeoutExpired, tomllib.TOMLDecodeError) as error:
        print(f"[runtimeRustClientSdk] FAIL. {error}", file=sys.stderr)
        return 2
    print("[runtimeRustClientSdk] OK. packed protocol and client crates compile for an external consumer.")
    return 0


def main() -> int:
    """Select selftest or the real gate."""
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print("usage: runtimeRustClientSdk.py [--selftest]", file=sys.stderr)
        return 1
    return run()


if __name__ == "__main__":
    raise SystemExit(main())
