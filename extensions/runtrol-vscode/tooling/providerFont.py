# /// script
# requires-python = ">=3.10"
# dependencies = ["fonttools==4.64.0"]
# ///
"""Build terminal icon fonts from the existing SVG projection and shared glyph assignments."""

import json
from pathlib import Path
import sys
import xml.etree.ElementTree as etree

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path
from fontTools.ttLib import TTFont


def emptyGlyph():
    return TTGlyphPen(None).glyph()


def readOutline(source):
    root = etree.parse(source).getroot()
    left, top, width, height = map(float, root.attrib["viewBox"].split())
    if width <= 0 or height <= 0:
        raise ValueError(f"provider icon has an invalid viewBox: {source.name}")
    # Preserve the SVG viewBox, including its padding, with the font's upward-pointing Y axis.
    scale = 1024 / max(width, height)
    offsetX = (1024 - width * scale) / 2 - left * scale
    offsetY = (1024 - height * scale) / 2 + (top + height) * scale
    pen = TTGlyphPen(None)
    transformed = TransformPen(
        Cu2QuPen(pen, max_err=0.5, reverse_direction=False),
        (scale, 0, 0, -scale, offsetX, offsetY),
    )
    paths = 0
    for node in root.iter():
        tag = node.tag.rsplit("}", 1)[-1]
        if tag not in {"svg", "g", "style", "path"}:
            raise ValueError(f"provider icon needs a filled path projection: {source.name} ({tag})")
        if any(key in node.attrib for key in ("transform", "style", "opacity", "clip-path")):
            raise ValueError(f"provider icon needs plain filled outlines: {source.name}")
        if node.attrib.get("fill-rule", "nonzero") != "nonzero":
            raise ValueError(f"provider icon needs nonzero fill winding: {source.name}")
        if node.attrib.get("stroke", "none") != "none" or node.attrib.get("fill") == "none":
            raise ValueError(f"provider icon needs filled outlines: {source.name}")
        if tag == "path":
            parse_path(node.attrib["d"], transformed)
            paths += 1
    if paths == 0:
        raise ValueError(f"provider icon has no filled paths: {source.name}")
    return pen.glyph()


def buildFont(contract):
    directory = Path(contract["directory"])
    glyphs = {".notdef": emptyGlyph()}
    for name in contract["names"]:
        glyphs[f"{name}.outline"] = readOutline(directory / f"{name}.svg")
    colorLayers = {}
    characters = {}
    for assignment in contract["glyphs"]:
        glyphName = f"glyph{assignment['codepoint']:x}"
        glyphs[glyphName] = emptyGlyph()
        colorLayers[glyphName] = [(f"{assignment['name']}.outline", assignment["paletteIndex"])]
        characters[assignment["codepoint"]] = glyphName
    builder = FontBuilder(1024, isTTF=True)
    builder.setupGlyphOrder(list(glyphs))
    builder.setupCharacterMap(characters)
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({name: (1024, getattr(glyph, "xMin", 0)) for name, glyph in glyphs.items()})
    builder.setupHorizontalHeader(ascent=1024, descent=0)
    builder.setupNameTable({
        "familyName": "Runtrol Provider Icons",
        "styleName": "Regular",
        "uniqueFontIdentifier": "RuntrolProviderIcons",
        "fullName": "Runtrol Provider Icons",
        "psName": "RuntrolProviderIcons",
    })
    builder.setupOS2(sTypoAscender=1024, sTypoDescender=0, usWinAscent=1024, usWinDescent=0)
    builder.setupPost()
    builder.setupCOLR(colorLayers, version=0)
    builder.setupCPAL([[
        tuple(int(color[offset:offset + 2], 16) / 255 for offset in (1, 3, 5)) + (1.0,)
        for color in contract["accents"]
    ]])
    # Generated assets must not change with wall-clock time or acquire a runtime dependency.
    builder.font["head"].created = 3030000000
    builder.font["head"].modified = 3030000000
    builder.font.recalcTimestamp = False
    builder.font.flavor = "woff"
    output = directory / "providerIcons.woff"
    builder.save(output)
    verifyFont(output, contract, characters)


def verifyFont(output, contract, characters):
    with TTFont(output, recalcTimestamp=False) as loaded:
        if loaded["COLR"].version != 0 or loaded.getBestCmap() != characters:
            raise ValueError("generated provider font does not match its shared character assignments")
        for assignment in contract["glyphs"]:
            layers = loaded["COLR"].ColorLayers[characters[assignment["codepoint"]]]
            expectedOutline = f"{assignment['name']}.outline"
            if len(layers) != 1 or layers[0].name != expectedOutline or layers[0].colorID != assignment["paletteIndex"]:
                raise ValueError("generated provider font has a mismatched outline or accent")
            if len(loaded["glyf"][expectedOutline].coordinates) == 0:
                raise ValueError("generated provider font has an empty outline")
        palettes = loaded["CPAL"].palettes
        if len(palettes) != 1 or [color.hex().lower() for color in palettes[0]] != [
            color + "ff" for color in contract["accents"]
        ]:
            raise ValueError("generated provider font changed the canonical project palette")


if __name__ == "__main__":
    buildFont(json.load(sys.stdin))
