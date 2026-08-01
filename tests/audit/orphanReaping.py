"""Gate: killing a supervising parent does not leave its child running.

The real process test belongs to `runtrol-childproc`, beside the platform containment code. This named gate
invokes that test so the security board and every runner can require the guarantee without copying it.
"""

from __future__ import annotations

import subprocess
import sys
from collections.abc import Callable
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
COMMAND = ["cargo", "test", "-p", "runtrol-childproc", "--test", "containment"]
Runner = Callable[..., subprocess.CompletedProcess[object]]


def runGate(runner: Runner = subprocess.run) -> int:
    """Run the process containment test and preserve its result."""
    completed = runner(COMMAND, cwd=ROOT, check=False)
    return completed.returncode


def selftest() -> int:
    """Prove a failed process test makes this gate fail."""
    calls: list[tuple[list[str], Path, bool]] = []

    def fails(command: list[str], *, cwd: Path, check: bool) -> subprocess.CompletedProcess[object]:
        calls.append((command, cwd, check))
        return subprocess.CompletedProcess(command, 19)

    result = runGate(fails)
    if result != 19 or calls != [(COMMAND, ROOT, False)]:
        print("[orphanReaping --selftest] FAIL. a surviving child did not make the gate red.", file=sys.stderr)
        return 2
    print("[orphanReaping --selftest] OK. a containment failure exits red.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or the real process smoke."""
    return selftest() if "--selftest" in argv else runGate()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
