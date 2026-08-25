"""Renders every brand file in this folder from the geometry table in README.md.

The mark is four stroked arms (a vertical bar, a quarter arc, a horizontal bar) and two of them carry the
accent while the other two carry the ink. The ink follows the surface: graphite on light, white on dark.
Nothing here is traced from an image; a regenerated file is byte-identical for the same inputs, which is
what keeps the rasters honest when the geometry table changes.

Two geometries exist on purpose. `MARK` is the true mark. `HINTED` is the 16 to 32 px variant whose stroke,
radius, and centre lines land on the pixel grid (stroke 2/4/6 px at 16/32/48 px). See README.md.

The rasteriser is dependency-free: stroke coverage comes from a signed distance to the arm centre lines,
the wordmark outline is flattened to polygons and filled by even-odd scanline, and PNG/ICO are written
with the standard library.

Usage:
    python -X utf8 assets/brand/render.py
"""

from __future__ import annotations

import math
import re
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent

CORAL = (245, 101, 101)
GRAPHITE = (11, 13, 15)
WHITE = (255, 255, 255)

CORAL_HEX = "#F56565"
GRAPHITE_HEX = "#0B0D0F"
WHITE_HEX = "#FFFFFF"

# (stroke width, arc radius, bar centre line offset from the box centre, box size)
MARK = (14.0, 20.0, 10.5, 100.0)
HINTED = (12.5, 18.75, 12.5, 100.0)

LOCKUP_WIDTH = 479.07
WORDMARK_X = 133.14
WORDMARK_Y = 14.825
SUPERSAMPLE = 4


# SVG sources -----------------------------------------------------------------------------------------


def arm_paths(geometry: tuple[float, float, float, float]) -> list[str]:
    """The four arm path strings in reading order: top-left, top-right, bottom-left, bottom-right."""
    _, radius, offset, box = geometry
    near = box / 2 - offset
    far = box / 2 + offset
    end = near - radius
    return [
        f"M{near:g} 0V{end:g}A{radius:g} {radius:g} 0 0 1 {end:g} {near:g}H0",
        f"M{far:g} 0V{end:g}A{radius:g} {radius:g} 0 0 0 {box - end:g} {near:g}H{box:g}",
        f"M{near:g} {box:g}V{box - end:g}A{radius:g} {radius:g} 0 0 0 {end:g} {far:g}H0",
        f"M{far:g} {box:g}V{box - end:g}A{radius:g} {radius:g} 0 0 1 {box - end:g} {far:g}H{box:g}",
    ]


def mark_group(geometry: tuple[float, float, float, float], accent: str, ink: str, indent: str = "  ") -> str:
    width = geometry[0]
    top_left, top_right, bottom_left, bottom_right = arm_paths(geometry)
    return "\n".join(
        [
            f'{indent}<g fill="none" stroke-width="{width:g}">',
            f'{indent}  <path class="accent" stroke="{accent}" d="{top_left}"/>',
            f'{indent}  <path class="ink" stroke="{ink}" d="{top_right}"/>',
            f'{indent}  <path class="ink" stroke="{ink}" d="{bottom_left}"/>',
            f'{indent}  <path class="accent" stroke="{accent}" d="{bottom_right}"/>',
            f"{indent}</g>",
        ]
    )


def symbol_svg(accent: str, ink: str, comment: str | None = None) -> str:
    head = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100" role="img" aria-label="runtrol">'
    lines = [head, "  <title>runtrol</title>"]
    if comment:
        lines.append(f"  <!-- {comment} -->")
    lines.append(mark_group(MARK, accent, ink))
    lines.append("</svg>")
    return "\n".join(lines) + "\n"


TILE_RADIUS = 0.2
TILE_MARK = 0.75


def favicon_svg() -> str:
    """Graphite tile with the coral and white mark: the same face on a light or a dark tab strip."""
    size = 100.0
    inset = size * (1 - TILE_MARK) / 2
    return "\n".join(
        [
            '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100" role="img" aria-label="runtrol">',
            "  <title>runtrol</title>",
            "  <!-- hinted for 16 to 32px. the brand geometry is in symbol.svg. the tile keeps the white arms visible on a light tab strip -->",
            f'  <rect width="100" height="100" rx="{size * TILE_RADIUS:g}" fill="{GRAPHITE_HEX}"/>',
            f'  <g transform="translate({inset:g} {inset:g}) scale({TILE_MARK:g})">',
            mark_group(HINTED, CORAL_HEX, WHITE_HEX, indent="    "),
            "  </g>",
            "</svg>",
        ]
    ) + "\n"


def wordmark_path() -> str:
    source = (HERE / "wordmark.svg").read_text(encoding="utf-8")
    match = re.search(r'd="([^"]+)"', source)
    if match is None:
        raise SystemExit("wordmark.svg carries no path")
    return match.group(1)


