"""Operator gate: two installed providers complete their lifecycle in the production GUI.

This gate never sends a prompt.  Successful resume therefore needs one already existing native conversation for
each provider.  The operator supplies those metadata pointers in a JSON file outside the repository; the gate
uses an isolated runtrol home and never reads a provider transcript or session path.

Usage::

    python -X utf8 tests/audit/desktopRealProviderGuiSmoke.py --selftest
    set RUNTROL_REAL_PROVIDER_TARGETS=C:\\path\\to\\targets.json
    python -X utf8 tests/audit/desktopRealProviderGuiSmoke.py --built-product

The target file is deliberately narrow::

    {"schema": 1, "targets": [
      {"provider": "provider-id", "native": "existing-native-id", "workspace": "C:\\safe\\work"},
      {"provider": "another-id", "native": "existing-native-id", "workspace": "C:\\safe\\work"}
    ]}

Native identifiers are never printed.  The product may show provider-owned content while loading an existing
conversation, but neither half of this gate reads, screenshots, traces, or stores that content.
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import actualShellSmoke as shell
import genericAcpSmoke as acp
import guiMemoryContract as production
import sessionLifecycleSmoke as lifecycle

ROOT = Path(__file__).resolve().parents[2]
UI = ROOT / "crates" / "runtrol-gui" / "ui"
DRIVER = UI / "tests" / "actualProductLifecycle.mjs"
MANIFESTS = ROOT / "crates" / "runtrol-drivers" / "manifests"
TARGETS_ENV = "RUNTROL_REAL_PROVIDER_TARGETS"
WEBVIEW_ARGUMENTS_ENV = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"
TARGET_COUNT = 2
COMMAND_TIMEOUT_S = 120.0
WINDOW_WAIT_S = 20.0
CDP_WAIT_S = 20.0
DRIVER_TIMEOUT_S = 12 * 60.0


class Failed(Exception):
    """The real-provider product-window journey did not hold."""


@dataclass(frozen=True)
class ProviderTarget:
    """One provider-owned conversation metadata pointer, never its content."""

    provider: str
    display_name: str
    native: str
    workspace: Path


def descriptorMap() -> dict[str, tuple[str, list[str]]]:
    """Return provider display names and executable candidates from compiled manifests."""
    descriptors: dict[str, tuple[str, list[str]]] = {}
    for path in sorted(MANIFESTS.glob("*.toml")):
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
        identifier = manifest.get("id")
        display_name = manifest.get("display_name")
        names = (manifest.get("bin") or {}).get("names") or []
        if isinstance(identifier, str) and isinstance(display_name, str) and all(
            isinstance(name, str) for name in names
        ):
            descriptors[identifier] = (display_name, list(names))
    return descriptors


def targetProblems(raw: object, descriptors: dict[str, tuple[str, list[str]]]) -> list[str]:
    """Return content-free reasons a target document cannot drive exactly two real providers."""
    if not isinstance(raw, dict):
        return ["target document is not an object"]
    problems: list[str] = []
    if set(raw) != {"schema", "targets"}:
        problems.append("target document fields are not exactly schema and targets")
    if raw.get("schema") != 1:
        problems.append("target schema is not 1")
    entries = raw.get("targets")
    if not isinstance(entries, list):
        problems.append("targets is not a list")
        return problems
    if len(entries) != TARGET_COUNT:
        problems.append(f"target count is not {TARGET_COUNT}")

    seen: set[str] = set()
    for index, entry in enumerate(entries):
        where = f"target {index + 1}"
        if not isinstance(entry, dict):
            problems.append(f"{where} is not an object")
            continue
        if set(entry) != {"provider", "native", "workspace"}:
            problems.append(f"{where} fields are not exactly provider, native, and workspace")
        provider = entry.get("provider")
        native = entry.get("native")
        workspace = entry.get("workspace")
        if not isinstance(provider, str) or not provider:
            problems.append(f"{where} has no provider identifier")
        elif provider not in descriptors:
            problems.append(f"{where} provider is not shipped")
        elif provider in seen:
            problems.append(f"{where} repeats a provider")
        else:
            seen.add(provider)
            if not lifecycle.installed(descriptors[provider][1]):
                problems.append(f"{where} provider CLI is not installed")
        if (
            not isinstance(native, str)
            or not native
            or len(native) > 4096
            or any(character in native for character in "\r\n\0")
        ):
            problems.append(f"{where} has no usable native identifier")
        if not isinstance(workspace, str) or not workspace:
            problems.append(f"{where} has no workspace")
        elif not Path(workspace).expanduser().is_dir():
            problems.append(f"{where} workspace is not an existing directory")
    return problems


def loadTargets(path: Path) -> list[ProviderTarget]:
    """Load the narrow operator input without ever echoing its native identifiers."""
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise Failed("the real-provider target file could not be read") from error
    descriptors = descriptorMap()
    problems = targetProblems(raw, descriptors)
    if problems:
        raise Failed("the real-provider target precondition failed: " + "; ".join(problems))
    entries = raw["targets"]
    return [
        ProviderTarget(
            provider=entry["provider"],
            display_name=descriptors[entry["provider"]][0],
            native=entry["native"],
            workspace=Path(entry["workspace"]).expanduser().resolve(),
        )
        for entry in entries
    ]


def runProduct(
    binary: Path,
    environment: dict[str, str],
    words: list[str],
    stage: str,
) -> str:
    """Run one product command while keeping arguments and provider output out of diagnostics."""
    try:
        process = subprocess.run(
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
    except (OSError, subprocess.SubprocessError) as error:
        raise Failed(f"{stage} did not complete") from error
    if process.returncode != 0:
        raise Failed(f"{stage} was refused")
    return (process.stdout or "").strip() or (process.stderr or "").strip()


def listing(binary: Path, environment: dict[str, str], stage: str) -> list[lifecycle.SessionLine]:
    """Read only session metadata and hide malformed provider-owned values from diagnostics."""
    text = runProduct(binary, environment, ["list"], stage)
    try:
        return lifecycle.parseListing(text)
    except lifecycle.Failed as error:
        raise Failed(f"{stage} returned an unreadable session listing") from error


def requireRow(
    rows: list[lifecycle.SessionLine],
    session: str,
    target: ProviderTarget,
    stage: str,
) -> lifecycle.SessionLine:
    """Require the session pointer to retain its provider and native metadata."""
    row = lifecycle.rowFor(rows, session)
    if row is None:
        raise Failed(f"{stage} lost its session row")
    if row.provider != target.provider:
        raise Failed(f"{stage} changed the row provider")
    if row.native != target.native:
        raise Failed(f"{stage} changed the provider-native identifier")
    return row


def reserveLoopbackPort() -> int:
    """Reserve an ephemeral loopback port long enough to learn its number."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def guiEnvironment(base: dict[str, str], home: Path, port: int) -> dict[str, str]:
    """Enable CDP only in the gate-owned production GUI process."""
    environment = dict(base)
    existing = environment.get(WEBVIEW_ARGUMENTS_ENV, "").strip()
    if "--remote-debugging-port" in existing or "--remote-debugging-pipe" in existing:
        raise Failed("the operator environment already controls WebView2 remote debugging")
    arguments = [
        value
        for value in (
            existing,
            f"--remote-debugging-port={port}",
            "--remote-debugging-address=127.0.0.1",
        )
        if value
    ]
    environment[WEBVIEW_ARGUMENTS_ENV] = " ".join(arguments)
    environment["WEBVIEW2_USER_DATA_FOLDER"] = str(home / "webview")
    return environment


