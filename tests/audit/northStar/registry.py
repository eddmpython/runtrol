"""North Star board data, loaded and held against the repository.

`board.toml` is the single source for what is scored and what evidence is claimed for it. This
module loads that file and answers one question: does the claim survive contact with the tree. A
tier above `manual` names gates, and those gates have to exist as files and be reachable from a
runner, or the claim is prose with a number next to it.

Two things are deliberately not reimplemented here. Runner reachability belongs to
`gateCoverage.py`, which already owns that question for the whole audit tree, and the scoring rules
belong to `rubric.py`. A second copy of either would drift from the first without anyone noticing,
which is the failure this whole engine exists to prevent.

The gate prose ("what does this gate assert") lives in `docs/northStarEvidence.md` and nowhere
else. This module checks that the two name sets match in both directions, so a gate cannot be
scored without being described, and a described gate cannot be forgotten by the board.
"""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

import rubric

HERE = Path(__file__).resolve().parent
AUDIT = HERE.parent
ROOT = AUDIT.parents[1]
BOARD = HERE / "board.toml"
EVIDENCE = ROOT / "docs" / "northStarEvidence.md"

# The four README languages, in the order the board prints them.
LANGUAGES = ("ko", "en", "zh", "ja")

STATUSES = ("built", "planned")

# The evidence document is prose, so its gate table is fenced by markers rather than found by
# guessing which table is which. The kind table above it also puts backticked words in its first
# column, and a parser that mistook those for gate names would report a mismatch that is not there.
GATE_SECTION = re.compile(r"<!--\s*gates:begin\s*-->(.*?)<!--\s*gates:end\s*-->", re.DOTALL)
GATE_ROW = re.compile(r"^\|\s*`([A-Za-z][A-Za-z0-9]*)`\s*\|", re.MULTILINE)


@dataclass(frozen=True)
class Gate:
    """A named check that backs an axis."""

    name: str
    kind: str


@dataclass(frozen=True)
class Axis:
    """One scored outcome, with the evidence its score is computed from."""

    key: str
    names: dict[str, str]
    tier: str
    additives: tuple[str, ...]
    caps: tuple[str, ...]
    gates: tuple[Gate, ...]

    def declaredKinds(self) -> frozenset[str]:
        """Kinds this axis plans to have. Sets the ceiling, including for gates not yet built."""
        return frozenset(gate.kind for gate in self.gates)


@dataclass(frozen=True)
class Floor:
    """A binary rule. Never scored, never summed, red blocks a merge."""

    category: str
    name: str
    status: str
    file: str | None


@dataclass(frozen=True)
class Board:
    """Everything `board.toml` declares."""

    axes: tuple[Axis, ...]
    floors: tuple[Floor, ...]

    def total(self) -> float:
        """Sum of the axis scores as they stand."""
        return sum(self.scoreOf(axis).value for axis in self.axes)

    def maxTotal(self) -> float:
        """What the board sums to when every axis is perfect."""
        return rubric.MAXIMUM * len(self.axes)

    def average(self) -> float:
        """Board total per axis. Zero axes would be a shape error caught before this runs."""
        return self.total() / len(self.axes) if self.axes else 0.0

    def scoreOf(self, axis: Axis) -> rubric.Score:
        """The axis score. Ceiling comes from declared kinds, so a planned gate still shows up."""
        return rubric.scoreOf(axis.tier, axis.additives, axis.caps, axis.declaredKinds())

    def gateNames(self) -> set[str]:
        """Every gate name the board mentions, axis gates and floor gates together."""
        return {gate.name for axis in self.axes for gate in axis.gates} | {floor.name for floor in self.floors}


