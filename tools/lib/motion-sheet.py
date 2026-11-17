#!/usr/bin/env python3
"""motion-sheet.py: contact sheets and motion checks for frame sequences.
  sheet <out.png> <dir> <name> <n> <x,y,w,h> <cols>   web (top) over native (bottom): the frames <name>-web-NN.png / <name>-native-NN.png, cropped, in a grid
  diff <dir> <name-side> <n> <x,y,w,h>                {"pairs", "changed", "min", "max"}: how many consecutive frame pairs differ (and by how many pixels)"""
import sys, json
from PIL import Image, ImageChops, ImageDraw

def load(d, name, side, i, crop):
    im = Image.open(f"{d}/{name}-{side}-{i:02d}.png").convert('RGB')
    x, y, w, h = crop
    if side == 'native': x += 162; y += 118   # the native window's content area inside the headless workspace
    return im.crop((x, y, min(x + w, im.width), min(y + h, im.height)))

def sheet(out, d, name, n, crop, cols):
    x, y, w, h = crop
    tw = 480 if w > 900 else 400; th = int(tw * h / w)
    rows = (n + cols - 1) // cols
    W, H = cols * tw + (cols + 1) * 4, 2 * (rows * th + (rows + 1) * 4 + 22)
    sheet = Image.new('RGB', (W, H), (24, 22, 30)); dr = ImageDraw.Draw(sheet)
    for k, side in enumerate(('web', 'native')):
        oy = k * (rows * th + (rows + 1) * 4 + 22)
        dr.text((6, oy + 4), f"{side}: {name}, {n} frames, left to right, top to bottom", fill=(233, 231, 239))
        for i in range(n):
            f = load(d, name, side, i, crop).resize((tw, th), Image.LANCZOS)
            sheet.paste(f, (4 + (i % cols) * (tw + 4), oy + 22 + 4 + (i // cols) * (th + 4)))
    sheet.save(out)

def diff(d, nameside, n, crop):
    name, side = nameside.rsplit('-', 1)
    counts = []
    prev = load(d, name, side, 0, crop)
    for i in range(1, n):
        cur = load(d, name, side, i, crop)
        bb = ImageChops.difference(prev, cur).convert('L').point(lambda v: 255 if v > 12 else 0)
        counts.append(sum(1 for v in bb.getdata() if v))
        prev = cur
    # a pair "changed" when more than a speck of pixels differ
    return {"pairs": len(counts), "changed": sum(1 for c in counts if c > 40), "min": min(counts), "max": max(counts), "counts": counts}

if __name__ == '__main__':
    c = sys.argv[1]
    if c == 'sheet':
        out, d, name, n, crop, cols = sys.argv[2:8]
        sheet(out, d, name, int(n), [int(v) for v in crop.split(',')], int(cols))
    elif c == 'diff':
        d, ns, n, crop = sys.argv[2:6]
        print(json.dumps(diff(d, ns, int(n), [int(v) for v in crop.split(',')])))
