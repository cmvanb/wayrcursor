# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "pillow"]
# ///
"""Asserts ring geometry in frames recorded by e2e.sh.

Usage: check_frames.py OUT_DIR CURSOR_X CURSOR_Y SCALE [locate args...]
Env: SETTLED_AT, seconds after launch when the cursor reached CURSOR_X/Y.
     EXPECT_NONE=1 asserts the ring never appears.
"""

import math
import os
import sys
from pathlib import Path

import numpy as np
from PIL import Image

BACKGROUND = np.array([0x33, 0x66, 0x99])
CURSOR_LOGICAL = 26


def flag(args, name, default):
    return float(args[args.index(name) + 1]) if name in args else default


def ring_pixels(path, cx, cy, scale):
    img = np.asarray(Image.open(path).convert("RGB")).astype(int)
    mask = np.abs(img - BACKGROUND).max(axis=2) > 24
    cursor = int(CURSOR_LOGICAL * scale)
    mask[max(cy - 2, 0) : cy + cursor, max(cx - 2, 0) : cx + cursor] = False
    ys, xs = np.nonzero(mask)
    return np.hypot(xs - cx, ys - cy), np.arctan2(ys - cy, xs - cx)


def main():
    out, cx, cy, scale = Path(sys.argv[1]), float(sys.argv[2]), float(sys.argv[3]), float(sys.argv[4])
    args = sys.argv[5:]
    radius = flag(args, "--radius", 40) * scale
    reach = (flag(args, "--width", 6) / 2 + flag(args, "--outline-width", 1) + 1.5) * scale
    px, py = round(cx * scale), round(cy * scale)

    rows = []
    for line in (out / "frames.txt").read_text().splitlines():
        path, t = line.split()
        dist, angle = ring_pixels(path, px, py, scale)
        rows.append((Path(path).name, float(t), dist, angle))

    failures = []
    landed = []
    for name, t, dist, angle in rows:
        if dist.size == 0:
            continue
        bins = np.unique(((angle + math.pi) / (2 * math.pi) * 36).astype(int) % 36).size
        is_landed = dist.min() >= radius - reach and dist.max() <= radius + reach and bins == 36
        landed.append(is_landed)
        print(f"{name} t={t:5.3f}s pixels={dist.size:7d} dist={dist.min():7.1f}..{dist.max():7.1f} arcs={bins:2d}{' landed' if is_landed else ''}")

    visible = [r for r in rows if r[2].size]
    if os.environ.get("EXPECT_NONE"):
        print(f"FAIL: ring appeared in {len(visible)} frames" if visible else "PASS")
        sys.exit(1 if visible else 0)
    if not visible:
        failures.append("ring never appeared")
    if not any(landed):
        failures.append(f"ring never landed as a full circle at radius {radius:.1f}px ±{reach:.1f}")
    first_landed = landed.index(True) if True in landed else len(landed)
    settled = float(os.environ.get("SETTLED_AT", 0))
    inbound = [r[2].max() for r in visible[:first_landed] if r[1] >= settled]
    if not any(d > 3 * radius for d in inbound):
        failures.append("no inbound frame captured far from the cursor")
    if any(b > a + 2 for a, b in zip(inbound, inbound[1:])):
        failures.append(f"ring grew while closing in: {[round(d) for d in inbound]}")
    after, _ = ring_pixels(out / "after.ppm", px, py, scale)
    if after.size:
        failures.append(f"{after.size} ring pixels remain after exit")

    for f in failures:
        print(f"FAIL: {f}")
    print("PASS" if not failures else "")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
