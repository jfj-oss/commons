#!/usr/bin/env python3
"""frameprobe.py <png> [x y ...]: the bounding box of the window's dark band (#151515 focused, #121212 not) and the pixels asked for.
Prints JSON {band: [x0,y0,x1,y1] | null, tone: 0x15|0x12, px: [[r,g,b]...]}."""
import sys, json
from PIL import Image
im = Image.open(sys.argv[1]).convert('RGB'); W, H = im.size; p = im.load()
res = {'band': None, 'px': []}
for tone in (0x15, 0x12):
    rows = [y for y in range(H) if sum(1 for x in range(W) if p[x, y] == (tone, tone, tone)) >= 100]
    if len(rows) >= 2:
        y0, y1 = rows[0], rows[-1]; mid = (y0 + y1) // 2
        xs = [x for x in range(W) if p[x, mid] == (tone, tone, tone)]
        if xs: res['band'] = [xs[0], y0, xs[-1], y1]; res['tone'] = tone; break
a = sys.argv[2:]
for i in range(0, len(a) - 1, 2): res['px'].append(list(p[int(a[i]), int(a[i + 1])]))
print(json.dumps(res))
