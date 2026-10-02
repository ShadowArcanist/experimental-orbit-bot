#!/usr/bin/env python3
"""Draw Orbit's status and priority icons as 128x128 PNGs for Discord
application emojis.

The shapes copy Orbit's web components (14x14 viewBox):
- TaskStatusIcon.tsx: the category picks the shape, the status colour the
  colour. Marks inside filled circles are transparent cut-outs.
- PriorityIcon.tsx: bars for none/low/medium/high, a red square with "!" for
  urgent, in Orbit's dark-theme colours.

Usage:
  python3 scripts/make_icons.py

Files: `status_<category>.png` (Orbit's default status colours) and
`priority_<level>.png`. Upload them in the Developer Portal (your app →
Emojis) without renaming; the bot finds them by name.
No third-party packages are needed.
"""

import math
import os
import struct
import zlib

SIZE = 128
SAMPLES = 4  # supersampling per axis, for smooth edges
OUT_DIR = os.path.join(os.path.dirname(__file__), "..", "icons")

# Orbit's default status colour per category (DEFAULT_STATUSES on the server).
STATUS_ICONS = [
    ("unstarted", "8b8f98"),
    ("started", "f2c94c"),
    ("completed", "4cb782"),
    ("cancelled", "8b8f98"),
    ("duplicate", "8b8f98"),
]

STROKE = 1.5  # Orbit's stroke width in viewBox units
HALF = STROKE / 2


def segment_distance(px, py, ax, ay, bx, by):
    """Distance from a point to a line segment."""
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def near(px, py, segments):
    """True if the point is on one of the round-capped strokes."""
    return any(segment_distance(px, py, *s) <= HALF for s in segments)


# Marks cut out of the filled circles (Orbit paths, viewBox units).
CHECK = [(4.4, 7.2, 6.2, 9.0), (6.2, 9.0, 9.6, 5.4)]
CROSS = [(4.8, 4.8, 9.2, 9.2), (9.2, 4.8, 4.8, 9.2)]
EQUALS = [(4.5, 5.6, 9.5, 5.6), (4.5, 8.4, 9.5, 8.4)]


def rgb(colour):
    return tuple(int(colour[i : i + 2], 16) for i in (0, 2, 4))


def in_round_rect(x, y, left, top, width, height, radius):
    dx = max(abs(x - (left + width / 2)) - (width / 2 - radius), 0)
    dy = max(abs(y - (top + height / 2)) - (height / 2 - radius), 0)
    return math.hypot(dx, dy) <= radius


# Status icons are drawn smaller than the 14x14 box, so they match the
# priority icons and the text next to them.
STATUS_SCALE = 0.85


def status_shape(category, colour):
    """A function giving the colour at a viewBox point, or None."""
    paint = rgb(colour)

    def shape(x, y):
        # Scale about the centre (7, 7).
        x, y = 7 + (x - 7) / STATUS_SCALE, 7 + (y - 7) / STATUS_SCALE
        return paint if inside(category, x, y) else None

    return shape


# Orbit dark theme: --muted-foreground, --muted, --destructive.
ACTIVE, INACTIVE, URGENT, WHITE = rgb("a1a1a1"), rgb("262626"), rgb("ff6467"), rgb("ffffff")
PRIORITY_LEVELS = {"none": 0, "low": 1, "medium": 2, "high": 3}
# Orbit's bars sit at the bottom of the box (y 2.5-12.5) and most of their area
# is low, so next to text they look dropped. Lift them to look centred.
BAR_LIFT = 1.0
BARS = [(1.5, 8 - BAR_LIFT, 2.6, 4.5), (5.7, 5.5 - BAR_LIFT, 2.6, 7), (9.9, 2.5 - BAR_LIFT, 2.6, 10)]


def priority_shape(priority):
    if priority == "urgent":

        def urgent(x, y):
            if not in_round_rect(x, y, 1, 1, 12, 12, 3):
                return None
            mark = segment_distance(x, y, 7, 3.8, 7, 8) <= 0.8 or math.hypot(x - 7, y - 10.4) <= 0.9
            return WHITE if mark else URGENT

        return urgent

    level = PRIORITY_LEVELS[priority]

    def bars(x, y):
        for n, (left, top, width, height) in enumerate(BARS):
            if in_round_rect(x, y, left, top, width, height, 1):
                return ACTIVE if level >= n + 1 else INACTIVE
        return None

    return bars


def inside(category, x, y):
    """True if the viewBox point (x, y) is painted."""
    r = math.hypot(x - 7, y - 7)
    ring = abs(r - 5.5) <= HALF
    if category == "unstarted":
        return ring
    if category == "started":
        # Ring plus the right half of a disc of radius 3.5.
        return ring or (r <= 3.5 and x >= 7)
    if r > 6:
        return False
    marks = {"completed": CHECK, "cancelled": CROSS, "duplicate": EQUALS}[category]
    return not near(x, y, marks)


def render(shape):
    """Rasterise `shape(x, y) -> (r, g, b) | None` with supersampling."""
    scale = 14 / SIZE
    rows = []
    for py in range(SIZE):
        row = bytearray([0])  # PNG filter type: none
        for px in range(SIZE):
            hits, total = 0, [0, 0, 0]
            for sy in range(SAMPLES):
                for sx in range(SAMPLES):
                    paint = shape((px + (sx + 0.5) / SAMPLES) * scale, (py + (sy + 0.5) / SAMPLES) * scale)
                    if paint:
                        hits += 1
                        total = [t + c for t, c in zip(total, paint)]
            colour = [round(t / hits) for t in total] if hits else [0, 0, 0]
            row += bytes(colour + [round(255 * hits / SAMPLES**2)])
        rows.append(bytes(row))
    return png(SIZE, SIZE, b"".join(rows))


def png(width, height, raw):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)  # 8-bit RGBA
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    files = [(f"status_{c}.png", status_shape(c, colour)) for c, colour in STATUS_ICONS]
    files += [(f"priority_{p}.png", priority_shape(p)) for p in ("urgent", "high", "medium", "low", "none")]
    for name, shape in files:
        with open(os.path.join(OUT_DIR, name), "wb") as f:
            f.write(render(shape))
        print(f"icons/{name}")


if __name__ == "__main__":
    main()
