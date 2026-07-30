"""North Star scoring rubric. Pure rules, no repository access.

A score used to be one of five rungs, and every jump between rungs bundled four or five separate
facts: is the counterpart real, how many providers, how many operating systems, is fault injection
carried, is there a ratchet. Bundled facts cannot be checked one at a time, so the number on the
board was whichever number the person typing it believed.

This module takes the rungs apart into the facts they were made of::

    score = min(tier + additives, tightest cap, MAXIMUM)

Exactly one tier applies and it is a ceiling, not an opening bid. Additives attach only at the top
tier, because breadth measured against a fake counterpart is breadth over nothing. Caps push down
over anything claimed above them.

The four additives sum to exactly `MAXIMUM - TIERS[TOP_TIER]`, so a perfect score is reachable only
by holding all four at once. No arrangement of partial evidence arrives at 10.

What this module refuses to model is as deliberate as what it models. Modularity and clean code get
no fraction of a point, because they are floor rules and a floor rule at 7/10 is a floor rule being
broken. Innovation gets no number, because it is the axis list itself and a second number for it
would count the same thing twice.
"""

from __future__ import annotations

from dataclasses import dataclass

MAXIMUM = 10.0

# Scores move in halves. A finer grid would invite arguing a claim up by a tenth.
STEP = 0.5

# Exactly one tier applies per axis, and it is the ceiling before additives.
TIERS: dict[str, tuple[float, str]] = {
    "none": (0.0, "no gate asserts this axis"),
    "manual": (3.0, "someone ran it by hand. no gate is registered in a runner"),
    "mock": (5.0, "a registered gate runs against a mock CLI, a stub provider, or a simulated phone"),
    "realOneKind": (6.0, "the real counterpart, but only one kind of gate is registered"),
    "realBothKinds": (7.0, "the real counterpart, with a static gate and a live gate both registered"),
}

TOP_TIER = "realBothKinds"

# Additives attach only at TOP_TIER and sum to exactly MAXIMUM - TIERS[TOP_TIER].
ADDITIVES: dict[str, tuple[float, str]] = {
    "multiProvider": (1.0, "the same gate is green against two or more providers"),
    "multiOs": (1.0, "the same gate is green on two or more operating systems, Windows included"),
    "faultInjection": (0.5, "the gate carries fault injection (kill the daemon, cut the network)"),
    "ratchet": (0.5, "a regression ratchet goes red as soon as the measured number gets worse"),
}

# Caps are facts about the last run, not judgements. They push down over tier and additives both.
CAPS: dict[str, tuple[float, str]] = {
    "skipped": (5.0, "the gate skipped in the last run. a skip is not a pass"),
    "flaky": (5.0, "the gate passed on a retry"),
    "unregistered": (0.0, "the gate file exists and no runner invokes it. same as having no gate"),
}

# Gate kinds. `contract` is static analysis and pure contract checks. `smoke` and `bench` put a real
# binary or a real browser on the line. `operator` needs a human holding a device, so it can never
# back a score and is excluded from the total.
STATIC_KINDS = frozenset({"contract"})
LIVE_KINDS = frozenset({"smoke", "bench"})
SCORELESS_KINDS = frozenset({"operator"})
KINDS = STATIC_KINDS | LIVE_KINDS | SCORELESS_KINDS

# An additive is a claim about how a gate ran, so it needs a gate that can run that way. Breadth
# across providers or operating systems means nothing without a live gate to be broad with, fault
# injection needs something to inject into, and a ratchet is what a bench gate is for.
ADDITIVE_REQUIRES: dict[str, frozenset[str]] = {
    "multiProvider": LIVE_KINDS,
    "multiOs": LIVE_KINDS,
    "faultInjection": frozenset({"smoke"}),
    "ratchet": frozenset({"bench"}),
}

# Tiers that assert a gate is running somewhere, as opposed to a person having watched it once.
TIERS_NEEDING_A_GATE = frozenset({"mock", "realOneKind", "realBothKinds"})


@dataclass(frozen=True)
class Score:
    """One axis score, with the reason it is not higher."""

    value: float
    ceiling: float
    heldBy: str


