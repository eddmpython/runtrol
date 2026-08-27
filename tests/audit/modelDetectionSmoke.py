"""Gate: model choices come from the installed CLIs, never from source literals.

The manifests decide which providers exist and how their executables are found. A provider with stable aliases
declared in its manifest must return those aliases honestly. A provider without aliases must enumerate at least
one current model through its own runtime surface, or state that its protocol has no such surface. Stating it is a
complete answer because a provider that enumerates nothing has no identifier that could have leaked into source;
answering `unknown` is not, because a discovery that broke says exactly the same thing. No prompt is sent, so this
spends no tokens or rate limit.

Every enumerated model identifier is then searched for as a string literal throughout production source. Finding one
means a current model has leaked into code instead of remaining runtime data and will go stale there.

Everything runs under a temporary `RUNTROL_HOME`. The gate stops that home's daemon before removing the directory.

Usage::

    python -X utf8 tests/audit/modelDetectionSmoke.py
    python -X utf8 tests/audit/modelDetectionSmoke.py --require-all
    python -X utf8 tests/audit/modelDetectionSmoke.py --selftest
    python -X utf8 tests/audit/modelDetectionSmoke.py --seed-fixtures

Exit codes:
    0 every installed provider answered honestly, or none was installed and the skip was stated
    2 discovery failed, required provider coverage was absent, its shape was dishonest, or a discovered identifier
      is hardcoded in production source
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path

import claudeApprovalSmoke as provider_gate
import rustSource

ROOT = Path(__file__).resolve().parents[2]
CRATES = ROOT / "crates"
DRIVERS = CRATES / "runtrol-drivers"
MANIFESTS = DRIVERS / "manifests"
COMMAND_TIMEOUT_S = 120.0
DAEMON_START_TIMEOUT_S = 10.0
DAEMON_STOP_TIMEOUT_S = 2.0
EXPECTED_ENV = "RUNTROL_MODEL_GATE_EXPECTED"
OPERATOR_HOME_ENV = "RUNTROL_MODEL_GATE_OPERATOR_HOME"
CREDENTIAL_ENV = (
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "CODEX_ACCESS_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
)


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


def requireCoverage(expected: set[str], present: set[str]) -> None:
    """Refuse a required run that did not resolve every shipped provider CLI."""
    if not expected:
        raise Failed("no shipped provider declares model discovery")
    missing = sorted(expected - present)
    if missing:
        raise Failed(f"required provider CLIs are not installed: {', '.join(missing)}")


def decodeExpected(raw: str | None) -> dict[str, tuple[str, ...]]:
    """Decode provider-specific sentinels a hosted gate placed in provider-owned state."""
    if raw is None:
        return {}
    try:
        decoded = json.loads(raw)
    except json.JSONDecodeError as error:
        raise Failed(f"{EXPECTED_ENV} is not valid JSON: {error.msg}") from error
    if not isinstance(decoded, dict):
        raise Failed(f"{EXPECTED_ENV} must be a JSON object")
    expected: dict[str, tuple[str, ...]] = {}
    for provider, choices in decoded.items():
        if (
            not isinstance(provider, str)
            or not provider
            or not isinstance(choices, list)
            or not choices
            or any(not isinstance(choice, str) or not choice for choice in choices)
        ):
            raise Failed(f"{EXPECTED_ENV} contains an unreadable provider expectation")
        expected[provider] = tuple(choices)
    return expected


def expectedChoices() -> dict[str, tuple[str, ...]]:
    """Read hosted expectations without making them part of the product contract."""
    return decodeExpected(os.environ.get(EXPECTED_ENV))


def credentialFreeEnvironment(source: dict[str, str] | None = None) -> dict[str, str]:
    """Copy a child environment while removing every credential variable this gate knows how to forbid."""
    environment = dict(os.environ if source is None else source)
    for name in CREDENTIAL_ENV:
        environment.pop(name, None)
    return environment


def gateEnvironment(home: Path, source: dict[str, str] | None = None) -> dict[str, str]:
    """Build the one credential-free environment shared by this gate's daemon and commands."""
    environment = credentialFreeEnvironment(source)
    environment["RUNTROL_HOME"] = str(home)
    if operator_home := environment.get(OPERATOR_HOME_ENV):
        environment["HOME"] = operator_home
        environment["USERPROFILE"] = operator_home
    provider_gate.configureMacOSKeychain(environment)
    return environment


