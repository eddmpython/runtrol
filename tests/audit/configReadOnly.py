"""Gate: product code cannot write provider configuration files directly.

The rule is enforced as a closed list of production files that may mutate the filesystem. A path
cannot be proven safe from a string search after it has been assembled at runtime, so the gate
controls the capability instead: storage, runtrol home creation, the probe cache, the provider
update safety journal, local endpoint cleanup, isolated worktree ownership, and Unix process guards are the only existing disk
writers. Provider drivers have no direct filesystem mutation API available in their source.

Official provider commands are still allowed. They are child processes and own their configuration;
runtrol does not recreate their file format or write around them.

Usage::

    python -X utf8 tests/audit/configReadOnly.py
    python -X utf8 tests/audit/configReadOnly.py --selftest

Exit codes:
    0 every production disk mutation is in the reviewed allowlist
    2 an unreviewed mutation exists, or the selftest cannot detect one
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

import rustSource

ROOT = Path(__file__).resolve().parents[2]
CRATES = ROOT / "crates"

# These files own runtrol data or its local endpoint. The list is intentionally exact and printed on
# every successful run, so adding a new disk writer is a visible architecture decision.
MAY_MUTATE_DISK = {
    "crates/runtrol-childproc/src/contain/registry.rs": "owns runtrol's durable Unix process guards",
    "crates/runtrol-childproc/src/contain/tracked.rs": "owns runtrol's Unix bootstrap handoff files",
    "crates/runtrol-childproc/src/shims.rs": "owns only Runtrol-marked provider command shims in "
    "the installation-selected shim directory and refuses every foreign entry",
    "crates/runtrol-core/src/home/mod.rs": "creates runtrol's own state directories",
    "crates/runtrol-core/src/probe/cache.rs": "atomically replaces runtrol's disposable probe cache",
    "crates/runtrol-ipc/src/transport.rs": "creates and removes runtrol's local Unix socket",
    "crates/runtrol-vault/src/lib.rs": "atomically creates runtrol's OS-protected machine identity file",
    "crates/runtrol-agent-tools/src/credentials.rs": "owns project-scoped protected Runtime identities "
    "and their public grants inside runtrol home",
    "crates/runtrol-childproc/src/bin/containedParent.rs": "the containment proof binary rehearses "
    "the update rename dance against its own disposable copy, never a provider file",
    "crates/runtrol-daemon/src/crash.rs": "the detached daemon's panic hook appends to its own "
    "bounded crash file inside the runtrol home",
    "crates/runtrol-daemon/src/native_deletions.rs": "appends one bounded line per native conversation this Runtime removed, inside the runtrol home, so the next deletion is answerable",
    "crates/runtrol-daemon/src/generations.rs": "daemons own the locator of their own home: each writes "
    "only its own entry, under the home's advisory lock, by atomic rename",
    "crates/runtrol-daemon/src/isolated_workspace.rs": "owns the bounded ordinary-chat worktree registry "
    "inside runtrol home and asks Git to create or remove only exact Core-owned linked worktrees",
    "crates/runtrol-daemon/src/provider_update.rs": "owns the bounded provider update version floor "
    "and rollback pin journal inside the runtrol home; provider package changes still go through npm",
    "crates/runtrol-daemon/src/runtime_locator.rs": "atomically owns the public Runtime instance and "
    "locator records inside the runtrol home",
    "crates/runtrol-runtime-protocol/src/bin/export_schema.rs": "writes only the generated checked "
    "public Runtime schema from its Rust DTO source of truth",
    "crates/runtrol-security/src/root_identity.rs": "opens an approved directory read-only for its "
    "kernel-issued Windows file identity and never writes provider or workspace data",
    "crates/runtrol-childproc/src/held.rs": "opens a provider's own lock file read-only with no sharing, "
    "to ask whether a live process holds it, and never writes; the open lasts microseconds and the file is "
    "not read (reviewed 2026-08-29)",
    "crates/runtrol-childproc/src/console_mirror.rs": "opens the console devices (CONIN$, CONOUT$) of a "
    "console it mirrors; those are kernel console handles, not files, and no provider or workspace data is "
    "written (reviewed 2026-08-29)",
    # The one reviewed write into a provider's own store, and the only entry here that is not runtrol data.
    # Claude Code publishes no delete command (measured 2.1.241), so the driver removes only the complete
    # measured artifact set of the operator-selected native identity and verifies absence. It creates no
    # recovery copy and remains reachable only under the machine-granted delete scope.
    "crates/runtrol-drivers/src/claude/deletion.rs": "permanently removes one operator-chosen Claude "
    "conversation, its sidecar and history rows, then verifies that the native identity is absent",
}
MAY_MUTATE_PREFIXES = {
    "crates/runtrol-store/src/": "the database crate owns runtrol's session pointer store",
}

# Rust permits a large test module to live in its own source file. These exact files are reached only
# through a `#[cfg(test)]` module declaration in their parent; keeping the list exact prevents an
# ordinary production module under a generically named directory from escaping the capability scan.
TEST_ONLY_SOURCE = {
    "crates/runtrol-daemon/src/runtime_serve/tests/dispatch.rs",
    "crates/runtrol-daemon/src/runtime_serve/tests/official_attach.rs",
}

MUTATION = [
    re.compile(
        r"\b(?:(?:std|tokio)::)?fs::(?:write|rename|copy|remove_file|remove_dir(?:_all)?|"
        r"create_dir(?:_all)?|set_permissions)\s*\("
    ),
    re.compile(r"\b(?:std::fs::)?File::create\s*\("),
    re.compile(r"\b(?:std::fs::)?OpenOptions\b"),
    re.compile(r"\buse\s+(?:std|tokio)::fs::\{?[^;]*(?:write|rename|copy|remove_|create_|set_permissions)"),
]


def allowed(relative: str) -> bool:
    """Whether this production file is a reviewed owner of disk mutation."""
    if relative in MAY_MUTATE_DISK:
        return True
    return any(relative.startswith(prefix) for prefix in MAY_MUTATE_PREFIXES)


def mutationsIn(text: str, relative: str) -> list[str]:
    """Unreviewed mutation calls in one Rust source string."""
    if relative in TEST_ONLY_SOURCE or allowed(relative):
        return []

    lines = text.splitlines()
    tests = rustSource.testRegions(lines)
    found: list[str] = []
    for index, line in enumerate(lines):
        if rustSource.inRegions(index, tests):
            continue
        code = rustSource.withoutComments(line)
        if any(pattern.search(code) for pattern in MUTATION):
            found.append(f"{relative}:{index + 1}: {code.strip()}")
    return found


def failures() -> list[str]:
    """Every unreviewed production disk mutation in the workspace."""
    found: list[str] = []
    for path in sorted(CRATES.rglob("*.rs")):
        relative = path.relative_to(ROOT).as_posix()
        if "/src/" not in relative:
            continue
        found.extend(mutationsIn(path.read_text(encoding="utf-8"), relative))
    return found


def selftest() -> int:
    """Prove mutation detection and that reviewed ownership remains file-exact."""
    example = "crates/example/src/lib.rs"
    registry = "crates/runtrol-childproc/src/contain/registry.rs"
    tracked = "crates/runtrol-childproc/src/contain/tracked.rs"
    adjacent = "crates/runtrol-childproc/src/contain/bootstrap.rs"
    testModule = "crates/runtrol-daemon/src/runtime_serve/tests/dispatch.rs"
    adjacentTestDirectory = "crates/runtrol-daemon/src/other/tests/dispatch.rs"
    fixtures = [
        ("direct write", example, 'fn change() { std::fs::write("settings", b"x"); }', 1),
        ("aliased write", example, 'fn change() { fs::rename("a", "b"); }', 1),
        ("open options", example, "fn change() { let _file = OpenOptions::new(); }", 1),
        ("read only", example, 'fn inspect() { drop(std::fs::read("settings")); }', 0),
        (
            "test fixture",
            example,
            '#[cfg(test)]\nmod tests {\n  fn fixture() { std::fs::write("settings", b"x"); }\n}',
            0,
        ),
        ("reviewed guard registry", registry, 'fn change() { std::fs::write("guard", b"x"); }', 0),
        ("reviewed guard handoff", tracked, 'fn change() { std::fs::write("plan", b"x"); }', 0),
        ("exact test-only source", testModule, 'fn fixture() { std::fs::write("home", b"x"); }', 0),
        (
            "unlisted file in a tests directory",
            adjacentTestDirectory,
            'fn change() { std::fs::write("settings", b"x"); }',
            1,
        ),
        (
            "unreviewed adjacent containment file",
            adjacent,
            'fn change() { std::fs::write("guard", b"x"); }',
            1,
        ),
    ]
    problems: list[str] = []
    for name, relative, source, expected in fixtures:
        actual = len(mutationsIn(source, relative))
        if actual != expected:
            problems.append(f"{name}: expected {expected} finding(s), got {actual}")

    if problems:
        print("[configReadOnly --selftest] the gate cannot detect what it claims to.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print(f"[configReadOnly --selftest] OK. {len(fixtures)} injected cases behaved as expected.")
    return 0


def main(argv: list[str]) -> int:
    """Run the selftest or scan the workspace."""
    if "--selftest" in argv:
        return selftest()

    found = failures()
    if found:
        print("[configReadOnly] unreviewed production disk mutation:", file=sys.stderr)
        for one in found:
            print(f"  - {one}", file=sys.stderr)
        print(
            "Provider configuration must be changed through the provider's official command. "
            "If this is runtrol-owned data, review and add its exact owner to MAY_MUTATE_DISK.",
            file=sys.stderr,
        )
        return 2

    owners = len(MAY_MUTATE_DISK) + len(MAY_MUTATE_PREFIXES)
    print(f"[configReadOnly] OK. disk mutation remains confined to {owners} reviewed owners.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
