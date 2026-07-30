"""Gate: a gate that lives in this repository is a gate that runs.

There are four ways for a check to exist and never execute, and every one of them has already
happened here:

1. A `tests/audit/*.rs` file that nobody registered as a `[[test]]`. The audit package sets
   `autotests = false`, so an unregistered file is not compiled, not run, and not reported. It looks
   like coverage and is not.
2. A `tests/audit/*.py` script that no runner invokes. It sits in the tree, gets updated by whoever
   edits nearby code, and answers no question.
3. A check that one runner has and the other does not. Local green then means something different
   from CI green, and whichever direction the gap points, somebody is trusting a result that was
   never produced.
4. A gate in a subdirectory, invisible to a scan that only looks at the top level. This one caught
   this very file: `tests/audit/northStar/` arrived with four unregistered scripts in it and the
   inventory below reported OK, because the glob it used stopped at the first directory level.

This file closes all four. It is deliberately blunt: anything uncovered must be named in a ledger
below with a reason, so skipping a check becomes a visible decision instead of an oversight.

Usage::

    python -X utf8 tests/audit/gateCoverage.py

Exit codes:
    0 every gate in the tree is reachable from a runner
    2 something is uncovered
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
AUDIT = ROOT / "tests" / "audit"
WORKFLOW = ROOT / ".github" / "workflows" / "gates.yml"
GITHOOKS = ROOT / ".githooks"

# Python files under `tests/audit/` that are not gates, keyed by path relative to that directory.
# Each needs a reason, because the default answer to "why does this script never run" is that
# somebody forgot to wire it up.
NOT_A_GATE: dict[str, str] = {
    "preflight.py": "the runner itself. it invokes the gates rather than being one",
    "northStar/rubric.py": "the scoring rules as pure data and arithmetic. northStar/board.py is "
    "the gate that applies them",
    "northStar/registry.py": "loads board.toml and holds it against the tree. imported by both "
    "northStar gates rather than run on its own",
}

# Preflight gates that intentionally have no counterpart step in the workflow.
LOCAL_ONLY: dict[str, str] = {
    "audit": "the workflow's `cargo test --all` already compiles and runs the audit package, "
    "because it is a workspace member. this entry exists so a local run can target it alone",
    "clippyCrossCfg": "the workflow runs this gate too, and it has to name a different target: each "
    "runner cross-compiles toward the platform it is not, so the local command targets linux and the "
    "linux runner targets windows. the gate is present on both sides; only the argument differs, which "
    "is why an exact-command comparison cannot see it",
}

# Workflow invocations that intentionally have no counterpart preflight gate.
CI_ONLY: dict[str, str] = {
    "tests/audit/checkNoAiMarkers.py": "enforced locally by .githooks/commit-msg on every commit, "
    "which is stricter than a preflight run. the workflow needs its own invocation because a "
    "clone has no hooks installed until `git config core.hooksPath` is set",
    "cargo binstall": "installs the tool the following step runs. not a check",
}


def failures() -> list[str]:
    """Everything uncovered, as operator-readable sentences."""
    found: list[str] = []
    found += rustGatesAreRegistered()
    found += pythonGatesHaveARunner()
    found += preflightGatesRunInCi()
    found += ciStepsRunInPreflight()
    return found


def rustGatesAreRegistered() -> list[str]:
    """Every `tests/audit/*.rs` is a declared test target.

    `autotests = false` is deliberate (it keeps camelCase file names from tripping `non_snake_case`),
    and its cost is that cargo no longer discovers test files on its own.
    """
    manifest = AUDIT / "Cargo.toml"
    if not manifest.is_file():
        return [f"{manifest.relative_to(ROOT)} is missing. the audit package defines the gates"]

    text = manifest.read_text(encoding="utf-8")
    if "autotests = false" not in text:
        # If autotests is ever turned back on, cargo discovers files itself and this check is moot.
        # Saying so is better than silently passing for a reason nobody can see.
        return []

    declared = set(re.findall(r'^\s*path\s*=\s*"([^"]+)"', text, re.MULTILINE))
    problems: list[str] = []
    for relative in auditSources(".rs"):
        if relative not in declared:
            stem = relative.rsplit("/", maxsplit=1)[-1].removesuffix(".rs")
            problems.append(
                f"tests/audit/{relative} is not a `[[test]]` in tests/audit/Cargo.toml. "
                f"with `autotests = false` it is never compiled and never run. add:\n"
                f'    [[test]]\n    name = "{stem}"\n    path = "{relative}"'
            )
    return problems


def pythonGatesHaveARunner() -> list[str]:
    """Every `tests/audit/**/*.py` is invoked by preflight, by the workflow, or by a git hook."""
    haystack = runnerText()
    problems: list[str] = []
    for relative in auditSources(".py"):
        if relative in NOT_A_GATE:
            continue
        if relative not in haystack:
            problems.append(
                f"tests/audit/{relative} is never invoked. add it to preflight's GATES, to "
                f".github/workflows/gates.yml, or to NOT_A_GATE in this file with a reason"
            )
    return problems


def auditSources(suffix: str) -> list[str]:
    """Gate sources under `tests/audit/`, as paths relative to it, caches excluded.

    Recursive on purpose. A top level glob was what let `tests/audit/northStar/` arrive with four
    scripts in it and this inventory still print OK.
    """
    return sorted(
        path.relative_to(AUDIT).as_posix()
        for path in AUDIT.rglob(f"*{suffix}")
        if path.is_file() and "__pycache__" not in path.parts
    )


def preflightGatesRunInCi() -> list[str]:
    """Every preflight gate has a counterpart in the workflow.

    A gate that only runs locally makes local green stronger than CI green, which means a pull
    request can merge without it.
    """
    workflow = workflowText()
    if workflow is None:
        return [f"{WORKFLOW.relative_to(ROOT)} is missing. it is the CI side of every gate"]

    problems: list[str] = []
    for name, signature in preflightSignatures().items():
        if name in LOCAL_ONLY:
            continue
        if signature not in workflow:
            problems.append(
                f"preflight gate '{name}' runs `{signature}` locally, and the workflow has no such "
                f"step. add it to .github/workflows/gates.yml, or to LOCAL_ONLY in this file with a "
                f"reason"
            )
    return problems


def ciStepsRunInPreflight() -> list[str]:
    """Every check the workflow runs is also reachable from preflight.

    This is the direction that bites hardest: CI catching something a local run cannot means the
    author learns about it after pushing, and the fix arrives as a second commit.
    """
    workflow = workflowText()
    if workflow is None:
        return []

    signatures = list(preflightSignatures().values())
    problems: list[str] = []

    invoked = set(re.findall(r"tests/audit/(?:[A-Za-z0-9_]+/)*[A-Za-z0-9_]+\.py", workflow))
    invoked |= {f"cargo {sub}" for sub in re.findall(r"\bcargo ([a-z][a-z-]*)", workflow)}

    for invocation in sorted(invoked):
        if invocation in CI_ONLY:
            continue
        if not any(signature.startswith(invocation) for signature in signatures):
            problems.append(
                f"the workflow runs `{invocation}` and preflight does not. add it to preflight's "
                f"GATES, or to CI_ONLY in this file with a reason"
            )
    return problems


def preflightSignatures() -> dict[str, str]:
    """Gate name to the command it runs, with the interpreter prefix removed.

    Imported from preflight rather than copied, so the two cannot drift. Preflight's `GATES` is the
    single source for what runs locally, and this file compares it against the other runners without
    becoming a second copy of it.
    """
    sys.path.insert(0, str(AUDIT))
    import preflight  # noqa: PLC0415  (deliberate: the import is the point of this function)

    signatures: dict[str, str] = {}
    for name, (_description, command) in preflight.GATES.items():
        tokens = [token for token in command if token not in {sys.executable, "-X", "utf8"}]
        signatures[name] = " ".join(tokens)
    return signatures


def workflowText() -> str | None:
    """The workflow file with comments removed, or None when it does not exist.

    Comments have to go before anything is read as an invocation. This file's own first run failed
    because the workflow's header comment mentions `preflight.py`, and a gate that counts a mention
    as an execution is measuring prose.
    """
    if not WORKFLOW.is_file():
        return None
    return stripComments(WORKFLOW.read_text(encoding="utf-8"))


def stripComments(text: str) -> str:
    """Drop `#` comments, keeping only what a shell or YAML parser would act on.

    Both YAML and shell use `#`, and both need whitespace (or line start) in front of it, so shell
    parameter expansions like `${name#prefix}` survive. This is lexically coarse and only ever used
    for substring searches, where over-deleting cannot invent an invocation that is not there.
    """
    kept: list[str] = []
    for line in text.splitlines():
        if line.lstrip().startswith("#"):
            continue
        kept.append(re.sub(r"\s#.*$", "", line))
    return "\n".join(kept)


def runnerText() -> str:
    """Every place a gate can actually be invoked from, concatenated.

    Preflight contributes its resolved command list rather than its source text, because it builds
    paths from an `f`-string and the literal `tests/audit/noScriptsDir.py` never appears in the file.
    Searching the source would have passed for the wrong reason.
    """
    parts: list[str] = list(preflightSignatures().values())
    workflow = workflowText()
    if workflow is not None:
        parts.append(workflow)
    if GITHOOKS.is_dir():
        for hook in sorted(GITHOOKS.iterdir()):
            if hook.is_file():
                parts.append(stripComments(hook.read_text(encoding="utf-8")))
    return "\n".join(parts)


def main() -> int:
    """Report every uncovered gate, or confirm the inventory."""
    problems = failures()
    if problems:
        print("[gateCoverage] gates exist in this tree that no runner executes.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2

    rust = len(auditSources(".rs"))
    python = len([relative for relative in auditSources(".py") if relative not in NOT_A_GATE])
    print(f"[gateCoverage] OK. {rust} rust and {python} python gates all reachable from a runner.")
    if LOCAL_ONLY or CI_ONLY:
        print(f"  declared exceptions: {len(LOCAL_ONLY)} local-only, {len(CI_ONLY)} ci-only.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