def scoreOf(tier: str, additives: tuple[str, ...], caps: tuple[str, ...], kinds: frozenset[str]) -> Score:
    """Compute an axis score from its declared evidence.

    Callers validate first with `problemsFor`. This function assumes the names are known, so an
    unknown one raises rather than scoring around it.
    """
    base = TIERS[tier][0]
    added = sum(ADDITIVES[name][0] for name in additives) if tier == TOP_TIER else 0.0
    earned = min(base + added, MAXIMUM)

    capName, capValue = tightestCap(caps)
    value = min(earned, capValue) if capName else earned

    return Score(value=value, ceiling=ceilingFor(kinds), heldBy=reasonFor(tier, additives, capName, capValue, earned))


def tightestCap(caps: tuple[str, ...]) -> tuple[str | None, float]:
    """The cap that binds, and its value. `(None, MAXIMUM)` when nothing caps the axis."""
    if not caps:
        return None, MAXIMUM
    name = min(caps, key=lambda cap: CAPS[cap][0])
    return name, CAPS[name][0]


def ceilingFor(kinds: frozenset[str]) -> float:
    """The highest score the currently registered gate kinds could ever reach.

    This is the number that says an axis is stuck. An axis backed only by a smoke gate cannot pass
    6 however green it runs, and seeing that on the board is the point: the fix is a contract gate,
    not more runs of the one that exists.
    """
    scoring = kinds - SCORELESS_KINDS
    if not scoring:
        return TIERS["manual"][0]
    if scoring & STATIC_KINDS and scoring & LIVE_KINDS:
        return MAXIMUM
    return TIERS["realOneKind"][0]


def reasonFor(tier: str, additives: tuple[str, ...], capName: str | None, capValue: float, earned: float) -> str:
    """Why the axis sits where it sits, as one operator-readable clause."""
    if capName and capValue < earned:
        return f"cap `{capName}`: {CAPS[capName][1]}"
    if tier != TOP_TIER:
        return f"tier `{tier}`: {TIERS[tier][1]}"
    missing = [name for name in ADDITIVES if name not in additives]
    if missing:
        return f"missing additives: {', '.join(missing)}"
    return "nothing. this axis is at maximum"


def problemsFor(tier: str, additives: tuple[str, ...], caps: tuple[str, ...], kinds: frozenset[str]) -> list[str]:
    """Every way a declared score contradicts the rubric, as operator-readable sentences."""
    problems: list[str] = []

    if tier not in TIERS:
        return [f"tier `{tier}` is not one of {', '.join(TIERS)}"]

    problems += unknownNames(additives, ADDITIVES, "additive")
    problems += unknownNames(caps, CAPS, "cap")
    if problems:
        return problems

    if additives and tier != TOP_TIER:
        problems.append(
            f"tier `{tier}` claims additives ({', '.join(additives)}). additives attach only at "
            f"`{TOP_TIER}`, because breadth over a counterpart that is not real is breadth over nothing"
        )

    for additive in additives:
        required = ADDITIVE_REQUIRES[additive]
        if not (kinds & required):
            problems.append(
                f"additive `{additive}` needs a gate of kind {' or '.join(sorted(required))}, and "
                f"this axis registers {', '.join(sorted(kinds)) or 'none'}"
            )

    scoring = kinds - SCORELESS_KINDS
    if tier in TIERS_NEEDING_A_GATE and not scoring:
        problems.append(
            f"tier `{tier}` asserts a gate runs, and this axis names no gate that counts toward a "
            f"score (operator gates are excluded)"
        )

    if tier == TOP_TIER:
        if not scoring & STATIC_KINDS:
            problems.append(f"tier `{TOP_TIER}` needs a static gate ({', '.join(sorted(STATIC_KINDS))})")
        if not scoring & LIVE_KINDS:
            problems.append(f"tier `{TOP_TIER}` needs a live gate ({', '.join(sorted(LIVE_KINDS))})")

    return problems


def unknownNames(names: tuple[str, ...], table: dict[str, tuple[float, str]], label: str) -> list[str]:
    """Names that are not in the rubric, and names claimed twice."""
    problems = [f"{label} `{name}` is not one of {', '.join(table)}" for name in names if name not in table]
    seen = {name for name in names if names.count(name) > 1}
    problems += [f"{label} `{name}` is declared more than once" for name in sorted(seen)]
    return problems


def additivesTotal() -> float:
    """What the additives add up to. The board is only self consistent when this closes the gap."""
    return sum(points for points, _ in ADDITIVES.values())
