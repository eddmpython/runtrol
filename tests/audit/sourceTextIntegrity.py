"""Gate: a source file holds the characters somebody meant to write.

A control byte reaches a source file when text passes through a shell on its way in. A heredoc, a quoted argument,
an editor writing what it was handed: any of them can turn an escape sequence into the byte it names, and nothing
downstream notices. The compiler does not care. The type checker does not care. The test suite does not care,
because the corrupted value usually still behaves like the one that was meant.

Three of these were found in this repository on one day, and they had been there for weeks:

- ``webview.css`` held ``content: "<0x15>B8"`` where a disclosure triangle belonged. It shipped, and every reader
  saw a control character and the letters ``B8`` where an arrow should have been. Neither the type check nor the
  unit tests could see it; a screenshot did.
- ``conversationList.test.ts`` held ``"chat<NUL>gone<NUL>gone"`` where a conversation key belonged. The test still
  passed, because an unknown key is an unknown key whichever bytes it is made of. It was born corrupt in the
  commit that added it.
- The same file held a character class written with raw control bytes instead of the escapes that name them. It
  behaves identically and cannot be read, edited, or grepped: ``git grep`` reports the whole file as binary and
  silently stops searching it, which is how the other two were nearly missed.

That last consequence is the reason this gate is worth its weight. One raw byte turns a source file into
something the repository's own search cannot read, and every later investigation of that file starts blind.

# What is allowed

Tab, newline, and carriage return. Nothing else below ``0x20``, and not ``0x7f``. A file that genuinely needs a
control character writes the escape its language provides, which every language here has.

Usage::

    python -X utf8 tests/audit/sourceTextIntegrity.py
    python -X utf8 tests/audit/sourceTextIntegrity.py --selftest

Exit codes:
    0 every tracked source file is readable text
    2 a source file carries a control byte, or the selftest could not make this gate fail
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# The extensions whose files are read and edited as text by a person. A binary asset is not source and is not
# checked; listing what is checked rather than what is skipped keeps a new binary format from silently opting in.
SOURCE_SUFFIXES = frozenset(
    {
        ".rs", ".ts", ".tsx", ".js", ".mjs", ".cjs", ".py", ".css", ".html",
        ".md", ".toml", ".json", ".yml", ".yaml", ".sh", ".ps1", ".cmd", ".txt",
    }
)

ALLOWED_CONTROL = frozenset({0x09, 0x0A, 0x0D})

# Third-party code this repository carries but did not write.
#
# The reason this gate exists is that a raw control byte makes *our* source unreadable and ungreppable, and the
# answer to that is to write the escape the language provides. Neither half applies to a vendored bundle: it is
# minified, it is not edited here, and rewriting its bytes would break the digest that makes vendoring honest.
# The same boundary the silent-failure gate draws (`tests/audit/checkSilentFail.py`).
VENDORED = ("/vendor/",)


def offending(data: bytes) -> list[tuple[int, int]]:
    """Return every (offset, byte) that has no business in a source file."""
    return [
        (offset, byte)
        for offset, byte in enumerate(data)
        if (byte < 0x20 and byte not in ALLOWED_CONTROL) or byte == 0x7F
    ]


def placeOf(data: bytes, offset: int) -> str:
    """Return a line and column for an offset, so the report points somewhere a person can go."""
    line = data.count(b"\n", 0, offset) + 1
    lineStart = data.rfind(b"\n", 0, offset) + 1
    return f"line {line}, column {offset - lineStart + 1}"


def trackedSources() -> list[Path]:
    """Return the tracked files this gate reads."""
    listed = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=ROOT,
        capture_output=True,
        check=True,
    )
    deleted = subprocess.run(
        ["git", "ls-files", "-z", "--deleted"],
        cwd=ROOT,
        capture_output=True,
        check=True,
    )
    names = listed.stdout.decode("utf-8", "surrogateescape").split("\0")
    deleted_names = frozenset(
        deleted.stdout.decode("utf-8", "surrogateescape").split("\0")
    )
    return [
        ROOT / name
        for name in names
        if name
        and name not in deleted_names
        and Path(name).suffix.lower() in SOURCE_SUFFIXES
        and not any(marker in f"/{name}" for marker in VENDORED)
    ]


def main() -> int:
    """Read every tracked source file and report the control bytes in it."""
    findings: list[str] = []
    read = 0
    for path in trackedSources():
        try:
            data = path.read_bytes()
        except OSError as error:
            # A tracked file that cannot be read is a finding, not a reason to pass. Reporting it beats a green
            # result that silently covered one file fewer.
            findings.append(f"{path.relative_to(ROOT).as_posix()}: could not be read ({error})")
            continue
        read += 1
        for offset, byte in offending(data)[:4]:
            findings.append(
                f"{path.relative_to(ROOT).as_posix()}: {placeOf(data, offset)} "
                f"holds byte {byte:#04x}, which no source file should carry"
            )
    if findings:
        print("[sourceTextIntegrity] control bytes in source:", file=sys.stderr)
        for finding in findings:
            print(f"  - {finding}", file=sys.stderr)
        print(
            "\nWrite the escape the language provides instead. Text that passes through a shell on its way "
            "into a file is how these arrive.",
            file=sys.stderr,
        )
        return 2
    print(f"[sourceTextIntegrity] OK. {read} tracked source files are readable text.")
    return 0


def selftest() -> int:
    """Prove each byte class this gate exists to catch makes it red, and that ordinary text does not."""
    clean = b'const key = "chat:gone:gone";\n\tconst pattern = /[\\x00-\\x1f]/u;\r\n'
    if offending(clean):
        print("[sourceTextIntegrity --selftest] FAIL. ordinary text was rejected.", file=sys.stderr)
        return 2
    injected = {
        "a NUL where a separator belonged": b'const key = "chat\x00gone";\n',
        "a raw control character inside a pattern": b"const pattern = /[\x00-\x1f]/u;\n",
        "the byte that shipped in a stylesheet": b'  content: "\x15B8";\n',
        "a delete character": b"const name = \x7f;\n",
    }
    for what, data in injected.items():
        if not offending(data):
            print(f"[sourceTextIntegrity --selftest] FAIL. {what} escaped.", file=sys.stderr)
            return 2
    if placeOf(b"one\ntwo\x00three\n", 7) != "line 2, column 4":
        print("[sourceTextIntegrity --selftest] FAIL. the report points at the wrong place.", file=sys.stderr)
        return 2
    print("[sourceTextIntegrity --selftest] OK. four injected byte classes make this gate red.")
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest() if "--selftest" in sys.argv else main())
