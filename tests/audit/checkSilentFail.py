"""silent failure 차단 lint (Rust 우선, TypeScript·Python 동반).

CLAUDE.md 강행규칙 `에러를 삼키지 않는다` 의 기계 강제.

silent failure 란 실패를 잡고서 아무 흔적도 남기지 않고 흘려보내는 코드다.
runtrol 에서 이것이 특히 치명적인 이유: 자식 CLI 프로세스의 비정상 종료와 프로토콜
파싱 실패가 삼켜지면 세션은 "살아있다" 고 보고되면서 실제로는 죽어 있다. 모바일에서
그 세션을 열면 아무 일도 일어나지 않고 원인도 남지 않는다.

허용 조건 (모든 언어 공통):
    위반 라인 또는 바로 앞 3 줄에 `ok:` 로 시작하는 주석이 있으면 통과한다.
    그 주석은 "왜 이게 안전한가 (다음 진행이 보장되는가)" 를 설명해야 한다.

        // ok: hook 자체 실패가 사용자 작업을 막으면 안 된다. fail-open.
        let _ = writer.flush();

Rust 규칙:
    letUnderscore   `let _ = ...`            . 반환값 (대개 Result) 을 이름 없이 버린다
    okDiscard       `... .ok();`             . Result 를 Option 으로 강등하고 버린다
    emptyErrArm     `Err(_) => {}`           . 오류 갈래가 비어 있다
    unwrap          `.unwrap()` `.expect(`   . 비테스트 코드의 패닉 경로
    allowMustUse    `#[allow(unused_must_use)]`
    unsafeNoSafety  `unsafe {` 바로 위 연속 주석 블록에 `SAFETY:` 없음

    `#[cfg(test)]` 모듈 안, `tests/` 아래, `build.rs` 는 unwrap 규칙에서 면제된다.

TypeScript·JavaScript 규칙:
    emptyCatch      `catch (e) {}` · `.catch(() => {})`

Python 규칙 (게이트 harness 용):
    xlpod 계열과 동일한 AST 기반 except 검사.

사용::

    python -X utf8 .claude/hooks/checkSilentFail.py                 # 기본 대상 전체
    python -X utf8 .claude/hooks/checkSilentFail.py crates/a/src/b.rs
    python -X utf8 .claude/hooks/checkSilentFail.py --selftest      # 검출기 자체 검증

`--selftest` 는 이 게이트가 **실패할 수 있는지**를 먼저 증명한다. 잡아야 할 결함을
일부러 넣은 픽스처에서 red 가 나오지 않으면 게이트는 통과 도장일 뿐 검출기가 아니다.

종료 코드:
    0 통과
    2 위반
"""

from __future__ import annotations

import ast
import re
import sys
from pathlib import Path

import rustSource

ROOT = Path(__file__).resolve().parents[2]
OK_MARKER = re.compile(r"(?://|#)\s*ok:")
SAFETY_MARKER = re.compile(r"//\s*SAFETY:")
LOOKBACK = 3

SCAN_DIRS = ("crates", "pwa/src", "relay/src", "tests/audit")
SCAN_SUFFIXES = (".rs", ".ts", ".tsx", ".js", ".mjs", ".py")
SKIP_PARTS = frozenset({"target", "node_modules", "_attempts", ".git", "dist"})

RUST_RULES: tuple[tuple[str, re.Pattern[str], str], ...] = (
    ("letUnderscore", re.compile(r"^\s*let\s+_\s*="), "반환값을 이름 없이 버린다. `Result` 면 실패가 사라진다."),
    ("okDiscard", re.compile(r"\.ok\(\)\s*;"), "`Result` 를 `Option` 으로 강등하고 버린다."),
    ("emptyErrArm", re.compile(r"Err\(\s*_\s*\)\s*=>\s*(\{\s*\}|\(\))"), "오류 갈래가 비어 있다."),
    ("allowMustUse", re.compile(r"#\[allow\(unused_must_use\)\]"), "`must_use` 경고를 통째로 끈다."),
)
RUST_UNWRAP = re.compile(r"\.(unwrap|expect)\s*\(")
RUST_UNSAFE = re.compile(r"\bunsafe\s*\{")

