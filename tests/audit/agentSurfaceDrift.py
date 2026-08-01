"""Gate: every unauthenticated provider probe runtrol declares still matches the installed CLIs.

The probe surface is read from each driver's `bound.rs`; method and flag names are never copied into this gate.
For a schema-producing CLI, generated methods are compared with bound methods. For a CLI without a schema, every
bound flag is asked of its real argument parser with the manifest's safe arguments and two invented control flags.
Event and control frames that require an authenticated turn are outside this gate.

New vendor methods are information and do not fail this gate. Removal of a probed method or flag is red.

Usage::

    python -X utf8 tests/audit/agentSurfaceDrift.py
    python -X utf8 tests/audit/agentSurfaceDrift.py --require-all
    python -X utf8 tests/audit/agentSurfaceDrift.py --selftest

Exit codes:
    0 every installed supported CLI still passes its declared method or flag probe, or none is installed with a loud skip
      unless --require-all was passed
    2 a bound surface disappeared, a probe was unreadable, or the selftest cannot detect an injected defect
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
DRIVERS = ROOT / "crates" / "runtrol-drivers"
MANIFESTS = DRIVERS / "manifests"
TIMEOUT_S = 120.0
CONTROLS = ("--runtrol-drift-absent-alpha", "--runtrol-drift-absent-omega")
FLAG = re.compile(r'\bflag:\s*"(?P<name>--[a-z0-9-]+)"')
METHOD = re.compile(r'\bmethod:\s*"(?P<name>[A-Za-z0-9/]+)"')


class Failed(Exception):
    """A declared provider probe no longer matches or could not be executed."""


def manifests() -> list[dict[str, Any]]:
    """Every built-in provider declaration, in file-name order."""
    return [tomllib.loads(path.read_text(encoding="utf-8")) for path in sorted(MANIFESTS.glob("*.toml"))]


def installed(names: list[str]) -> str | None:
    """The first executable name the operator's search path resolves."""
    for name in names:
        found = shutil.which(name)
        if found:
            return found
    return None


def boundSource(driver: str) -> str:
    """The one source file that declares a driver's drift exposure."""
    path = DRIVERS / "src" / driver / "bound.rs"
    if not path.is_file():
        raise Failed(f"{path.relative_to(ROOT)} is missing")
    return path.read_text(encoding="utf-8")


def methodEnums(value: Any) -> set[str]:
    """Method discriminator values from a generated JSON schema tree."""
    found: set[str] = set()
    if isinstance(value, dict):
        properties = value.get("properties")
        if isinstance(properties, dict):
            method = properties.get("method")
            if isinstance(method, dict):
                choices = method.get("enum")
                if isinstance(choices, list):
                    found.update(choice for choice in choices if isinstance(choice, str))
        for child in value.values():
            found.update(methodEnums(child))
    elif isinstance(value, list):
        for child in value:
            found.update(methodEnums(child))
    return found


def requireAll(provider: str, bound: set[str], offered: set[str], surface: str) -> None:
    """Fail when a method or flag runtrol consumes is absent, while permitting vendor additions."""
    missing = sorted(bound - offered)
    if missing:
        raise Failed(f"{provider} removed bound {surface}: {', '.join(missing)}")


