# SPDX-License-Identifier: MPL-2.0
"""Render AirDock's SVG into platform icons; needs CairoSVG and Pillow.

These are design-time tools. The application embeds only its small PNGs and
Windows resources; rendering and resizing never run in the playback path.
"""
import io
from pathlib import Path

import cairosvg
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'rust/assets/icons'
source = (ASSETS / 'airdock.svg').read_bytes()
for size in (32, 128, 256, 512, 1024):
    data = cairosvg.svg2png(bytestring=source, output_width=size * 4, output_height=size * 4)
    image = Image.open(io.BytesIO(data)).convert('RGBA').resize((size, size), Image.Resampling.LANCZOS)
    image.save(ASSETS / f'airdock-{size}.png', optimize=True)
image = Image.open(ASSETS / 'airdock-1024.png')
image.save(ASSETS / 'airdock.ico', sizes=[(n, n) for n in (16, 24, 32, 48, 64, 128, 256)])
image.save(ASSETS / 'airdock.icns')
