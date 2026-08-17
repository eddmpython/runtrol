"""Tracked files may only point at things a stranger who cloned this repository actually has.

Two failures this catches, both of which reach a first-time reader before they reach us.

**A link to a file that is not there.** This repository keeps four README translations and nineteen
operational documents that cross-reference each other. A path that was right when it was written
survives a rename as a link that 404s on GitHub, and nothing else in the gate set reads a link target.

**A public file citing a provisional initiative.** `mainPlan/` holds initiatives, and an initiative is
deleted the moment it is finished (its knowledge is promoted to `docs/`). So a tracked file that cites
`mainPlan/` is not merely fragile: it is guaranteed to dangle, and it points a contributor at a design
sketch that may already disagree with the code. `runtimeDocumentation.py` asserted this for the six
public Runtime documents; this gate is the whole-repository version, and that narrower copy defers here.

Both checks read `git ls-files`, so "exists" means tracked rather than present on disk. That is the
question a stranger's clone asks, and it is the only question a local directory listing cannot answer.

Usage:
    python -X utf8 tests/audit/publicReferences.py
    python -X utf8 tests/audit/publicReferences.py --selftest
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

# `[text](target)` with the target captured. Angle-bracket targets (`[t](<a b>)`) are accepted because
# the syntax exists for paths with spaces, which is exactly the case most likely to be typed wrong.
LINK = re.compile(r"\[(?:[^\]]*)\]\(\s*<?([^)\s>]+)>?[^)]*\)")

# Targets that are not repository paths and cannot be checked by looking at the tree.
EXTERNAL = re.compile(r"^(?:[a-z][a-z0-9+.-]*:|//|#)", re.IGNORECASE)

# The provisional layer. A tracked file citing this is the failure, so the pattern is matched against
# text rather than only against link targets: prose that names the folder dangles just as hard.
PROVISIONAL = "mainPlan/"

# Files that must name the provisional layer to do their job. Each is a rule about `mainPlan/`, not a
# reference to its contents, so neither one dangles when an initiative is deleted.
PROVISIONAL_EXEMPT = frozenset(
    {
        # Declares the path untracked in the first place.
        ".gitignore",
        # This gate, which has to hold the pattern it forbids.
        "tests/audit/publicReferences.py",
        # Keeps the folder legal as a top-level directory on an operator's disk.
        "tests/audit/workspaceHygiene.py",
        # States that the table outranks any document under that folder.
        "tests/audit/dependencyDirection.rs",
    }
)

TEXT_SUFFIXES = frozenset({".md", ".rs", ".ts", ".js", ".py", ".toml", ".yml", ".yaml", ".json"})


def trackedFiles() -> list[str]:
    """Every path git tracks, as forward-slash repo-relative strings."""
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=REPO,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.decode("utf-8", errors="replace").strip()
        raise SystemExit(f"[publicReferences] `git ls-files` failed: {detail}")
    return [entry for entry in result.stdout.decode("utf-8").split("\0") if entry]


def readText(path: Path) -> str | None:
    """The file's text, or None when it is not text this gate can read."""
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        # A tracked binary or an unreadable file is not this gate's concern. Both are visible to
        # other gates, and guessing at an encoding here would invent findings.
        return None


def resolveTarget(source: str, target: str) -> str | None:
    """The repo-relative path a link points at, or None when it is not a repository path."""
    if EXTERNAL.match(target):
        return None
    # `path#L42` and `path#section` both address the same file.
    cleaned = target.split("#", 1)[0].split("?", 1)[0].strip()
    if not cleaned:
        return None
    if cleaned.startswith("/"):
        base = Path(cleaned.lstrip("/"))
    else:
        base = Path(source).parent / cleaned
    parts: list[str] = []
    for part in base.as_posix().split("/"):
        if part in ("", "."):
            continue
        if part == "..":
            if not parts:
                # Escapes the repository. Not a repository path, so not this gate's business.
                return None
            parts.pop()
            continue
        parts.append(part)
    return "/".join(parts) if parts else None


