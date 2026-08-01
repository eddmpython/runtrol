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

# 이 기계와 cfg 분기가 다른 대상 하나. 컴파일만 하므로 실행 환경이 필요 없고,
# `#[cfg(unix)]` 쪽 코드가 로컬에서 검사된다는 것이 요점이다.
CROSS_TARGET = "x86_64-unknown-linux-gnu" if sys.platform == "win32" else "x86_64-pc-windows-msvc"

# 게이트 이름 -> (설명, 명령)
GATES: dict[str, tuple[str, list[str]]] = {
    "noScriptsDir": ("`scripts/` 폴더 금지", [*PY, f"{HOOKS}/noScriptsDir.py"]),
    "workspaceHygiene": ("루트 allowlist + 스크래치 부패", [*PY, f"{HOOKS}/workspaceHygiene.py"]),
    "silentFailSelftest": ("silent failure 검출기 자체 검증", [*PY, f"{HOOKS}/checkSilentFail.py", "--selftest"]),
    "checkSilentFail": ("silent failure 금지", [*PY, f"{HOOKS}/checkSilentFail.py"]),
    # provider 이름이 드라이버 밖에 나타나면 그 자체가 설계 결함이다. 자체 검증이 먼저 도는 이유는
    # 게이트가 통과를 보이기 전에 실패할 수 있는지부터 확인해야 하기 때문이다.
    "providerIsolationSelftest": (
        "provider 격리 검출기 자체 검증",
        [*PY, f"{HOOKS}/providerIsolation.py", "--selftest"],
    ),
    "providerIsolation": ("provider 이름은 드라이버 밖에 없다", [*PY, f"{HOOKS}/providerIsolation.py"]),
    # 워크스페이스 lint 표를 벗어난 crate 는 아무 경고도 없이 규칙 밖에 산다.
    "workspaceLintsSelftest": (
        "lint 표 검출기 자체 검증",
        [*PY, f"{HOOKS}/workspaceLints.py", "--selftest"],
    ),
    "workspaceLints": ("crate 는 워크스페이스 lint 표를 따른다", [*PY, f"{HOOKS}/workspaceLints.py"]),
    # 이 제품의 유일한 절대 규칙을 기계가 볼 수 있는 형태로 적은 것. 대화를 담을 수 있는 타입은
    # 저장소 crate 에 나타나지 못한다.
    "noTranscriptCopySelftest": (
        "대화 사본 검출기 자체 검증",
        [*PY, f"{HOOKS}/noTranscriptCopy.py", "--selftest"],
    ),
    "noTranscriptCopy": ("대화 사본을 갖지 않는다", [*PY, f"{HOOKS}/noTranscriptCopy.py"]),
    # 요청마다 누가 할 수 있는지 규칙이 있고, 벽이 다른 무엇보다 먼저 물어진다. 컴파일러는 crate
    # 경계 너머로 빠진 요청을 말해주지 못한다.
    "scopeWallSelftest": ("스코프 벽 검출기 자체 검증", [*PY, f"{HOOKS}/scopeWall.py", "--selftest"]),
    "scopeWall": ("모든 요청에 권한 규칙이 있다", [*PY, f"{HOOKS}/scopeWall.py"]),
    "configReadOnlySelftest": (
        "provider config write gate selftest",
        [*PY, f"{HOOKS}/configReadOnly.py", "--selftest"],
    ),
    "configReadOnly": ("provider configuration stays read-only", [*PY, f"{HOOKS}/configReadOnly.py"]),
    "orphanReapingSelftest": (
        "자식 회수 게이트 자체 검증",
        [*PY, f"{HOOKS}/orphanReaping.py", "--selftest"],
    ),
    "orphanReaping": ("부모 강제 종료 뒤 자식 회수", [*PY, f"{HOOKS}/orphanReaping.py"]),
    # 게이트가 저장소에 있는 것과 도는 것은 다른 말이다. 이 게이트가 그 차이를 감시한다.
    "gateCoverageSelftest": (
        "게이트 러너 커버리지 자체 검증",
        [*PY, f"{HOOKS}/gateCoverage.py", "--selftest"],
    ),
    "gateCoverage": ("게이트 러너 커버리지", [*PY, f"{HOOKS}/gateCoverage.py"]),
    # 북극성 점수는 사람이 타이핑하는 숫자가 아니라 board.toml 에서 계산된다.
    "northStarBoardSelftest": (
        "북극성 점수판 자체 검증",
        [*PY, f"{HOOKS}/northStar/board.py", "--selftest"],
    ),
    "northStarBoard": ("북극성 점수판 (증거 구조)", [*PY, f"{HOOKS}/northStar/board.py"]),
    "readmeParitySelftest": (
        "4 개 언어 README 대조 자체 검증",
        [*PY, f"{HOOKS}/northStar/readmeParity.py", "--selftest"],
    ),
    "readmeParity": ("4 개 언어 README 점수판 대조", [*PY, f"{HOOKS}/northStar/readmeParity.py"]),
    "frontendBuildSelftest": (
        "프런트엔드 빌드 게이트 자체 검증",
        [*PY, f"{HOOKS}/frontendBuild.py", "--selftest"],
    ),
    "frontendBuild": ("데스크톱 프런트엔드 타입 검사 + 번들", [*PY, f"{HOOKS}/frontendBuild.py"]),
    "interactionLatencyBudgetSelftest": (
        "데스크톱 상호작용 예산 게이트 자체 검증",
        [*PY, f"{HOOKS}/interactionLatencyBudget.py", "--selftest"],
    ),
    "interactionLatencyBudget": (
        "실물 브라우저 데스크톱 상호작용 예산",
        [*PY, f"{HOOKS}/interactionLatencyBudget.py"],
    ),
    "scrollUnderLoadSmokeSelftest": (
        "출력 폭주 게이트 자체 검증",
        [*PY, f"{HOOKS}/scrollUnderLoadSmoke.py", "--selftest"],
    ),
    "scrollUnderLoadSmoke": (
        "초당 3,000 프레임에서 스크롤과 입력",
        [*PY, f"{HOOKS}/scrollUnderLoadSmoke.py"],
    ),
    "reconnectContinuitySmokeSelftest": (
        "재접속 연속성 게이트 자체 검증",
        [*PY, f"{HOOKS}/reconnectContinuitySmoke.py", "--selftest"],
    ),
    "reconnectContinuitySmoke": (
        "마지막 프레임과 재접속 커서 연속성",
        [*PY, f"{HOOKS}/reconnectContinuitySmoke.py"],
    ),
    "liveMemoryBudgetSelftest": (
        "실시간 메모리 계약 자체 검증",
        [*PY, f"{HOOKS}/liveMemoryBudget.py", "--selftest"],
    ),
    "liveMemoryBudget": (
        "hot 세션과 구독자 4개 RSS 상한",
        [*PY, f"{HOOKS}/liveMemoryBudget.py"],
    ),
    "desktopConvenienceSmokeSelftest": (
        "데스크톱 편의 계약 게이트 자체 검증",
        [*PY, f"{HOOKS}/desktopConvenienceSmoke.py", "--selftest"],
    ),
    "desktopConvenienceSmoke": (
        "마지막 공급자와 사용량 및 한도 표시",
        [*PY, f"{HOOKS}/desktopConvenienceSmoke.py"],
    ),
    "cargoFmt": ("cargo fmt --check", ["cargo", "fmt", "--all", "--check"]),
    "cargoClippy": (
        "cargo clippy (경고 = 실패)",
        ["cargo", "clippy", "--all-targets", "--all-features", "--", "-D", "warnings"],
    ),
    # **다른 플랫폼의 cfg 분기를 로컬에서 본다.** 이것이 없으면 unix 전용 결함 (미사용 mut,
    # 죽은 상수, 도달 불가 variant, cfg 로 갈라진 함수) 이 전부 CI 에서만 드러나고 수정이
    # 두 번째 커밋으로 온다. 실제로 그렇게 됐고, 그래서 이 게이트가 있다.
    # 대상이 설치돼 있지 않으면 건너뛴다고 밝히고 건너뛴다.
    # 데스크톱 셸을 링크하는 두 crate 는 뺀다. 다른 플랫폼용으로 **컴파일조차** 되지 않기 때문이다:
    # Tauri 의 Linux 백엔드는 dbus·gtk·webkit2gtk 개발 라이브러리를 요구하고, Windows 기계에는 없다.
    # 게이트의 목적은 우리가 쓴 cfg 분기를 보는 것이고, 그 분기를 가진 crate (runtrol-childproc 의
    # job object 와 process group 이 대표다) 는 전부 그대로 검사된다. 이 둘에는 cfg 분기가 없다.
    "clippyCrossCfg": (
        "cargo clippy (다른 플랫폼 cfg 분기)",
        [
            "cargo",
            "clippy",
            "--all-targets",
            "--workspace",
            "--exclude",
            "runtrol-gui",
            "--exclude",
            "runtrol",
            "--target",
            CROSS_TARGET,
            "--",
            "-D",
            "warnings",
        ],
    ),
    "cargoBuild": (
        "memory budget gate product binary",
        ["cargo", "build", "-p", "runtrol", "--bin", "runtrol"],
    ),
    "cargoTest": ("cargo test", ["cargo", "test", "--all"]),
    "genericAcpSmokeSelftest": (
        "범용 ACP 게이트 자체 검증",
        [*PY, f"{HOOKS}/genericAcpSmoke.py", "--selftest"],
    ),
    "genericAcpSmoke": (
        "외부 TOML 공급자의 ACP 자식 프로세스 턴",
        [*PY, f"{HOOKS}/genericAcpSmoke.py"],
    ),
    "externalAcpSmokeSelftest": (
        "독립 ACP 구현 게이트 자체 검증",
        [*PY, f"{HOOKS}/externalAcpSmoke.py", "--selftest"],
    ),
    "externalAcpSmoke": (
        "독립 설치 ACP 구현의 두 턴과 native load",
        [*PY, f"{HOOKS}/externalAcpSmoke.py", "--require-external"],
    ),
    "claudeApprovalSmokeSelftest": (
        "실물 Claude hidden approval 게이트 자체 검증",
        [*PY, f"{HOOKS}/claudeApprovalSmoke.py", "--selftest"],
    ),
    "claudeApprovalSmoke": (
        "실물 Claude hidden approval 거부 왕복",
        [*PY, f"{HOOKS}/claudeApprovalSmoke.py", "--require-external"],
    ),
    "uninstallLeavesNoTraceSelftest": (
        "제거 독립성 게이트 자체 검증",
        [*PY, f"{HOOKS}/uninstallLeavesNoTrace.py", "--selftest"],
    ),
    "uninstallLeavesNoTrace": (
        "runtrol home 삭제 뒤 provider 세션 재개",
        [*PY, f"{HOOKS}/uninstallLeavesNoTrace.py"],
    ),
    # 북극성 `하나의 세션 목록` 축을 떠받치는 유일한 게이트. 실물 CLI 를 몰고 세션을 시작·목록·닫기·
    # 재개까지 돌린다. 프롬프트를 보내지 않으므로 토큰도 rate limit 도 쓰지 않고, 그래서 야간이 아니라
    # 매 preflight 에 돌 수 있다. 자체 검증이 먼저 도는 이유는 통과를 보이기 전에 실패할 수 있는지부터
    # 확인해야 하기 때문이다.
    "sessionLifecycleSmokeSelftest": (
        "세션 생애주기 게이트 자체 검증",
        [*PY, f"{HOOKS}/sessionLifecycleSmoke.py", "--selftest"],
    ),
    "sessionLifecycleSmoke": (
        "세션 시작·목록·닫기·재개 (실물 CLI)",
        [*PY, f"{HOOKS}/sessionLifecycleSmoke.py"],
    ),
    "modelDetectionSmokeSelftest": (
        "모델 자동 인식 게이트 자체 검증",
        [*PY, f"{HOOKS}/modelDetectionSmoke.py", "--selftest"],
    ),
    "modelDetectionSmoke": (
        "실물 CLI 모델 자동 인식",
        [*PY, f"{HOOKS}/modelDetectionSmoke.py"],
    ),
    "agentSurfaceDriftSelftest": (
        "provider surface drift gate selftest",
        [*PY, f"{HOOKS}/agentSurfaceDrift.py", "--selftest"],
    ),
    "agentSurfaceDrift": (
        "installed provider surfaces still match their bindings",
        [*PY, f"{HOOKS}/agentSurfaceDrift.py"],
    ),
    "audit": ("저장소 계약 게이트 (tests/audit)", ["cargo", "test", "-p", "runtrol-audit"]),
    # 의존성 부패. `[workspace.dependencies]` 미사용 항목까지 잡는 것이 cargo-shear 를 고른 이유다
    # (버전 SSOT 가 거기 살기 때문). 설치돼 있지 않으면 건너뛴다고 밝히고 건너뛴다.
    "cargoShear": ("미사용 의존성 (cargo-shear)", ["cargo", "shear"]),
    # 공급망 + 기각한 crate 의 기계 기억 (`deny.toml`). 제3자 의존이 들어오는 순간부터
    # 이것이 돌지 않으면 원장은 읽히지 않는 문서일 뿐이다.
    "cargoDeny": ("공급망·기각 원장 (cargo-deny)", ["cargo", "deny", "check"]),
}

