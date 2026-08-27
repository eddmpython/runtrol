"""Gate the public Python Runtime client source and its platform wheel."""

from __future__ import annotations

import argparse
import ast
import fnmatch
import hashlib
import json
import subprocess
import sys
import tomllib
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = ROOT / "clients" / "python"
PUBLIC_CRATES = {"runtrol-runtime-client", "runtrol-runtime-protocol"}
WHEEL_PATTERNS = {
    "darwin-arm64": "runtrol_runtime_client-*-cp311-abi3-*macosx*arm64.whl",
    "darwin-x64": "runtrol_runtime_client-*-cp311-abi3-*macosx*x86_64.whl",
    "linux-arm64": "runtrol_runtime_client-*-cp311-abi3-*manylinux*aarch64.whl",
    "linux-x64": "runtrol_runtime_client-*-cp311-abi3-*manylinux*x86_64.whl",
    "win32-arm64": "runtrol_runtime_client-*-cp311-abi3-*win_arm64.whl",
    "win32-x64": "runtrol_runtime_client-*-cp311-abi3-*win_amd64.whl",
}


def classMethods(source: str, name: str) -> set[str]:
    module = ast.parse(source)
    selected = next(
        item for item in module.body if isinstance(item, ast.ClassDef) and item.name == name
    )
    return {
        item.name
        for item in selected.body
        if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))
    }


