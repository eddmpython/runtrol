"""Gate: model choices come from the installed CLIs, never from source literals.

The manifests decide which providers exist and how their executables are found. A provider with stable aliases
declared in its manifest must return those aliases honestly. A provider without aliases must enumerate at least
one current model through its own runtime surface. No prompt is sent, so this spends no tokens or rate limit.

Every enumerated model identifier is then searched for as a string literal outside the driver crate. Finding one
means a current model has leaked into core, IPC, CLI, or GUI source and will go stale there.

Everything runs under a temporary `RUNTROL_HOME`. The gate stops that home's daemon before removing the directory.

Usage::

    python -X utf8 tests/audit/modelDetectionSmoke.py
    python -X utf8 tests/audit/modelDetectionSmoke.py --selftest

Exit codes:
    0 every installed provider answered honestly, or none was installed and the skip was stated
    2 discovery failed, its shape was dishonest, or a discovered identifier is hardcoded outside drivers
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path

import rustSource

ROOT = Path(__file__).resolve().parents[2]
CRATES = ROOT / "crates"
DRIVERS = CRATES / "runtrol-drivers"
MANIFESTS = DRIVERS / "manifests"
COMMAND_TIMEOUT_S = 120.0


class Failed(Exception):
    """Discovery did not hold. The message is what an operator reads."""


@dataclass(frozen=True)
class ProviderSpec:
    """The discovery facts one manifest declares."""

    identifier: str
    executables: tuple[str, ...]
    aliases: tuple[str, ...]


@dataclass(frozen=True)
class Discovery:
    """What the terminal surface said about one provider."""

    kind: str
    choices: tuple[str, ...]
    why: str | None


def shippedProviders() -> list[ProviderSpec]:
    """Read providers and stable aliases from the manifests compiled into the build."""
    providers: list[ProviderSpec] = []
    for path in sorted(MANIFESTS.glob("*.toml")):
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
        identifier = manifest.get("id")
        names = tuple((manifest.get("bin") or {}).get("names") or ())
        aliases = tuple((manifest.get("models") or {}).get("aliases") or ())
        if isinstance(identifier, str) and identifier and names:
            providers.append(ProviderSpec(identifier, names, aliases))
    return providers


def installed(spec: ProviderSpec) -> bool:
    """Whether any executable name declared by this provider resolves."""
    return any(shutil.which(name) is not None for name in spec.executables)


def buildBinary() -> Path:
    """Build and return the executable this gate drives."""
    subprocess.run(
        ["cargo", "build", "-p", "runtrol"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    binary = ROOT / "target" / "debug" / ("runtrol.exe" if sys.platform == "win32" else "runtrol")
    if not binary.is_file():
        raise Failed(f"cargo built without error and {binary.relative_to(ROOT)} is absent")
    return binary


def run(binary: Path, home: Path, words: list[str]) -> str:
    """Run one command against the gate's isolated daemon."""
    environment = dict(os.environ)
    environment["RUNTROL_HOME"] = str(home)
    try:
        proc = subprocess.run(
            [str(binary), *words],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=COMMAND_TIMEOUT_S,
            check=False,
        )
    except subprocess.TimeoutExpired as expired:
        raise Failed(f"`runtrol {' '.join(words)}` did not answer in {COMMAND_TIMEOUT_S:.0f} s") from expired
    said = (proc.stdout or "").strip() or (proc.stderr or "").strip()
    if proc.returncode != 0:
        raise Failed(f"`runtrol {' '.join(words)}` failed: {said}")
    return said


def parseDiscovery(text: str) -> Discovery:
    """Read the CLI's intentionally simple model output, refusing malformed or empty answers."""
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines:
        raise Failed("model discovery printed no answer")

    first = lines[0]
    if first.startswith("aliases only:"):
        why = first.removeprefix("aliases only:").strip()
        aliases = tuple(line.removeprefix("alias  ") for line in lines[1:] if line.startswith("alias  "))
        if not why or not aliases or len(aliases) != len(lines) - 1 or any(not alias for alias in aliases):
            raise Failed(f"alias discovery has an unreadable shape: {text!r}")
        return Discovery("aliases", aliases, why)

    if first.startswith("model catalogue unknown:"):
        why = first.removeprefix("model catalogue unknown:").strip()
        if not why or len(lines) != 1:
            raise Failed(f"unknown discovery has an unreadable shape: {text!r}")
        return Discovery("unknown", (), why)

    if first == "no models reported":
        return Discovery("known", (), None)

    choices: list[str] = []
    for line in lines:
        identifier, separator, _label = line.partition("  ")
        if not separator or not identifier or any(ch.isspace() for ch in identifier):
            raise Failed(f"enumerated model line has an unreadable shape: {line!r}")
        choices.append(identifier)
    return Discovery("known", tuple(choices), None)


def productionText(path: Path) -> str:
    """Production source text, with Rust test regions removed."""
    text = path.read_text(encoding="utf-8")
    if path.suffix != ".rs":
        return text
    lines = text.splitlines()
    tests = rustSource.testRegions(lines)
    return "\n".join(line for index, line in enumerate(lines) if not rustSource.inRegions(index, tests))


