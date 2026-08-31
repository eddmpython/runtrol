"""Gate: nothing below the drivers knows a provider by name.

Adding a CLI must not touch anything else. `dependencyDirection` already holds one half of that as a dependency
edge: the kernel cannot see the drivers, so it cannot call into one. This holds the other half, which no dependency
edge can see, because a name is not an edge.

A provider name appearing outside the drivers is one of three things, and all three are the same defect:

1. **A branch.** `if provider == "codex"` in the kernel, which is the rule this gate is named for.
2. **A reach.** The assembly layer importing one driver's constant, so every provider silently inherits that
   driver's answer. Found in the tree the first time this gate ran: the daemon took how long to wait for a
   graceful close from the claude driver, so a second provider would have been given claude's patience.
3. **A default.** A path, a flag or a model spelled in a crate that has no business knowing them.

The names are discovered, never listed here. They come from the manifests the drivers ship and from the driver
module names, so a provider added tomorrow is covered by this gate the moment its manifest exists. A hardcoded
list would go stale in exactly the direction that matters: the new provider would be the one nobody checked.

Test code is exempt. A fixture naming a provider is sample data, not a branch, and a gate that refused it would
push every test toward inventing names that no longer resemble what the code will meet.

One line can be exempted with a `provider-name:` comment carrying the reason, on the line or just above it. The
case it exists for is real and narrow: the scope wall denies a few credential directories that happen to belong to
coding CLIs, and it denies them whether or not this build can drive those CLIs, because the agent that reads them
is not the one they belong to. Every exemption is printed on a green run, so the list cannot grow unseen.

Usage::

    python -X utf8 tests/audit/providerIsolation.py

Exit codes:
    0 no crate below the drivers names a provider
    2 something does
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

import rustSource

ROOT = Path(__file__).resolve().parents[2]
CRATES = ROOT / "crates"
MANIFESTS = CRATES / "runtrol-drivers" / "manifests"

# The crate whose whole job is knowing these names. Everything else is checked.
THE_DRIVERS = "runtrol-drivers"

# Crates that must never name a provider, each with what it would mean if one did.
#
# The daemon is on this list even though it is where providers are assembled. Assembling means reading a table and
# handing the entries on; it never means naming one. That is the difference between a build that can gain a
# provider by shipping a manifest and one that gains it by being edited.
MUST_NOT_NAME: dict[str, str] = {
    "runtrol-provider": "the vocabulary is what a third-party provider author writes against. a name here is "
    "one provider's shape becoming everybody's contract",
    "runtrol-security": "the scope wall decides what a request may do, never who it is for",
    "runtrol-childproc": "starting a process is the same operation whichever program it is",
    "runtrol-store": "what runtrol keeps is identifiers and cursors, and neither has a vendor",
    "runtrol-ipc": "the wire carries a provider's name as a value. spelling one here would make the format know "
    "which providers exist",
    "runtrol-core": "the kernel defines the traits and never the implementations. this is the rule the whole "
    "layering exists for",
    "runtrol-daemon": "assembly reads the table and hands entries on. naming one entry is how a build starts "
    "gaining providers by being edited instead of by shipping a manifest",
    "runtrol-cli": "what somebody types is passed through as a value. the command surface has no opinion about "
    "which providers exist",
    "runtrol": "the binary picks a personality from argv. it names the table of drivers, never a driver",
}

# A name only counts as named when it stands on its own. Without this a crate could not mention `runtrol-drivers`
# in a comment, and worse, a name that happens to be a substring of an ordinary word would be reported forever.
WORD = "[A-Za-z0-9_]"

# One line saying why it is allowed to name a provider. The same shape as the `ok:` marker `checkSilentFail` uses,
# so there is one convention for "this rule does not apply here, and here is why" rather than two.
MARKER = re.compile(r"//\s*provider-name:\s*(?P<why>\S.*)$")

# How far above a line the marker may sit. A reason needing several lines is written as several comment lines, and
# only the first has to carry the marker.
LOOKBACK = 6


def providerNames() -> set[str]:
    """Every name a provider is known by, discovered rather than listed.

    The manifests give the identifier and the kind. The driver module names give what the code calls them, which
    can differ from both and is what an import would spell.
    """
    names: set[str] = set()

    for manifest in sorted(MANIFESTS.glob("*.toml")):
        declared = tomllib.loads(manifest.read_text(encoding="utf-8"))
        for key in ("id", "kind"):
            value = declared.get(key)
            if isinstance(value, str) and value:
                names.add(value)
        names.add(manifest.stem)

    drivers = CRATES / THE_DRIVERS / "src"
    for entry in sorted(drivers.iterdir()) if drivers.is_dir() else []:
        # A driver is a module directory beside the crate's own files. `framing` and the rest are techniques the
        # drivers share, and they are not provider names, so anything without a manifest is left out.
        if entry.is_dir() and entry.name in names:
            names.add(entry.name)

    return names


def exemptedAt(lines: list[str], index: int) -> str | None:
    """The reason this line may name a provider, when one is written on it or just above it."""
    start = max(0, index - LOOKBACK)
    for cursor in range(index, start - 1, -1):
        found = MARKER.search(lines[cursor])
        if found:
            return found.group("why").strip()
    return None


def offences(crate: str, path: Path, names: set[str], exemptions: list[str] | None = None) -> list[str]:
    """Every place this file names a provider outside of its tests."""
    source = path.read_text(encoding="utf-8")
    lines = source.splitlines()
    regions = rustSource.testRegions(lines)
    rel = path.relative_to(ROOT).as_posix()

    patterns = {name: re.compile(rf"(?<!{WORD}){re.escape(name)}(?!{WORD})", re.IGNORECASE) for name in names}

    found: list[str] = []
    for index, line in enumerate(lines):
        if rustSource.inRegions(index, regions):
            continue
        # A comment may discuss a provider. Explaining why the code does not know a name is not knowing it, and a
        # gate that forbade the explanation would delete the reasoning it exists to protect.
        #
        # The strings stay. A hardcoded name lives in one, so a gate that removed them would report every file as
        # clean, which is what the first version of this did until its own selftest said so.
        cleaned = rustSource.withoutComments(line)
        if not cleaned.strip():
            continue
        for name, pattern in patterns.items():
            if not pattern.search(cleaned):
                continue
            why = exemptedAt(lines, index)
            if why is None:
                found.append(f"  - {rel}:{index + 1} names `{name}`: {MUST_NOT_NAME[crate]}")
            elif exemptions is not None:
                exemptions.append(f"  - {rel}:{index + 1} `{name}`: {why}")
    return found


def main() -> int:
    names = providerNames()
    if not names:
        print("[providerIsolation] no provider manifests found, so this gate would pass on nothing")
        return 2

    problems: list[str] = []
    exemptions: list[str] = []
    checked = 0

    for crate in sorted(MUST_NOT_NAME):
        source = CRATES / crate / "src"
        if not source.is_dir():
            problems.append(f"  - crate `{crate}` is on the list and has no source directory")
            continue
        for path in sorted(source.rglob("*.rs")):
            if "tests" in path.relative_to(source).parts:
                continue
            checked += 1
            problems.extend(offences(crate, path, names, exemptions))

    known = ", ".join(sorted(names))
    if problems:
        print(f"[providerIsolation] a provider is named outside {THE_DRIVERS}:")
        print("\n".join(problems))
        print(f"[providerIsolation] names checked for: {known}")
        return 2

    print(f"[providerIsolation] OK. {checked} files, none names any of: {known}")
    if exemptions:
        # Printed on a green run, so the list cannot grow without somebody seeing it.
        print(f"[providerIsolation] {len(exemptions)} exempted, each with its reason:")
        print("\n".join(exemptions))
    return 0


def selftest() -> int:
    """Check that this gate can still fail.

    A gate is only worth having if it goes red on the thing it is for, so the two shapes that have actually
    happened are injected here rather than trusted.
    """
    names = {"claude", "codex"}
    problems: list[str] = []

    branching = (
        "pub fn grace(provider: &str) -> u64 {\n"
        '    if provider == "codex" {\n'
        "        return 1;\n"
        "    }\n"
        "    0\n"
        "}\n"
    )
    reaching = "use runtrol_drivers::claude::DEFAULT_GRACE_MS;\n"
    exempt = (
        "#[cfg(test)]\n"
        "mod tests {\n"
        '    const SAMPLE: &str = "claude";\n'
        "}\n"
        "pub fn after() {}\n"
    )

    marked = (
        "// provider-name: a credential directory, not a provider this crate knows about.\n"
        '    under_home: ".claude",\n'
    )
    farAway = (
        "// provider-name: a reason about something else\n"
        + "\n" * (LOOKBACK + 2)
        + '    let name = "claude";\n'
    )

    scratch = ROOT / ".tmp"
    scratch.mkdir(exist_ok=True)
    probe = scratch / "providerIsolationSelftest.rs"
    try:
        for what, source, shouldFail in (
            ("a branch on a provider", branching, True),
            ("a reach into one driver", reaching, True),
            ("a name used as test data", exempt, False),
            ("a line carrying its own reason", marked, False),
            ("a reason too far above to be about the line", farAway, True),
        ):
            probe.write_text(source, encoding="utf-8", newline="\n")
            found = offences("runtrol-core", probe, names)
            if shouldFail and not found:
                problems.append(f"{what} was not caught")
            if not shouldFail and found:
                problems.append(f"{what} was reported: {found}")
    finally:
        probe.unlink(missing_ok=True)

    for one in problems:
        print(f"[providerIsolation] selftest: {one}")
    return 2 if problems else 0


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        raise SystemExit(max(selftest(), rustSource.selftest()))
    raise SystemExit(main())