def load() -> Board:
    """Read `board.toml`. Shape errors raise, because a board that will not parse has no score."""
    data = tomllib.loads(BOARD.read_text(encoding="utf-8"))

    axes = tuple(
        Axis(
            key=entry["key"],
            names=dict(entry.get("name", {})),
            tier=entry["tier"],
            additives=tuple(entry.get("additives", [])),
            caps=tuple(entry.get("caps", [])),
            gates=tuple(Gate(name=gate["name"], kind=gate["kind"]) for gate in entry.get("gate", [])),
        )
        for entry in data.get("axis", [])
    )
    floors = tuple(
        Floor(
            category=entry["category"],
            name=entry["name"],
            status=entry["status"],
            file=entry.get("file"),
        )
        for entry in data.get("floor", [])
    )
    return Board(axes=axes, floors=floors)


def problems(board: Board) -> list[str]:
    """Every way the board contradicts the rubric, the tree, or the evidence document."""
    found: list[str] = []
    found += rubricIsSelfConsistent()
    found += axesAreWellFormed(board)
    found += axisClaimsAreBacked(board)
    found += floorsMatchTheTree(board)
    found += gatesAreDocumented(board)
    return found


def rubricIsSelfConsistent() -> list[str]:
    """The additives close the gap between the top tier and a perfect score, exactly.

    If they ever sum to less, 10 becomes unreachable and the board silently tops out. If they sum
    to more, some subset reaches 10 and the last piece of evidence stops mattering.
    """
    gap = rubric.MAXIMUM - rubric.TIERS[rubric.TOP_TIER][0]
    if rubric.additivesTotal() != gap:
        return [
            f"the additives sum to {rubric.additivesTotal()} and the gap from tier "
            f"`{rubric.TOP_TIER}` to {rubric.MAXIMUM} is {gap}. a perfect score has to need all of "
            f"them and nothing more"
        ]
    return []


def axesAreWellFormed(board: Board) -> list[str]:
    """Keys are unique, names cover every language, kinds are known, scores land on the grid."""
    found: list[str] = []

    if not board.axes:
        return ["board.toml declares no axes. the board total would be a division by zero"]

    found += duplicates([axis.key for axis in board.axes], "axis key")
    found += duplicates(sorted(allGateNames(board)), "gate name")

    for axis in board.axes:
        for language in LANGUAGES:
            if not axis.names.get(language):
                found.append(f"axis `{axis.key}` has no `{language}` name, and README_{language} prints one")

        for gate in axis.gates:
            if gate.kind not in rubric.KINDS:
                found.append(f"gate `{gate.name}` has kind `{gate.kind}`, not one of {', '.join(sorted(rubric.KINDS))}")

        score = board.scoreOf(axis)
        if score.value % rubric.STEP != 0:
            found.append(f"axis `{axis.key}` computes to {score.value}, which is not a multiple of {rubric.STEP}")

    return found


def axisClaimsAreBacked(board: Board) -> list[str]:
    """A tier is a claim about gates that run. Check it against gates that exist and are invoked.

    The rubric is asked about *active* kinds (built and reachable) rather than declared ones, so an
    axis cannot claim a tier on the strength of a gate that is still only an idea. Declared kinds
    still set the ceiling, because a plan is worth showing as a plan.
    """
    found: list[str] = []
    haystack = runnerHaystack()
    stems = auditStems()

    for axis in board.axes:
        active: set[str] = set()
        for gate in axis.gates:
            built = gate.name in stems
            reachable = gate.name in haystack
            if built and not reachable:
                found.append(
                    f"axis `{axis.key}` counts gate `{gate.name}`, whose file exists and which no "
                    f"runner invokes. that is the `unregistered` cap, not evidence"
                )
            if built and reachable:
                active.add(gate.kind)

        found += [f"axis `{axis.key}`: {problem}" for problem in rubric.problemsFor(axis.tier, axis.additives, axis.caps, frozenset(active))]

        if axis.tier == "manual" and active:
            found.append(
                f"axis `{axis.key}` claims tier `manual`, which means nobody automated it, and "
                f"{', '.join(sorted(active))} gates are registered and running for it"
            )

    return found


