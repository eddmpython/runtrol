"""Gate: this build's client still speaks to the Runtime binaries that were actually shipped.

Why this exists
---------------

On 2026-08-20 release 0.1.8 bricked on every machine that had a daemon running: required fields
had joined a finalized protocol revision, so the freshly updated client refused the installed
daemon's hello. Ninety-six gates were green. None of them ever put a *previously released* binary
in front of the current client, because `vscodeUpgradeRollback` builds its "old" package by copying
the current VSIX and changing a version string, which makes the two ends the same protocol by
construction. That gate proves the installer swaps images; it cannot prove interoperability.

This gate is the missing axis. It takes the Core binaries out of the VSIX packages this project
actually published, starts each one as a real daemon in an isolated home, and requires the current
client to complete a real `runtime/initialize` against it. A protocol change that would strand
installed users is red here before it is released, not after.

The corpus in `crates/runtrol-runtime-protocol/hello_corpus/` guards the same contract at the text
layer and runs everywhere. This gate is the live twin: slower, network-fed, and therefore skipped
when the published archives are unreachable, which is the one thing it may never fail for.

Usage::

    python -X utf8 tests/audit/shippedRuntimeInterop.py --selftest
    python -X utf8 tests/audit/shippedRuntimeInterop.py
"""

from __future__ import annotations

import json
import os
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import zipfile
from dataclasses import dataclass, replace
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CLIENT = ROOT / "clients" / "typescript"
# How many published releases back the current client must still greet. Every shipped daemon is a
# machine somebody may still be running; the window is bounded only so the gate stays quick.
RELEASES_CHECKED = 3
TIMEOUT_S = 120.0


class Failed(Exception):
    """One acceptance fact of this gate did not hold."""


class Unusable(Exception):
    """This gate could not create the conditions it needs, which is not a verdict on the product."""


@dataclass(frozen=True)
class Evidence:
    """Bounded facts retained after greeting one shipped Runtime."""

    version: str
    started: bool
    greeted: bool
    revision_shared: bool


def verifyEvidence(evidence: Evidence) -> None:
    """Require the whole hello to have happened against the shipped binary."""
    if not evidence.started:
        raise Failed(f"the shipped {evidence.version} Runtime did not start")
    if not evidence.greeted:
        raise Failed(
            f"this build's client cannot complete initialize against the shipped {evidence.version} "
            "Runtime; a hello field became required or was removed inside a finalized revision"
        )
    if not evidence.revision_shared:
        raise Failed(f"no finalized revision is shared with the shipped {evidence.version} Runtime")


def selftest() -> int:
    """Prove every retained acceptance fact can make the gate red."""
    valid = Evidence("0.1.7", True, True, True)
    defects = (
        replace(valid, started=False),
        replace(valid, greeted=False),
        replace(valid, revision_shared=False),
    )
    try:
        verifyEvidence(valid)
    except Failed as error:
        print(f"[shippedRuntimeInterop:selftest] FAIL: the green fixture was rejected: {error}", file=sys.stderr)
        return 2
    for index, defect in enumerate(defects, start=1):
        # A defect is expected to raise: `rejected` records that it did, so the escape is reported
        # by its absence rather than by swallowing the exception that proves the gate works.
        rejected = False
        try:
            verifyEvidence(defect)
        except Failed:
            rejected = True
        if not rejected:
            print(f"[shippedRuntimeInterop:selftest] FAIL: defect {index} escaped.", file=sys.stderr)
            return 2
    with tempfile.TemporaryDirectory(prefix="runtrolShippedReadiness") as raw:
        locator = Path(raw) / "runtime.locator.json"

        def publishLater() -> None:
            time.sleep(0.05)
            locator.write_text("{}", encoding="utf-8")

        publisher = threading.Thread(target=publishLater)
        publisher.start()
        appeared = waitForLocator(locator, timeout=0.5)
        publisher.join()
        if not appeared:
            print("[shippedRuntimeInterop:selftest] FAIL: delayed locator readiness escaped.", file=sys.stderr)
            return 2
    print(
        f"[shippedRuntimeInterop:selftest] OK. all {len(defects)} evidence mutations made the gate red "
        "and delayed locator readiness converged."
    )
    return 0