def sourceProblems() -> list[str]:
    found: list[str] = []
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    version = workspace["workspace"]["package"]["version"]
    cargo = tomllib.loads((PACKAGE / "Cargo.toml").read_text(encoding="utf-8"))
    project = tomllib.loads((PACKAGE / "pyproject.toml").read_text(encoding="utf-8"))
    package = cargo["package"]
    metadata = project["project"]
    runtrolEdges = {
        name for name in cargo["dependencies"] if name.startswith("runtrol-")
    }
    if runtrolEdges != PUBLIC_CRATES:
        found.append(f"native binding links Runtime crates {sorted(runtrolEdges)}")
    if package.get("publish") is not False:
        found.append("the native Cargo package can be published independently")
    if package.get("license") != "Apache-2.0" or metadata.get("license") != "Apache-2.0":
        found.append("the Python package does not declare Apache-2.0 on both surfaces")
    if metadata.get("name") != "runtrol-runtime-client":
        found.append("the Python distribution name drifted")
    if metadata.get("version") != version:
        found.append("the Python distribution version differs from the Runtime workspace")
    if metadata.get("requires-python") != ">=3.11":
        found.append("the Python minimum is not exactly 3.11")
    maturin = project.get("tool", {}).get("maturin", {})
    if "pyo3/abi3-py311" not in maturin.get("features", []):
        found.append("the Python wheel does not fix the CPython 3.11 stable ABI")
    checkedSchema = (PACKAGE / "schema" / "runtime.schema.json").read_bytes()
    publicSchema = (
        ROOT
        / "crates"
        / "runtrol-runtime-protocol"
        / "schema"
        / "runtime.schema.json"
    ).read_bytes()
    if checkedSchema != publicSchema:
        found.append("the wheel schema differs from the public Rust protocol schema")
    clientSource = (
        PACKAGE / "python" / "runtrol_runtime" / "client.py"
    ).read_text(encoding="utf-8")
    asyncMethods = classMethods(clientSource, "AsyncRuntimeClient") - {"__init__"}
    syncMethods = classMethods(clientSource, "RuntimeClient") - {"__init__"}
    if asyncMethods != syncMethods:
        found.append(
            f"sync and async client methods differ: {sorted(asyncMethods ^ syncMethods)}"
        )
    asyncTerminal = classMethods(clientSource, "AsyncTerminalView") - {"__init__"}
    syncTerminal = classMethods(clientSource, "TerminalView") - {"__init__"}
    if asyncTerminal != syncTerminal:
        found.append(
            f"sync and async terminal methods differ: {sorted(asyncTerminal ^ syncTerminal)}"
        )
    nativeSource = "\n".join(
        path.read_text(encoding="utf-8") for path in sorted((PACKAGE / "src").glob("*.rs"))
    ).lower()
    for forbidden in ("claude", "codex", "grok", "npm install", "transcript"):
        if forbidden in nativeSource:
            found.append(f"native Python binding contains provider or ownership policy {forbidden}")
    generated = subprocess.run(
        [sys.executable, "-X", "utf8", str(PACKAGE / "tooling" / "generate.py"), "--check"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if generated.returncode != 0:
        found.append("generated Python declarations drifted from the checked schema")
    return found


def wheelProblems(directory: Path, target: str) -> tuple[list[str], Path | None]:
    found: list[str] = []
    pattern = WHEEL_PATTERNS.get(target)
    if pattern is None:
        return [f"unknown native target {target}"], None
    wheels = list(directory.glob("*.whl"))
    selected = [path for path in wheels if fnmatch.fnmatch(path.name, pattern)]
    if len(selected) != 1:
        return [f"target {target} matched {len(selected)} wheels instead of one"], None
    wheel = selected[0]
    if len(wheels) != 1:
        found.append("the platform build produced more than one wheel")
    if list(directory.glob("*.tar.gz")) or list(directory.glob("*.zip")):
        found.append("the Python build produced a source distribution")
    try:
        with zipfile.ZipFile(wheel) as archive:
            names = archive.namelist()
            if len(names) != len(set(names)):
                found.append("the wheel contains duplicate paths")
            for name in names:
                parts = PurePosixPath(name).parts
                if not parts or any(part in {"", ".", ".."} for part in parts):
                    found.append(f"the wheel contains unsafe path {name}")
            lowered = [name.lower() for name in names]
            native = [
                name
                for name in lowered
                if name.endswith(".pyd") or name.endswith(".so")
            ]
            if len(native) != 1 or "runtrol_runtime/_native" not in native[0]:
                found.append("the wheel does not contain exactly one client native module")
            requiredSuffixes = (
                "runtrol_runtime/client.py",
                "runtrol_runtime/generated.py",
                "runtrol_runtime/py.typed",
                "schema/runtime.schema.json",
                ".dist-info/licenses/license",
            )
            for suffix in requiredSuffixes:
                if not any(name.endswith(suffix) for name in lowered):
                    found.append(f"the wheel is missing {suffix}")
            for name in lowered:
                leaf = PurePosixPath(name).name
                if "__pycache__" in name or name.endswith(".pyc"):
                    found.append(f"the wheel contains disposable Python output {name}")
                if leaf in {"runtrol", "runtrol.exe"} or "provider-cli" in leaf:
                    found.append(f"the wheel embeds an executable it does not own: {name}")
            generated = next(
                (name for name in names if name.lower().endswith("runtrol_runtime/generated.py")),
                None,
            )
            if generated is not None:
                body = archive.read(generated).decode("utf-8")
                schemaHash = hashlib.sha256(
                    (PACKAGE / "schema" / "runtime.schema.json").read_bytes()
                ).hexdigest()
                if f"SCHEMA_SHA256 = '{schemaHash}'" not in body:
                    found.append("generated declarations do not identify their exact source schema")
    except (OSError, zipfile.BadZipFile, UnicodeDecodeError, json.JSONDecodeError) as error:
        found.append(f"cannot inspect wheel: {error}")
    return found, wheel


def report(found: list[str]) -> int:
    if found:
        print("[runtimePythonClientSdk] FAIL. Python client defects:", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[runtimePythonClientSdk] OK. public abi3 Python client is bounded and consumable.")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-only", action="store_true")
    parser.add_argument("--wheel-directory", type=Path)
    parser.add_argument("--target", choices=sorted(WHEEL_PATTERNS))
    arguments = parser.parse_args(argv)
    if arguments.source_only:
        if arguments.wheel_directory or arguments.target:
            parser.error("--source-only cannot be combined with wheel arguments")
        return report(sourceProblems())
    if arguments.wheel_directory is None or arguments.target is None:
        parser.error("wheel inspection requires --wheel-directory and --target")
    found = sourceProblems()
    wheelFound, wheel = wheelProblems(arguments.wheel_directory.resolve(), arguments.target)
    found.extend(wheelFound)
    if not found and wheel is not None:
        consumed = subprocess.run(
            [sys.executable, "-X", "utf8", str(PACKAGE / "tooling" / "packed_consumer.py"), str(wheel)],
            cwd=ROOT,
            check=False,
        )
        if consumed.returncode != 0:
            found.append("the wheel failed its isolated external consumer journey")
    return report(found)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
