"""Tracked files may only point at things a stranger who cloned this repository actually has.

Three failures this catches, all of which reach a first-time reader before they reach us.

**A link to a file that is not there.** This repository keeps four README translations and nineteen
operational documents that cross-reference each other. A path that was right when it was written
survives a rename as a link that 404s on GitHub, and nothing else in the gate set reads a link target.

**A backtick citation of a file that is not there.** Most references here are not markdown links. A
Cargo comment says which gate pins a lint table, a design document names the gate that guards an
isolation rule, and both write the path in backticks inside prose. Nothing renders those, so a rename
leaves them silently wrong, and they are precisely the references a reader follows to check that a
stated rule is really enforced. Found the first time this check ran: three tracked files pointed at
`.rs` gates that had been rewritten in Python, so every one of them sent a reader to a missing file
while this gate reported green.

A backtick token is treated as a repository path only when it names exactly one file: it carries a
known extension, holds no glob or interpolation, sits under no generated directory, and starts with a
directory git tracks. It resolves when it equals any tail of a tracked path, which is what lets a crate
cite its own `tests/containment.rs` without spelling the whole way down from the root.

Everything else in backticks stays out of reach on purpose, and each exclusion answers something this
tree actually writes. RPC method names (`sessions/list`) and media types (`image/png`) carry no
extension. Globs name a set rather than a file. Template literals name whatever the program puts in
them, and this tree holds twenty of those. Generated trees (`pwa/dist`, `crates/*/target`) are in no
clone by construction. The last one is the load-bearing one: a citation that does not start at a
tracked directory is a relative path whose base this gate cannot know, so it is left alone. That is a
real cost, it is wider than the cases above, and it is what keeps the check from arguing with prose.
A gate that argued with prose would be turned off within a week.

**A public file citing a provisional initiative.** `mainPlan/` holds initiatives, and an initiative is
deleted the moment it is finished (its knowledge is promoted to `docs/`). So a tracked file that cites
`mainPlan/` is not merely fragile: it is guaranteed to dangle, and it points a contributor at a design
sketch that may already disagree with the code. `runtimeDocumentation.py` asserted this for the six
public Runtime documents; this gate is the whole-repository version, and that narrower copy defers here.

All three checks read `git ls-files`, so "exists" means tracked rather than present on disk. That is the
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

# `` `token` `` with the token captured. Prose cites paths this way far more often than it links them.
CITED = re.compile(r"`([^`\s]+)`")

# A token holding any of these names a set or a computed name, never one file. Globs (`docs/*.md`) are
# the obvious half. The other half is interpolation: this tree writes `crates/${name}/Cargo.toml` in
# TypeScript and `providers/<id>.toml` in prose, and resolving either against the tree is meaningless.
NOT_ONE_PATH = re.compile(r"[*?\[\]{}<>$]")

# Directory names that hold build output. Git tracks nothing under any of them (measured), so a citation
# of one names an artifact that no clone has and no rename can break.
GENERATED = frozenset({"target", "dist", "build", "node_modules"})

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

# Text this gate opens and scans. `.mjs` belongs here because the extension's tooling is written in it,
# and a file the gate never opens is a file whose dangling citations it can never see.
READ_SUFFIXES = frozenset(
    {
        ".md", ".rs", ".ts", ".tsx", ".js", ".mjs", ".py",
        ".toml", ".yml", ".yaml", ".json", ".jsonc", ".css", ".html", ".ps1",
    }
)  # fmt: skip

# Extensions a backtick token must carry to be claiming it is a file. Wider than what the gate reads,
# because prose cites assets it would be pointless to scan: a renamed `assets/brand/symbol.svg` dangles
# exactly as hard as a renamed module, and a binary is the one thing a reader cannot guess the fate of.
CITED_SUFFIXES = READ_SUFFIXES | frozenset(
    {".svg", ".png", ".jpg", ".ico", ".woff2", ".wasm", ".webmanifest", ".sh", ".lock"}
)


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


def trackedTails(tracked: frozenset[str]) -> frozenset[str]:
    """Every tail of every tracked path, which is what a citation is allowed to spell."""
    tails: set[str] = set()
    for entry in tracked:
        parts = entry.split("/")
        for index in range(len(parts)):
            tails.add("/".join(parts[index:]))
    return frozenset(tails)


def citationTarget(token: str) -> str | None:
    """The single repository path a backtick token names, or None when it names no single path."""
    if NOT_ONE_PATH.search(token):
        return None
    # `path.md#section` addresses the same file, and prose ends a citation with the sentence's own
    # punctuation. `resolveTarget` already strips both for links; the two checks must agree.
    cleaned = token.split("#", 1)[0].rstrip(".,;:)")
    if "/" not in cleaned:
        return None
    if Path(cleaned).suffix.lower() not in CITED_SUFFIXES:
        return None
    if GENERATED.intersection(cleaned.split("/")):
        return None
    return cleaned


def citedPaths(tops: frozenset[str], tails: frozenset[str], name: str, body: str) -> list[str]:
    """Every backtick-quoted repository path in one file that git does not track."""
    found: list[str] = []
    for match in CITED.finditer(body):
        target = citationTarget(match.group(1))
        if target is None or target in tails:
            continue
        # Only a token rooted in a directory git tracks is claiming to be a path from the root. Anything
        # else is relative to a base this gate cannot know, and guessing at one would invent findings.
        if target.split("/", 1)[0] not in tops:
            continue
        found.append(f"{name} cites `{match.group(1)}`, which git does not track")
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
    tails = trackedTails(tracked)
    tops = frozenset(entry.split("/", 1)[0] for entry in tracked)
    found: list[str] = []
    for name in sorted(tracked):
        if Path(name).suffix.lower() not in READ_SUFFIXES:
            continue
        body = readText(REPO / name)
        if body is None:
            continue
        if name.endswith(".md"):
            found.extend(brokenLinks(tracked, name, body))
        found.extend(citedPaths(tops, tails, name, body))
        found.extend(citesProvisional(name, body))
    return found


def selftest() -> int:
    """Proves each check can fail, because a check that cannot fail is worse than no check."""
    failures: list[str] = []

    tracked = frozenset({"docs/README.md", "docs/positioning.md", "crates/runtrol-core/src/lib.rs"})

    # A second tree, whose top-level names this repository does not have. The citation cases below have
    # to spell paths that do not exist, and this gate scans its own source like any other file. Naming
    # them under `docs/` would make every counterexample a real finding against the real tree, and the
    # only way out of that would be to exempt this file from its own check. A fixture root costs nothing
    # and leaves the gate policing the thirty lines of docstring above, which cite four other gates.
    cited = frozenset({"shelf/README.md", "shelf/deep/positioning.md", "vault/runtrol-core/src/lib.rs"})
    citedTops = frozenset(entry.split("/", 1)[0] for entry in cited)
    citedTails = trackedTails(cited)

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
            "a backtick citation of a tracked file passes",
            citedPaths(citedTops, citedTails, "shelf/README.md", "pinned by `shelf/deep/positioning.md`"),
            False,
        ),
        (
            "a backtick citation of a missing file under a tracked directory fails",
            citedPaths(citedTops, citedTails, "shelf/README.md", "pinned by `shelf/removed.md`"),
            True,
        ),
        (
            "a citation spelling only the tail of a tracked path resolves",
            citedPaths(citedTops, citedTails, "shelf/README.md", "the crate's own `src/lib.rs`"),
            False,
        ),
        (
            "an anchor does not make a tracked file look missing",
            citedPaths(citedTops, citedTails, "shelf/README.md", "see `shelf/deep/positioning.md#top`"),
            False,
        ),
        (
            "an anchor does not hide a missing file either",
            citedPaths(citedTops, citedTails, "shelf/README.md", "see `shelf/removed.md#top`"),
            True,
        ),
        (
            "the sentence's own punctuation is not part of the path",
            citedPaths(citedTops, citedTails, "shelf/README.md", "see `shelf/removed.md`, later"),
            True,
        ),
        (
            "a cited asset dangles as hard as a cited module",
            citedPaths(citedTops, citedTails, "shelf/README.md", "the mark is `shelf/mark.svg`"),
            True,
        ),
        (
            "a glob names a set, not a file",
            citedPaths(citedTops, citedTails, "shelf/README.md", "every `shelf/*.md` is translated"),
            False,
        ),
        (
            "a template literal names whatever the program puts in it",
            citedPaths(citedTops, citedTails, "shelf/README.md", "reads `shelf/${view}.ts` at run time"),
            False,
        ),
        (
            "an angle-bracket placeholder is a template too",
            citedPaths(citedTops, citedTails, "shelf/README.md", "one manifest per `shelf/<id>.toml`"),
            False,
        ),
        (
            "a generated tree is in no clone by construction",
            citedPaths(citedTops, citedTails, "shelf/README.md", "built to `shelf/target/debug/out.rs`"),
            False,
        ),
        (
            "an rpc method name is not a path",
            citedPaths(citedTops, citedTails, "shelf/README.md", "the window calls `sessions/list`"),
            False,
        ),
        (
            "a media type is not a path",
            citedPaths(citedTops, citedTails, "shelf/README.md", "screenshots arrive as `image/png`"),
            False,
        ),
        (
            "a path not rooted in a tracked directory is not claiming to start at the root",
            citedPaths(citedTops, citedTails, "shelf/README.md", "from `schema/runtime.schema.json`"),
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
