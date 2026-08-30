"""Write one release's own section of the changelog, for the release page to carry.

The whole changelog used to be the release body. It stopped fitting: GitHub refuses a release body over
125,000 characters, and on 2026-08-30 the file reached 126,528 and every release creation answered
``HTTP 422: Validation Failed`` after all six targets had already been built and published. A release page
should say what that release changed anyway, and the file it comes from keeps the rest.

The section is taken verbatim between this version's heading and the next one. Nothing is reworded here: the
changelog is the canon, and this is a projection of one of its parts.

Usage::

    python -X utf8 .github/scripts/release/releaseNotes.py <version> <output file>
    python -X utf8 .github/scripts/release/releaseNotes.py --selftest

Exit codes:
    0 the section was written
    2 the changelog has no section for that version, or the section is empty
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
CHANGELOG = ROOT / "CHANGELOG.md"

# What GitHub accepts in a release body. The section is far smaller, and a version whose notes somehow grew
# past this is truncated with a pointer rather than refused after the whole release has been built.
MAX_BODY = 125_000
TRUNCATED = "\n\n(The rest of this release's notes are in `CHANGELOG.md`.)\n"


def section(changelog: str, version: str) -> str:
    """The changelog's own text for one version, without its heading."""
    opening = f"## [{version}]"
    start = changelog.find(opening)
    if start < 0:
        return ""
    body = changelog[start + len(opening) :]
    # Past the rest of the heading line, then up to the next version heading.
    after_heading = body.find("\n")
    if after_heading < 0:
        return ""
    body = body[after_heading + 1 :]
    end = body.find("\n## ")
    if end >= 0:
        body = body[:end]
    return body.strip()


def notes(changelog: str, version: str) -> str:
    """One version's section, bounded to what a release body may carry."""
    text = section(changelog, version)
    if len(text) <= MAX_BODY:
        return text
    return text[: MAX_BODY - len(TRUNCATED)] + TRUNCATED


def selftest() -> int:
    """Every shape this has to get right, including the one that broke a release."""
    changelog = "\n".join(
        [
            "# Changelog",
            "",
            "## [Unreleased]",
            "",
            "### Fixed",
            "",
            "- something not released yet",
            "",
            "## [0.1.39] - 2026-08-30",
            "",
            "### Fixed",
            "",
            "- the thing this release fixed",
            "",
            "## [0.1.38] - 2026-08-29",
            "",
            "- an older thing",
            "",
        ]
    )
    found: list[str] = []
    if section(changelog, "0.1.39") != "### Fixed\n\n- the thing this release fixed":
        found.append("a version's own section was not taken verbatim")
    if section(changelog, "0.1.38") != "- an older thing":
        found.append("the last section in the file was not taken")
    if section(changelog, "Unreleased") != "### Fixed\n\n- something not released yet":
        found.append("the unreleased section was not taken")
    if section(changelog, "9.9.9") != "":
        found.append("a version the changelog does not have said something")
    long_one = "## [1.0.0]\n\n" + ("x" * (MAX_BODY + 500))
    written = notes(long_one, "1.0.0")
    if len(written) > MAX_BODY or not written.endswith(TRUNCATED):
        found.append("an oversized section was not bounded with a pointer")
    if found:
        for problem in found:
            print(f"[releaseNotes --selftest] FAIL. {problem}", file=sys.stderr)
        return 2
    print("[releaseNotes --selftest] OK. four sections and the size bound.")
    return 0


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    if len(argv) != 2:
        print(
            "usage: releaseNotes.py <version> <output file>",
            file=sys.stderr,
        )
        return 2
    version, destination = argv
    text = notes(CHANGELOG.read_text(encoding="utf-8"), version)
    if not text:
        print(
            f"[releaseNotes] FAIL. CHANGELOG.md has no section for {version}.",
            file=sys.stderr,
        )
        return 2
    Path(destination).write_text(f"{text}\n", encoding="utf-8")
    print(f"[releaseNotes] OK. {len(text)} characters for {version}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