def brokenLinks(tracked: frozenset[str], name: str, body: str) -> list[str]:
    """Every markdown link in one file whose target git does not track."""
    directories = {entry.rsplit("/", 1)[0] for entry in tracked if "/" in entry}
    found: list[str] = []
    for match in LINK.finditer(body):
        target = resolveTarget(name, match.group(1))
        if target is None or target in tracked:
            continue
        # A link to a folder is satisfied by the folder holding something tracked.
        if target in directories or any(entry.startswith(f"{target}/") for entry in tracked):
            continue
        found.append(f"{name} links to `{match.group(1)}`, which git does not track")
    return found


def citesProvisional(name: str, body: str) -> list[str]:
    """Whether a tracked file points at the initiative layer."""
    if name in PROVISIONAL_EXEMPT or PROVISIONAL not in body:
        return []
    return [
        f"{name} cites `{PROVISIONAL}`, which is untracked and is deleted when an initiative finishes"
    ]


def audit() -> list[str]:
    """Every finding, in a stable order."""
    tracked = frozenset(trackedFiles())
    found: list[str] = []
    for name in sorted(tracked):
        if Path(name).suffix.lower() not in TEXT_SUFFIXES:
            continue
        body = readText(REPO / name)
        if body is None:
            continue
        if name.endswith(".md"):
            found.extend(brokenLinks(tracked, name, body))
        found.extend(citesProvisional(name, body))
    return found


def selftest() -> int:
    """Proves each check can fail, because a check that cannot fail is worse than no check."""
    failures: list[str] = []

    tracked = frozenset({"docs/README.md", "docs/positioning.md", "crates/runtrol-core/src/lib.rs"})

    cases: list[tuple[str, list[str], bool]] = [
        (
            "a link to a tracked sibling passes",
            brokenLinks(tracked, "docs/README.md", "see [positioning](positioning.md)"),
            False,
        ),
        (
            "a link to an untracked sibling fails",
            brokenLinks(tracked, "docs/README.md", "see [gone](removed.md)"),
            True,
        ),
        (
            "a link out of the docs folder resolves against the file, not the root",
            brokenLinks(tracked, "docs/README.md", "see [core](../crates/runtrol-core/src/lib.rs)"),
            False,
        ),
        (
            "a root-anchored link resolves against the repository root",
            brokenLinks(tracked, "docs/README.md", "see [core](/crates/runtrol-core/src/lib.rs)"),
            False,
        ),
        (
            "a line anchor does not make a tracked file look missing",
            brokenLinks(tracked, "docs/README.md", "see [core](../crates/runtrol-core/src/lib.rs#L4)"),
            False,
        ),
        (
            "a folder link is satisfied by the folder holding something tracked",
            brokenLinks(tracked, "docs/README.md", "see [crates](../crates/runtrol-core/)"),
            False,
        ),
        (
            "an external link is not treated as a path",
            brokenLinks(tracked, "docs/README.md", "see [site](https://example.invalid/x.md)"),
            False,
        ),
        (
            "a bare anchor is not treated as a path",
            brokenLinks(tracked, "docs/README.md", "see [top](#heading)"),
            False,
        ),
        (
            "a link escaping the repository is left alone",
            brokenLinks(tracked, "docs/README.md", "see [out](../../elsewhere/x.md)"),
            False,
        ),
        (
            "citing the initiative layer fails",
            citesProvisional("README.md", "see [plan](mainPlan/README.md)"),
            True,
        ),
        (
            "citing it in prose fails too",
            citesProvisional("docs/README.md", "the decision came from mainPlan/ documents"),
            True,
        ),
        (
            "a file whose job is to name the layer is exempt",
            citesProvisional(".gitignore", "mainPlan/"),
            False,
        ),
    ]

    for label, result, shouldFail in cases:
        if bool(result) != shouldFail:
            failures.append(f"{label}: expected {'a finding' if shouldFail else 'none'}, got {result}")

    if failures:
        for failure in failures:
            sys.stderr.write(f"[publicReferences] selftest {failure}\n")
        return 1
    sys.stdout.write(f"[publicReferences] selftest ok ({len(cases)} cases)\n")
    return 0


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    found = audit()
    if not found:
        sys.stdout.write("[publicReferences] ok\n")
        return 0
    for finding in found:
        sys.stderr.write(f"[publicReferences] {finding}\n")
    sys.stderr.write(
        f"\n{len(found)} finding(s). A tracked file may only point at what a fresh clone has.\n"
        "Rule SSOT: CLAUDE.md `SSOT 단일화` and the three information layers.\n"
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
