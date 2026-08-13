"""Gate: Mission layers stay provider-neutral, content-blind, and below daemon composition."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LAYERS = ("runtrol-ledger", "runtrol-orchestrator", "runtrol-growth")
FORBIDDEN_DEPS = {
    "runtrol-drivers",
    "runtrol-ipc",
    "runtrol-transport",
    "runtrol-daemon",
    "runtrol-store",
}


def inspect(manifests: dict[str, str], sources: dict[str, str]) -> list[str]:
    failures: list[str] = []
    for layer, manifest in manifests.items():
        for dependency in FORBIDDEN_DEPS:
            if re.search(rf"(?m)^\s*{re.escape(dependency)}\s*=", manifest):
                failures.append(f"{layer} depends on forbidden {dependency}")
    for path, source in sources.items():
        production = source.split("#[cfg(test)]", maxsplit=1)[0].lower()
        for provider in ("claude", "codex"):
            if f'"{provider}"' in production:
                failures.append(f"{path} contains provider-name logic for {provider}")
        if re.search(r"scheduler[^\n]{0,80}(prompt|instruction_body|input_text)", production):
            failures.append(f"{path} gives the scheduler an input body")
    return failures


def repository_inputs() -> tuple[dict[str, str], dict[str, str]]:
    manifests: dict[str, str] = {}
    sources: dict[str, str] = {}
    for layer in LAYERS:
        directory = ROOT / "crates" / layer
        if not directory.is_dir():
            continue
        manifests[layer] = (directory / "Cargo.toml").read_text(encoding="utf-8")
        for path in sorted((directory / "src").rglob("*.rs")):
            sources[path.relative_to(ROOT).as_posix()] = path.read_text(encoding="utf-8")
    return manifests, sources


def selftest() -> int:
    clean = inspect(
        {"runtrol-ledger": "[dependencies]\nruntrol-provider = {}\n"},
        {"lib.rs": "pub struct Ledger;"},
    )
    if clean:
        print(f"[orchestrationThinBoundary --selftest] FAIL. clean fixture: {clean}", file=sys.stderr)
        return 2
    mutations = [
        ({"runtrol-ledger": "[dependencies]\nruntrol-drivers = {}\n"}, {"lib.rs": "pub struct Ledger;"}),
        ({"runtrol-ledger": "[dependencies]\n"}, {"lib.rs": 'if provider == "codex" {}'}),
        ({"runtrol-ledger": "[dependencies]\n"}, {"lib.rs": "scheduler.prompt(input_text);"}),
    ]
    if any(not inspect(*mutation) for mutation in mutations):
        print("[orchestrationThinBoundary --selftest] FAIL. a mutation escaped", file=sys.stderr)
        return 2
    print(f"[orchestrationThinBoundary --selftest] OK. all {len(mutations)} defects make the gate red.")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print("usage: orchestrationThinBoundary.py [--selftest]", file=sys.stderr)
        return 1
    failures = inspect(*repository_inputs())
    if failures:
        print("[orchestrationThinBoundary] FAIL:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 2
    print("[orchestrationThinBoundary] OK. Mission layers are content-blind and provider-neutral.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