def options(argv: list[str]) -> tuple[bool, bool, bool]:
    """Read the gate modes and reject misspelled or conflicting options instead of ignoring them."""
    known = {"--selftest", "--require-all", "--seed-fixtures"}
    unknown = sorted(set(argv) - known)
    if unknown:
        raise Failed(f"unknown option(s): {', '.join(unknown)}")
    selftest_mode = "--selftest" in argv
    seed_mode = "--seed-fixtures" in argv
    if selftest_mode and seed_mode:
        raise Failed("--selftest and --seed-fixtures cannot be combined")
    if seed_mode and "--require-all" in argv:
        raise Failed("--seed-fixtures and --require-all cannot be combined")
    return selftest_mode, "--require-all" in argv, seed_mode


def seedFixtures() -> int:
    """Create provider-owned, credential-free state at the exact native paths the gate will pass through."""
    operator_home = os.environ.get(OPERATOR_HOME_ENV)
    codex_home = os.environ.get("CODEX_HOME")
    expected = expectedChoices()
    claude_choices = expected.get("claude")
    if not operator_home or not codex_home or not claude_choices:
        raise Failed(
            f"--seed-fixtures requires {OPERATOR_HOME_ENV}, CODEX_HOME, and a claude choice in {EXPECTED_ENV}"
        )

    operator = Path(operator_home)
    codex = Path(codex_home)
    cache = {
        "additionalModelOptionsCache": [
            {
                "value": choice,
                "label": "Hosted cache sentinel",
                "description": "Hosted read-only discovery proof",
            }
            for choice in claude_choices
        ]
    }
    try:
        operator.mkdir(parents=True, exist_ok=True)
        codex.mkdir(parents=True, exist_ok=True)
        (operator / ".claude.json").write_text(
            json.dumps(cache, separators=(",", ":")) + "\n",
            encoding="utf-8",
            newline="\n",
        )
    except OSError as error:
        raise Failed(f"could not seed native provider homes: {error}") from error
    print(f"[modelDetectionSmoke --seed-fixtures] OK. native provider homes exist under {operator.parent}.")
    return 0


def buildBinary(
    *,
    runner=subprocess.run,
    source_environment: dict[str, str] | None = None,
    binary_path: Path | None = None,
) -> Path:
    """Build and return the executable this gate drives."""
    runner(
        ["cargo", "build", "-p", "runtrol"],
        cwd=ROOT,
        env=credentialFreeEnvironment(source_environment),
        check=True,
        capture_output=True,
        text=True,
    )
    binary = binary_path or ROOT / "target" / "debug" / ("runtrol.exe" if sys.platform == "win32" else "runtrol")
    if not binary.is_file():
        raise Failed(f"cargo built without error and {binary.relative_to(ROOT)} is absent")
    return binary


def run(
    binary: Path,
    home: Path,
    words: list[str],
    *,
    runner=subprocess.run,
    source_environment: dict[str, str] | None = None,
    timeout_s: float = COMMAND_TIMEOUT_S,
) -> str:
    """Run one command against the gate's isolated daemon."""
    environment = gateEnvironment(home, source_environment)
    try:
        proc = runner(
            [str(binary), *words],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout_s,
            check=False,
        )
    except subprocess.TimeoutExpired as expired:
        raise Failed(f"`runtrol {' '.join(words)}` did not answer in {timeout_s:.0f} s") from expired
    said = (proc.stdout or "").strip() or (proc.stderr or "").strip()
    if proc.returncode != 0:
        raise Failed(f"`runtrol {' '.join(words)}` failed: {said}")
    return said


