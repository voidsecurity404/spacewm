#!/usr/bin/env python3
"""Tiny PNG reader for test assertions (8-bit RGB/RGBA, no dependencies).

  pixel.py FILE X Y            -> "R G B"
  pixel.py diff A B            -> fraction of pixels that differ (0.0 .. 1.0)
  pixel.py near FILE X Y R G B [TOL]  -> exit 0 if the pixel is within TOL (default 40)
"""
import struct
import sys
import zlib


def read_png(path):
    with open(path, "rb") as f:
        data = f.read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos, idat, w = 8, b"", None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            w, h, depth, color = struct.unpack(">IIBB", body[:10])
            assert depth == 8 and color in (2, 6), "only 8-bit RGB/RGBA"
            bpp = 4 if color == 6 else 3
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    raw = zlib.decompress(idat)
    stride = w * bpp
    rows, prev = [], bytearray(stride)
    for y in range(h):
        filt = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - bpp] if i >= bpp else 0
            b = prev[i]
            c = prev[i - bpp] if i >= bpp else 0
            if filt == 1:
                line[i] = (line[i] + a) & 255
            elif filt == 2:
                line[i] = (line[i] + b) & 255
            elif filt == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif filt == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 255
        rows.append(line)
        prev = line
    return w, h, bpp, rows


def pixel(img, x, y):
    w, h, bpp, rows = img
    r = rows[y]
    return tuple(r[x * bpp:x * bpp + 3])


def main():
    args = sys.argv[1:]
    if args[0] == "diff":
        a, b = read_png(args[1]), read_png(args[2])
        if a[:2] != b[:2]:
            print(1.0)
            return
        w, h = a[0], a[1]
        changed = sum(1 for y in range(0, h, 2) for x in range(0, w, 2) if pixel(a, x, y) != pixel(b, x, y))
        print(changed / ((w // 2) * (h // 2)))
    elif args[0] == "near":
        img = read_png(args[1])
        x, y, r, g, b = map(int, args[2:7])
        tol = int(args[7]) if len(args) > 7 else 40
        got = pixel(img, x, y)
        ok = all(abs(p - q) <= tol for p, q in zip(got, (r, g, b)))
        print(*got)
        sys.exit(0 if ok else 1)
    else:
        print(*pixel(read_png(args[0]), int(args[1]), int(args[2])))


main()