def lockup_svg(accent: str, ink: str, text: str) -> str:
    return "\n".join(
        [
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {LOCKUP_WIDTH:g} 100" width="{LOCKUP_WIDTH:g}" height="100" role="img" aria-label="runtrol">',
            "  <title>runtrol</title>",
            mark_group(MARK, accent, ink),
            f'  <g transform="translate({WORDMARK_X:g} {WORDMARK_Y:g})">',
            f'    <path fill="{text}" fill-rule="evenodd" d="{wordmark_path()}"/>',
            "  </g>",
            "</svg>",
        ]
    ) + "\n"


# Rasteriser ------------------------------------------------------------------------------------------


class Canvas:
    def __init__(self, width: int, height: int, background: tuple[int, int, int] | None):
        self.width = width
        self.height = height
        if background is None:
            self.pixels = [[0.0, 0.0, 0.0, 0.0] for _ in range(width * height)]
        else:
            self.pixels = [[float(background[0]), float(background[1]), float(background[2]), 1.0] for _ in range(width * height)]

    def blend(self, x: int, y: int, color: tuple[int, int, int], coverage: float) -> None:
        if coverage <= 0.0:
            return
        pixel = self.pixels[y * self.width + x]
        alpha = min(1.0, coverage)
        out_alpha = alpha + pixel[3] * (1.0 - alpha)
        if out_alpha == 0.0:
            return
        for index in range(3):
            pixel[index] = (color[index] * alpha + pixel[index] * pixel[3] * (1.0 - alpha)) / out_alpha
        pixel[3] = out_alpha

    def png(self) -> bytes:
        raw = bytearray()
        for y in range(self.height):
            raw.append(0)
            for x in range(self.width):
                r, g, b, a = self.pixels[y * self.width + x]
                raw.extend((round(r), round(g), round(b), round(a * 255)))

        def chunk(kind: bytes, body: bytes) -> bytes:
            return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)

        header = struct.pack(">IIBBBBB", self.width, self.height, 8, 6, 0, 0, 0)
        return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b"")


def arm_primitives(geometry: tuple[float, float, float, float], scale: float, dx: float, dy: float):
    """Each arm as (segments, arc, color role). Coordinates are already placed on the canvas."""
    width, radius, offset, box = geometry
    near = box / 2 - offset
    far = box / 2 + offset
    end = near - radius
    arms = [
        # ((bar x, bar from y, bar to y), arc centre, arc quadrant signs, (horizontal y, from x, to x), role)
        ((near, 0.0, end), (end, end), (1, 1), (near, 0.0, end), "accent"),
        ((far, 0.0, end), (box - end, end), (-1, 1), (near, box - end, box), "ink"),
        ((near, box, box - end), (end, box - end), (1, -1), (far, 0.0, end), "ink"),
        ((far, box, box - end), (box - end, box - end), (-1, -1), (far, box - end, box), "accent"),
    ]
    placed = []
    for vertical, centre, quadrant, horizontal, role in arms:
        vx, vy0, vy1 = vertical
        hy, hx0, hx1 = horizontal
        cx, cy = centre
        placed.append(
            {
                "segments": [
                    ((vx * scale + dx, vy0 * scale + dy), (vx * scale + dx, vy1 * scale + dy)),
                    ((hx0 * scale + dx, hy * scale + dy), (hx1 * scale + dx, hy * scale + dy)),
                ],
                "arc": ((cx * scale + dx, cy * scale + dy), radius * scale, quadrant),
                "role": role,
            }
        )
    return placed, width * scale / 2


def distance_to_segment(px: float, py: float, a: tuple[float, float], b: tuple[float, float]) -> float:
    ax, ay = a
    bx, by = b
    vx, vy = bx - ax, by - ay
    length = vx * vx + vy * vy
    t = 0.0 if length == 0.0 else max(0.0, min(1.0, ((px - ax) * vx + (py - ay) * vy) / length))
    return math.hypot(px - (ax + t * vx), py - (ay + t * vy))


def distance_to_arc(px: float, py: float, centre: tuple[float, float], radius: float, quadrant: tuple[int, int]) -> float:
    cx, cy = centre
    rx, ry = px - cx, py - cy
    sx, sy = quadrant
    if rx * sx >= 0.0 and ry * sy >= 0.0:
        return abs(math.hypot(rx, ry) - radius)
    end_a = (cx + sx * radius, cy)
    end_b = (cx, cy + sy * radius)
    return min(math.hypot(px - end_a[0], py - end_a[1]), math.hypot(px - end_b[0], py - end_b[1]))


