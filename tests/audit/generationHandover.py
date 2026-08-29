"""Gate: a conversation kept alive across an update still opens, in the generation that owns it.

The product's promise (`docs/terminalSurface.md`, generation continuity): an update leaves the previous Runtime
generation draining beside the new one for as long as its terminals live, and a window that did not exist before
attaches to such a terminal in that exact generation. For four days every draining generation refused that
attach at its first request ("Runtime authorization audit storage is unavailable") while every gate stayed
green, because no gate ever made a new public connection to a draining generation. This one does.

The journey, with two real daemons of different bytes in one isolated home and no window anywhere:

1. generation A serves; a public client enrolls and opens a hosted terminal (the ACP fixture in its terminal mode)
2. generation B starts; A drains and hands its store over
3. a brand new public connection attaches to A's terminal, in A, and reads a screen
4. the terminal is stopped through A, and A, with nothing left to serve, leaves the locator

Usage::

    python -X utf8 tests/audit/generationHandover.py --selftest
    python -X utf8 tests/audit/generationHandover.py
    python -X utf8 tests/audit/generationHandover.py --core-a target/debug/runtrol.exe --core-b <other build>
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import genericAcpSmoke as acp  # noqa: E402
from resilienceFaultInjection import descendantIdentities, emergencyCleanup  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PROVIDER = "fixture-tui"
TIMEOUT_S = 240.0
SETTLE_S = 30.0
POLL_S = 0.25


class Failed(Exception):
    """One named gate failure."""


def evidenceProblems(evidence: dict[str, Any]) -> list[str]:
    """Return every promise the journey did not keep, from one evidence record."""
    found: list[str] = []
    a = evidence.get("generationA")
    b = evidence.get("generationB")
    if not isinstance(a, str) or not isinstance(b, str) or a == b:
        found.append("two distinct Runtime generations were not observed")
    opened = evidence.get("opened")
    if not isinstance(opened, dict) or opened.get("generation") != a:
        found.append("the terminal was not opened in generation A")
    if not evidence.get("aDrainingAfterB"):
        found.append("generation A did not drain after generation B started")
    attached = evidence.get("attached")
    if not isinstance(attached, dict):
        found.append("no new connection attached to the terminal after the handover")
    else:
        if attached.get("generation") != a:
            found.append("the attach did not land in generation A")
        if attached.get("draining") is not True:
            found.append("the attach did not happen while generation A was draining")
        if attached.get("listed") is not True:
            found.append("generation A did not list the terminal to the new connection")
        if attached.get("processState") != "Running":
            found.append("the terminal was not running when the new connection attached")
        if not isinstance(attached.get("screenBytes"), int) or attached["screenBytes"] <= 0:
            found.append("the new connection received no screen")
    if not evidence.get("aGoneAfterStop"):
        found.append("generation A did not leave the locator after its last terminal stopped")
    return found


def selftest() -> int:
    good = {
        "generationA": "a" * 64,
        "generationB": "b" * 64,
        "opened": {"generation": "a" * 64, "terminalId": "x"},
        "aDrainingAfterB": True,
        "attached": {
            "generation": "a" * 64,
            "draining": True,
            "listed": True,
            "processState": "Running",
            "screenBytes": 27,
        },
        "aGoneAfterStop": True,
    }
    if evidenceProblems(good):
        print("selftest: a complete journey was reported incomplete", file=sys.stderr)
        return 1
    refused = dict(good)
    refused.pop("attached")
    if "no new connection attached to the terminal after the handover" not in evidenceProblems(refused):
        print("selftest: a refused attach was not caught", file=sys.stderr)
        return 1
    wrong = dict(good, attached=dict(good["attached"], generation="b" * 64))
    if "the attach did not land in generation A" not in evidenceProblems(wrong):
        print("selftest: an attach redirected to the new generation was not caught", file=sys.stderr)
        return 1
    stuck = dict(good, aGoneAfterStop=False)
    if "generation A did not leave the locator after its last terminal stopped" not in evidenceProblems(stuck):
        print("selftest: a generation that never finished draining was not caught", file=sys.stderr)
        return 1
    print("generationHandover selftest OK")
    return 0


def manifest(home: Path, fixture: Path) -> None:
    """Declare the fixture as a provider with a terminal interface: its `--tui` mode."""
    providers = home / "providers"
    providers.mkdir(parents=True, exist_ok=True)
    text = f'''schema = 1
id = "{PROVIDER}"
display_name = "Terminal Fixture"
kind = "acp"

[bin]
names = ["{fixture.name}"]

[probe]
version = {{ args = ["--version"], parse = "semver-anywhere" }}

[transport]
argv = []
listen = "stdio"

[tui]
new = ["--tui"]
'''
    (providers / f"{PROVIDER}.toml").write_text(text, encoding="utf-8")


def buildCores(coreA: Path | None, coreB: Path | None) -> tuple[Path, Path, Path]:
    """The two Runtime builds and the probe. Bytes must differ, so B is built with another codegen split."""
    runtrol, fixture = acp.build()
    built = subprocess.run(
        ["cargo", "build", "-p", "runtrol", "--example", "handoverProbe"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        timeout=TIMEOUT_S * 4,
    )
    if built.returncode != 0:
        raise Failed((built.stderr or built.stdout or "cargo build of the probe failed").strip())
    suffix = ".exe" if sys.platform == "win32" else ""
    probe = ROOT / "target" / "debug" / "examples" / f"handoverProbe{suffix}"
    if not probe.is_file():
        raise Failed(f"cargo succeeded but {probe.relative_to(ROOT)} is missing")
    a = coreA or runtrol
    if coreB is None:
        environment = dict(os.environ)
        environment["RUSTFLAGS"] = "-C codegen-units=1"
        other = subprocess.run(
            ["cargo", "build", "-p", "runtrol", "--bin", "runtrol", "--target-dir", str(ROOT / "target" / "handover-b")],
            cwd=ROOT,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_S * 10,
        )
        if other.returncode != 0:
            raise Failed((other.stderr or other.stdout or "cargo build of generation B failed").strip())
        coreB = ROOT / "target" / "handover-b" / "debug" / f"runtrol{suffix}"
    if a.read_bytes() == coreB.read_bytes():
        raise Failed("the two Runtime builds are byte-identical, so they would be one generation")
    return a, coreB, probe


def statusOf(core: Path, environment: dict[str, str]) -> list[dict[str, Any]]:
    """The generation list as the product reports it."""
    said = acp.command(core, environment, ["status", "--json"])
    parsed = json.loads(said)
    generations = parsed.get("generations", parsed) if isinstance(parsed, dict) else parsed
    if not isinstance(generations, list):
        raise Failed(f"status --json did not list generations: {said[:200]}")
    return generations


def waitForStatus(core: Path, environment: dict[str, str], wanted, what: str) -> list[dict[str, Any]]:
    """Poll the generation list until `wanted` says it is right."""
    deadline = time.monotonic() + SETTLE_S
    last: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        try:
            last = statusOf(core, environment)
        except (Failed, json.JSONDecodeError):
            last = []
        if wanted(last):
            return last
        time.sleep(POLL_S)
    raise Failed(f"waited {SETTLE_S:.0f}s for {what}; last status: {last}")


def digestOf(generations: list[dict[str, Any]], *, draining: bool | None = None) -> str | None:
    for generation in generations:
        if draining is None or bool(generation.get("draining")) is draining:
            digest = generation.get("digest")
            if isinstance(digest, str):
                return digest
    return None


def probe(binary: Path, environment: dict[str, str], words: list[str]) -> dict[str, Any]:
    """Run one probe phase as its own process, which is its own public connection."""
    proc = subprocess.run(
        [str(binary), *words],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=TIMEOUT_S,
        check=False,
    )
    if proc.returncode != 0:
        raise Failed(f"handoverProbe {words[0]} failed: {(proc.stderr or proc.stdout).strip()}")
    line = (proc.stdout or "").strip().splitlines()[-1] if (proc.stdout or "").strip() else ""
    try:
        return json.loads(line)
    except json.JSONDecodeError as error:
        raise Failed(f"handoverProbe {words[0]} answered no JSON: {proc.stdout!r}") from error


def exercise(coreA: Path | None, coreB: Path | None) -> dict[str, Any]:
    """Drive the journey and return its evidence."""
    a, b, probeBinary = buildCores(coreA, coreB)
    _, fixture = acp.build()
    evidence: dict[str, Any] = {}
    with tempfile.TemporaryDirectory(prefix="runtrol-handover-") as raw:
        root = Path(raw)
        home = root / "home"
        workspace = root / "workspace"
        workspace.mkdir()
        home.mkdir()
        manifest(home, fixture)
        environment = acp.environment(home, fixture)
        identity = root / "probe-identity.json"
        first = acp.startDaemon(a, environment, home)
        second: subprocess.Popen[str] | None = None
        owned: dict[int, Any] = {}
        try:
            current = waitForStatus(a, environment, lambda gens: digestOf(gens, draining=False) is not None, "generation A")
            generationA = digestOf(current, draining=False)
            evidence["generationA"] = generationA
            evidence["enrolled"] = probe(probeBinary, environment, ["enroll", str(home), str(a), str(identity), str(workspace)])
            opened = probe(probeBinary, environment, ["open", str(home), str(identity), PROVIDER, str(workspace)])
            evidence["opened"] = opened
            owned.update(descendantIdentities(first.pid))

            second = acp.startDaemon(b, environment, home)
            after = waitForStatus(
                b,
                environment,
                lambda gens: len(gens) >= 2 and digestOf(gens, draining=False) not in (None, generationA)
                and any(g.get("digest") == generationA and g.get("draining") for g in gens),
                "generation B current and generation A draining",
            )
            evidence["generationB"] = digestOf(after, draining=False)
            evidence["aDrainingAfterB"] = any(g.get("digest") == generationA and g.get("draining") for g in after)

            # The promise itself: a connection that never existed before reaches the terminal in A.
            evidence["attached"] = probe(
                probeBinary, environment, ["attach", str(home), str(identity), generationA, str(opened["terminalId"])]
            )

            evidence["stopped"] = probe(
                probeBinary, environment, ["stop", str(home), str(identity), generationA, str(opened["terminalId"])]
            )
            gone = waitForStatus(
                b,
                environment,
                lambda gens: all(g.get("digest") != generationA for g in gens),
                "generation A to leave the locator",
            )
            evidence["aGoneAfterStop"] = all(g.get("digest") != generationA for g in gone)
        finally:
            errors: list[str] = []
            for daemon in (first, second):
                if daemon is None:
                    continue
                try:
                    if daemon.poll() is None:
                        owned.update(descendantIdentities(daemon.pid))
                        acp.stopDaemon(daemon)
                except (Failed, OSError, ValueError, subprocess.SubprocessError) as error:
                    errors.append(str(error))
            try:
                emergencyCleanup(owned)
            except (Failed, OSError, ValueError, subprocess.SubprocessError) as error:
                errors.append(str(error))
            if errors:
                raise Failed("; ".join(errors))
    return evidence


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    coreA = Path(argv[argv.index("--core-a") + 1]) if "--core-a" in argv else None
    coreB = Path(argv[argv.index("--core-b") + 1]) if "--core-b" in argv else None
    try:
        evidence = exercise(coreA, coreB)
    except Failed as error:
        print(f"generationHandover FAILED: {error}", file=sys.stderr)
        return 2
    problems = evidenceProblems(evidence)
    print(json.dumps(evidence, ensure_ascii=False, indent=2))
    if problems:
        for problem in problems:
            print(f"generationHandover FAILED: {problem}", file=sys.stderr)
        return 2
    print("generationHandover OK: a new connection attached to the draining generation's terminal")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