def schemaSurface(program: str, provider: str, driver: str) -> None:
    """Generate the current schema and compare all three JSON-RPC directions."""
    source = boundSource(driver)
    bound = set(METHOD.findall(source))
    if not bound:
        raise Failed(f"{driver}/bound.rs declares no methods")

    with tempfile.TemporaryDirectory(prefix="runtrolDriftSchema") as output:
        result = subprocess.run(
            [program, "app-server", "generate-json-schema", "--out", output, "--experimental"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=TIMEOUT_S,
            check=False,
        )
        if result.returncode != 0:
            said = (result.stdout or "") + (result.stderr or "")
            raise Failed(f"{provider} schema generation failed: {said.strip()}")

        offered: set[str] = set()
        for name in ("ClientRequest.json", "ServerNotification.json", "ServerRequest.json"):
            path = Path(output) / name
            try:
                offered.update(methodEnums(json.loads(path.read_text(encoding="utf-8"))))
            except (OSError, json.JSONDecodeError) as error:
                raise Failed(f"{provider} generated unreadable {name}: {error}") from error

    requireAll(provider, bound, offered, "JSON-RPC methods")
    print(f"  {provider}: {len(bound)} bound methods still exist in {len(offered)} generated methods")


def askFlag(program: str, safe: list[str], flag: str) -> str:
    """Ask one real argument parser and remove the queried token from its answer."""
    result = subprocess.run(
        [program, *safe, flag],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=TIMEOUT_S,
        check=False,
    )
    said = ((result.stdout or "") + (result.stderr or "")).replace(flag, "<flag>").strip()
    if not said:
        raise Failed(f"the argument parser said nothing when asked about {flag}")
    return said


def classifyFlags(provider: str, bound: set[str], controls: list[str], answers: dict[str, str]) -> None:
    """Require stable controls and an answer distinct from absence for every bound flag."""
    if len(controls) != 2 or controls[0] != controls[1]:
        raise Failed(f"{provider} answers two invented flags differently, so its parser cannot be compared")
    absent = controls[0]
    missing = {flag for flag in bound if answers.get(flag) == absent}
    requireAll(provider, bound, bound - missing, "flags")


def flagSurface(program: str, provider: str, driver: str, manifest: dict[str, Any]) -> None:
    """Ask the real parser about every flag the driver binds."""
    bound = set(FLAG.findall(boundSource(driver)))
    if not bound:
        raise Failed(f"{driver}/bound.rs declares no flags")
    safe = list((((manifest.get("probe") or {}).get("flags") or {}).get("safe_with") or []))
    if not safe:
        raise Failed(f"{provider} has no safe flag probe arguments")

    controls = [askFlag(program, safe, control) for control in CONTROLS]
    answers = {flag: askFlag(program, safe, flag) for flag in sorted(bound)}
    classifyFlags(provider, bound, controls, answers)
    print(f"  {provider}: all {len(bound)} bound flags remain accepted by its parser")


def requireCoverage(expected: set[str], checked: set[str]) -> None:
    """Refuse a hosted run that did not execute every built-in probe strategy."""
    if not expected:
        raise Failed("no built-in provider declares a drift probe")
    missing = sorted(expected - checked)
    if missing:
        raise Failed(f"required built-in probe strategies were not executed: {', '.join(missing)}")


def exercise(require_all: bool = False) -> int:
    """Probe every installed built-in whose driver has a drift strategy."""
    expected: set[str] = set()
    checked: set[str] = set()
    absent: list[str] = []
    unsupported: list[str] = []
    for manifest in manifests():
        provider = str(manifest["id"])
        kind = str(manifest["kind"])
        driver = kind.removesuffix("-app-server").removesuffix("-stream-json")
        if not kind.endswith(("-app-server", "-stream-json")):
            unsupported.append(f"{provider} ({kind})")
            continue

        expected.add(provider)
        program = installed(list((manifest.get("bin") or {}).get("names") or []))
        if program is None:
            absent.append(provider)
            continue

        if kind.endswith("-app-server"):
            schemaSurface(program, provider, driver)
        else:
            flagSurface(program, provider, driver, manifest)
        checked.add(provider)

    if unsupported:
        print(f"  no drift strategy for: {', '.join(unsupported)}")
    if absent:
        print(f"  not installed: {', '.join(absent)}")
    if require_all:
        requireCoverage(expected, checked)
    if not checked:
        print("[agentSurfaceDrift] SKIP: no installed provider has a drift strategy.")
        return 0
    print(f"[agentSurfaceDrift] OK. {len(checked)} installed provider probe strategy(s) passed.")
    return 0


def selftest() -> int:
    """Inject removed methods, removed flags, and unstable controls before trusting a green probe."""
    problems: list[str] = []
    cases = [
        ("removed method", lambda: requireAll("fixture", {"a", "b"}, {"a"}, "methods")),
        (
            "removed flag",
            lambda: classifyFlags("fixture", {"--a"}, ["absent", "absent"], {"--a": "absent"}),
        ),
        (
            "unstable controls",
            lambda: classifyFlags("fixture", {"--a"}, ["first", "second"], {"--a": "known"}),
        ),
        (
            "required provider was not inspected",
            lambda: requireCoverage({"first", "second"}, {"first"}),
        ),
        ("no drift probe was declared", lambda: requireCoverage(set(), set())),
    ]
    for name, defect in cases:
        caught = False
        try:
            defect()
        except Failed:
            caught = True
        if not caught:
            problems.append(f"{name} was accepted")

    sample = {
        "oneOf": [
            {"properties": {"method": {"enum": ["thread/start"]}}},
            {"properties": {"method": {"enum": ["vendor/new"]}}},
        ]
    }
    if methodEnums(sample) != {"thread/start", "vendor/new"}:
        problems.append("method discriminators were not read from a schema tree")

    try:
        requireCoverage({"first", "second"}, {"first", "second"})
    except Failed:
        problems.append("complete required provider coverage was refused")

    if problems:
        print("[agentSurfaceDrift --selftest] the gate cannot detect what it claims to.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print(f"[agentSurfaceDrift --selftest] OK. {len(cases) + 1} injected cases all caught.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or the real provider probes."""
    known = {"--selftest", "--require-all"}
    unknown = sorted(set(argv) - known)
    if unknown:
        print(f"[agentSurfaceDrift] FAIL: unknown option(s): {', '.join(unknown)}", file=sys.stderr)
        return 2
    if "--selftest" in argv:
        return selftest()
    try:
        return exercise(require_all="--require-all" in argv)
    except (Failed, subprocess.TimeoutExpired) as error:
        print(f"[agentSurfaceDrift] FAIL: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
