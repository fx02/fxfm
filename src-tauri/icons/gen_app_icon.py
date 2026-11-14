#!/usr/bin/env python3
"""Generates app-icon-source.png (1024x1024): a dark-blue rounded square with two white
upright "panels" and, below them, "fxfm" in bold blocky white lowercase letters
(strokes and rounded corners). Standard library only. Then run:
  python3 src-tauri/icons/gen_app_icon.py && cargo tauri icon src-tauri/icons/app-icon-source.png
"""
import os
import struct
import zlib

N, SS = 1024, 3
MARGIN, RADIUS = 100, 180  # macOS-style inset rounded square

# Two panels (upright rectangles) above the text; the group is centred.
PANELS = ((340, 196, 480, 466), (544, 196, 684, 466))
PANEL_R = 24
# Lowercase letters: ascender top, x-height top, baseline, descender bottom.
ASC, XTOP, BASE, DESC = 560, 640, 780, 780
STROKE = 42
WIDTHS, GAP = (112, 136, 112, 196), 28  # f, x, f, m
CORNER = 62  # outer radius of the curves


def inside_round_rect(x, y):
    lo, hi = MARGIN, N - MARGIN
    if not (lo <= x <= hi and lo <= y <= hi):
        return False
    cx = min(max(x, lo + RADIUS), hi - RADIUS)
    cy = min(max(y, lo + RADIUS), hi - RADIUS)
    return (x - cx) ** 2 + (y - cy) ** 2 <= RADIUS ** 2


def in_panels(x, y):
    return any(in_rounded(x, y, *p, PANEL_R) for p in PANELS)


def in_rounded(x, y, x0, y0, x1, y1, r, corners="tl tr bl br"):
    """Inside the box with the named corners rounded by `r`."""
    if not (x0 <= x <= x1 and y0 <= y <= y1) or x1 <= x0 or y1 <= y0:
        return False
    if r <= 0:
        return True
    for name, cx, cy, in_corner in (
        ("tl", x0 + r, y0 + r, x < x0 + r and y < y0 + r),
        ("tr", x1 - r, y0 + r, x > x1 - r and y < y0 + r),
        ("bl", x0 + r, y1 - r, x < x0 + r and y > y1 - r),
        ("br", x1 - r, y1 - r, x > x1 - r and y > y1 - r),
    ):
        if name in corners and in_corner:
            return (x - cx) ** 2 + (y - cy) ** 2 <= r * r
    return True


def in_ring(x, y, x0, y0, x1, y1, corners, r=CORNER):
    """A stroke-wide outline of a box with rounded `corners`."""
    s = STROKE
    return in_rounded(x, y, x0, y0, x1, y1, r, corners) and not in_rounded(
        x, y, x0 + s, y0 + s, x1 - s, y1 - s, max(r - s, 0), corners)


def near_segment(x, y, ax, ay, bx, by, half):
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)))
    px, py = ax + t * dx, ay + t * dy
    return (x - px) ** 2 + (y - py) ** 2 <= half * half


def letter_f(x, y, x0, x1):
    s = STROKE
    stem = x0 + 26
    # Stem with a hook to the right at the top: the top and left edges of a ring.
    hook = in_ring(x, y, stem, ASC, x1, BASE + CORNER, "tl") and (
        y <= ASC + s or x <= stem + s) and y <= BASE
    bar = x0 <= x <= x1 - 18 and XTOP <= y <= XTOP + s
    return hook or bar


def letter_x(x, y, x0, x1):
    """Two crossing strokes, cut flat at the x-height and the baseline."""
    if not (XTOP <= y <= BASE and x0 <= x <= x1):
        return False
    half = STROKE * 0.56
    w = (x1 - x0) - 2 * half
    return (near_segment(x, y, x0 + half, XTOP, x0 + half + w, BASE, half)
            or near_segment(x, y, x1 - half, XTOP, x1 - half - w, BASE, half))


def letter_m(x, y, x0, x1):
    s, xc = STROKE, (x0 + x1) / 2
    if y > BASE:
        return False
    stem = x0 <= x <= x0 + s and XTOP <= y <= BASE
    r = 50
    arch1 = in_ring(x, y, x0, XTOP, xc + s / 2, BASE + r, "tl tr", r)
    arch2 = in_ring(x, y, xc - s / 2, XTOP, x1, BASE + r, "tl tr", r)
    return stem or arch1 or arch2


LETTERS = []
_x = (N - sum(WIDTHS) - GAP * (len(WIDTHS) - 1)) / 2
for _f, _w in zip((letter_f, letter_x, letter_f, letter_m), WIDTHS):
    LETTERS.append((_f, _x, _x + _w))
    _x += _w + GAP


def in_text(x, y):
    if not (ASC <= y <= DESC):
        return False
    return any(x0 <= x <= x1 and f(x, y, x0, x1) for f, x0, x1 in LETTERS)


def color(x, y):
    """Returns (r, g, b, a) float 0..1 for a sub-sample."""
    if not inside_round_rect(x, y):
        return (0, 0, 0, 0)
    t = (y - MARGIN) / (N - 2 * MARGIN)
    base = (0.07 - 0.03 * t, 0.15 - 0.05 * t, 0.42 - 0.12 * t)
    if in_panels(x, y) or in_text(x, y):
        return (1, 1, 1, 1)
    return (*base, 1)


rows = []
for py in range(N):
    row = bytearray([0])
    for px in range(N):
        r = g = b = a = 0.0
        for sy in range(SS):
            for sx in range(SS):
                cr, cg, cb, ca = color(px + (sx + 0.5) / SS, py + (sy + 0.5) / SS)
                r += cr * ca; g += cg * ca; b += cb * ca; a += ca
        if a:
            row += bytes([round(255 * r / a), round(255 * g / a), round(255 * b / a),
                          round(255 * a / (SS * SS))])
        else:
            row += bytes(4)
    rows.append(bytes(row))


def chunk(tag, data):
    c = struct.pack(">I", len(data)) + tag + data
    return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)


png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0))
       + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b""))
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "app-icon-source.png")
open(out, "wb").write(png)
print("wrote", out)