def waitForCdp(gui: subprocess.Popen[bytes], endpoint: str) -> None:
    """Wait for this GUI's loopback debugger endpoint without reading any page content."""
    deadline = time.monotonic() + CDP_WAIT_S
    version_url = f"{endpoint}/json/version"
    while time.monotonic() < deadline:
        if gui.poll() is not None:
            raise Failed("the production GUI exited before its WebView2 endpoint was ready")
        try:
            with urllib.request.urlopen(version_url, timeout=0.25) as response:
                parsed = json.loads(response.read(16 * 1024).decode("utf-8"))
            if isinstance(parsed, dict) and isinstance(parsed.get("webSocketDebuggerUrl"), str):
                return
        except (OSError, urllib.error.URLError, json.JSONDecodeError, UnicodeDecodeError):
            time.sleep(0.05)
    raise Failed("the production GUI exposed no bounded loopback WebView2 endpoint")


def writeDriverSpec(
    path: Path,
    binary: Path,
    targets: list[ProviderTarget],
    seeded: dict[str, str],
    start_root: Path,
) -> None:
    """Write ephemeral metadata used by the CDP driver, never conversation content."""
    entries = []
    for index, target in enumerate(targets):
        workspace = start_root / f"provider-{index + 1}"
        workspace.mkdir()
        entries.append(
            {
                "provider": target.provider,
                "displayName": target.display_name,
                "native": target.native,
                "seedSession": seeded[target.provider],
                "startWorkspace": str(workspace),
            }
        )
    path.write_text(
        json.dumps(
            {
                "schema": 1,
                "binary": str(binary),
                "targets": entries,
            },
            ensure_ascii=True,
        ),
        encoding="utf-8",
    )


