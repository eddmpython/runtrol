"""Gate: the desktop frontend type-checks and produces a production bundle.

The local preflight invokes commands without a shell. On Windows, npm is a command file and must be
launched through the operating system command processor. Keeping that platform detail here gives local and
hosted runners one gate entry point and one result.

Usage::

    python -X utf8 tests/audit/frontendBuild.py --selftest
    python -X utf8 tests/audit/frontendBuild.py
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from collections.abc import Callable, Sequence
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
UI = "crates/runtrol-gui/ui"
Runner = Callable[..., subprocess.CompletedProcess[object]]


def npmCommand(platform: str, npm: str) -> list[str]:
    """Return an argv that can launch npm without an implicit shell."""
    if platform == "win32":
        command = os.environ.get("ComSpec", r"C:\Windows\System32\cmd.exe")
        return [command, "/d", "/c", npm]
    return [npm]


def runBuild(prefix: Sequence[str], runner: Runner = subprocess.run) -> int:
    """Run the bundle command and preserve its exit status."""
    completed = runner(
        [*prefix, "--prefix", UI, "run", "build"],
        cwd=ROOT,
        check=False,
    )
    return completed.returncode


def selftest() -> int:
    """Prove the gate reports a failed build and invokes the intended command."""
    calls: list[tuple[list[str], Path]] = []

    def fails(command: list[str], *, cwd: Path, check: bool) -> subprocess.CompletedProcess[object]:
        calls.append((command, cwd))
        if check:
            raise AssertionError("the gate must preserve the process result")
        return subprocess.CompletedProcess(command, 17)

    result = runBuild(["npm"], fails)
    expected = ["npm", "--prefix", UI, "run", "build"]
    if result != 17 or calls != [(expected, ROOT)]:
        print("[frontendBuild --selftest] FAIL. a broken bundle did not make the gate red.", file=sys.stderr)
        return 2

    windows = npmCommand("win32", r"C:\tools\npm.cmd")
    unix = npmCommand("linux", "/usr/bin/npm")
    if windows[-3:] != ["/d", "/c", r"C:\tools\npm.cmd"] or unix != ["/usr/bin/npm"]:
        print("[frontendBuild --selftest] FAIL. npm launch routing is wrong.", file=sys.stderr)
        return 2

    print("[frontendBuild --selftest] OK. a failed bundle exits red on Windows and Unix.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or the real frontend build."""
    if "--selftest" in argv:
        return selftest()

    wanted = "npm.cmd" if sys.platform == "win32" else "npm"
    npm = shutil.which(wanted) or shutil.which("npm")
    if npm is None:
        print("[frontendBuild] npm is missing; the desktop frontend was not verified.", file=sys.stderr)
        return 2
    return runBuild(npmCommand(sys.platform, npm))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
