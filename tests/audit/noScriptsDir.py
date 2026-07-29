"""`scripts/` 폴더 신설 차단.

CLAUDE.md 강행규칙 ``scripts/ 폴더 절대 금지`` 의 기계 강제. 예외 없음.
하위폴더 (`scripts/dev/`, `scripts/build/`, ...) 도 전부 차단한다.

이유: `scripts/` 는 소유자가 없는 폴더라 무엇이든 들어가고 아무도 지우지 않는다.
도구는 그 도구가 지키는 도메인 옆에 둔다:

    .claude/hooks/                  세션 harness 게이트 (일회용, `crates/` 생기면 tests/audit 로 졸업)
    tests/audit/                    저장소 계약 게이트 (정본)
    crates/<crate>/src/bin/         제품 CLI 진입점
    .github/scripts/{ci,release}/   CI 인프라

종료 코드:
    0 통과
    2 `scripts/` 존재
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def findScriptsDir() -> Path | None:
    """repo 루트의 `scripts/` 디렉토리를 찾는다. 없으면 None."""
    candidate = ROOT / "scripts"
    return candidate if candidate.is_dir() else None


def main() -> int:
    """`scripts/` 가 있으면 2 를 반환해 커밋을 차단한다."""
    found = findScriptsDir()
    if found is None:
        return 0

    entries = sorted(p.name for p in found.iterdir())
    sys.stderr.write("[noScriptsDir] repo 루트 `scripts/` 발견. CLAUDE.md 강행규칙 위반.\n")
    for name in entries[:20]:
        sys.stderr.write(f"  - scripts/{name}\n")
    sys.stderr.write(
        "\n도구는 도메인 폴더로 옮겨라:\n"
        "  세션 harness   -> .claude/hooks/\n"
        "  저장소 계약    -> tests/audit/\n"
        "  제품 CLI       -> crates/<crate>/src/bin/\n"
        "  CI 인프라      -> .github/scripts/{ci,release}/\n"
    )
    return 2


if __name__ == "__main__":
    sys.exit(main())