def floorsMatchTheTree(board: Board) -> list[str]:
    """`built` floors run today. `planned` floors do not exist yet, and saying so stays true."""
    found: list[str] = []
    haystack = runnerHaystack()
    stems = auditStems()

    found += duplicates([floor.name for floor in board.floors], "floor name")

    for floor in board.floors:
        if floor.status not in STATUSES:
            found.append(f"floor `{floor.name}` has status `{floor.status}`, not one of {', '.join(STATUSES)}")
            continue

        if floor.status == "built":
            if floor.name not in haystack:
                found.append(
                    f"floor `{floor.name}` is marked built and no runner invokes it. register it in "
                    f"preflight's GATES, in the workflow, or in a git hook, or mark it planned"
                )
            if floor.file and not (AUDIT / floor.file).is_file():
                found.append(f"floor `{floor.name}` names tests/audit/{floor.file}, which does not exist")
            continue

        if floor.file:
            found.append(f"floor `{floor.name}` is marked planned and names a file. a planned gate has no file yet")
        if floor.name in stems or floor.name in haystack:
            found.append(
                f"floor `{floor.name}` is marked planned and now exists in the tree or in a runner. "
                f"flip it to built in the commit that builds it, so the board stops understating itself"
            )

    return found


def gatesAreDocumented(board: Board) -> list[str]:
    """Every gate on the board is described in the evidence document, and nothing extra is."""
    if not EVIDENCE.is_file():
        return [f"{EVIDENCE.relative_to(ROOT)} is missing. it is where a gate says what it asserts"]

    section = GATE_SECTION.search(EVIDENCE.read_text(encoding="utf-8"))
    if section is None:
        return [
            f"{EVIDENCE.relative_to(ROOT)} has no `<!-- gates:begin -->` .. `<!-- gates:end -->` "
            f"section. without it there is no way to tell the gate table from the other tables"
        ]

    documented = set(GATE_ROW.findall(section.group(1)))
    declared = board.gateNames()

    found: list[str] = []
    for name in sorted(declared - documented):
        found.append(f"gate `{name}` is on the board and {EVIDENCE.name} does not say what it asserts")
    for name in sorted(documented - declared):
        found.append(f"gate `{name}` is described in {EVIDENCE.name} and no axis or floor claims it")
    return found


def allGateNames(board: Board) -> list[str]:
    """Gate names as declared, duplicates included, so the caller can find the collisions."""
    return [gate.name for axis in board.axes for gate in axis.gates] + [floor.name for floor in board.floors]


def duplicates(names: list[str], label: str) -> list[str]:
    """Names that appear more than once, as operator-readable sentences."""
    repeated = sorted({name for name in names if names.count(name) > 1})
    return [f"{label} `{name}` is declared more than once. a name means one thing" for name in repeated]


def auditStems() -> set[str]:
    """File stems under `tests/audit/`. A gate exists when a file is named after it."""
    return {path.stem for path in auditFiles()}


def auditFiles() -> list[Path]:
    """Every gate source under `tests/audit/`, caches excluded."""
    return [
        path
        for path in AUDIT.rglob("*")
        if path.is_file() and path.suffix in {".py", ".rs"} and "__pycache__" not in path.parts
    ]


def runnerHaystack() -> str:
    """Every text a gate can be named in and actually be run by, concatenated.

    Coarse on purpose: this is only ever searched for whole gate names, which are specific enough
    that a substring hit means the gate is wired up. Comments are stripped first, because the audit
    manifest mentions gates that do not exist yet and a comment is not an invocation.
    """
    sys.path.insert(0, str(AUDIT))
    import gateCoverage  # noqa: PLC0415  (deliberate: reachability has one owner and this is it)
    import preflight  # noqa: PLC0415

    parts = [gateCoverage.runnerText(), "\n".join(preflight.GATES)]
    manifest = AUDIT / "Cargo.toml"
    if manifest.is_file():
        parts.append(gateCoverage.stripComments(manifest.read_text(encoding="utf-8")))
    return "\n".join(parts)
