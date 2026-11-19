#!/usr/bin/env python3
"""sidebyside.py <left.png> <right.png> <out.png>: the two pictures next to each other, a thin gap between."""
import sys
from PIL import Image
a, b = Image.open(sys.argv[1]).convert('RGB'), Image.open(sys.argv[2]).convert('RGB')
h = max(a.height, b.height)
o = Image.new('RGB', (a.width + b.width + 8, h), (255, 0, 255))
o.paste(a, (0, 0)); o.paste(b, (a.width + 8, 0)); o.save(sys.argv[3])
