"""Gate: the desktop cannot make a durable copy of conversation data.

Only two scalar preferences may use browser storage: the theme and the last successful provider. The Rust shell
may transport frames but may not open files, and the Tauri page receives no filesystem capability.

Usage::

    python -X utf8 tests/audit/desktopThinBoundary.py --selftest
    python -X utf8 tests/audit/desktopThinBoundary.py
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GUI = ROOT / "crates" / "runtrol-gui"
UI_SOURCE = GUI / "ui" / "src"
RUST_SOURCE = GUI / "src"

ALLOWED_LOCAL_STORAGE = {
    "preferences.ts": ("LAST_PROVIDER", "runtrol.lastProvider"),
    "theme.ts": ("THEME_KEY", "runtrol.theme"),
}
FORBIDDEN_BROWSER = re.compile(r"\b(?:sessionStorage|indexedDB|CacheStorage|caches\s*\.)")
FORBIDDEN_RUST = re.compile(
    r"\b(?:std|tokio|async_std)::fs\b|\b(?:File|OpenOptions)::(?:open|create|new)\b"
)
FILESYSTEM_DEPENDENCY = re.compile(r"(?:plugin-fs|@tauri-apps/plugin-fs|tauri_plugin_fs)")


def source_problems(files: dict[str, str]) -> list[str]:
    """Return forbidden storage APIs and preference modules that grew beyond one scalar key."""
    found: list[str] = []
    for name, source in files.items():
        if FORBIDDEN_BROWSER.search(source):
            found.append(f"{name} names durable browser storage outside localStorage")
        if "localStorage" not in source:
            continue
        expected = ALLOWED_LOCAL_STORAGE.get(name)
        if expected is None:
            found.append(f"{name} uses localStorage outside the two preference modules")
            continue
        symbol, key = expected
        methods = set(re.findall(r"localStorage\.([A-Za-z]+)", source))
        arguments = {
            argument.strip()
            for argument in re.findall(r"localStorage\.(?:getItem|setItem)\(([^,)]+)", source)
        }
        declaration = re.compile(rf'const\s+{re.escape(symbol)}\s*=\s*"{re.escape(key)}"')
        if not declaration.search(source) or arguments != {symbol}:
            found.append(f"{name} may only use the scalar key {key} through {symbol}")
        if not methods or not methods <= {"getItem", "setItem"}:
            found.append(f"{name} uses a localStorage operation other than getItem or setItem")
    for name in ALLOWED_LOCAL_STORAGE:
        if name not in files:
            found.append(f"the preference module {name} is missing")
    return found


def capability_problems(capability: dict[str, object], dependency_text: str) -> list[str]:
    """Return toolkit authority that could let the page persist conversation data."""
    found: list[str] = []
    permissions = capability.get("permissions")
    if permissions != ["core:event:default"]:
        found.append("the desktop capability grants more than session event listening")
    if FILESYSTEM_DEPENDENCY.search(dependency_text):
        found.append("the desktop includes a filesystem plugin")
    return found


def selftest() -> int:
    """Prove browser, Rust, and capability violations each make the gate red."""
    clean = {
        "preferences.ts": 'const LAST_PROVIDER = "runtrol.lastProvider"; localStorage.getItem(LAST_PROVIDER);',
        "theme.ts": 'const THEME_KEY = "runtrol.theme"; localStorage.setItem(THEME_KEY, mode);',
    }
    cases = (
        ("unexpected localStorage", {**clean, "frames.ts": 'localStorage.setItem("tail", frame)'}, {}, ""),
        (
            "duplicate basename",
            {**clean, "components/theme.ts": 'localStorage.setItem("tail", frame)'},
            {},
            "",
        ),
        ("IndexedDB", {**clean, "frames.ts": "indexedDB.open('frames')"}, {}, ""),
        (
            "second preference key",
            {**clean, "theme.ts": clean["theme.ts"] + ' localStorage.setItem("tail", frame);'},
            {},
            "",
        ),
        (
            "filesystem permission",
            clean,
            {"permissions": ["core:event:default", "fs:default"]},
            "",
        ),
        ("filesystem plugin", clean, {"permissions": ["core:event:default"]}, "tauri_plugin_fs"),
    )
    problems: list[str] = []
    for label, files, capability, dependencies in cases:
        found = source_problems(files) + capability_problems(capability, dependencies)
        if not found:
            problems.append(f"{label} escaped")
    if source_problems(clean) or capability_problems({"permissions": ["core:event:default"]}, ""):
        problems.append("a clean fixture was rejected")
    rust_cases = ("std::fs::read(path)", "File::create(path)", "tokio::fs::write(path, frame)")
    for source in rust_cases:
        if not FORBIDDEN_RUST.search(source):
            problems.append(f"Rust file access escaped: {source}")
    for problem in problems:
        print(f"[desktopThinBoundary --selftest] FAIL. {problem}", file=sys.stderr)
    if problems:
        return 2
    print("[desktopThinBoundary --selftest] OK. browser, Rust, and capability violations make the gate red.")
    return 0


def main() -> int:
    """Inspect the production desktop sources and capability manifest."""
    files = {
        path.relative_to(UI_SOURCE).as_posix(): path.read_text(encoding="utf-8")
        for path in sorted(UI_SOURCE.rglob("*"))
        if path.suffix in {".ts", ".tsx"}
    }
    problems = source_problems(files)
    for path in sorted(RUST_SOURCE.rglob("*.rs")):
        source = path.read_text(encoding="utf-8")
        if FORBIDDEN_RUST.search(source):
            problems.append(f"{path.relative_to(ROOT).as_posix()} can open or create files")

    capability_path = GUI / "capabilities" / "default.json"
    capability = json.loads(capability_path.read_text(encoding="utf-8"))
    dependency_text = "\n".join(
        path.read_text(encoding="utf-8")
        for path in (GUI / "Cargo.toml", GUI / "ui" / "package.json", GUI / "tauri.conf.json")
    )
    problems.extend(capability_problems(capability, dependency_text))
    if problems:
        print("[desktopThinBoundary] FAIL. the desktop could retain conversation data:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[desktopThinBoundary] OK. only two scalar preferences persist and the shell has no file authority.")
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest() if "--selftest" in sys.argv else main())