SUITES: dict[str, tuple[str, ...]] = {
    "lint": (
        "noScriptsDir",
        "workspaceHygiene",
        "silentFailSelftest",
        "checkSilentFail",
        "providerIsolationSelftest",
        "providerIsolation",
        "workspaceLintsSelftest",
        "workspaceLints",
        "noTranscriptCopySelftest",
        "noTranscriptCopy",
        "scopeWallSelftest",
        "scopeWall",
        "orphanReapingSelftest",
        "orphanReaping",
        "gateCoverageSelftest",
        "gateCoverage",
        "northStarBoardSelftest",
        "northStarBoard",
        "readmeParitySelftest",
        "readmeParity",
        "frontendBuildSelftest",
        "frontendBuild",
        "interactionLatencyBudgetSelftest",
        "interactionLatencyBudget",
        "scrollUnderLoadSmokeSelftest",
        "scrollUnderLoadSmoke",
        "reconnectContinuitySmokeSelftest",
        "reconnectContinuitySmoke",
        "liveMemoryBudgetSelftest",
        "liveMemoryBudget",
        "desktopConvenienceSmokeSelftest",
        "desktopConvenienceSmoke",
        "cargoFmt",
        "cargoClippy",
    ),
    "preflight": tuple(GATES),
}

CARGO_GATES = frozenset(
    {
        "cargoFmt",
        "cargoClippy",
        "clippyCrossCfg",
        "cargoBuild",
        "cargoTest",
        "genericAcpSmokeSelftest",
        "genericAcpSmoke",
        "externalAcpSmokeSelftest",
        "externalAcpSmoke",
        "claudeApprovalSmokeSelftest",
        "claudeApprovalSmoke",
        "uninstallLeavesNoTraceSelftest",
        "uninstallLeavesNoTrace",
        "audit",
        "cargoShear",
        "cargoDeny",
    }
)


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
    if name in {"frontendBuild", "interactionLatencyBudget", "scrollUnderLoadSmoke", "reconnectContinuitySmoke"} and shutil.which("npm") is None:
        return "npm 없음"
    if name in {"interactionLatencyBudget", "scrollUnderLoadSmoke", "reconnectContinuitySmoke"} and shutil.which("node") is None:
        return "node 없음"
    if name == "audit" and not hasAuditCrate():
        return "tests/audit crate 없음"
    if name == "cargoShear" and shutil.which("cargo-shear") is None:
        return "cargo-shear 미설치 (cargo binstall cargo-shear)"
    if name == "cargoDeny" and shutil.which("cargo-deny") is None:
        return "cargo-deny 미설치 (cargo binstall cargo-deny)"
    if name == "clippyCrossCfg" and not hasCrossTarget():
        return f"{CROSS_TARGET} 미설치 (rustup target add {CROSS_TARGET})"
    if name == "externalAcpSmoke" and shutil.which("opencode") is None:
        return "독립 ACP CLI 미설치 (npm install --global opencode-ai@1.2.27)"
    if name == "claudeApprovalSmoke" and shutil.which("claude") is None:
        return "Claude Code 미설치 (npm install --global @anthropic-ai/claude-code@2.1.220)"
    return None


def hasCrossTarget() -> bool:
    """교차 cfg 검사 대상이 설치돼 있는가."""
    proc = subprocess.run(
        ["rustup", "target", "list", "--installed"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    return CROSS_TARGET in proc.stdout


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
