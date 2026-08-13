"""Gate: durable Mission records expose no field capable of retaining conversation or process bodies."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / "crates" / "runtrol-ledger" / "src"
FORBIDDEN_FIELDS = {
    "prompt",
    "reply",
    "transcript",
    "event_payload",
    "instruction_body",
    "raw_argv",
    "stdout",
    "stderr",
    "output",
    "environment",
    "env_value",
    "secret",
    "token",
    "cookie",
    "api_key",
}


def durable_fields(source: str) -> set[str]:
    return set(re.findall(r"(?m)^\s*pub\s+([a-z][a-z0-9_]*)\s*:", source))


def inspect(sources: dict[str, str]) -> list[str]:
    failures: list[str] = []
    for path, source in sources.items():
        for field in sorted(durable_fields(source).intersection(FORBIDDEN_FIELDS)):
            failures.append(f"{path} exposes forbidden durable field {field}")
    return failures


def selftest() -> int:
    if inspect({"clean.rs": "pub struct Row { pub digest: [u8; 32] }"}):
        print("[evidenceBoundary --selftest] FAIL. clean fixture rejected", file=sys.stderr)
        return 2
    mutations = [
        {"row.rs": f"pub struct Row {{\n    pub {field}: Box<str>,\n}}"}
        for field in sorted(FORBIDDEN_FIELDS)
    ]
    if any(not inspect(mutation) for mutation in mutations):
        print("[evidenceBoundary --selftest] FAIL. a forbidden field escaped", file=sys.stderr)
        return 2
    print(f"[evidenceBoundary --selftest] OK. all {len(mutations)} forbidden fields make the gate red.")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print("usage: evidenceBoundary.py [--selftest]", file=sys.stderr)
        return 1
    sources = {
        path.relative_to(ROOT).as_posix(): path.read_text(encoding="utf-8")
        for path in sorted(LEDGER.rglob("*.rs"))
    }
    failures = inspect(sources)
    if failures:
        print("[evidenceBoundary] FAIL:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 2
    print("[evidenceBoundary] OK. durable evidence accepts identities and digests only.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