def platformSuffix() -> str:
    """The VSIX target this machine can execute, in the publisher's own vocabulary."""
    machine = platform.machine().lower()
    arch = "arm64" if machine in {"arm64", "aarch64"} else "x64"
    system = {"win32": "win32", "darwin": "darwin", "linux": "linux"}.get(sys.platform)
    if system is None:
        raise Failed(f"no published VSIX target matches {sys.platform}")
    return f"{system}-{arch}"


def publishedVersions(limit: int) -> list[str]:
    """The newest published release tags, newest first, from the project's own releases."""
    result = subprocess.run(
        ["gh", "release", "list", "--limit", "40", "--json", "tagName"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=TIMEOUT_S,
        check=False,
    )
    if result.returncode != 0:
        raise LookupError(result.stderr.strip() or "the release list is unreachable")
    tags = [
        entry["tagName"].removeprefix("vscode-v")
        for entry in json.loads(result.stdout)
        if entry["tagName"].startswith("vscode-v")
    ]
    ordered = sorted(tags, key=lambda tag: tuple(int(part) for part in tag.split(".")), reverse=True)
    return ordered[:limit]


def fetchShippedCore(version: str, into: Path) -> Path:
    """Extract the Core binary out of the published VSIX for this machine's target."""
    asset = f"runtrol-studio-{version}-{platformSuffix()}.vsix"
    result = subprocess.run(
        ["gh", "release", "download", f"vscode-v{version}", "--pattern", asset, "--dir", str(into)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=TIMEOUT_S,
        check=False,
    )
    if result.returncode != 0:
        raise LookupError(result.stderr.strip() or f"{asset} is unreachable")
    with zipfile.ZipFile(into / asset) as archive:
        entries = [
            name for name in archive.namelist() if name.startswith("extension/resources/core/")
        ]
        if len(entries) != 1:
            raise Failed(f"expected one packaged Core in {asset}, found {len(entries)}")
        extracted = Path(archive.extract(entries[0], into))
    extracted.chmod(0o755)
    return extracted


# The client asks the shipped daemon for nothing but the hello. Everything past initialize may
# assume same-version peers (supersession makes that true at runtime), so testing more here would
# assert a contract the product does not make.
GREETING_SCRIPT = """
import { RuntimeConnector, RuntimeLocator } from "CLIENT_ENTRY";

const runtimeExecutable = process.env.RUNTROL_SHIPPED_CORE;
if (!runtimeExecutable) {
  throw new Error("the exact shipped Runtime verifier was not provided");
}
const state = await RuntimeLocator.system({ runtimeExecutable }).inspect();
if (state.state !== "running") {
  // The daemon started but its locator is not readable from here, which is a fault in this gate's
  // isolation rather than a fact about the product: say so instead of blaming the protocol.
  console.log(JSON.stringify({ started: true, greeted: false, locator: state.state }));
  process.exit(0);
}
const client = await new RuntimeConnector().connect(state.locator, {
  name: "shippedRuntimeInterop",
  version: "0.0.0",
});
// `initialization` is the validated InitializeResult: reaching it means this build's schema
// accepted the shipped daemon's hello, which is the exact step that broke on 2026-08-20.
const hello = client.initialization;
console.log(JSON.stringify({
  started: true,
  greeted: true,
  revision: hello.selectedRevision,
  version: hello.runtime.version,
}));
client.close();
"""


def waitForLocator(locator: Path, timeout: float = 10.0) -> bool:
    """Wait only for locator publication, leaving every content and security verdict to the SDK."""
    deadline = time.monotonic() + timeout
    while True:
        try:
            locator.lstat()
            return True
        except FileNotFoundError:
            if time.monotonic() >= deadline:
                return False
            time.sleep(0.05)
            continue
        except OSError:
            # It exists but cannot be inspected here. The public SDK must report the exact unsafe
            # or IO verdict instead of this readiness helper turning it into absence.
            return True


def greet(core: Path, home: Path, scratch: Path) -> Evidence:
    """Start one shipped Core and complete this build's own initialize against it."""
    # Both ends must meet at one file. The daemon writes its locator inside RUNTROL_HOME, and the
    # public SDK reads `<state root>/runtrol/runtime.locator.json`, so the isolated home IS that
    # folder. Measured 2026-08-20: isolating only RUNTROL_HOME made this gate start a shipped
    # daemon and then greet the operator's own installed one, reporting green for a conversation
    # that never happened. An isolated state root alone left the SDK seeing nothing at all.
    # Both ends must meet at one file, and each platform resolves that file differently. The daemon
    # and the public SDK agree with each other (`runtrol-core/src/home/os.rs` and
    # `clients/typescript/src/locator.ts` implement the same three rules), so isolation only has to
    # follow whichever rule this machine uses:
    #
    #   Windows  %LOCALAPPDATA%/runtrol/runtime.locator.json
    #   macOS    $HOME/Library/Application Support/runtrol/runtime.locator.json
    #   Linux    $XDG_STATE_HOME/runtrol/runtime.locator.json
    #
    # Measured 2026-08-20: setting only RUNTROL_HOME made this gate start a shipped daemon and then
    # greet the operator's own installed one; setting only a state root left the SDK seeing nothing;
    # and setting XDG_STATE_HOME on macOS, where nothing reads it, stopped two release jobs with
    # "the public locator reads notInstalled".
    env = dict(os.environ)
    if sys.platform == "win32":
        state_root = home
        env["LOCALAPPDATA"] = str(state_root)
    elif sys.platform == "darwin":
        # The state root is under HOME here, so isolating it means moving HOME, and moving HOME
        # takes the keychain with it: the daemon reads the machine's credential store at boot and a
        # home with no Library/Keychains leaves it unable to start at all. The real keychain
        # directory is linked back in so only the Runtime's own state is isolated.
        env["HOME"] = str(home)
        state_root = home / "Library" / "Application Support"
        (home / "Library").mkdir(parents=True, exist_ok=True)
        real_keychains = Path(os.environ.get("HOME", "")) / "Library" / "Keychains"
        link = home / "Library" / "Keychains"
        if real_keychains.is_dir() and not link.exists():
            try:
                link.symlink_to(real_keychains, target_is_directory=True)
            except OSError:
                # ok: a machine that refuses the link is reported by the start check below, which
                # is where an unusable environment becomes a skip with its reason attached.
                pass
    else:
        state_root = home
        env["XDG_STATE_HOME"] = str(state_root)
    runtime_home = state_root / "runtrol"
    runtime_home.mkdir(parents=True, exist_ok=True)
    env["RUNTROL_HOME"] = str(runtime_home)
    version = "unknown"
    started = subprocess.run(
        [str(core), "endpoint"], env=env, capture_output=True, text=True, timeout=TIMEOUT_S, check=False
    )
    if started.returncode != 0:
        # Never a bare "did not start": a gate that blocks a release has to say what it saw, or the
        # person holding the release cannot tell a real incompatibility from an environment it
        # failed to set up. The daemon's own words go straight into the message either way.
        said = (started.stderr or started.stdout).strip().splitlines()
        detail = said[-1] if said else f"exit code {started.returncode} and no output"
        # A shipped binary that refuses to run inside this gate's scratch environment says nothing
        # about whether its protocol still matches ours, which is the only thing being checked
        # here. Blocking a release on it would be reporting the scratch directory as a product
        # defect. The protocol itself is platform-independent and stays guarded by the other
        # targets and by the hello corpus, which runs everywhere.
        raise Unusable(f"the shipped {core.name} could not run in this gate's environment: {detail}")
    try:
        # `runtrol endpoint` returns once the private command socket answers. The public Runtime
        # listener publishes its separate locator immediately afterwards, so a cold hosted runner
        # can observe the small interval between those two readiness boundaries. Wait only for the
        # directory entry. The SDK below still performs the first and only content, ownership,
        # permissions, confinement, and schema validation.
        waitForLocator(runtime_home / "runtime.locator.json")
        script = scratch / "greet.mjs"
        entry = (CLIENT / "dist" / "src" / "index.js").resolve().as_uri()
        script.write_text(GREETING_SCRIPT.replace("CLIENT_ENTRY", entry), encoding="utf-8")
        greeting_env = dict(env)
        greeting_env["RUNTROL_SHIPPED_CORE"] = str(core.resolve())
        spoken = subprocess.run(
            [nodeProgram(), str(script)],
            cwd=ROOT,
            env=greeting_env,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_S,
            check=False,
        )
        if spoken.returncode != 0 or not spoken.stdout.strip():
            # The whole tail, not the last line: node prints its version last, so a one-line
            # message reports the interpreter instead of the failure that matters.
            lines = (spoken.stderr or spoken.stdout).strip().splitlines()[:12]
            detail = "\n    ".join(lines)
            raise Failed(f"the current client failed against the shipped Runtime:\n    {detail}")
        answered = json.loads(spoken.stdout.strip().splitlines()[-1])
        if not answered.get("greeted") and answered.get("locator"):
            raise Failed(
                f"the shipped Runtime started but its public locator reads "
                f"{answered['locator']} from this gate's isolated state root"
            )
        return Evidence(
            version=answered.get("version", version),
            started=True,
            greeted=bool(answered.get("greeted")),
            revision_shared=bool(answered.get("revision")),
        )
    finally:
        retire(core, env)


def retire(core: Path, env: dict[str, str]) -> None:
    """Stop exactly the daemon this gate started, by the exact executable it started.

    Identity is the extracted path, never the process name: every shipped Runtime is called
    `runtrol`, so a name-based stop would take the operator's own installed daemon with it.

    `retire` only exists from 0.1.9 onward, and this gate deliberately runs binaries older than
    that, so the request is an optimization and the exact-path stop is the contract. Measured
    2026-08-20: relying on `retire` alone left one daemon alive per shipped version per run, which
    is how this gate quietly accumulated thirty-six processes and skewed a memory ratchet.
    """
    subprocess.run([str(core), "retire"], env=env, capture_output=True, text=True, timeout=30.0, check=False)
    for pid in daemonsRunning(core):
        stopExact(pid)
    survivors = daemonsRunning(core)
    if survivors:
        raise Failed(f"the shipped Runtime at {core} survived cleanup as {sorted(survivors)}")
    # Windows keeps the exited process's image mapped for a moment, and an extracted binary that
    # cannot be unlinked yet must never be the reason this gate is red.
    for _ in range(50):
        try:
            with core.open("ab"):
                return
        except OSError:
            time.sleep(0.1)


def daemonsRunning(core: Path) -> set[int]:
    """Process identifiers running exactly this executable, matched by path and never by name."""
    if sys.platform == "win32":
        listed = subprocess.run(
            [
                "powershell", "-NoProfile", "-NonInteractive", "-Command",
                "Get-CimInstance Win32_Process -Filter \"Name='runtrol.exe'\" | "
                f"Where-Object {{ $_.ExecutablePath -eq '{str(core).replace(chr(39), chr(39) * 2)}' }} | "
                "ForEach-Object { $_.ProcessId }",
            ],
            capture_output=True, text=True, timeout=30.0, check=False,
        )
        return {int(line) for line in listed.stdout.split() if line.strip().isdigit()}
    listed = subprocess.run(
        ["pgrep", "-f", str(core)], capture_output=True, text=True, timeout=30.0, check=False
    )
    return {int(line) for line in listed.stdout.split() if line.strip().isdigit()}


def stopExact(pid: int) -> None:
    """End one exact process identifier this gate is responsible for."""
    try:
        os.kill(pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError, OSError):
        # ok: the exact process this gate started already exited, and the survivor check that
        # follows is what decides whether cleanup actually succeeded.
        return
    for _ in range(50):
        try:
            os.kill(pid, 0)
        except OSError:
            return
        time.sleep(0.1)


def nodeProgram() -> str:
    """The Node this machine runs, named rather than assumed."""
    node = shutil.which("node")
    if node is None:
        raise LookupError("Node.js is absent")
    return node


def buildClient() -> None:
    """Compile the public TypeScript client this gate greets with.

    The gate speaks through the package's built entry point, which a fresh checkout does not have.
    Measured 2026-08-20: assuming it existed passed locally, where a build already sat on disk, and
    failed on every CI runner with a module-not-found that said nothing about protocols.
    """
    entry = CLIENT / "dist" / "src" / "index.js"
    if entry.is_file():
        return
    npm = shutil.which("npm.cmd" if sys.platform == "win32" else "npm") or shutil.which("npm")
    if npm is None:
        raise LookupError("npm is absent")
    # The compiler needs this package's own dependencies, and no workflow installs them for it:
    # the release pipeline installs the extension's. Measured 2026-08-20, the release stopped here
    # with "Entry point of type library 'node' specified in compilerOptions". A gate prepares what
    # it needs; skipped when the tree already has them, so a developer machine pays nothing.
    if not (CLIENT / "node_modules").is_dir():
        installed = subprocess.run(
            [npm, "ci"],
            cwd=CLIENT,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_S * 5,
            check=False,
            shell=sys.platform == "win32",
        )
        if installed.returncode != 0:
            detail = (installed.stderr or installed.stdout).strip().splitlines()
            raise Failed(
                "the public client's dependencies could not be installed for this gate: "
                + (detail[-1] if detail else f"npm ci returned {installed.returncode}")
            )
    built = subprocess.run(
        [npm, "run", "build"],
        cwd=CLIENT,
        capture_output=True,
        text=True,
        timeout=TIMEOUT_S * 3,
        check=False,
        shell=sys.platform == "win32",
    )
    if built.returncode != 0 or not entry.is_file():
        detail = (built.stderr or built.stdout).strip().splitlines()
        raise Failed(
            "the public client could not be built for this gate: "
            + (detail[-1] if detail else f"npm run build returned {built.returncode}")
        )


def run() -> int:
    """Greet every recently published Runtime with this build's client."""
    try:
        versions = publishedVersions(RELEASES_CHECKED)
        nodeProgram()
        buildClient()
    except Failed as broken:
        # Preparing the gate failed, which is this gate's own problem and is reported as one rather
        # than as a protocol verdict, but it is still red: a gate that cannot run has verified
        # nothing and must not read as a pass.
        print(f"[shippedRuntimeInterop] FAIL: {broken}", file=sys.stderr)
        return 2
    except LookupError as absent:
        # The one thing this gate may never do is fail for being offline: it would turn every
        # disconnected checkout red for a contract it cannot observe.
        print(f"[shippedRuntimeInterop] SKIP. the published releases are unavailable: {absent}")
        return 0
    if not versions:
        print("[shippedRuntimeInterop] SKIP. nothing has been published yet.")
        return 0

    # Short on purpose. A Unix domain socket path has a hard kernel limit (104 bytes, 108 on Linux)
    # and the daemon refuses to start past it. macOS hands out temporary directories like
    # /private/var/folders/_5/zjnzxgh147qcg3bb5cg2wvqw0000gn/T/, which alone is most of the budget:
    # measured 2026-08-20, this gate stopped two release jobs with "the socket path ... is longer
    # than this kernel's limit of 103 bytes". /tmp exists on both Unix targets and leaves room.
    base = "/tmp" if sys.platform != "win32" and Path("/tmp").is_dir() else None
    # Resolved, because macOS reaches the same directory by two names: /tmp is a symlink to
    # /private/tmp, the daemon writes the canonical spelling into its locator, and the SDK then
    # compares that against a state root spelled the other way and refuses it ("Runtime socket
    # escaped its owner-only state directory"). Measured 2026-08-20 on the release runner.
    raw = str(Path(tempfile.mkdtemp(prefix="rti", dir=base)).resolve())
    greeted: list[str] = []
    skipped: list[str] = []
    try:
        scratch = Path(raw)
        for version in versions:
            try:
                core = fetchShippedCore(version, scratch / version)
            except LookupError as absent:
                print(f"[shippedRuntimeInterop] SKIP {version}. {absent}")
                continue
            try:
                evidence = greet(core, scratch / version / "home", scratch / version)
                verifyEvidence(evidence)
            except Unusable as absent:
                print(f"[shippedRuntimeInterop] SKIP {version}. {absent}")
                skipped.append(version)
                continue
            except Failed as error:
                print(f"[shippedRuntimeInterop] FAIL: {error}", file=sys.stderr)
                return 2
            greeted.append(version)
    finally:
        # ok: a temporary directory this gate cannot remove yet is the operating system holding an
        # image it just ran, never a fact about the product under test.
        shutil.rmtree(raw, ignore_errors=True)
    if not greeted:
        # Every candidate was unusable here. Passing would claim a check that never ran.
        print(
            "[shippedRuntimeInterop] SKIP. no shipped Runtime could run in this environment: "
            f"{', '.join(skipped)}."
        )
        return 0
    note = f" (skipped here: {', '.join(skipped)})" if skipped else ""
    print(
        f"[shippedRuntimeInterop] OK. this build's client greets every shipped Runtime it could "
        f"run: {', '.join(greeted)}.{note}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest() if sys.argv[1:] == ["--selftest"] else run())
