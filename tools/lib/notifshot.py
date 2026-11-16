#!/usr/bin/env python3
"""notifshot.py <notification log (jsonl)> <window.png> <out.png>: the mock notification service's last Notify call drawn the way a
KDE note looks (title, text, the Reply field and Open), beside the window's screenshot. It is a picture of what was sent to the
mock service, not a capture of any real popup."""
import json, sys
from PIL import Image, ImageDraw, ImageFont
notes = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
n = notes[-1]
win = Image.open(sys.argv[2]).convert('RGB')
W, H = win.size
canvas = Image.new('RGB', (W, H), (20, 20, 24)); canvas.paste(win, (0, 0))
d = ImageDraw.Draw(canvas)
def font(sz, bold=False):
    for p in (['/usr/share/fonts/noto/NotoSans-Bold.ttf'] if bold else []) + ['/usr/share/fonts/noto/NotoSans-Regular.ttf', '/usr/share/fonts/TTF/DejaVuSans.ttf']:
        try: return ImageFont.truetype(p, sz)
        except Exception: pass
    return ImageFont.load_default()
x0, y0, w, h = W - 440, H - 230, 420, 200
d.rounded_rectangle((x0, y0, x0 + w, y0 + h), 14, fill=(42, 46, 50), outline=(80, 86, 92))
d.text((x0 + 16, y0 + 12), n['app'] + ' — mock notification service', font=font(12), fill=(150, 156, 162))
d.text((x0 + 16, y0 + 34), n['summary'], font=font(18, True), fill=(240, 240, 240))
d.text((x0 + 16, y0 + 62), n['body'], font=font(15), fill=(220, 220, 220))
labels = n['actions'][1::2]; keys = n['actions'][0::2]
if 'inline-reply' in keys:
    d.rounded_rectangle((x0 + 16, y0 + 100, x0 + w - 96, y0 + 134), 8, fill=(30, 33, 36), outline=(100, 106, 112))
    d.text((x0 + 26, y0 + 108), 'Reply…' if len(sys.argv) < 5 else sys.argv[4], font=font(14), fill=(170, 176, 182))
    d.rounded_rectangle((x0 + w - 84, y0 + 100, x0 + w - 16, y0 + 134), 8, fill=(61, 174, 233))
    d.text((x0 + w - 70, y0 + 108), 'Send', font=font(14, True), fill=(255, 255, 255))
d.rounded_rectangle((x0 + 16, y0 + 150, x0 + 90, y0 + 182), 8, outline=(100, 106, 112)); d.text((x0 + 34, y0 + 157), 'Open', font=font(14), fill=(230, 230, 230))
d.text((x0 + 110, y0 + 160), 'actions: ' + ', '.join(keys), font=font(11), fill=(130, 136, 142))
canvas.save(sys.argv[3]); print(sys.argv[3])