def arm_distance(px: float, py: float, arm) -> float:
    best = min(distance_to_segment(px, py, a, b) for a, b in arm["segments"])
    centre, radius, quadrant = arm["arc"]
    return min(best, distance_to_arc(px, py, centre, radius, quadrant))


def draw_mark(canvas: Canvas, geometry, size: float, dx: float, dy: float, accent, ink) -> None:
    arms, half = arm_primitives(geometry, size / geometry[3], dx, dy)
    colors = {"accent": accent, "ink": ink}
    x0, y0 = max(0, int(dx) - 1), max(0, int(dy) - 1)
    x1, y1 = min(canvas.width, int(dx + size) + 2), min(canvas.height, int(dy + size) + 2)
    step = 1.0 / SUPERSAMPLE
    for y in range(y0, y1):
        for x in range(x0, x1):
            cx, cy = x + 0.5, y + 0.5
            for arm in arms:
                if arm_distance(cx, cy, arm) > half + 1.0:
                    continue
                inside = 0
                for sy in range(SUPERSAMPLE):
                    for sx in range(SUPERSAMPLE):
                        if arm_distance(x + (sx + 0.5) * step, y + (sy + 0.5) * step, arm) <= half:
                            inside += 1
                canvas.blend(x, y, colors[arm["role"]], inside / (SUPERSAMPLE * SUPERSAMPLE))


def flatten_path(d: str, scale: float, dx: float, dy: float) -> list[list[tuple[float, float]]]:
    """Flattens M/L/H/V/C/Z (absolute or relative) into closed polygons."""
    tokens = re.findall(r"[MLHVCZmlhvcz]|-?\d*\.?\d+(?:e-?\d+)?", d)
    polygons: list[list[tuple[float, float]]] = []
    current: list[tuple[float, float]] = []
    x = y = 0.0
    command = ""
    index = 0

    def number() -> float:
        nonlocal index
        value = float(tokens[index])
        index += 1
        return value

    def place(px: float, py: float) -> tuple[float, float]:
        return (px * scale + dx, py * scale + dy)

    while index < len(tokens):
        token = tokens[index]
        if token.isalpha():
            command = token
            index += 1
            if command in "Zz":
                if current:
                    polygons.append(current)
                current = []
                continue
        relative = command.islower()
        kind = command.upper()
        if kind == "M":
            nx, ny = number(), number()
            x, y = (x + nx, y + ny) if relative else (nx, ny)
            if current:
                polygons.append(current)
            current = [place(x, y)]
            command = "l" if relative else "L"
        elif kind == "L":
            nx, ny = number(), number()
            x, y = (x + nx, y + ny) if relative else (nx, ny)
            current.append(place(x, y))
        elif kind == "H":
            nx = number()
            x = x + nx if relative else nx
            current.append(place(x, y))
        elif kind == "V":
            ny = number()
            y = y + ny if relative else ny
            current.append(place(x, y))
        elif kind == "C":
            c1x, c1y, c2x, c2y, ex, ey = (number() for _ in range(6))
            if relative:
                c1x, c1y, c2x, c2y, ex, ey = x + c1x, y + c1y, x + c2x, y + c2y, x + ex, y + ey
            steps = 12
            for step in range(1, steps + 1):
                t = step / steps
                u = 1.0 - t
                bx = u * u * u * x + 3 * u * u * t * c1x + 3 * u * t * t * c2x + t * t * t * ex
                by = u * u * u * y + 3 * u * u * t * c1y + 3 * u * t * t * c2y + t * t * t * ey
                current.append(place(bx, by))
            x, y = ex, ey
        else:
            raise SystemExit(f"unsupported path command in wordmark: {command}")
    if current:
        polygons.append(current)
    return polygons


def fill_even_odd(canvas: Canvas, polygons: list[list[tuple[float, float]]], color: tuple[int, int, int]) -> None:
    edges = []
    for polygon in polygons:
        for index in range(len(polygon)):
            a = polygon[index]
            b = polygon[(index + 1) % len(polygon)]
            if a[1] != b[1]:
                edges.append((a, b))
    if not edges:
        return
    min_y = max(0, int(min(min(a[1], b[1]) for a, b in edges)))
    max_y = min(canvas.height, int(max(max(a[1], b[1]) for a, b in edges)) + 1)
    samples = SUPERSAMPLE
    coverage_rows: dict[int, dict[int, int]] = {}
    for y in range(min_y, max_y):
        row: dict[int, int] = {}
        for sy in range(samples):
            sample_y = y + (sy + 0.5) / samples
            crossings = []
            for (ax, ay), (bx, by) in edges:
                if (ay <= sample_y < by) or (by <= sample_y < ay):
                    crossings.append(ax + (sample_y - ay) * (bx - ax) / (by - ay))
            crossings.sort()
            for start, stop in zip(crossings[0::2], crossings[1::2]):
                first = int(start * samples)
                last = int(stop * samples)
                for sub in range(first, last + 1):
                    left = max(start, sub / samples)
                    right = min(stop, (sub + 1) / samples)
                    if right > left:
                        x = sub // samples
                        if 0 <= x < canvas.width:
                            row[x] = row.get(x, 0) + int(round((right - left) * samples))
        coverage_rows[y] = row
    for y, row in coverage_rows.items():
        for x, count in row.items():
            canvas.blend(x, y, color, min(1.0, count / (samples * samples)))


