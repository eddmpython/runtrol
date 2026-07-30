"""Reading Rust source the way a gate has to, in one place.

Every gate that scans source needs the same two things: text with the parts that only look like code taken out, and
the line ranges that belong to tests. Both are subtle, and the second one has already been wrong here once (a raw
string containing a brace cut a test region 168 lines short, and the code after it was reported as production
code). One copy of that logic is the whole point of this file: a second copy would be a second thing to get wrong,
and the one that was not fixed would be the one nobody was looking at.

This is not a Rust parser and does not try to be. It is the smallest amount of lexing that makes brace counting
reliable, which is all any gate here has needed.

Not a gate itself. `tests/audit/gateCoverage.py` knows that, and the ledger there says why.
"""

from __future__ import annotations

import re

# What has to come out before braces are counted.
#
# **Raw strings go first.** A pattern that only knew ordinary strings would take `"{"` out of `r#"{"id":1}"#` and
# count the leftover brace as code, which is exactly how a test region came to end in the wrong place.
#
# Character literals go too, because `'{'` is a brace one character long. The pattern matches exactly three
# characters, so a lifetime like `&'static str` is left alone.
NOISE = re.compile(
    r"r(#+)\".*?\"\1"  # a raw string, its hashes matched by backreference
    r'|r"[^"]*"'  # a raw string with no hashes
    r'|"(?:\\.|[^"\\])*"'  # an ordinary string
    r"|'(?:\\.|[^'\\])'"  # a character literal
    r"|//.*$"  # a line comment
)


def withoutNoise(line: str) -> str:
    """One line with its strings, character literals and trailing comment removed."""
    return NOISE.sub("", line)


def withoutComments(line: str) -> str:
    """One line with its trailing comment removed and its strings left alone.

    The opposite trade from [`withoutNoise`], and both are needed. Counting braces wants the strings gone; looking
    for a value that should not be there wants them kept, because a hardcoded name lives in a string literal and a
    gate that removed strings would report every file as clean. That was the first version of the provider gate,
    and its own selftest is what said so.

    A `//` inside a string does not start a comment, so this walks the line rather than searching it.
    """
    inString = False
    escaped = False
    quote = ""
    for index, character in enumerate(line):
        if inString:
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == quote:
                inString = False
            continue
        if character in ('"', "'"):
            inString = True
            quote = character
            continue
        if character == "/" and line[index : index + 2] == "//":
            return line[:index]
    return line


def testRegions(lines: list[str]) -> list[tuple[int, int]]:
    """The (first, last) line index of every `#[cfg(test)]` block.

    The end is found by brace depth over lines the noise has been taken out of. When the next item arrives before
    any opening brace does (the attribute was on a single function), that one item is the region.
    """
    regions: list[tuple[int, int]] = []
    i = 0
    while i < len(lines):
        if "#[cfg(test)]" not in lines[i]:
            i += 1
            continue
        depth = 0
        opened = False
        j = i
        while j < len(lines):
            cleaned = withoutNoise(lines[j])
            depth += cleaned.count("{") - cleaned.count("}")
            if cleaned.count("{"):
                opened = True
            if opened and depth <= 0:
                break
            j += 1
        regions.append((i, min(j, len(lines) - 1)))
        i = j + 1
    return regions


def inRegions(index: int, regions: list[tuple[int, int]]) -> bool:
    """Whether a line index falls inside any of the regions."""
    return any(start <= index <= end for start, end in regions)


def selftest() -> int:
    """Check that this can still tell test code from the code around it.

    Run by whichever gates import this, so a change here is caught by the gates that depend on it rather than by
    the next person to read a report that was quietly wrong.
    """
    problems: list[str] = []

    # The defect this file exists for. The brace inside the raw string must not be counted.
    source = (
        "#[cfg(test)]\n"
        "mod tests {\n"
        '    const FRAME: &str = r#"{"id":1}"#;\n'
        "    fn one() {}\n"
        "}\n"
        "fn production() {}\n"
    )
    lines = source.splitlines()
    regions = testRegions(lines)
    if not inRegions(3, regions):
        problems.append("a brace inside a raw string ended the test region early")
    if inRegions(5, regions):
        problems.append("production code after a test module was read as test code")

    # An attribute on one item, with no module around it.
    lines = "#[cfg(test)]\nfn only_this() {\n    let x = 1;\n}\nfn after() {}\n".splitlines()
    regions = testRegions(lines)
    if not inRegions(2, regions):
        problems.append("a cfg(test) attribute on one function did not cover its body")
    if inRegions(4, regions):
        problems.append("the item after a cfg(test) function was read as test code")

    # A character literal is one brace long.
    if withoutNoise("    let brace = '{';").count("{"):
        problems.append("a character literal was counted as a brace")

    # And a lifetime is not a character literal.
    if "static" not in withoutNoise("    fn f(s: &'static str) {}"):
        problems.append("a lifetime was mistaken for a character literal")

    # Taking comments off has to leave the strings, or a gate looking for a hardcoded value sees nothing.
    kept = withoutComments('    if provider == "codex" { // a branch')
    if "codex" not in kept:
        problems.append("a string literal was removed by the comment stripper")
    if "a branch" in kept:
        problems.append("a trailing comment survived the comment stripper")

    # A double slash inside a string does not start a comment.
    if "example.com" not in withoutComments('    let url = "https://example.com";'):
        problems.append("a double slash inside a string was read as a comment")

    for one in problems:
        print(f"[rustSource] {one}")
    return 2 if problems else 0


if __name__ == "__main__":
    raise SystemExit(selftest())
