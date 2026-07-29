"""로컬 CI 단일 진입점.

CI 와 같은 게이트를 로컬에서 돌린다. push 준비 보고 직전에 반드시 통과해야 한다.

사용::

    python -X utf8 tests/audit/preflight.py            # 전체
    python -X utf8 tests/audit/preflight.py lint       # lint 만 (빠름)
    python -X utf8 tests/audit/preflight.py --list     # 게이트 목록

부트스트랩 단계 (아직 `Cargo.toml` 이 없음) 에서는 cargo 게이트를 **건너뛴다고 밝히고**
건너뛴다. 조용히 통과시키지 않는다. 무엇이 안 돌았는지 화면에 남는다.

한 항목이라도 실패하면 푸시 준비 보고를 하지 않는다. 고친 뒤 재검증한다.

종료 코드:
    0 전 게이트 통과
    1 사용법 오류
    2 게이트 실패
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PY = [sys.executable, "-X", "utf8"]
HOOKS = "tests/audit"

# 게이트 이름 -> (설명, 명령)
GATES: dict[str, tuple[str, list[str]]] = {
    "noScriptsDir": ("`scripts/` 폴더 금지", [*PY, f"{HOOKS}/noScriptsDir.py"]),
    "workspaceHygiene": ("루트 allowlist + 스크래치 부패", [*PY, f"{HOOKS}/workspaceHygiene.py"]),
    "silentFailSelftest": ("silent failure 검출기 자체 검증", [*PY, f"{HOOKS}/checkSilentFail.py", "--selftest"]),
    "checkSilentFail": ("silent failure 금지", [*PY, f"{HOOKS}/checkSilentFail.py"]),
    "cargoFmt": ("cargo fmt --check", ["cargo", "fmt", "--all", "--check"]),
    "cargoClippy": (
        "cargo clippy (경고 = 실패)",
        ["cargo", "clippy", "--all-targets", "--all-features", "--", "-D", "warnings"],
    ),
    "cargoTest": ("cargo test", ["cargo", "test", "--all"]),
    "audit": ("저장소 계약 게이트 (tests/audit)", ["cargo", "test", "-p", "runtrol-audit"]),
}

SUITES: dict[str, tuple[str, ...]] = {
    "lint": ("noScriptsDir", "workspaceHygiene", "silentFailSelftest", "checkSilentFail", "cargoFmt", "cargoClippy"),
    "preflight": tuple(GATES),
}

CARGO_GATES = frozenset({"cargoFmt", "cargoClippy", "cargoTest", "audit"})


def hasCargoWorkspace() -> bool:
    """루트 `Cargo.toml` 이 있는가. 없으면 cargo 게이트는 돌 대상이 없다."""
    return (ROOT / "Cargo.toml").exists()


def hasAuditCrate() -> bool:
    """`tests/audit` 계약 게이트 crate 가 존재하는가."""
    return (ROOT / "tests" / "audit" / "Cargo.toml").exists()


def skipReasonFor(name: str) -> str | None:
    """게이트를 건너뛸 이유. 돌아야 하면 None."""
    if name in CARGO_GATES:
        if shutil.which("cargo") is None:
            return "cargo 없음"
        if not hasCargoWorkspace():
            return "Cargo.toml 없음 (부트스트랩 단계)"
    if name == "audit" and not hasAuditCrate():
        return "tests/audit crate 없음"
    return None


def runGate(name: str) -> int:
    """단일 게이트를 실행하고 반환 코드를 돌려준다."""
    description, cmd = GATES[name]

    skip = skipReasonFor(name)
    if skip:
        print(f"\n=== [{name}] {description} . SKIP ({skip}) ===", flush=True)
        return 0

    print(f"\n=== [{name}] {description} ===", flush=True)
    proc = subprocess.run(cmd, cwd=ROOT, check=False)
    return proc.returncode


def main(argv: list[str]) -> int:
    """suite 를 골라 게이트를 순차 실행한다. 실패 게이트를 모아 보고한다."""
    if "--list" in argv:
        for name, (description, cmd) in GATES.items():
            skip = skipReasonFor(name)
            mark = f"  (SKIP: {skip})" if skip else ""
            print(f"  {name:20s} {description}{mark}")
            print(f"  {'':20s}   $ {' '.join(cmd)}")
        return 0

    args = [a for a in argv if not a.startswith("--")]
    suite = args[0] if args else "preflight"
    if suite not in SUITES:
        print(f"알 수 없는 suite '{suite}'. 사용 가능: {', '.join(SUITES)}", file=sys.stderr)
        return 1

    failed: list[str] = []
    skipped: list[str] = []
    for name in SUITES[suite]:
        if skipReasonFor(name):
            skipped.append(name)
        if runGate(name) != 0:
            failed.append(name)

    print()
    if skipped:
        print(f"[preflight] SKIP {len(skipped)} 개: {', '.join(skipped)}. 이 표면은 검증되지 않았다.")
    if failed:
        print(f"[preflight] {suite} FAIL. 실패 게이트: {', '.join(failed)}", file=sys.stderr)
        return 2
    ran = len(SUITES[suite]) - len(skipped)
    print(f"[preflight] {suite} GREEN. 게이트 {ran} 개 실행, {len(skipped)} 개 건너뜀.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
