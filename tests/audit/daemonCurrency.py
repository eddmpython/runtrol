"""Gate: the daemon that serves a freshly installed Runtrol is the build that Runtrol bundles.

Measured 2026-08-23 to 2026-08-25 on the operator's machine: an installed update sat behind a Core from two days
earlier, because nothing ever proved that the daemon answering was the one the extension shipped. This gate is
that proof. It installs the exact VSIX into an isolated VS Code profile with an isolated Runtime home, lets the
extension activate and start its Core, then reads that home's own generation list (`runtrol status --json`) and
requires that the newest generation that is not draining runs exactly the bundled Core's digest, and answers.

The journey itself is the shipped-package journey `crossPlatformMatrix` drives; this gate reads the generation
evidence that journey now carries and judges only currency. It runs in the local release procedure and in CI.

Usage::

    python -X utf8 tests/audit/daemonCurrency.py --selftest
    python -X utf8 tests/audit/daemonCurrency.py
    python -X utf8 tests/audit/daemonCurrency.py --archive release/current-platform.vsix
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import crossPlatformMatrix as journey  # noqa: E402  (the same journey; this gate only judges currency)

DIGEST_LENGTH = 64


def currencyProblems(evidence: dict[str, Any]) -> list[str]:
    """Return every way the running generation fails to be the bundled build."""
    found: list[str] = []
    bundled = evidence.get("bundledDigest")
    if not isinstance(bundled, str) or len(bundled) != DIGEST_LENGTH:
        found.append("evidence carries no bundled Core digest")
        return found
    generations = evidence.get("generations")
    if not isinstance(generations, list):
        found.append("evidence carries no generation list")
        return found
    current = [
        generation for generation in generations
        if isinstance(generation, dict) and generation.get("draining") is False
    ]
    if not current:
        found.append("no generation is serving the isolated home")
        return found
    newest = max(current, key=lambda generation: (generation.get("startedAtMs", 0), generation.get("processId", 0)))
    if newest.get("digest") != bundled:
        found.append(
            f"the serving generation runs {str(newest.get('digest'))[:16]} and the VSIX bundles {bundled[:16]}"
        )
    if newest.get("answering") is not True:
        found.append("the serving generation does not answer on its control endpoint")
    return found


def selftest() -> int:
    """Prove the judgement can fail: a foreign, a draining-only, and an unanswering generation are red."""
    bundled = "a" * DIGEST_LENGTH
    serving = {"digest": bundled, "draining": False, "answering": True, "startedAtMs": 2, "processId": 2}
    green = currencyProblems({"bundledDigest": bundled, "generations": [serving]})
    foreign = currencyProblems({"bundledDigest": bundled, "generations": [{**serving, "digest": "b" * DIGEST_LENGTH}]})
    older_wins = currencyProblems({
        "bundledDigest": bundled,
        "generations": [serving, {**serving, "digest": "c" * DIGEST_LENGTH, "startedAtMs": 3}],
    })
    draining_only = currencyProblems({"bundledDigest": bundled, "generations": [{**serving, "draining": True}]})
    silent = currencyProblems({"bundledDigest": bundled, "generations": [{**serving, "answering": False}]})
    if green or not foreign or not older_wins or not draining_only or not silent:
        print("[daemonCurrency --selftest] the judgement did not fail where it must.", file=sys.stderr)
        return 2
    print("[daemonCurrency --selftest] OK. foreign, superseded, draining-only and silent generations are red.")
    return 0


def run(archive: Path | None) -> int:
    """Install the package, activate, and judge the generation that answers."""
    try:
        with tempfile.TemporaryDirectory(prefix="runtrol-daemon-currency-") as raw:
            package = archive if archive is not None else journey.buildArchive(Path(raw), journey.nativeTarget())
            if not package.is_file():
                raise journey.Failed(f"archive is absent: {package}")
            evidence = exercise(package)
            problems = currencyProblems(evidence)
            if problems:
                raise journey.Failed("the daemon serving the installed Runtrol is not its build:\n  - " + "\n  - ".join(problems))
            print(
                f"[daemonCurrency] OK. the installed VSIX activated and the serving Core generation is its bundled "
                f"build {str(evidence['bundledDigest'])[:16]}."
            )
    except (journey.Failed, OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"[daemonCurrency] FAIL: {error}", file=sys.stderr)
        return 2
    return 0


def exercise(archive: Path) -> dict[str, Any]:
    """Drive the shipped-package journey and return its evidence record."""
    import os
    import shutil

    node = shutil.which("node.exe" if sys.platform == "win32" else "node") or shutil.which("node")
    if node is None:
        raise journey.Failed("Node.js is required to launch the installed-package journey")
    program = [node, str(journey.INSTALLER), str(archive)]
    if sys.platform.startswith("linux") and not os.environ.get("DISPLAY"):
        xvfb = shutil.which("xvfb-run")
        if xvfb is None:
            raise journey.Failed("xvfb-run is required to test the installed VSIX without a Linux display")
        program = [xvfb, "-a", *program]
    return journey.readEvidence(journey.command(program, timeout=240.0))


def main(argv: list[str]) -> int:
    """Select defect injection or the live currency judgement."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--archive", type=Path)
    arguments = parser.parse_args(argv)
    if arguments.selftest:
        return selftest()
    return run(arguments.archive)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