WEB_RULES: tuple[tuple[str, re.Pattern[str], str], ...] = (
    ("emptyCatch", re.compile(r"catch\s*(\([^)]*\))?\s*\{\s*\}"), "catch 본문이 비어 있다."),
    ("emptyCatchArrow", re.compile(r"\.catch\(\s*\([^)]*\)\s*=>\s*\{\s*\}\s*\)"), "`.catch` 본문이 비어 있다."),
)

PY_OK_MARKER = "# ok:"

def _hasOkComment(lines: list[str], index: int) -> bool:
    """위반 라인 또는 바로 앞 LOOKBACK 줄에 `ok:` 주석이 있는가."""
    start = max(0, index - LOOKBACK)
    return any(OK_MARKER.search(lines[i]) for i in range(start, index + 1))


def _hasSafetyComment(lines: list[str], index: int) -> bool:
    """`unsafe` 바로 위에 붙은 주석 블록 안에 `SAFETY:` 근거가 있는가.

    고정 줄 수가 아니라 **연속 주석 블록**을 본다. 앞 3 줄만 보던 판은 근거가 길수록
    불리했다 (4 줄로 제대로 논증하면 게이트가 근거 없음으로 판정). 근거의 길이를
    벌주는 게이트는 짧고 무의미한 근거를 유도한다.

    블록은 코드나 빈 줄에서 끊긴다. 그래서 코드를 건너뛴 위쪽의 `SAFETY:` 는 여전히
    인정되지 않는다. 인접성은 지키고 길이 제한만 없앤다.
    """
    if SAFETY_MARKER.search(lines[index]):
        return True
    cursor = index - 1
    while cursor >= 0:
        stripped = lines[cursor].strip()
        if not stripped.startswith("//"):
            # 주석 블록의 끝. 여기서 멈추므로 코드 위쪽의 근거는 잡히지 않는다.
            return False
        if SAFETY_MARKER.search(lines[cursor]):
            return True
        cursor -= 1
    return False


def lintRust(rel: str, source: str, unwrapExempt: bool) -> list[str]:
    """단일 Rust 파일의 위반 목록."""
    lines = source.splitlines()
    testRegions = rustSource.testRegions(lines)
    violations: list[str] = []

    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("//"):
            continue
        if _hasOkComment(lines, index):
            continue

        for ruleName, pattern, why in RUST_RULES:
            if pattern.search(line):
                violations.append(f"  - {rel}:{index + 1} [{ruleName}] {why}")

        if RUST_UNWRAP.search(line) and not unwrapExempt and not rustSource.inRegions(index, testRegions):
            violations.append(f"  - {rel}:{index + 1} [unwrap] 비테스트 코드의 패닉 경로. `?` 또는 명시 처리로 바꿔라.")

        if RUST_UNSAFE.search(line) and not _hasSafetyComment(lines, index):
            violations.append(f"  - {rel}:{index + 1} [unsafeNoSafety] `unsafe` 앞에 `// SAFETY:` 근거가 없다.")

    return violations


def lintWeb(rel: str, source: str) -> list[str]:
    """단일 TypeScript·JavaScript 파일의 위반 목록."""
    lines = source.splitlines()
    violations: list[str] = []
    for index, line in enumerate(lines):
        if _hasOkComment(lines, index):
            continue
        for ruleName, pattern, why in WEB_RULES:
            if pattern.search(line):
                violations.append(f"  - {rel}:{index + 1} [{ruleName}] {why}")
    return violations


def _pyBodyIsNoop(body: list[ast.stmt]) -> str | None:
    """except 본문이 무행동 (pass / Ellipsis / continue) 이면 그 종류를 반환한다."""
    if len(body) != 1:
        return None
    stmt = body[0]
    if isinstance(stmt, ast.Pass):
        return "pass"
    if isinstance(stmt, ast.Continue):
        return "continue"
    if isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Constant) and stmt.value.value is Ellipsis:
        return "..."
    return None