def daemonOutput(daemon: subprocess.Popen[str]) -> str:
    """Read diagnostics only after a daemon has exited, when communicate cannot block."""
    stdout, stderr = daemon.communicate()
    return (stderr or stdout or "daemon exited without output").strip()


def requireOwnedDaemon(daemon: subprocess.Popen[str], when: str) -> None:
    """Refuse a result if the exact daemon this gate owns has been replaced or has exited."""
    if daemon.poll() is not None:
        raise Failed(f"the owned daemon exited {when}: {daemonOutput(daemon)}")


def stopDaemon(
    daemon: subprocess.Popen[str],
    *,
    timeout_s: float = DAEMON_STOP_TIMEOUT_S,
) -> None:
    """Terminate exactly the daemon process this gate started and prove that it exited."""
    if daemon.poll() is not None:
        return
    try:
        daemon.terminate()
        try:
            daemon.wait(timeout=timeout_s)
        except subprocess.TimeoutExpired:
            daemon.kill()
            try:
                daemon.wait(timeout=timeout_s)
            except subprocess.TimeoutExpired as error:
                raise Failed("the isolated daemon remained alive after terminate and kill") from error
    except OSError as error:
        raise Failed(f"could not stop the isolated daemon process: {error}") from error
    if daemon.poll() is None:
        raise Failed("the isolated daemon reported no exit after terminate and kill")


