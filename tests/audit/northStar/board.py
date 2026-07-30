"""Gate: the North Star board computes, and every number on it is backed.

The board used to be a number a person typed into four README files. Nothing checked that the
number followed from anything, that the gate named as its evidence existed, or that the four
languages agreed. This gate makes the score a computation over `board.toml` and refuses the ones
that do not survive it.

What goes red here:

1. A score that the rubric cannot produce. Additives below the top tier, a tier that needs a kind
   of gate the axis does not have, a value off the 0.5 grid.
2. A tier claimed on evidence that is not running. The gate file has to exist and a runner has to
   invoke it, or the claim is prose with a number next to it.
3. A floor gate whose ledger entry stopped being true, in either direction: marked built with
   nobody running it, or marked planned after somebody built it.
4. A gate the board scores and `docs/northStarEvidence.md` never describes, or the reverse.

Usage::

    python -X utf8 tests/audit/northStar/board.py

Exit codes:
    0 every number on the board follows from evidence that exists
    2 something on the board is not backed
"""

from __future__ import annotations

import sys
from collections import defaultdict

import registry
import rubric


def main() -> int:
    """Print the board, or report every claim that does not hold."""
    board = registry.load()
    found = registry.problems(board)
    if found:
        print("[northStarBoard] the board claims things the repository does not support.", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2

    printBoard(board)
    printFloors(board)
    return 0


def printBoard(board: registry.Board) -> None:
    """The scored axes, with the reason each one is where it is."""
    total = number(board.total())
    print(f"[northStarBoard] {total}/{number(board.maxTotal())}, average {board.average():.1f}/10, {len(board.axes)} axes.")
    print()
    print(f"  {'axis':<28}{'score':>6}{'ceiling':>9}  held by")
    for axis in board.axes:
        score = board.scoreOf(axis)
        print(f"  {axis.key:<28}{number(score.value):>6}{number(score.ceiling):>9}  {score.heldBy}")

    stuck = [axis for axis in board.axes if board.scoreOf(axis).ceiling < rubric.MAXIMUM]
    if stuck:
        print()
        print(
            f"  {len(stuck)} of {len(board.axes)} axes cannot reach {number(rubric.MAXIMUM)} on the "
            f"evidence registered for them. a ceiling below maximum is a missing gate kind, not a "
            f"missing run: {', '.join(axis.key for axis in stuck)}"
        )


def printFloors(board: registry.Board) -> None:
    """The binary board. Never summed, so it prints as a count and a list, never as a score."""
    byCategory: dict[str, list[registry.Floor]] = defaultdict(list)
    for floor in board.floors:
        byCategory[floor.category].append(floor)

    built = sum(1 for floor in board.floors if floor.status == "built")
    print()
    print(f"  floors: {built} built, {len(board.floors) - built} planned. binary, never scored.")
    for category in sorted(byCategory):
        entries = ", ".join(f"{floor.name} ({floor.status})" for floor in sorted(byCategory[category], key=lambda f: f.name))
        print(f"    {category:<12}{entries}")


def number(value: float) -> str:
    """A score as the README writes it: `0`, `7.5`, `10`. Trailing zeros are noise on a board."""
    return f"{value:g}"


if __name__ == "__main__":
    sys.exit(main())
