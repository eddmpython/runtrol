"""Gate: the four README files print the board the engine computed, not a memory of it.

runtrol ships its README in Korean, English, Chinese, and Japanese. A translated file rots in a
particular way: the Korean original gets a new score and the other three keep yesterday's, so three
out of four readers are told something false by a document that looks maintained.

This gate holds all four to `board.toml`. Every axis, in order, with the computed score. The total
and the average as prose. And the rubric itself, so a tier cannot be worth 7 points in one language
and 8 in another.

Usage::

    python -X utf8 tests/audit/northStar/readmeParity.py

Exit codes:
    0 all four languages print the same board
    2 at least one language disagrees with the engine
"""

from __future__ import annotations

import re
import sys

import registry
import rubric

# Language to the file that prints it. Korean is the original; the other three are translations of
# it, and none of them is allowed to be a different board.
READMES: dict[str, str] = {
    "ko": "README.md",
    "en": "README_EN.md",
    "zh": "README_ZH.md",
    "ja": "README_JA.md",
}

# A North Star row: the axis name, then its score out of ten. The `/10` is what separates these
# rows from the rubric rows below them, which carry a bare number.
AXIS_ROW = re.compile(r"^\|\s*(.+?)\s*\|\s*(\d+(?:\.\d+)?)/10\s*\|", re.MULTILINE)

# A rubric row: a backticked rule name, then the points it is worth. `+1` and `1` are the same
# claim written two ways, so the sign is optional.
RUBRIC_ROW = re.compile(r"^\|\s*`([A-Za-z]+)`\s*\|\s*\+?(\d+(?:\.\d+)?)\s*\|", re.MULTILINE)


def main() -> int:
    """Compare every README against the computed board."""
    board = registry.load()
    structural = registry.problems(board)
    if structural:
        print("[readmeParity] the board itself does not hold. run northStar/board.py first.", file=sys.stderr)
        return 2

    found: list[str] = []
    for language, filename in READMES.items():
        found += checkOne(board, language, filename)

    if found:
        print("[readmeParity] a README prints a board the engine did not compute.", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2

    print(f"[readmeParity] OK. {len(READMES)} languages agree on {len(board.axes)} axes, the total, and the rubric.")
    return 0


def checkOne(board: registry.Board, language: str, filename: str) -> list[str]:
    """Everything one README gets wrong about the board."""
    path = registry.ROOT / filename
    if not path.is_file():
        return [f"{filename} is missing, and {language} is one of the four languages the board ships in"]

    text = path.read_text(encoding="utf-8")
    return axisTableMatches(board, language, filename, text) + totalsMatch(board, filename, text) + rubricMatches(filename, text)


def axisTableMatches(board: registry.Board, language: str, filename: str, text: str) -> list[str]:
    """The axis rows, in order, with the computed scores."""
    printed = AXIS_ROW.findall(text)
    expected = [(axis.names[language], number(board.scoreOf(axis).value)) for axis in board.axes]

    if len(printed) != len(expected):
        return [
            f"{filename} prints {len(printed)} axis rows and the board has {len(expected)}. "
            f"an axis was added or removed in one language only"
        ]

    problems: list[str] = []
    for index, ((printedName, printedScore), (expectedName, expectedScore)) in enumerate(zip(printed, expected), start=1):
        if printedName != expectedName:
            problems.append(f"{filename} row {index} names `{printedName}` and board.toml has `{expectedName}`")
        elif printedScore != expectedScore:
            problems.append(
                f"{filename} scores `{printedName}` at {printedScore}/10 and the engine computes "
                f"{expectedScore}/10 from the evidence declared for it"
            )
    return problems


def totalsMatch(board: registry.Board, filename: str, text: str) -> list[str]:
    """The total and the average, as they appear in prose above the table."""
    total = f"{number(board.total())}/{number(board.maxTotal())}"
    average = f"{board.average():.1f}/10"

    problems: list[str] = []
    if total not in text:
        problems.append(f"{filename} does not say `{total}`, which is what the axis scores sum to")
    if average not in text:
        problems.append(f"{filename} does not say `{average}`, which is that total over {len(board.axes)} axes")
    return problems


def rubricMatches(filename: str, text: str) -> list[str]:
    """Every tier and additive, worth the same in every language.

    A README may describe the rubric at whatever length reads well in its language. What it may not
    do is put a different number next to a rule than the engine scores with.
    """
    expected = {
        **{name: points for name, (points, _) in rubric.TIERS.items()},
        **{name: points for name, (points, _) in rubric.ADDITIVES.items()},
    }
    printed = {name: float(value) for name, value in RUBRIC_ROW.findall(text)}

    problems: list[str] = []
    for name, points in expected.items():
        if name not in printed:
            problems.append(f"{filename} does not list rubric rule `{name}`, so a reader cannot check a score against it")
        elif printed[name] != points:
            problems.append(f"{filename} puts {number(printed[name])} next to `{name}` and the engine uses {number(points)}")
    for name in sorted(set(printed) - set(expected)):
        problems.append(f"{filename} lists a rubric rule `{name}` that the engine does not know")
    return problems


def number(value: float) -> str:
    """A score as the README writes it: `0`, `7.5`, `10`."""
    return f"{value:g}"


def selftest() -> int:
    """Prove that score, total, and rubric drift each make the parity gate red."""
    board = registry.load()
    rows = [
        f"| {axis.names['ko']} | {number(board.scoreOf(axis).value)}/10 | today | target |"
        for axis in board.axes
    ]
    wrongRows = list(rows)
    wrongRows[0] = re.sub(r"\d+(?:\.\d+)?/10", "9.5/10", wrongRows[0], count=1)

    injected = {
        "axis score": axisTableMatches(board, "ko", "fixture.md", "\n".join(wrongRows)),
        "totals": totalsMatch(board, "fixture.md", "no totals here"),
        "rubric": rubricMatches("fixture.md", "no rubric here"),
    }
    missed = [name for name, problems in injected.items() if not problems]
    if missed:
        print(f"[readmeParity --selftest] injected drift was missed: {', '.join(missed)}", file=sys.stderr)
        return 2
    print("[readmeParity --selftest] OK. score, total, and rubric drift all make the gate red.")
    return 0


if __name__ == "__main__":
    sys.exit(selftest() if "--selftest" in sys.argv[1:] else main())