def evidenceProblems(result: object, providers: list[str]) -> list[str]:
    """Return every claimed real-product lifecycle fact that is absent."""
    if not isinstance(result, dict):
        return ["driver result is not an object"]
    problems: list[str] = []
    if result.get("schema") != 1:
        problems.append("driver schema")
    if result.get("actualProduct") is not True:
        problems.append("actual product identity")
    if result.get("mockBridge") is not False:
        problems.append("mock bridge absence")
    if result.get("simultaneousStarted") is not True:
        problems.append("two-provider simultaneous list")
    if result.get("cancelKeptRow") is not True:
        problems.append("cancelled deletion")
    if result.get("finalDomRows") != 0:
        problems.append("empty final DOM list")
    if result.get("finalBackendRows") != 0:
        problems.append("empty final backend list")

    invokes = result.get("invokes")
    expected_invokes = {
        "start": len(providers),
        "resume": len(providers),
        "close": len(providers) * 2,
        "prompt": 0,
    }
    if not isinstance(invokes, dict) or invokes != expected_invokes:
        problems.append("exact command counts with zero prompts")

    entries = result.get("providers")
    if not isinstance(entries, list) or len(entries) != len(providers):
        problems.append("provider evidence count")
        return problems
    by_provider = {
        entry.get("provider"): entry
        for entry in entries
        if isinstance(entry, dict) and isinstance(entry.get("provider"), str)
    }
    if set(by_provider) != set(providers):
        problems.append("provider evidence set")
        return problems

    all_sessions: set[str] = set()
    for provider in providers:
        entry = by_provider[provider]
        if entry.get("resumeNativeMatched") is not True:
            problems.append(f"{provider} resume native continuity")
        if entry.get("badgesMatched") is not True:
            problems.append(f"{provider} row badges")
        if entry.get("deleted") is not True:
            problems.append(f"{provider} confirmed deletion")
        sessions = [entry.get(name) for name in ("seedSession", "resumedSession", "startedSession")]
        if not all(isinstance(session, str) and lifecycle.isSessionId(session) for session in sessions):
            problems.append(f"{provider} session identifiers")
            continue
        if len(set(sessions)) != len(sessions):
            problems.append(f"{provider} resume replaced the old identity")
        for session in sessions:
            if session in all_sessions:
                problems.append("session identity reused across providers")
            all_sessions.add(session)
    return problems