def draw_lockup(canvas: Canvas, size: float, dx: float, dy: float, accent, ink, text) -> None:
    draw_mark(canvas, MARK, size, dx, dy, accent, ink)
    scale = size / 100.0
    fill_even_odd(canvas, flatten_path(wordmark_path(), scale, dx + WORDMARK_X * scale, dy + WORDMARK_Y * scale), text)


def draw_tile(size: int, geometry, mark: float, radius: float) -> "Canvas":
    """Graphite tile with rounded corners, transparent outside the corners, mark centred at `mark` of the size."""
    canvas = Canvas(size, size, None)
    corner = size * radius
    step = 1.0 / SUPERSAMPLE
    for y in range(size):
        for x in range(size):
            inside = 0
            for sy in range(SUPERSAMPLE):
                for sx in range(SUPERSAMPLE):
                    px, py = x + (sx + 0.5) * step, y + (sy + 0.5) * step
                    qx = max(corner - px, px - (size - corner), 0.0)
                    qy = max(corner - py, py - (size - corner), 0.0)
                    if math.hypot(qx, qy) <= corner:
                        inside += 1
            canvas.blend(x, y, GRAPHITE, inside / (SUPERSAMPLE * SUPERSAMPLE))
    inset = size * (1 - mark) / 2
    draw_mark(canvas, geometry, size * mark, inset, inset, CORAL, WHITE)
    return canvas


def ico(pngs: list[tuple[int, bytes]]) -> bytes:
    header = struct.pack("<HHH", 0, 1, len(pngs))
    offset = 6 + 16 * len(pngs)
    entries = b""
    bodies = b""
    for size, body in pngs:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(body), offset)
        bodies += body
        offset += len(body)
    return header + entries + bodies


# Outputs ---------------------------------------------------------------------------------------------


def write(name: str, data: bytes | str) -> None:
    path = HERE / name
    if isinstance(data, str):
        path.write_text(data, encoding="utf-8", newline="\n")
    else:
        path.write_bytes(data)
    print(f"  {name}")


def main() -> None:
    print("brand vectors")
    write("symbol.svg", symbol_svg("currentColor", "currentColor", "monochrome. CSS color decides. use this in icon fonts and the editor"))
    write("symbol-light.svg", symbol_svg(CORAL_HEX, GRAPHITE_HEX, "two-tone for light surfaces"))
    write("symbol-dark.svg", symbol_svg(CORAL_HEX, WHITE_HEX, "two-tone for dark surfaces"))
    write("favicon.svg", favicon_svg())
    write("lockup.svg", lockup_svg(CORAL_HEX, "currentColor", "currentColor"))
    write("lockup-light.svg", lockup_svg(CORAL_HEX, GRAPHITE_HEX, GRAPHITE_HEX))
    write("lockup-dark.svg", lockup_svg(CORAL_HEX, WHITE_HEX, WHITE_HEX))

    print("brand rasters")
    hinted = {size: draw_tile(size, HINTED, TILE_MARK, TILE_RADIUS).png() for size in (16, 32, 48)}
    write("icon-16.png", hinted[16])
    write("icon-32.png", hinted[32])
    write("favicon.ico", ico([(16, hinted[16]), (32, hinted[32]), (48, hinted[48])]))

    for size in (192, 512):
        write(f"icon-{size}.png", draw_tile(size, MARK, 0.64, 0.0).png())

    canvas = Canvas(180, 180, GRAPHITE)
    draw_mark(canvas, MARK, 180 * 0.64, 180 * 0.18, 180 * 0.18, CORAL, WHITE)
    write("apple-touch-icon.png", canvas.png())

    for name, background, ink in (("social-card.png", WHITE, GRAPHITE), ("social-card-dark.png", GRAPHITE, WHITE)):
        canvas = Canvas(1200, 630, background)
        size = 120.0
        width = LOCKUP_WIDTH * size / 100.0
        draw_lockup(canvas, size, (1200 - width) / 2, (630 - size) / 2, CORAL, ink, ink)
        write(name, canvas.png())


if __name__ == "__main__":
    main()