def startDaemon(
    binary: Path,
    home: Path,
    *,
    popen=subprocess.Popen,
    runner=subprocess.run,
    source_environment: dict[str, str] | None = None,
    ready_marker: Path | None = None,
    timeout_s: float = DAEMON_START_TIMEOUT_S,
) -> subprocess.Popen[str]:
    """Start one owned daemon and require a real command round trip before model discovery."""
    environment = gateEnvironment(home, source_environment)
    try:
        daemon = popen(
            [str(binary), "daemon"],
            cwd=ROOT,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except OSError as error:
        raise Failed(f"could not start the isolated daemon process: {error}") from error

    marker = ready_marker or home / ("runtrol.redb" if sys.platform == "win32" else "runtime.locator.json")
    deadline = time.monotonic() + timeout_s
    try:
        while not marker.exists():
            if daemon.poll() is not None:
                raise Failed(f"the isolated daemon exited before readiness: {daemonOutput(daemon)}")
            if time.monotonic() >= deadline:
                raise Failed(f"the isolated daemon created no endpoint within {timeout_s:.0f} s")
            time.sleep(0.025)

        # The Windows database exists just before its named pipe is bound. A successful List round trip proves the
        # endpoint, greeting, request, and response are all live instead of relying on that earlier filesystem fact.
        # Give the pipe bind one scheduling turn first so the readiness command never needs the CLI's auto-start path.
        if sys.platform == "win32":
            time.sleep(0.1)
        run(
            binary,
            home,
            ["list"],
            runner=runner,
            source_environment=source_environment,
            timeout_s=timeout_s,
        )
        requireOwnedDaemon(daemon, "during readiness")
        return daemon
    except Failed as starting:
        try:
            stopDaemon(daemon)
        except Failed as stopping:
            raise Failed(f"{starting}; cleanup also failed: {stopping}") from starting
        raise


def parseDiscovery(text: str) -> Discovery:
    """Read the CLI's intentionally simple model output, refusing malformed or empty answers."""
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines:
        raise Failed("model discovery printed no answer")

    first = lines[0]
    if first.startswith("aliases only:"):
        why = first.removeprefix("aliases only:").strip()
        aliases: list[str] = []
        for line in lines[1:]:
            if line.startswith("alias  "):
                aliases.append(line.removeprefix("alias  "))
            elif line.startswith("effort  "):
                validateEffortLine(line, set(aliases), allow_global=True)
            else:
                raise Failed(f"alias discovery has an unreadable line: {line!r}")
        if not why or not aliases or any(not alias or any(ch.isspace() for ch in alias) for alias in aliases):
            raise Failed(f"alias discovery has an unreadable shape: {text!r}")
        return Discovery("aliases", tuple(aliases), why)

    if first.startswith("partial catalogue:"):
        why = first.removeprefix("partial catalogue:").strip()
        choices: list[str] = []
        for line in lines[1:]:
            if line.startswith("alias  "):
                identifier = line.removeprefix("alias  ")
            elif line.startswith("model  "):
                identifier, separator, _label = line.removeprefix("model  ").partition("  ")
                if not separator:
                    raise Failed(f"partial model line has an unreadable shape: {line!r}")
            elif line.startswith("effort  "):
                validateEffortLine(line, set(choices), allow_global=True)
                continue
            else:
                raise Failed(f"partial discovery has an unreadable line: {line!r}")
            if not identifier or any(ch.isspace() for ch in identifier):
                raise Failed(f"partial discovery has an unreadable choice: {line!r}")
            choices.append(identifier)
        if not why or not choices:
            raise Failed(f"partial discovery has an unreadable shape: {text!r}")
        return Discovery("partial", tuple(choices), why)

    if first.startswith("model catalogue unknown:"):
        why = first.removeprefix("model catalogue unknown:").strip()
        if not why or len(lines) != 1:
            raise Failed(f"unknown discovery has an unreadable shape: {text!r}")
        return Discovery("unknown", (), why)

    if first.startswith("model catalogue unsupported:"):
        why = first.removeprefix("model catalogue unsupported:").strip()
        if not why or len(lines) != 1:
            raise Failed(f"unsupported discovery has an unreadable shape: {text!r}")
        return Discovery("unsupported", (), why)

    if first == "no models reported":
        return Discovery("known", (), None)

    choices: list[str] = []
    for line in lines:
        if line.startswith("effort  "):
            validateEffortLine(line, set(choices), allow_global=False)
            continue
        identifier, separator, _label = line.partition("  ")
        if not separator or not identifier or any(ch.isspace() for ch in identifier):
            raise Failed(f"enumerated model line has an unreadable shape: {line!r}")
        choices.append(identifier)
    return Discovery("known", tuple(choices), None)


def validateEffortLine(line: str, known_models: set[str], *, allow_global: bool) -> None:
    """Validate an opaque effort row without treating it as a model identifier."""
    fields = line.removeprefix("effort  ").split("  ")
    if len(fields) == 1:
        if not allow_global:
            raise Failed(f"enumerated effort does not name its model: {line!r}")
        effort = fields[0]
    elif len(fields) == 2:
        model, effort = fields
        if model not in known_models:
            raise Failed(f"effort names an unreported model: {line!r}")
    else:
        raise Failed(f"effort line has an unreadable shape: {line!r}")
    if not effort or any(ch.isspace() for ch in effort):
        raise Failed(f"effort line has an unreadable choice: {line!r}")


def productionText(path: Path) -> str:
    """Production source text, with Rust test regions removed."""
    text = path.read_text(encoding="utf-8")
    if path.suffix != ".rs":
        return text
    lines = text.splitlines()
    tests = rustSource.testRegions(lines)
    return "\n".join(line for index, line in enumerate(lines) if not rustSource.inRegions(index, tests))


def literalOffences(identifiers: set[str], roots: tuple[Path, ...] | None = None) -> list[str]:
    """Current model identifiers written as exact production string literals."""
    if not identifiers:
        return []
    search_roots = roots or tuple(sorted(CRATES.iterdir()))
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
        if discovery.kind not in {"aliases", "partial"}:
            raise Failed(f"{spec.identifier} declares aliases and answered as {discovery.kind}")
        if discovery.choices[: len(spec.aliases)] != spec.aliases:
            raise Failed(
                f"{spec.identifier} declared aliases {spec.aliases!r} and the runtime surface returned "
                f"{discovery.choices!r}"
            )
        if discovery.kind == "aliases" and discovery.choices != spec.aliases:
            raise Failed(f"{spec.identifier} returned undeclared choices as aliases")
        return

    # A provider whose protocol has no model surface at all says so, and that is a complete answer. This gate
    # exists so that model identifiers come from the installed CLI instead of source literals, and a provider that
    # enumerates nothing has nothing that could have leaked. `unknown` stays illegal on purpose: it is what a
    # discovery that broke also says, and an absent surface must not be spelled the same as a failed one.
    if discovery.kind == "unsupported":
        return

    if discovery.kind != "known" or not discovery.choices:
        raise Failed(
            f"{spec.identifier} declares no aliases, so its runtime discovery must enumerate at least one model "
            f"or state that its protocol has none; it answered {discovery.kind} with "
            f"{len(discovery.choices)} choices"
        )


def safeRemove(path: Path) -> None:
    """Remove only a directory created directly under the system temporary directory."""
    resolved = path.resolve()
    temporary = Path(tempfile.gettempdir()).resolve()
    if resolved.parent != temporary or not resolved.name.startswith("runtrolModelGate"):
        raise Failed(f"refusing to remove unexpected temporary path {resolved}")
    shutil.rmtree(resolved, ignore_errors=False)


def cleanupDaemonHome(
    daemon: subprocess.Popen[str] | None,
    home: Path,
    *,
    stopper=stopDaemon,
    remover=safeRemove,
) -> Failed | None:
    """Remove the home only after the owned daemon is proven stopped."""
    if daemon is not None:
        try:
            stopper(daemon)
        except (Failed, OSError) as error:
            return Failed(f"could not stop the gate's daemon: {error}")
    try:
        remover(home)
    except (Failed, OSError) as error:
        return Failed(f"could not remove the gate's temporary home: {error}")
    return None


def main(require_all: bool = False) -> int:
    """Discover models from every installed provider and check source isolation."""
    if shutil.which("cargo") is None:
        if require_all:
            print(
                "[modelDetectionSmoke] cargo is required to exercise every shipped provider.",
                file=sys.stderr,
            )
            return 2
        print("[modelDetectionSmoke] SKIP: cargo is not installed, so there is no binary to drive.")
        return 0

    shipped = shippedProviders()
    try:
        expected = expectedChoices()
    except Failed as error:
        print(f"[modelDetectionSmoke] {error}", file=sys.stderr)
        return 2
    unknown_expected = sorted(set(expected) - {spec.identifier for spec in shipped})
    if unknown_expected:
        print(
            f"[modelDetectionSmoke] {EXPECTED_ENV} names unshipped providers: {', '.join(unknown_expected)}",
            file=sys.stderr,
        )
        return 2
    present = [spec for spec in shipped if installed(spec)]
    absent = [spec.identifier for spec in shipped if spec not in present]
    if require_all:
        try:
            requireCoverage(
                {spec.identifier for spec in shipped},
                {spec.identifier for spec in present},
            )
        except Failed as error:
            print(f"[modelDetectionSmoke] {error}", file=sys.stderr)
            return 2
    if not present:
        print(
            "[modelDetectionSmoke] SKIP: no shipped provider CLI is installed. model discovery is unverified here."
        )
        return 0

    binary = buildBinary()
    home = Path(tempfile.mkdtemp(prefix="runtrolModelGate"))
    discovered_ids: set[str] = set()
    failure: Failed | None = None
    daemon: subprocess.Popen[str] | None = None
    try:
        daemon = startDaemon(binary, home)
        for spec in present:
            requireOwnedDaemon(daemon, f"before discovering {spec.identifier} models")
            answer = run(binary, home, ["models", spec.identifier])
            requireOwnedDaemon(daemon, f"after discovering {spec.identifier} models")
            discovery = parseDiscovery(answer)
            verify(spec, discovery)
            missing_expected = [
                choice for choice in expected.get(spec.identifier, ()) if choice not in discovery.choices
            ]
            if missing_expected:
                raise Failed(
                    f"{spec.identifier} did not return expected provider-owned choice(s): "
                    + ", ".join(missing_expected)
                )
            if discovery.kind == "known":
                discovered_ids.update(discovery.choices)
            elif discovery.kind == "partial":
                discovered_ids.update(discovery.choices[len(spec.aliases) :])
            print(f"  {spec.identifier}: {discovery.kind}, {len(discovery.choices)} choice(s)")

        offences = literalOffences(discovered_ids)
        if offences:
            raise Failed("current model identifiers escaped the driver crate:\n  - " + "\n  - ".join(offences))
    except Failed as caught:
        failure = caught
    finally:
        cleanup_failure = cleanupDaemonHome(daemon, home)
        failure = failure or cleanup_failure

    if failure:
        print(f"[modelDetectionSmoke] {failure}", file=sys.stderr)
        return 2
    if absent:
        print(f"  not exercised (not installed): {', '.join(absent)}")
    print(
        f"[modelDetectionSmoke] OK. {len(present)} installed provider(s), "
        f"{len(discovered_ids)} runtime model identifier(s), none hardcoded in production source."
    )
    return 0


def selftest() -> int:
    """Inject malformed output and a leaked literal before trusting the green path."""
    problems: list[str] = []
    credential_fixture = {name: f"secret-{name}" for name in CREDENTIAL_ENV}
    credential_fixture["PATH"] = "fixture-path"
    credential_fixture[OPERATOR_HOME_ENV] = "fixture-operator-home"
    scrubbed = credentialFreeEnvironment(credential_fixture)
    leaked = sorted(set(CREDENTIAL_ENV) & set(scrubbed))
    if leaked or scrubbed.get("PATH") != "fixture-path":
        problems.append(f"credential-free child environment was incomplete: leaked={leaked!r}")

    subprocess_calls: list[tuple[list[str], dict[str, object]]] = []
    popen_calls: list[tuple[list[str], dict[str, object]]] = []

    def fakeRunner(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        subprocess_calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, stdout="fixture output\n", stderr="")

    class FakeDaemon:
        def __init__(
            self,
            *,
            timeout_once: bool = False,
            never_stops: bool = False,
            returncode: int | None = None,
            stderr: str = "",
        ) -> None:
            self.returncode = returncode
            self.stderr = stderr
            self.timeout_once = timeout_once
            self.never_stops = never_stops
            self.terminated = False
            self.killed = False
            self.waits = 0

        def poll(self) -> int | None:
            return self.returncode

        def terminate(self) -> None:
            self.terminated = True

        def kill(self) -> None:
            self.killed = True

        def wait(self, timeout: float) -> int:
            self.waits += 1
            if self.never_stops or (self.timeout_once and self.waits == 1):
                raise subprocess.TimeoutExpired("fixture-daemon", timeout)
            self.returncode = -1
            return self.returncode

        def communicate(self) -> tuple[str, str]:
            return "", self.stderr

    owned_daemon = FakeDaemon()

    def fakePopen(command: list[str], **kwargs: object) -> FakeDaemon:
        popen_calls.append((command, kwargs))
        return owned_daemon

    fixture_binary = Path(__file__).resolve()
    fixture_home = Path("fixture-runtrol-home")
    try:
        built = buildBinary(
            runner=fakeRunner,
            source_environment=credential_fixture,
            binary_path=fixture_binary,
        )
        said = run(
            fixture_binary,
            fixture_home,
            ["models", "fixture"],
            runner=fakeRunner,
            source_environment=credential_fixture,
        )
        started = startDaemon(
            fixture_binary,
            fixture_home,
            popen=fakePopen,
            runner=fakeRunner,
            source_environment=credential_fixture,
            ready_marker=fixture_binary,
        )
        stopDaemon(started)
    except (Failed, OSError) as error:
        problems.append(f"fake subprocess boundary did not complete: {error}")
    else:
        expected_build_environment = credentialFreeEnvironment(credential_fixture)
        expected_run_environment = dict(expected_build_environment)
        expected_run_environment["RUNTROL_HOME"] = str(fixture_home)
        expected_run_environment["HOME"] = "fixture-operator-home"
        expected_run_environment["USERPROFILE"] = "fixture-operator-home"
        expected_calls = (
            (["cargo", "build", "-p", "runtrol"], expected_build_environment),
            ([str(fixture_binary), "models", "fixture"], expected_run_environment),
            ([str(fixture_binary), "list"], expected_run_environment),
        )
        if built != fixture_binary or said != "fixture output" or started is not owned_daemon:
            problems.append("fake subprocess results were not propagated")
        if len(subprocess_calls) != len(expected_calls):
            problems.append(f"expected three fake subprocess calls, got {len(subprocess_calls)}")
        else:
            for index, ((command, kwargs), (expected_command, expected_environment)) in enumerate(
                zip(subprocess_calls, expected_calls, strict=True), start=1
            ):
                if command != expected_command:
                    problems.append(f"fake subprocess call {index} received the wrong command")
                if kwargs.get("env") != expected_environment:
                    problems.append(f"fake subprocess call {index} did not receive the credential-free environment")
        if len(popen_calls) != 1:
            problems.append(f"expected one fake daemon start, got {len(popen_calls)}")
        else:
            command, kwargs = popen_calls[0]
            if command != [str(fixture_binary), "daemon"]:
                problems.append("the fake daemon start received the wrong command")
            if kwargs.get("env") != expected_run_environment:
                problems.append("the fake daemon did not receive the command's credential-free environment")
        if not owned_daemon.terminated or owned_daemon.killed or owned_daemon.poll() is None:
            problems.append("a normally stopping owned daemon was not terminated and reaped exactly once")

    slow_daemon = FakeDaemon(timeout_once=True)
    try:
        stopDaemon(slow_daemon)
    except Failed as error:
        problems.append(f"a daemon that stopped after kill was refused: {error}")
    if not slow_daemon.terminated or not slow_daemon.killed or slow_daemon.poll() is None:
        problems.append("a daemon that ignored terminate was not killed and reaped")

    stubborn_daemon = FakeDaemon(never_stops=True)
    try:
        stopDaemon(stubborn_daemon)
    except Failed as error:
        if "remained alive after terminate and kill" not in str(error):
            problems.append(f"an unstoppable daemon failure lost its reason: {error}")
    else:
        problems.append("an unstoppable daemon was reported as stopped")

    exited_daemon = FakeDaemon(returncode=9, stderr="fixture startup failure")

    def exitedPopen(_command: list[str], **_kwargs: object) -> FakeDaemon:
        return exited_daemon

    try:
        startDaemon(
            fixture_binary,
            fixture_home,
            popen=exitedPopen,
            runner=fakeRunner,
            source_environment=credential_fixture,
            ready_marker=Path("fixture-readiness-marker-that-does-not-exist"),
        )
    except Failed as error:
        if "fixture startup failure" not in str(error):
            problems.append(f"an early daemon exit lost its diagnostics: {error}")
    else:
        problems.append("a daemon that exited before readiness was accepted")

    failed_cleanup_order: list[str] = []

    def failingStop(_daemon: FakeDaemon) -> None:
        failed_cleanup_order.append("stop")
        raise Failed("fixture stop failure")

    def forbiddenRemove(_home: Path) -> None:
        failed_cleanup_order.append("remove")

    cleanup_failure = cleanupDaemonHome(
        FakeDaemon(),
        fixture_home,
        stopper=failingStop,
        remover=forbiddenRemove,
    )
    if failed_cleanup_order != ["stop"]:
        problems.append(f"home removal was attempted after daemon cleanup failed: {failed_cleanup_order!r}")
    if cleanup_failure is None or "fixture stop failure" not in str(cleanup_failure):
        problems.append("daemon cleanup failure was not returned while preserving its home")

    completed_cleanup_order: list[str] = []

    def recordingStop(_daemon: FakeDaemon) -> None:
        completed_cleanup_order.append("stop")

    def recordingRemove(_home: Path) -> None:
        completed_cleanup_order.append("remove")

    cleanup_failure = cleanupDaemonHome(
        FakeDaemon(),
        fixture_home,
        stopper=recordingStop,
        remover=recordingRemove,
    )
    if completed_cleanup_order != ["stop", "remove"]:
        problems.append(f"successful cleanup ran out of order: {completed_cleanup_order!r}")
    if cleanup_failure is not None:
        problems.append(f"successful ordered cleanup was reported as failed: {cleanup_failure}")

    known = parseDiscovery(
        "runtime-model-42  Runtime Model 42  (default)\neffort  runtime-model-42  provider-effort"
    )
    if known != Discovery("known", ("runtime-model-42",), None):
        problems.append("a valid enumerated model was not read")
    aliases = parseDiscovery(
        "aliases only: no enumerable catalogue\nalias  fast\nalias  deep\neffort  provider-effort"
    )
    if aliases.choices != ("fast", "deep"):
        problems.append("valid aliases were not read")
    partial = parseDiscovery(
        "partial catalogue: provider-owned cache only\nalias  fast\neffort  provider-effort\n"
        "model  runtime-model-42  Runtime Model\neffort  runtime-model-42  model-effort"
    )
    if partial != Discovery("partial", ("fast", "runtime-model-42"), "provider-owned cache only"):
        problems.append("a valid partial catalogue was not read")

    for malformed in (
        "",
        "aliases only: \nalias  ",
        "identifier-without-a-label",
        "runtime-model-42  Runtime Model\neffort  missing-model  high",
    ):
        rejected = False
        try:
            parseDiscovery(malformed)
        except Failed:
            # The refusal is the expected result and is recorded below rather than discarded.
            rejected = True
        if not rejected:
            problems.append(f"malformed discovery was accepted: {malformed!r}")

    cases = [
        (
            "one required provider was not inspected",
            lambda: requireCoverage({"first", "second"}, {"first"}),
        ),
        (
            "all required providers were absent",
            lambda: requireCoverage({"first", "second"}, set()),
        ),
        (
            "no provider discovery was declared",
            lambda: requireCoverage(set(), set()),
        ),
        ("an unknown option was accepted", lambda: options(["--requre-all"])),
        ("a malformed hosted expectation was accepted", lambda: decodeExpected("[]")),
        (
            "an empty hosted expectation was accepted",
            lambda: decodeExpected('{"fixture":[]}'),
        ),
    ]
    for name, defect in cases:
        caught = False
        try:
            defect()
        except Failed:
            caught = True
        if not caught:
            problems.append(name)

    try:
        requireCoverage({"first", "second"}, {"first", "second"})
    except Failed:
        problems.append("complete required provider coverage was refused")

    scratch = ROOT / ".tmp" / "modelDetectionSelftest.rs"
    scratch.parent.mkdir(exist_ok=True)
    try:
        scratch.write_text('const MODEL: &str = "runtime-model-42";\n', encoding="utf-8", newline="\n")
        if not literalOffences({"runtime-model-42"}, (scratch,)):
            problems.append("a current model identifier in production source was not caught")
        scratch.write_text('const MODEL: &str = "discovered-at-runtime";\n', encoding="utf-8", newline="\n")
        if literalOffences({"runtime-model-42"}, (scratch,)):
            problems.append("an unrelated runtime value was reported as a model literal")
    finally:
        scratch.unlink(missing_ok=True)

    for problem in problems:
        print(f"[modelDetectionSmoke --selftest] {problem}", file=sys.stderr)
    if problems:
        return 2
    print(
        "[modelDetectionSmoke --selftest] OK. malformed output, missing coverage, unknown options, "
        "and leaked literals were caught."
    )
    return 0


if __name__ == "__main__":
    try:
        selftest_mode, require_all, seed_mode = options(sys.argv[1:])
    except Failed as error:
        print(f"[modelDetectionSmoke] {error}", file=sys.stderr)
        raise SystemExit(2) from None
    if selftest_mode:
        raise SystemExit(selftest())
    if seed_mode:
        try:
            raise SystemExit(seedFixtures())
        except Failed as error:
            print(f"[modelDetectionSmoke] {error}", file=sys.stderr)
            raise SystemExit(2) from None
    raise SystemExit(main(require_all=require_all))
