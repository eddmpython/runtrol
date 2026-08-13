"""Gate: the public TypeScript Runtime client is reproducible and independently consumable."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CLIENT = ROOT / "clients" / "typescript"


def boundaryViolations(package: dict[str, object], sources: dict[str, str]) -> list[str]:
    """Return static public-package boundary violations."""
    found: list[str] = []
    dependencies = package.get("dependencies")
    if isinstance(dependencies, dict) and dependencies:
        found.append("the published client has runtime dependencies")

    exports = package.get("exports")
    if not isinstance(exports, dict) or set(exports) != {".", "./testing", "./schema"}:
        found.append("the package export map is not the exact public, testing, and schema surface")

    scripts = package.get("scripts")
    requiredScripts = {"generate:check", "check", "test", "test:packed", "pack:check"}
    if not isinstance(scripts, dict) or not requiredScripts.issubset(scripts):
        found.append("the package does not expose every generation, test, and packed-consumer gate")

    allSource = "\n".join(sources.values())
    for forbidden in (
        "extensions/runtrol-vscode",
        "runtrol-core",
        "runtrol-daemon",
        "runtrol-drivers",
        "runtrol-ipc",
        "runtrol-store",
        "runtrol-vault",
    ):
        if forbidden in allSource:
            found.append(f"public TypeScript source reaches private authority `{forbidden}`")
    index = sources.get("src/index.ts", "")
    for testingOnly in ("runtimeLocatorAtForTesting", "validatedLocatorForTesting"):
        if testingOnly in index:
            found.append(f"the primary package exports testing-only symbol `{testingOnly}`")
    return found


def selftest() -> int:
    """Prove every static defect class makes the gate red."""
    package = {
        "exports": {".": {}, "./testing": {}, "./schema": {}},
        "scripts": {
            "generate:check": "one",
            "check": "two",
            "test": "three",
            "test:packed": "four",
            "pack:check": "five",
        },
    }
    sources = {"src/index.ts": "export { RuntimeLocator } from './locator.js';"}
    if boundaryViolations(package, sources):
        print("[runtimeClientSdk --selftest] FAIL. the green fixture was rejected.", file=sys.stderr)
        return 2
    mutations = [
        ({**package, "dependencies": {"ajv": "1"}}, sources),
        ({**package, "exports": {".": {}}}, sources),
        ({**package, "scripts": {}}, sources),
        (package, {"src/index.ts": "extensions/runtrol-vscode"}),
        (package, {"src/index.ts": "runtimeLocatorAtForTesting"}),
    ]
    for index, (changedPackage, changedSources) in enumerate(mutations, start=1):
        if not boundaryViolations(changedPackage, changedSources):
            print(f"[runtimeClientSdk --selftest] FAIL. mutation {index} escaped.", file=sys.stderr)
            return 2
    print(f"[runtimeClientSdk --selftest] OK. all {len(mutations)} defects make the gate red.")
    return 0


def npmCommand() -> list[str]:
    """Return an explicit npm launcher without shell interpretation."""
    npm = shutil.which("npm.cmd" if sys.platform == "win32" else "npm") or shutil.which("npm")
    if npm is None:
        raise RuntimeError("npm is missing")
    if sys.platform == "win32":
        command = os.environ.get("ComSpec", r"C:\Windows\System32\cmd.exe")
        return [command, "/d", "/c", npm]
    return [npm]


def run() -> int:
    """Inspect the boundary and execute generation, tests, and external package consumption."""
    manifest = CLIENT / "package.json"
    lock = CLIENT / "package-lock.json"
    if not manifest.is_file() or not lock.is_file():
        print("[runtimeClientSdk] FAIL. package.json and package-lock.json are required.", file=sys.stderr)
        return 2
    package = json.loads(manifest.read_text(encoding="utf-8"))
    sources = {
        path.relative_to(CLIENT).as_posix(): path.read_text(encoding="utf-8")
        for base in (CLIENT / "src", CLIENT / "tooling", CLIENT / "test")
        for path in base.rglob("*")
        if path.is_file() and path.suffix in {".ts", ".mjs"}
    }
    failures = boundaryViolations(package, sources)
    checkedSchema = ROOT / "crates" / "runtrol-runtime-protocol" / "schema" / "runtime.schema.json"
    packagedSchema = CLIENT / "schema" / "runtime.schema.json"
    if (
        not checkedSchema.is_file()
        or not packagedSchema.is_file()
        or checkedSchema.read_bytes() != packagedSchema.read_bytes()
    ):
        failures.append("the packaged schema differs from the Rust-generated public schema")
    if failures:
        print("[runtimeClientSdk] FAIL. public package boundary violations:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 2

    command = npmCommand()
    commands = (
        [*command, "ci", "--ignore-scripts"],
        [*command, "run", "check"],
        [*command, "test"],
        [*command, "run", "pack:check"],
    )
    for invocation in commands:
        result = subprocess.run(
            invocation,
            cwd=CLIENT,
            check=False,
            text=True,
            capture_output=True,
            timeout=180,
        )
        if result.returncode != 0:
            print(result.stdout, file=sys.stderr)
            print(result.stderr, file=sys.stderr)
            print(
                f"[runtimeClientSdk] FAIL. {' '.join(invocation)} returned {result.returncode}.",
                file=sys.stderr,
            )
            return 2
    print("[runtimeClientSdk] OK. generated bindings and packed external consumer verified.")
    return 0


def main() -> int:
    """Select selftest or the real gate."""
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print("usage: runtimeClientSdk.py [--selftest]", file=sys.stderr)
        return 1
    return run()


if __name__ == "__main__":
    raise SystemExit(main())
