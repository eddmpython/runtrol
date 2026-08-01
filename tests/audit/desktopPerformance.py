"""Shared launcher for the two real-browser desktop performance gates."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
UI = ROOT / "crates" / "runtrol-gui" / "ui"
Runner = Callable[..., subprocess.CompletedProcess[str]]


def npmCommand() -> list[str]:
    """Return an explicit npm launcher for this operating system."""
    if sys.platform == "win32":
        return [os.environ.get("ComSpec", r"C:\Windows\System32\cmd.exe"), "/d", "/c", "npm"]
    return ["npm"]


def measurement(mode: str, runner: Runner = subprocess.run) -> tuple[dict[str, Any] | None, str]:
    """Build the production bundle and return one browser measurement."""
    built = runner(
        [*npmCommand(), "run", "build"],
        cwd=UI,
        capture_output=True,
        text=True,
        check=False,
    )
    if built.returncode != 0:
        return None, f"production bundle failed:\n{built.stdout}{built.stderr}"

    measured = runner(
        ["node", "tests/performance.mjs", mode],
        cwd=UI,
        capture_output=True,
        text=True,
        check=False,
    )
    if measured.returncode != 0:
        return None, f"browser measurement failed:\n{measured.stdout}{measured.stderr}"

    lines = [line for line in measured.stdout.splitlines() if line.strip()]
    if not lines:
        return None, "browser measurement produced no JSON result"
    try:
        parsed = json.loads(lines[-1])
    except json.JSONDecodeError as error:
        return None, f"browser measurement ended with invalid JSON: {error}"
    if not isinstance(parsed, dict) or parsed.get("mode") != mode:
        return None, f"browser measurement returned the wrong mode: {parsed!r}"
    return parsed, measured.stderr


def missingNumbers(metrics: dict[str, Any], names: tuple[str, ...]) -> list[str]:
    """Return metric names whose values cannot be compared to a budget."""
    return [name for name in names if not isinstance(metrics.get(name), (int, float))]
