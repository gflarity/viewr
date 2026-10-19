#!/usr/bin/env python3
"""Generate packaging/icon-1024.png (the viewr app icon) with no image libraries.

A dark rounded tile holding a photo glyph: a white frame, an amber sun and two
mountain ridges. Drawn scanline-by-scanline; shapes below return the x-spans
they cover on a given row, and layers paint back-to-front.
"""
import struct
import zlib

S = 1024


def rrect_spans(y, x0, y0, x1, y1, r):
    """Spans of a rounded rectangle on row y."""
    if y < y0 or y >= y1:
        return []
    if y0 + r <= y < y1 - r:
        return [(x0, x1)]
    cy = y0 + r if y < y0 + r else y1 - r - 1
    dy = y - cy
    half = (r * r - dy * dy) ** 0.5
    return [(int(x0 + r - half), int(x1 - r + half))]


def circle_spans(y, cx, cy, r):
    dy = y - cy
    if abs(dy) >= r:
        return []
    half = (r * r - dy * dy) ** 0.5
    return [(int(cx - half), int(cx + half))]


def poly_spans(y, pts):
    """Spans of a polygon on row y (even-odd edge intersections)."""
    xs = []
    n = len(pts)
    for i in range(n):
        x1, y1 = pts[i]
        x2, y2 = pts[(i + 1) % n]
        if (y1 <= y < y2) or (y2 <= y < y1):
            xs.append(x1 + (y - y1) * (x2 - x1) / (y2 - y1))
    xs.sort()
    return [(int(xs[i]), int(xs[i + 1])) for i in range(0, len(xs) - 1, 2)]


def subtract(spans, holes):
    out = []
    for a, b in spans:
        cur = [(a, b)]
        for ha, hb in holes:
            nxt = []
            for s, e in cur:
                if hb <= s or ha >= e:
                    nxt.append((s, e))
                else:
                    if s < ha:
                        nxt.append((s, ha))
                    if hb < e:
                        nxt.append((hb, e))
            cur = nxt
        out.extend(cur)
    return out


def main():
    px = bytearray(S * S * 4)  # transparent

    def paint(spans, y, rgb):
        base = y * S * 4
        for a, b in spans:
            for x in range(max(a, 0), min(b, S)):
                i = base + x * 4
                px[i:i + 4] = bytes((*rgb, 255))

    tile = (92, 92, S - 92, S - 92, 190)
    for y in range(S):
        # tile: near-black with a gentle top-to-bottom falloff
        t = y / S
        bg = (int(34 - 14 * t), int(36 - 15 * t), int(44 - 18 * t))
        paint(rrect_spans(y, *tile), y, bg)
        # photo frame: outer rounded rect minus the inner one
        outer = rrect_spans(y, 218, 218, S - 218, S - 218, 64)
        inner = rrect_spans(y, 262, 262, S - 262, S - 262, 40)
        paint(subtract(outer, inner), y, (232, 234, 238))
        # sun
        paint(circle_spans(y, 668, 392, 74), y, (245, 185, 66))
        # mountain ridges, clipped to the frame's inside
        clip = rrect_spans(y, 262, 262, S - 262, S - 262, 40)
        back = poly_spans(y, [(262, 762), (470, 470), (640, 762)])
        front = poly_spans(y, [(430, 762), (640, 540), (762, 700), (762, 762)])
        for spans, rgb in ((back, (138, 148, 160)), (front, (222, 226, 232))):
            clipped = []
            for a, b in spans:
                for ca, cb in clip:
                    lo, hi = max(a, ca), min(b, cb)
                    if lo < hi:
                        clipped.append((lo, hi))
            paint(clipped, y, rgb)

    raw = b"".join(b"\x00" + bytes(px[y * S * 4:(y + 1) * S * 4]) for y in range(S))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", struct.pack(">IIBBBBB", S, S, 8, 6, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
           + chunk(b"IEND", b""))
    out = __file__.replace("make-icon.py", "icon-1024.png")
    with open(out, "wb") as f:
        f.write(png)
    print("wrote", out, len(png), "bytes")


if __name__ == "__main__":
    main()
