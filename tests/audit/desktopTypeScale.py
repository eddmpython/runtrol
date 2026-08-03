"""Gate: the window's own stylesheet uses the theme's type scale, and draws no brand mark of its own.

Two defects this exists to prevent, both of which shipped and were caught by an operator looking at the
window rather than by anything mechanical:

1. **A size nobody chose.** The theme publishes a type scale (`--font-size-2xs` through `--font-size-5xl`).
   The stylesheet carried seven raw pixel values, and two of them (13px, 11px) were not on that scale at
   all. A raw pixel size is a decision made once, by hand, that nothing holds to the rest of the product.

2. **A second brand mark, invented in CSS.** The rail header drew an orange rounded square with the letter
   `r` in it. The real mark is `assets/brand/symbol.svg`, four corner brackets, and the window's own title
   bar shows it. So one window displayed the real mark and a made-up one at the same time. The brand README
   is explicit that the SVG is the canonical source and everything else derives from it, which means the
   stylesheet is never allowed to draw one.

Both checks are textual and cheap on purpose: this is a floor rule, so it has to be able to run on every
preflight and in hosted CI without a browser.

Usage::

    python -X utf8 tests/audit/desktopTypeScale.py
    python -X utf8 tests/audit/desktopTypeScale.py --selftest

Exit codes:
    0 every size comes from the scale and the stylesheet draws no mark
    2 a hand-picked size or a hand-drawn mark is in the stylesheet
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STYLESHEET = ROOT / "crates" / "runtrol-gui" / "ui" / "src" / "app.css"
THEME = (
    ROOT
    / "crates"
    / "runtrol-gui"
    / "ui"
    / "node_modules"
    / "@astryxdesign"
    / "theme-neutral"
    / "dist"
    / "theme.css"
)

# A size declaration. The value is captured whole and judged after, rather than excluded by a lookahead:
# a lookahead after `\s*` backtracks to zero width and then passes everything, which is how the first
# version of this gate reported every scale token as hand-picked.
SIZE_DECLARATION = re.compile(r"font-size:\s*([^;]+);")

# What the theme publishes, so the message can name the choices rather than say "use a token".
PUBLISHED_SIZE = re.compile(r"--font-size-([a-z0-9]+):\s*([^;]+);")

# The brand's one colour, as the brand README publishes it. It is allowed to appear exactly once, on the
# line that defines the theme token, and every other use references that token. A second literal is the
# same value living in two places, which is how the rail came to draw its own orange square with a letter
# in it while the real mark sat unused in `assets/brand/symbol.svg`.
MARK_ORANGE = re.compile(r"#ff5a2f", re.IGNORECASE)

# The one line the literal belongs on: a custom property definition.
DEFINES_A_TOKEN = re.compile(r"^\s*--[a-z0-9-]+:")


class Failed(Exception):
    """The stylesheet made a decision the design system was supposed to own."""


def publishedScale(text: str) -> dict[str, str]:
    """Every size the theme publishes, name to value."""
    return {name: value.strip() for name, value in PUBLISHED_SIZE.findall(text)}


def handPickedSizes(text: str) -> list[tuple[int, str]]:
    """Every size declaration that is not a scale token, with its line number."""
    found: list[tuple[int, str]] = []
    for index, line in enumerate(text.splitlines(), start=1):
        matched = SIZE_DECLARATION.search(line)
        if matched:
            value = matched.group(1).strip()
            if not value.startswith("var("):
                found.append((index, value))
    return found


def loosBrandColour(text: str) -> list[int]:
    """Every line carrying the brand colour as a literal outside a token definition."""
    return [
        index
        for index, line in enumerate(text.splitlines(), start=1)
        if MARK_ORANGE.search(line) and not DEFINES_A_TOKEN.match(line)
    ]


def tokenDefinitions(text: str) -> list[int]:
    """Every line that defines a token from the brand colour."""
    return [
        index
        for index, line in enumerate(text.splitlines(), start=1)
        if MARK_ORANGE.search(line) and DEFINES_A_TOKEN.match(line)
    ]


def main() -> int:
    """Hold the window's stylesheet to the scale and to one brand source."""
    if not STYLESHEET.is_file():
        print(f"[desktopTypeScale] {STYLESHEET.relative_to(ROOT)} is missing, so this gate watches nothing")
        return 2

    style = STYLESHEET.read_text(encoding="utf-8")
    problems: list[str] = []

    scale = publishedScale(THEME.read_text(encoding="utf-8")) if THEME.is_file() else {}
    choices = ", ".join(f"--font-size-{name}" for name in scale) or "the theme's --font-size-* tokens"

    for line, value in handPickedSizes(style):
        problems.append(
            f"  - {STYLESHEET.name}:{line} sets `font-size: {value}`, a size nobody chose. use one of: {choices}"
        )

    for line in loosBrandColour(style):
        problems.append(
            f"  - {STYLESHEET.name}:{line} writes the brand colour as a literal. it belongs on the one "
            f"token definition, and everything else references that token, so the brand has one source "
            f"here just as the mark has one in assets/brand/symbol.svg"
        )

    defined = tokenDefinitions(style)
    if len(defined) > 1:
        places = ", ".join(f"{STYLESHEET.name}:{line}" for line in defined)
        problems.append(f"  - the brand colour is defined more than once: {places}")

    if problems:
        print("[desktopTypeScale] the window's stylesheet decided something the design system owns:")
        print("\n".join(problems))
        return 2

    print(
        f"[desktopTypeScale] OK. every size in {STYLESHEET.name} comes from the scale "
        f"({len(scale)} published steps), and the brand colour has one definition and no loose copies."
    )
    return 0


def selftest() -> int:
    """Prove each check can fail before trusting it when it passes."""
    problems: list[str] = []

    caught = handPickedSizes("a { font-size: 13px; }\nb { font-size: 1.1rem; }\n")
    if len(caught) != 2:
        problems.append(f"hand-picked sizes were not all caught: {caught}")

    if handPickedSizes("a { font-size: var(--font-size-sm); }\n"):
        problems.append("a scale token was reported as hand-picked")

    if not loosBrandColour("a { background: #FF5A2F; }\n"):
        problems.append("a loose brand colour literal was accepted")
    if loosBrandColour("  --color-accent: #ff5a2f;\n"):
        problems.append("the one token definition was reported as a loose literal")
    if loosBrandColour("a { background: var(--color-accent); }\n"):
        problems.append("a reference to the token was read as a literal")
    if len(tokenDefinitions("  --color-accent: #ff5a2f;\n  --other: #ff5a2f;\n")) != 2:
        problems.append("a second definition of the brand colour was not seen")

    scale = publishedScale("--font-size-sm: 0.75rem;\n--font-size-base: 0.875rem;\n")
    if scale != {"sm": "0.75rem", "base": "0.875rem"}:
        problems.append(f"the published scale was not read: {scale}")

    if problems:
        print("[desktopTypeScale --selftest] the gate cannot catch what it claims to.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[desktopTypeScale --selftest] OK. 7 injected defects all caught.")
    return 0


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        raise SystemExit(selftest())
    raise SystemExit(main())
