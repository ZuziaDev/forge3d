#!/usr/bin/env python3
"""Rasterise assets/logo.svg geometry (mirrors the SVG) into logo.png and icon.ico. Needs Pillow."""
import sys
from PIL import Image, ImageDraw
S = 4
N = 512 * S

def lerp(a, b, t): return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))
def hexc(h): return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))

def gradient(c0, c1, diag):
    import numpy as np
    y, x = np.mgrid[0:N, 0:N].astype('float32') / N
    t = (x + y) / 2 if diag else y
    a, b = np.array(hexc(c0), 'float32'), np.array(hexc(c1), 'float32')
    arr = (a + (b - a) * t[..., None]).astype('uint8')
    return Image.fromarray(arr, 'RGB')

def poly(pts): return [(x * S, y * S) for x, y in pts]

img = Image.new('RGBA', (N, N), (0, 0, 0, 0))
mask = Image.new('L', (N, N), 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, N - 1, N - 1), 104 * S, fill=255)
img.paste(gradient('#241a2e', '#0e0b14', True), (0, 0), mask)
faces = [
    ([(256, 80), (408, 168), (256, 256), (104, 168)], '#ffc247', '#ff7a2f', True),
    ([(104, 168), (256, 256), (256, 432), (104, 344)], '#ff5a3c', '#d6277a', False),
    ([(256, 256), (408, 168), (408, 344), (256, 432)], '#e03a74', '#8d1f8f', True),
]
for pts, c0, c1, diag in faces:
    m = Image.new('L', (N, N), 0)
    d = ImageDraw.Draw(m)
    d.polygon(poly(pts), fill=255)
    img.paste(gradient(c0, c1, diag), (0, 0), m)
    ImageDraw.Draw(img).line(poly(pts + [pts[0]]), fill=hexc('#0e0b14'), width=6 * S, joint='curve')
d = ImageDraw.Draw(img)
z = [(196, 214), (316, 214), (196, 322), (316, 322)]
d.line(poly(z), fill=hexc('#fff6e8'), width=34 * S, joint='curve')
for x, y in z:
    r = 17 * S
    d.ellipse((x * S - r, y * S - r, x * S + r, y * S + r), fill=hexc('#fff6e8'))
out = img.resize((512, 512), Image.LANCZOS)
out.save('assets/logo.png')
out.save('assets/icon.ico', sizes=[(16, 16), (32, 32), (48, 48), (256, 256)])

# --- NSIS installer artwork (24-bit BMP) -----------------------------------
def banner(size, vertical):
    w, h = size
    bg = Image.new('RGB', size, (22, 18, 30))
    px = bg.load()
    for y in range(h):
        for x in range(w):
            t = (y / h) if vertical else (x / w)
            px[x, y] = lerp((38, 26, 50), (14, 11, 20), t)
    return bg

side = banner((164, 314), True)
icon = out.resize((120, 120), Image.LANCZOS)
side.paste(icon, (22, 70), icon)
side.save('assets/installer-welcome.bmp')
head = banner((150, 57), False)
small = out.resize((44, 44), Image.LANCZOS)
head.paste(small, (98, 6), small)
head.save('assets/installer-header.bmp')
