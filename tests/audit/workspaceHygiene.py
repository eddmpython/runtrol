"""워크스페이스 위생 게이트.

CLAUDE.md 강행규칙 `워크스페이스 청결` 의 기계 강제. 두 가지를 본다.

1. repo 루트 직속 엔트리가 ALLOWED_ROOT 밖이면 FAIL.
   신규 최상위 항목은 여기에 등록해야 통과한다. 등록 없이 늘어나는 것이 회귀 신호다.
2. 스크래치 `.tmp/` 안에 7 일 넘은 파일이 있으면 FAIL (부패 검출).

repo 트리 안에 `tmp/` · `_partial/` 을 만들지 않는다. 임시 분석은 인라인 stdout,
정말 파일이 필요하면 OS 임시 디렉토리를 쓴다.

종료 코드:
    0 통과
    2 위반
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STALE_DAYS = 7
SCRATCH_DIR = ROOT / ".tmp"

ALLOWED_ROOT: frozenset[str] = frozenset(
    {
        # 버전 관리 · 도구 설정
        ".git",
        ".github",
        ".githooks",
        ".gitignore",
        ".gitattributes",
        ".claude",
        ".codex",
        ".agent",
        ".agents",
        ".vscode",
        ".venv",
        ".tmp",
        ".ruff_cache",
        ".env",
        ".env.example",
        # L-local (gitignored)
        "CLAUDE.md",
        "AGENTS.md",
        # 운영자와 AI 사이의 약속 기록 (L-memory). 운영자 지시 2026-08-20: 루트의 폴더로 두되
        # 깃 추적은 하지 않는다. 정본은 이 폴더이고 harness 쪽 memory/MEMORY.md 는 포인터다.
        "memory",
        "PLAN.md",
        "TODO.md",
        # L-public 문서. README 는 4 개 언어가 정본이다 (한국어가 기준, 나머지는 투영)
        "README.md",
        "README_EN.md",
        "README_ZH.md",
        "README_JA.md",
        "LICENSE",
        "NOTICE",
        "CHANGELOG.md",
        "SECURITY.md",
        "CONTRIBUTING.md",
        "CODE_OF_CONDUCT.md",
        "docs",
        "mainPlan",
        "assets",
        "extensions",
        # Public SDK packages distributed independently from the product surfaces.
        "clients",
        # Rust 워크스페이스
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "rustfmt.toml",
        "clippy.toml",
        "deny.toml",
        "crates",
        "target",
        # PWA 와 GitHub Pages 랜딩
        "pwa",
        # Independently deployable, untrusted ciphertext relay.
        "relay",
        "site",
        # 게이트
        "tests",
    }
)


def unregisteredRootEntries() -> list[str]:
    """ALLOWED_ROOT 에 없는 루트 직속 엔트리 목록."""
    return sorted(p.name for p in ROOT.iterdir() if p.name not in ALLOWED_ROOT)


def staleScratchFiles() -> list[tuple[str, int]]:
    """`.tmp/` 안에서 STALE_DAYS 를 넘긴 파일과 경과 일수."""
    if not SCRATCH_DIR.is_dir():
        return []
    now = time.time()
    cutoff = now - STALE_DAYS * 86400
    stale: list[tuple[str, int]] = []
    for p in SCRATCH_DIR.rglob("*"):
        if not p.is_file():
            continue
        mtime = p.stat().st_mtime
        if mtime < cutoff:
            ageDays = int((now - mtime) / 86400)
            stale.append((p.relative_to(ROOT).as_posix(), ageDays))
    return sorted(stale)


def main() -> int:
    """루트 allowlist 와 스크래치 부패를 검사한다. 위반이면 2."""
    failures = 0

    unregistered = unregisteredRootEntries()
    if unregistered:
        failures = 2
        sys.stderr.write("[workspaceHygiene] 등록되지 않은 루트 엔트리:\n")
        for name in unregistered:
            sys.stderr.write(f"  - {name}\n")
        sys.stderr.write(
            "\n정당한 신규 최상위면 .claude/hooks/workspaceHygiene.py 의 ALLOWED_ROOT 에 등록하라.\n"
            "임시 산출물이면 삭제하라 (repo 트리 안 임시 파일 금지).\n"
        )

    stale = staleScratchFiles()
    if stale:
        failures = 2
        sys.stderr.write(f"\n[workspaceHygiene] `.tmp/` 스크래치 부패 ({STALE_DAYS} 일 초과):\n")
        for path, ageDays in stale:
            sys.stderr.write(f"  - {path} ({ageDays} 일)\n")
        sys.stderr.write("\n스크래치는 일회용이다. 삭제하거나 정본 위치로 옮겨라.\n")

    if failures == 0:
        print("[workspaceHygiene] OK.")
    return failures


if __name__ == "__main__":
    sys.exit(main())