def literalOffences(identifiers: set[str], roots: tuple[Path, ...] | None = None) -> list[str]:
    """Current model identifiers written as exact string literals outside drivers."""
    if not identifiers:
        return []
    search_roots = roots or tuple(path for path in sorted(CRATES.iterdir()) if path != DRIVERS)
    patterns = {
        identifier: re.compile(rf"(?:\"{re.escape(identifier)}\"|'{re.escape(identifier)}')")
        for identifier in identifiers
    }
    found: list[str] = []
    for root in search_roots:
        paths = [root] if root.is_file() else sorted(root.rglob("*"))
        for path in paths:
            if not path.is_file() or path.suffix not in {".rs", ".ts", ".tsx"}:
                continue
            text = productionText(path)
            for identifier, pattern in patterns.items():
                if pattern.search(text):
                    found.append(f"{path.relative_to(ROOT).as_posix()} hardcodes {identifier!r}")
    return found


def verify(spec: ProviderSpec, discovery: Discovery) -> None:
    """Hold a runtime answer against what the provider manifest can honestly promise."""
    if spec.aliases:
        if discovery.kind != "aliases":
            raise Failed(f"{spec.identifier} declares aliases and answered as {discovery.kind}")
        if discovery.choices != spec.aliases:
            raise Failed(
                f"{spec.identifier} declared aliases {spec.aliases!r} and the runtime surface returned "
                f"{discovery.choices!r}"
            )
        return

    if discovery.kind != "known" or not discovery.choices:
        raise Failed(
            f"{spec.identifier} declares no aliases, so its runtime discovery must enumerate at least one model; "
            f"it answered {discovery.kind} with {len(discovery.choices)} choices"
        )


def safeRemove(path: Path) -> None:
    """Remove only a directory created directly under the system temporary directory."""
    resolved = path.resolve()
    temporary = Path(tempfile.gettempdir()).resolve()
    if resolved.parent != temporary or not resolved.name.startswith("runtrolModelGate"):
        raise Failed(f"refusing to remove unexpected temporary path {resolved}")
    shutil.rmtree(resolved, ignore_errors=False)


def main() -> int:
    """Discover models from every installed provider and check source isolation."""
    if shutil.which("cargo") is None:
        print("[modelDetectionSmoke] SKIP: cargo is not installed, so there is no binary to drive.")
        return 0

    shipped = shippedProviders()
    present = [spec for spec in shipped if installed(spec)]
    absent = [spec.identifier for spec in shipped if spec not in present]
    if not present:
        print(
            "[modelDetectionSmoke] SKIP: no shipped provider CLI is installed. model discovery is unverified here."
        )
        return 0

    binary = buildBinary()
    home = Path(tempfile.mkdtemp(prefix="runtrolModelGate"))
    discovered_ids: set[str] = set()
    failure: Failed | None = None
    try:
        for spec in present:
            discovery = parseDiscovery(run(binary, home, ["models", spec.identifier]))
            verify(spec, discovery)
            discovered_ids.update(discovery.choices if discovery.kind == "known" else ())
            print(f"  {spec.identifier}: {discovery.kind}, {len(discovery.choices)} choice(s)")

        offences = literalOffences(discovered_ids)
        if offences:
            raise Failed("current model identifiers escaped the driver crate:\n  - " + "\n  - ".join(offences))
    except Failed as caught:
        failure = caught
    finally:
        try:
            run(binary, home, ["panic"])
        except Failed as stopping:
            failure = failure or Failed(f"could not stop the gate's daemon: {stopping}")
        try:
            safeRemove(home)
        except (Failed, OSError) as removing:
            failure = failure or Failed(f"could not remove the gate's temporary home: {removing}")

    if failure:
        print(f"[modelDetectionSmoke] {failure}", file=sys.stderr)
        return 2
    if absent:
        print(f"  not exercised (not installed): {', '.join(absent)}")
    print(
        f"[modelDetectionSmoke] OK. {len(present)} installed provider(s), "
        f"{len(discovered_ids)} runtime model identifier(s), none hardcoded outside drivers."
    )
    return 0


def selftest() -> int:
    """Inject malformed output and a leaked literal before trusting the green path."""
    problems: list[str] = []
    known = parseDiscovery("runtime-model-42  Runtime Model 42  (default)")
    if known != Discovery("known", ("runtime-model-42",), None):
        problems.append("a valid enumerated model was not read")
    aliases = parseDiscovery("aliases only: no enumerable catalogue\nalias  fast\nalias  deep")
    if aliases.choices != ("fast", "deep"):
        problems.append("valid aliases were not read")

    for malformed in ("", "aliases only: \nalias  ", "identifier-without-a-label"):
        rejected = False
        try:
            parseDiscovery(malformed)
        except Failed:
            # The refusal is the expected result and is recorded below rather than discarded.
            rejected = True
        if not rejected:
            problems.append(f"malformed discovery was accepted: {malformed!r}")

    scratch = ROOT / ".tmp" / "modelDetectionSelftest.rs"
    scratch.parent.mkdir(exist_ok=True)
    try:
        scratch.write_text('const MODEL: &str = "runtime-model-42";\n', encoding="utf-8", newline="\n")
        if not literalOffences({"runtime-model-42"}, (scratch,)):
            problems.append("a current model identifier outside drivers was not caught")
        scratch.write_text('const MODEL: &str = "discovered-at-runtime";\n', encoding="utf-8", newline="\n")
        if literalOffences({"runtime-model-42"}, (scratch,)):
            problems.append("an unrelated runtime value was reported as a model literal")
    finally:
        scratch.unlink(missing_ok=True)

    for problem in problems:
        print(f"[modelDetectionSmoke --selftest] {problem}", file=sys.stderr)
    if problems:
        return 2
    print("[modelDetectionSmoke --selftest] OK. malformed output and leaked literals were caught.")
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest() if "--selftest" in sys.argv else main())
