"""Draws the app's reading of an image: every line boxed in its block's
color, numbered in reading order. python viz_layout.py <exe> <image> <out.png>"""
import re
import subprocess
import sys

from PIL import Image, ImageDraw

exe, src, dst = sys.argv[1:4]
out = subprocess.run([exe, "--layout", src], capture_output=True, timeout=180).stdout.decode("utf-8", "replace")
img = Image.open(src).convert("RGB")
draw = ImageDraw.Draw(img, "RGBA")
colors = [(230, 25, 75), (60, 180, 75), (0, 130, 200), (245, 130, 48), (145, 30, 180), (0, 150, 150), (200, 0, 200), (128, 128, 0), (170, 110, 40), (0, 0, 128)]
blocks = set()
n = 0
for m in re.finditer(r"block (\d+) line (\d+) x (-?\d+)\.\.(-?\d+) y (-?\d+)\.\.(-?\d+)", out):
    b, line, l, r, t, bt = map(int, m.groups())
    c = colors[b % len(colors)]
    draw.rectangle([l, t, r, bt], outline=c + (255,), fill=c + (40,), width=1)
    if b not in blocks:
        draw.text((l - 2, t - 11), str(b), fill=c + (255,))
        blocks.add(b)
    n += 1
scale = min(1.0, 1500 / max(img.size))
if scale < 1:
    img = img.resize((int(img.width * scale), int(img.height * scale)), Image.LANCZOS)
img.save(dst)
print(f"{src}: {n} lines in {len(blocks)} blocks, {img.size}")