def lintPython(rel: str, source: str) -> list[str]:
    """단일 Python 파일의 위반 목록. AST 기반."""
    try:
        tree = ast.parse(source)
    except SyntaxError:
        # ok: 문법 오류는 ruff 가 잡는다. 여기서 중복 보고하지 않는다.
        return []

    lines = source.splitlines()
    violations: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.ExceptHandler):
            continue
        start = node.lineno - 1
        end = node.end_lineno or node.lineno
        if any(PY_OK_MARKER in lines[i] for i in range(start, min(end, len(lines)))):
            continue

        noop = _pyBodyIsNoop(node.body)
        if noop:
            violations.append(f"  - {rel}:{node.lineno} [silentPy/{noop}] except 본문이 `{noop}` 뿐이다.")
        elif node.type is None:
            violations.append(f"  - {rel}:{node.lineno} [silentPy/bare] 잡을 예외를 지정하지 않았다.")
    return violations


def lintFile(path: Path) -> list[str]:
    """확장자에 맞는 linter 로 위임한다."""
    try:
        source = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        # ok: 읽을 수 없는 파일은 lint 대상이 아니다. 다음 파일로 진행한다.
        return []

    try:
        rel = path.resolve().relative_to(ROOT).as_posix()
    except ValueError:
        rel = path.as_posix()

    if path.suffix == ".rs":
        parts = set(path.parts)
        unwrapExempt = path.name == "build.rs" or "tests" in parts or "benches" in parts
        return lintRust(rel, source, unwrapExempt)
    if path.suffix in (".ts", ".tsx", ".js", ".mjs"):
        return lintWeb(rel, source)
    if path.suffix == ".py":
        return lintPython(rel, source)
    return []


def defaultTargets() -> list[Path]:
    """기본 검사 대상. 아직 없는 디렉토리는 조용히 건너뛴다 (부트스트랩 단계)."""
    targets: list[Path] = []
    for name in SCAN_DIRS:
        base = ROOT / name
        if not base.exists():
            continue
        for path in base.rglob("*"):
            if not path.is_file() or path.suffix not in SCAN_SUFFIXES:
                continue
            if SKIP_PARTS & set(path.parts):
                continue
            targets.append(path)
    return sorted(targets)


SELFTEST_CASES: tuple[tuple[str, str, int], ...] = (
    ("letUnderscore.rs", "fn main() {\n    let _ = writer.flush();\n}\n", 1),
    ("letUnderscoreOk.rs", "fn main() {\n    // ok: flush 실패해도 다음 tick 이 다시 쓴다.\n    let _ = writer.flush();\n}\n", 0),
    ("okDiscard.rs", "fn main() {\n    child.kill().ok();\n}\n", 1),
    ("emptyErrArm.rs", "fn main() {\n    match run() { Err(_) => {}, Ok(v) => v }\n}\n", 1),
    ("unwrapProd.rs", "fn main() {\n    let v = maybe().unwrap();\n}\n", 1),
    (
        "unwrapTest.rs",
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        let v = maybe().unwrap();\n    }\n}\n",
        0,
    ),
    # **실제로 났던 오검출.** raw string 안의 이스케이프된 인용부호가 보통 문자열 규칙을 속여서
    # 중괄호 하나가 코드로 세어졌고, `#[cfg(test)]` 블록의 끝이 실제보다 앞으로 잡혔다. 그 뒤의
    # 테스트 코드가 프로덕션 패닉 경로로 신고됐다. 픽스처는 그 줄의 모양 그대로다.
    (
        "unwrapTestAfterRawStringWithEscape.rs",
        "#[cfg(test)]\nmod tests {\n"
        '    const ID: &str = r#"{"id":"a\\"b","result":null}"#;\n'
        "    #[test]\n    fn t() {\n        let v = maybe().unwrap();\n    }\n}\n",
        0,
    ),
    # char 리터럴은 반대 방향으로 틀린다. `'{'` 를 코드로 세면 블록이 닫히지 않고 파일 끝까지
    # 늘어나서, 테스트 모듈 **뒤의** 프로덕션 코드가 면제된다. 놓치는 쪽이 더 위험하다.
    (
        "unwrapProdAfterCharLiteralInTests.rs",
        "#[cfg(test)]\nmod tests {\n"
        "    const OPEN: char = '{';\n"
        "    #[test]\n    fn t() {}\n}\n"
        "fn production() -> u32 {\n    maybe().unwrap()\n}\n",
        1,
    ),
    # lifetime 은 char 리터럴이 아니다. 함께 지워버리면 다른 규칙에서 새 오검출이 난다.
    (
        "unwrapProdWithLifetime.rs",
        "fn pick<'a>(x: &'a str) -> &'a str {\n    maybe().unwrap()\n}\n",
        1,
    ),
    ("unsafeNoSafety.rs", "fn main() {\n    unsafe { ptr.read() };\n}\n", 1),
    # 근거가 길어도 인정된다. 앞 3 줄만 보던 판은 이 픽스처에서 오검출을 냈다.
    (
        "unsafeLongSafety.rs",
        "fn main() {\n"
        "    // SAFETY: line one of the argument,\n"
        "    // line two,\n"
        "    // line three,\n"
        "    // line four.\n"
        "    unsafe { ptr.read() };\n"
        "}\n",
        0,
    ),
    # 코드를 건너뛴 위쪽의 근거는 인정되지 않는다. 인접성은 그대로다.
    (
        "unsafeDetachedSafety.rs",
        "fn main() {\n"
        "    // SAFETY: this argument is about something else entirely.\n"
        "    let value = compute();\n"
        "    unsafe { ptr.read() };\n"
        "}\n",
        1,
    ),
    ("unsafeWithSafety.rs", "fn main() {\n    // SAFETY: ptr 는 바로 위에서 non-null 로 검증됐다.\n    unsafe { ptr.read() };\n}\n", 0),
    ("emptyCatch.ts", "try { risky(); } catch (e) {}\n", 1),
)


