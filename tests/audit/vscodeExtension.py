"""Gate: the VS Code surface stays thin, bounded, buildable, and package-shaped.

The gate deliberately checks the source contract before invoking the toolchain. A bundle that compiles can still
poll, persist conversation data, keep hidden renderers alive, or ship runtime Node dependencies.

Usage::

    python -X utf8 tests/audit/vscodeExtension.py --selftest
    python -X utf8 tests/audit/vscodeExtension.py
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"


def sourceViolations(package: dict[str, object], sources: dict[str, str]) -> list[str]:
    """Return violations of the extension's static product contract."""
    found: list[str] = []
    dependencies = package.get("dependencies")
    if isinstance(dependencies, dict) and dependencies:
        found.append("the shipped extension has runtime Node dependencies")

    contributes = package.get("contributes")
    contribution_text = json.dumps(contributes, sort_keys=True)
    if "secondarySidebar" in contribution_text:
        found.append("the manifest contributes an unsupported secondary sidebar container")
    if '"activitybar"' not in contribution_text:
        found.append("the extension has no Activity Bar control surface")

    all_source = "\n".join(sources.values())
    forbidden = {
        "localStorage": "conversation-capable browser persistence",
        "sessionStorage": "conversation-capable browser persistence",
        "indexedDB": "conversation-capable browser persistence",
        "setInterval(": "polling loop",
        "scheduleRefresh": "session-list requery loop",
        "writeFile(": "filesystem write surface",
        "appendFile(": "filesystem write surface",
    }
    for token, meaning in forbidden.items():
        if token in all_source:
            found.append(f"{meaning} is reachable through `{token}`")

    required = {
        "core/framing.ts": ["MAX_FRAME_BYTES", "MAX_QUEUED_FRAMES", "MAX_QUEUED_BYTES", "setImmediate"],
        "webview/main.ts": ["MAX_VISIBLE_ITEMS", "MAX_VISIBLE_CHARACTERS", "MAX_BATCH"],
        "extension.ts": ["retainContextWhenHidden: false", "afterReady"],
        "controller.ts": [
            "private watchAbort",
            "private indexAbort",
            "this.watchAbort?.abort()",
            "reconnect",
            "workspaceCollisions",
            '"Start here anyway"',
        ],
        "core/client.ts": ["watchSessions", "commandConnection", "commandTail"],
        "core/locator.ts": ['["endpoint"]', 'candidates.push("runtrol")'],
    }
    for relative, tokens in required.items():
        source = sources.get(relative, "")
        for token in tokens:
            if token not in source:
                found.append(f"{relative} does not contain required contract `{token}`")
    return found


def selftest() -> int:
    """Prove the detector rejects each class of defect."""
    package = {"contributes": {"viewsContainers": {"activitybar": []}}}
    sources = {
        "core/framing.ts": "MAX_FRAME_BYTES MAX_QUEUED_FRAMES MAX_QUEUED_BYTES setImmediate",
        "webview/main.ts": "MAX_VISIBLE_ITEMS MAX_VISIBLE_CHARACTERS MAX_BATCH",
        "extension.ts": "retainContextWhenHidden: false afterReady",
        "controller.ts": (
            'private watchAbort; private indexAbort; this.watchAbort?.abort(); reconnect workspaceCollisions '
            '"Start here anyway"'
        ),
        "core/client.ts": "watchSessions commandConnection commandTail",
        "core/locator.ts": '["endpoint"] candidates.push("runtrol")',
    }
    if sourceViolations(package, sources):
        print("[vscodeExtension --selftest] FAIL. the green fixture was rejected.", file=sys.stderr)
        return 2

    mutations = [
        ({**package, "dependencies": {"some-runtime": "1"}}, sources),
        ({"contributes": {"viewsContainers": {"secondarySidebar": []}}}, sources),
        (package, {**sources, "webview/main.ts": "localStorage MAX_VISIBLE_ITEMS"}),
        (package, {**sources, "controller.ts": "setInterval("}),
        (package, {**sources, "core/framing.ts": "MAX_FRAME_BYTES"}),
        (package, {**sources, "controller.ts": sources["controller.ts"].replace("workspaceCollisions", "")}),
    ]
    for index, (changed_package, changed_sources) in enumerate(mutations, start=1):
        if not sourceViolations(changed_package, changed_sources):
            print(f"[vscodeExtension --selftest] FAIL. mutation {index} escaped.", file=sys.stderr)
            return 2
    print(f"[vscodeExtension --selftest] OK. all {len(mutations)} defects make the gate red.")
    return 0


def npmCommand() -> list[str]:
    """Return an explicit npm launcher without asking a shell to interpret product input."""
    npm = shutil.which("npm.cmd" if sys.platform == "win32" else "npm") or shutil.which("npm")
    if npm is None:
        raise RuntimeError("npm is missing")
    if sys.platform == "win32":
        command = osCommand()
        return [command, "/d", "/c", npm]
    return [npm]


def osCommand() -> str:
    """Find the Windows command host used only to launch npm.cmd."""
    import os

    return os.environ.get("ComSpec", r"C:\Windows\System32\cmd.exe")


def run() -> int:
    """Inspect sources, type-check, test, and bundle the extension."""
    package_path = EXTENSION / "package.json"
    lock = EXTENSION / "package-lock.json"
    if not package_path.is_file() or not lock.is_file():
        print("[vscodeExtension] FAIL. package.json and package-lock.json are required.", file=sys.stderr)
        return 2
    package = json.loads(package_path.read_text(encoding="utf-8"))
    sources = {
        path.relative_to(EXTENSION / "src").as_posix(): path.read_text(encoding="utf-8")
        for path in (EXTENSION / "src").rglob("*.ts")
        if not path.name.endswith(".test.ts") and path.name != "styles.d.ts"
    }
    failures = sourceViolations(package, sources)
    if failures:
        print("[vscodeExtension] FAIL. static contract violations:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 2

    command = npmCommand()
    for script in ("check", "test", "build"):
        result = subprocess.run(
            [*command, "run", script],
            cwd=EXTENSION,
            check=False,
            text=True,
            capture_output=True,
            timeout=180,
        )
        if result.returncode != 0:
            print(result.stdout, file=sys.stderr)
            print(result.stderr, file=sys.stderr)
            print(f"[vscodeExtension] FAIL. npm run {script} returned {result.returncode}.", file=sys.stderr)
            return 2

    bundles = [EXTENSION / "dist" / name for name in ("extension.js", "webview.js", "webview.css")]
    for bundle in bundles:
        if not bundle.is_file() or bundle.stat().st_size > 256 * 1024:
            failures.append(f"{bundle.relative_to(ROOT)} is missing or exceeds 256 KiB")
    if failures:
        print("[vscodeExtension] FAIL. bundle contract violations:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 2

    total = sum(bundle.stat().st_size for bundle in bundles)
    print(f"[vscodeExtension] OK. thin source contract and {total} bundled bytes verified.")
    return 0


def main() -> int:
    """Select the selftest or the real gate."""
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print("usage: vscodeExtension.py [--selftest]", file=sys.stderr)
        return 1
    return run()


if __name__ == "__main__":
    raise SystemExit(main())
