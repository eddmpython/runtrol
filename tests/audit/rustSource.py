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


# The two shapes the whole-file walk has to recognise on their own, kept beside the single-line pattern so
# the two readings of "this is not code" cannot drift apart.
RAW_OPENER = re.compile(r'r(#*)"')
CHAR_LITERAL = re.compile(r"'(?:\\.|[^'\\])'")
CFG_TEST = re.compile(
    r"#\s*\[\s*cfg\s*\([^]]*(?<![A-Za-z0-9_])test(?![A-Za-z0-9_])[^]]*\)\s*\]"
)


def withoutNoise(line: str) -> str:
    """One line with its strings, character literals and trailing comment removed.

    For a caller holding one line and nothing else. A caller that has the whole file uses
    [`withoutNoiseAcross`], which is the only one of the two that can see a string spanning lines.
    """
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


def withoutNoiseAcross(lines: list[str]) -> list[str]:
    """Every line with its strings, character literals and comments taken out, reading the file as one text.

    [`withoutNoise`] reads one line at a time, and a string that spans lines is invisible to it: the opening
    line has no closing quote to match, so the ordinary-string pattern eats whatever pairs of quotes it can
    find inside and leaves the rest as code. A test fixture written as

        r#"{"rate_limits":{"limits":[
            {"kind":"monthly_overage","percent":42}
        ]}}"#

    lost two of its opening braces that way and kept both of its closing ones, which closed the enclosing
    `#[cfg(test)]` block twenty lines early and reported the tests after it as production panic paths. The
    same miscount runs the other way too: a multi-line string with a spare opening brace stretches a test
    region over the production code below it, and that direction hides real findings instead of inventing
    them.

    So this walks the whole file with the state a string carries between lines. Still not a Rust parser: it
    knows line comments, block comments, ordinary strings, raw strings with any number of hashes, and
    character literals, which is every construct that has put a brace where brace counting could see it.
    """
    cleaned: list[str] = []
    rawHashes: int | None = None
    inString = False
    inBlockComment = False
    for line in lines:
        kept: list[str] = []
        index = 0
        while index < len(line):
            rest = line[index:]
            if rawHashes is not None:
                closing = '"' + "#" * rawHashes
                at = rest.find(closing)
                if at == -1:
                    break
                index += at + len(closing)
                rawHashes = None
                continue
            if inString:
                if rest.startswith("\\\\"):
                    index += 2
                    continue
                if rest.startswith('\\"'):
                    index += 2
                    continue
                if rest.startswith('"'):
                    inString = False
                index += 1
                continue
            if inBlockComment:
                at = rest.find("*/")
                if at == -1:
                    break
                index += at + 2
                inBlockComment = False
                continue
            if rest.startswith("//"):
                break
            if rest.startswith("/*"):
                inBlockComment = True
                index += 2
                continue
            opener = RAW_OPENER.match(rest)
            if opener:
                rawHashes = len(opener.group(1))
                index += opener.end()
                continue
            if rest.startswith('"'):
                inString = True
                index += 1
                continue
            character = CHAR_LITERAL.match(rest)
            if character:
                index += character.end()
                continue
            kept.append(line[index])
            index += 1
        cleaned.append("".join(kept))
    return cleaned


def testRegions(lines: list[str]) -> list[tuple[int, int]]:
    """The (first, last) line index of every `#[cfg(test)]` block.

    The end is found by brace depth over lines the noise has been taken out of, and the noise is taken out
    across the whole file rather than line by line, because a string that spans lines has to be one string.
    When the next item arrives before any opening brace does (the attribute was on a single function), that
    one item is the region.
    """
    cleanedLines = withoutNoiseAcross(lines)
    regions: list[tuple[int, int]] = []
    i = 0
    while i < len(lines):
        if not CFG_TEST.search(lines[i]):
            i += 1
            continue
        depth = 0
        opened = False
        j = i
        while j < len(lines):
            cleaned = cleanedLines[j]
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

    # Platform-qualified tests are still tests. The Windows root identity proof uses this exact shape, and
    # treating it as production reported every deliberate assertion as a panic path.
    lines = "#[cfg(all(test, windows))]\nmod tests {\n    fn only_here() {}\n}\nfn after() {}\n".splitlines()
    regions = testRegions(lines)
    if not inRegions(2, regions):
        problems.append("a platform-qualified cfg(test) module was read as production code")
    if inRegions(4, regions):
        problems.append("code after a platform-qualified test module was read as test code")

    # **The defect this walk exists for.** A raw string spanning lines is invisible one line at a time: the
    # opening line keeps braces the string owns and the closing line keeps its own, so the region closes
    # early and the tests after it are reported as production code.
    source = (
        "#[cfg(test)]\n"
        "mod tests {\n"
        "    #[test]\n"
        "    fn t() {\n"
        '        let frame = r#"{"a":{"b":[\n'
        '            {"c":1}\n'
        '        ]}}"#;\n'
        "        let v = maybe().unwrap();\n"
        "    }\n"
        "}\n"
        "fn production() {}\n"
    )
    lines = source.splitlines()
    regions = testRegions(lines)
    if not inRegions(7, regions):
        problems.append("a raw string spanning lines ended the test region early")
    if inRegions(10, regions):
        problems.append("production code after a multi-line raw string was read as test code")

    # And the same miscount the other way, which hides findings instead of inventing them.
    source = (
        "#[cfg(test)]\n"
        "mod tests {\n"
        '    const S: &str = "a {\n'
        'b";\n'
        "    #[test]\n    fn t() {}\n"
        "}\n"
        "fn production() -> u32 {\n    maybe().unwrap()\n}\n"
    )
    lines = source.splitlines()
    if inRegions(8, testRegions(lines)):
        problems.append("a brace inside a multi-line ordinary string stretched the test region")

    # A block comment is not code either.
    if withoutNoiseAcross(["    /* { */ let x = 1;"])[0].count("{"):
        problems.append("a brace inside a block comment was counted as code")

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
