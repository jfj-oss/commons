#!/usr/bin/env python3
"""kwinshot.py <file holding the private bus address> <out.png>: a screenshot of a private headless KWin (never the user's)."""
import os, sys, dbus
from PIL import Image
bus = dbus.bus.BusConnection(open(sys.argv[1]).read().strip())
o = bus.get_object('org.kde.KWin', '/org/kde/KWin/ScreenShot2')
r, w = os.pipe()
res = dbus.Interface(o, 'org.kde.KWin.ScreenShot2').CaptureWorkspace({'native-resolution': True}, dbus.types.UnixFd(w))
os.close(w)
width, height, stride = int(res['width']), int(res['height']), int(res['stride'])
with os.fdopen(r, 'rb') as f: buf = f.read()
raw = 'RGBA' if int(res.get('format', 0)) in (16, 17, 18) else 'BGRA'
Image.frombuffer('RGBA', (width, height), buf, 'raw', raw, stride, 1).convert('RGB').save(sys.argv[2]); print(sys.argv[2], width, height)