def runDriver(
    endpoint: str,
    spec: Path,
    environment: dict[str, str],
    providers: list[str],
) -> dict[str, Any]:
    """Let Playwright click the production WebView, then validate its content-free evidence."""
    if shutil.which("node") is None:
        raise Failed("node is required for the production WebView2 driver")
    try:
        process = subprocess.run(
            ["node", str(DRIVER), "--endpoint", endpoint, "--spec", str(spec)],
            cwd=UI,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=DRIVER_TIMEOUT_S,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise Failed("the production WebView2 lifecycle driver did not complete") from error
    if process.returncode != 0:
        detail = (process.stderr or "driver reported no diagnostic").strip().splitlines()[-1]
        raise Failed(f"the production WebView2 lifecycle driver failed: {detail}")
    lines = [line for line in process.stdout.splitlines() if line.strip()]
    if len(lines) != 1:
        raise Failed("the production WebView2 lifecycle driver returned an unexpected result shape")
    try:
        result = json.loads(lines[0])
    except json.JSONDecodeError as error:
        raise Failed("the production WebView2 lifecycle driver returned invalid JSON") from error
    problems = evidenceProblems(result, providers)
    if problems:
        raise Failed("the production WebView2 evidence is incomplete: " + "; ".join(problems))
    return result


def processIdentities(pid: int) -> set[production.ProcessIdentity]:
    """Capture one gate-owned process tree for identity-stable cleanup proof."""
    try:
        return production.treeIdentities(pid)
    except (OSError, production.Failed) as error:
        raise Failed("a gate-owned process tree could not be identified") from error


def closeAllSessions(binary: Path, environment: dict[str, str]) -> list[str]:
    """Best-effort exact cleanup of every pointer in the isolated home."""
    errors: list[str] = []
    try:
        rows = listing(binary, environment, "cleanup listing")
    except Failed as error:
        return [str(error)]
    for row in rows:
        try:
            runProduct(binary, environment, ["close", row.session, "--now"], "cleanup close")
        except Failed as error:
            errors.append(str(error))
    return errors


def cleanupOwned(
    binary: Path,
    environment: dict[str, str],
    gui: subprocess.Popen[bytes] | None,
    daemons: list[subprocess.Popen[str]],
    observed: set[production.ProcessIdentity],
) -> list[str]:
    """Stop everything this gate started and prove no captured process identity survived."""
    errors: list[str] = []
    for process in [gui, *daemons]:
        if process is not None and process.poll() is None:
            try:
                observed.update(processIdentities(process.pid))
            except Failed as error:
                errors.append(str(error))

    if gui is not None and gui.poll() is None:
        try:
            production.requestWindowClose(gui.pid)
            gui.wait(timeout=5.0)
        except (OSError, subprocess.SubprocessError, production.Failed):
            try:
                production.stop(gui)
            except (OSError, subprocess.SubprocessError) as error:
                errors.append(f"GUI cleanup failed: {error}")

    live_daemon = next((daemon for daemon in reversed(daemons) if daemon.poll() is None), None)
    if live_daemon is not None:
        errors.extend(closeAllSessions(binary, environment))
    for daemon in reversed(daemons):
        if daemon.poll() is None:
            try:
                acp.stopDaemon(daemon)
            except (OSError, subprocess.SubprocessError) as error:
                errors.append(f"daemon cleanup failed: {error}")

    try:
        survivors = production.waitIdentitiesGone(observed, 2.0)
        if survivors:
            production.forceCapturedIdentities(survivors)
    except production.Failed as error:
        errors.append(str(error))
    return errors


def exercise(reuse_built_product: bool, target_path: Path) -> None:
    """Drive two real installed providers entirely through the production Tauri session surface."""
    if sys.platform != "win32":
        raise Failed("the production WebView2 operator gate currently requires Windows")
    if not DRIVER.is_file():
        raise Failed("the production WebView2 lifecycle driver is missing")
    targets = loadTargets(target_path)
    binary, fixture = shell.builtProduct() if reuse_built_product else shell.build()
    initial_evidence = shell.productEvidence(binary, fixture)

    with tempfile.TemporaryDirectory(prefix="runtrol-real-gui-") as raw_home:
        home = Path(raw_home)
        start_root = home / "start-workspaces"
        start_root.mkdir()
        environment = dict(os.environ)
        environment["RUNTROL_HOME"] = str(home)
        daemon_one: subprocess.Popen[str] | None = None
        daemon_two: subprocess.Popen[str] | None = None
        gui: subprocess.Popen[bytes] | None = None
        daemons: list[subprocess.Popen[str]] = []
        observed: set[production.ProcessIdentity] = set()
        failure: BaseException | None = None
        try:
            daemon_one = acp.startDaemon(binary, environment, home)
            daemons.append(daemon_one)
            seeded: dict[str, str] = {}
            for target in targets:
                session = runProduct(
                    binary,
                    environment,
                    ["resume", target.provider, target.native, str(target.workspace)],
                    f"seeding provider {target.provider}",
                )
                if not lifecycle.isSessionId(session):
                    raise Failed(f"seeding provider {target.provider} returned no session identifier")
                seeded[target.provider] = session
                requireRow(
                    listing(binary, environment, f"listing seeded provider {target.provider}"),
                    session,
                    target,
                    f"seeded provider {target.provider}",
                )

            observed.update(processIdentities(daemon_one.pid))
            runProduct(binary, environment, ["panic"], "restarting the isolated daemon")
            try:
                daemon_one.wait(timeout=10.0)
            except subprocess.TimeoutExpired as error:
                raise Failed("the first isolated daemon did not exit for restart") from error

            daemon_two = acp.startDaemon(binary, environment, home)
            daemons.append(daemon_two)
            restarted = listing(binary, environment, "listing detached seed sessions")
            for target in targets:
                row = requireRow(
                    restarted,
                    seeded[target.provider],
                    target,
                    f"restarted provider {target.provider}",
                )
                if row.tier != "idle" or row.doing != "detached":
                    raise Failed(f"restarted provider {target.provider} did not become idle and detached")

            port = reserveLoopbackPort()
            endpoint = f"http://127.0.0.1:{port}"
            product_environment = guiEnvironment(environment, home, port)
            output_path = home / "gui-output.bin"
            output_file = output_path.open("wb")
            try:
                gui = subprocess.Popen(
                    [str(binary), "gui"],
                    cwd=ROOT,
                    env=product_environment,
                    stdin=subprocess.DEVNULL,
                    stdout=output_file,
                    stderr=subprocess.STDOUT,
                )
            finally:
                output_file.close()
            shell.waitFor(
                lambda: shell.visibleWindow(gui.pid),
                gui,
                time.monotonic() + WINDOW_WAIT_S,
                "a visible production Tauri window",
            )
            waitForCdp(gui, endpoint)
            observed.update(processIdentities(daemon_two.pid))
            observed.update(processIdentities(gui.pid))

            spec = home / "driver-spec.json"
            writeDriverSpec(spec, binary, targets, seeded, start_root)
            runDriver(endpoint, spec, product_environment, [target.provider for target in targets])

            production.requestWindowClose(gui.pid)
            try:
                gui.wait(timeout=10.0)
            except subprocess.TimeoutExpired as error:
                raise Failed("the production GUI did not close after a real window close request") from error
            if daemon_two.poll() is not None:
                raise Failed("closing the production GUI also stopped its isolated daemon")
            if listing(binary, environment, "checking the daemon after GUI close"):
                raise Failed("the real-provider GUI journey left a runtrol session pointer behind")

            ending_evidence = shell.productEvidence(binary, fixture)
            changed = shell.changedEvidence(initial_evidence, ending_evidence)
            if changed:
                raise Failed("production evidence changed during the real-provider GUI journey")
        except BaseException as error:
            failure = error
        finally:
            cleanup_errors = cleanupOwned(binary, environment, gui, daemons, observed)
        if cleanup_errors:
            raise Failed("; ".join(cleanup_errors))
        if failure is not None:
            raise failure

    print(
        "[desktopRealProviderGuiSmoke] OK. two installed providers were resumed, started, listed together, "
        "and removed through the production Tauri GUI with zero prompt invocations."
    )


def selftest() -> None:
    """Prove missing prerequisites and every independent evidence defect make the gate red."""
    first = "11111111-1111-1111-1111-111111111111"
    second = "22222222-2222-2222-2222-222222222222"
    third = "33333333-3333-3333-3333-333333333333"
    fourth = "44444444-4444-4444-4444-444444444444"
    fifth = "55555555-5555-5555-5555-555555555555"
    sixth = "66666666-6666-6666-6666-666666666666"
    providers = ["provider-a", "provider-b"]
    green: dict[str, Any] = {
        "schema": 1,
        "actualProduct": True,
        "mockBridge": False,
        "simultaneousStarted": True,
        "cancelKeptRow": True,
        "finalDomRows": 0,
        "finalBackendRows": 0,
        "invokes": {"start": 2, "resume": 2, "close": 4, "prompt": 0},
        "providers": [
            {
                "provider": "provider-a",
                "seedSession": first,
                "resumedSession": second,
                "startedSession": third,
                "resumeNativeMatched": True,
                "badgesMatched": True,
                "deleted": True,
            },
            {
                "provider": "provider-b",
                "seedSession": fourth,
                "resumedSession": fifth,
                "startedSession": sixth,
                "resumeNativeMatched": True,
                "badgesMatched": True,
                "deleted": True,
            },
        ],
    }
    if evidenceProblems(green, providers):
        raise Failed("selftest defect: complete lifecycle evidence was rejected")

    injections: list[tuple[str, Any]] = [
        ("schema", 2),
        ("actualProduct", False),
        ("mockBridge", True),
        ("simultaneousStarted", False),
        ("cancelKeptRow", False),
        ("finalDomRows", 1),
        ("finalBackendRows", 1),
    ]
    caught = 0
    for field, value in injections:
        broken = dict(green)
        broken[field] = value
        if not evidenceProblems(broken, providers):
            raise Failed(f"selftest defect: {field} could not make evidence red")
        caught += 1

    for command, value in (("start", 1), ("resume", 1), ("close", 3), ("prompt", 1)):
        broken = dict(green)
        broken["invokes"] = dict(green["invokes"])
        broken["invokes"][command] = value
        if not evidenceProblems(broken, providers):
            raise Failed(f"selftest defect: {command} invocation defect escaped")
        caught += 1

    for field, value in (
        ("resumeNativeMatched", False),
        ("badgesMatched", False),
        ("deleted", False),
        ("resumedSession", first),
    ):
        broken = dict(green)
        broken_entries = [dict(entry) for entry in green["providers"]]
        broken_entries[0][field] = value
        broken["providers"] = broken_entries
        if not evidenceProblems(broken, providers):
            raise Failed(f"selftest defect: provider field {field} escaped")
        caught += 1

    descriptors = {
        "provider-a": ("Provider A", [Path(sys.executable).name]),
        "provider-b": ("Provider B", [Path(sys.executable).name]),
    }
    with tempfile.TemporaryDirectory(prefix="runtrol-real-gui-selftest-") as raw:
        workspace = Path(raw)
        target_green = {
            "schema": 1,
            "targets": [
                {"provider": "provider-a", "native": "native-a", "workspace": str(workspace)},
                {"provider": "provider-b", "native": "native-b", "workspace": str(workspace)},
            ],
        }
        if targetProblems(target_green, descriptors):
            raise Failed("selftest defect: complete target prerequisites were rejected")
        target_injections = [
            {"schema": 2, "targets": target_green["targets"]},
            {"schema": 1, "targets": target_green["targets"][:1]},
            {
                "schema": 1,
                "targets": [target_green["targets"][0], target_green["targets"][0]],
            },
            {
                "schema": 1,
                "targets": [
                    target_green["targets"][0],
                    {"provider": "provider-b", "native": "bad\nvalue", "workspace": str(workspace)},
                ],
            },
            {
                "schema": 1,
                "targets": [
                    target_green["targets"][0],
                    {"provider": "provider-b", "native": "native-b", "workspace": str(workspace / "missing")},
                ],
            },
        ]
        for index, broken in enumerate(target_injections):
            if not targetProblems(broken, descriptors):
                raise Failed(f"selftest defect: target prerequisite {index + 1} escaped")
            caught += 1

    stable = ("1" * 64, "2" * 64, "3" * 64)
    if shell.changedEvidence(stable, stable):
        raise Failed("selftest defect: stable production evidence was rejected")
    if not shell.changedEvidence(stable, ("4" * 64, stable[1], stable[2])):
        raise Failed("selftest defect: changed production evidence escaped")
    caught += 1

    if shutil.which("node") is None:
        raise Failed("selftest needs node to prove the CDP driver can fail")
    node = subprocess.run(
        ["node", str(DRIVER), "--selftest"],
        cwd=UI,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=30.0,
        check=False,
    )
    if node.returncode != 0:
        raise Failed("the CDP driver selftest failed")
    caught += 1
    print(
        f"[desktopRealProviderGuiSmoke --selftest] OK. {caught} prerequisite and evidence defects "
        "made the operator gate red."
    )


def main(argv: list[str]) -> int:
    """Run the pure failure proof or the explicit operator journey."""
    try:
        known = {"--selftest", "--built-product"}
        unknown = sorted(set(argv).difference(known))
        if unknown:
            raise Failed("unknown argument")
        if "--selftest" in argv:
            if "--built-product" in argv:
                raise Failed("--selftest and --built-product are separate actions")
            selftest()
            return 0
        target_value = os.environ.get(TARGETS_ENV)
        if not target_value:
            raise Failed(f"{TARGETS_ENV} must name the external metadata target file")
        exercise("--built-product" in argv, Path(target_value).expanduser().resolve())
        return 0
    except (Failed, shell.Failed, acp.Failed, OSError, subprocess.SubprocessError) as error:
        print(f"[desktopRealProviderGuiSmoke] FAIL. {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