def selftest() -> int:
    """검출기가 실제로 red 를 낼 수 있는지 증명한다. 못 잡으면 게이트 자체가 결함이다."""
    import tempfile

    failures: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        for name, source, expected in SELFTEST_CASES:
            path = Path(tmp) / name
            path.write_text(source, encoding="utf-8")
            found = len(lintFile(path))
            status = "ok  " if found == expected else "FAIL"
            print(f"  {status} {name}: 기대 {expected} 건, 검출 {found} 건")
            if found != expected:
                failures.append(name)

    if failures:
        sys.stderr.write(f"\n[checkSilentFail --selftest] 검출기 결함 {len(failures)} 건: {', '.join(failures)}\n")
        sys.stderr.write("게이트가 잡아야 할 것을 못 잡거나, 안전한 코드를 잡고 있다. 규칙을 고쳐라.\n")
        return 2
    print(f"\n[checkSilentFail --selftest] OK. 픽스처 {len(SELFTEST_CASES)} 건 전부 기대대로 판정.")
    return 0


def main(argv: list[str]) -> int:
    """silent failure 를 검사한다. 위반이면 2."""
    if "--selftest" in argv:
        return selftest()

    args = [a for a in argv if not a.startswith("--")]
    targets = [Path(a) for a in args] if args else defaultTargets()
    if not targets:
        print("[checkSilentFail] 검사 대상 없음 (crates/ · pwa/src/ · tests/audit/ 미생성).")
        return 0

    violations: list[str] = []
    for path in targets:
        violations.extend(lintFile(path))

    if not violations:
        print(f"[checkSilentFail] OK. 검사 {len(targets)} 파일, 위반 0 건.")
        return 0

    sys.stderr.write(f"[checkSilentFail] silent failure {len(violations)} 건:\n")
    for line in violations:
        sys.stderr.write(line + "\n")
    sys.stderr.write(
        "\n룰 SSOT: CLAUDE.md `에러를 삼키지 않는다`.\n"
        "정말 삼켜야 한다면 그 줄 또는 바로 앞에 `// ok: <왜 안전한가 . 다음 진행이 어떻게 보장되는가>` 를 달아라.\n"
        "runtrol 에서 삼켜진 자식 프로세스 실패는 살아있다고 거짓 보고되는 죽은 세션이 된다.\n"
    )
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
