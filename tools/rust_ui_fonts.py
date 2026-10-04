# SPDX-License-Identifier: MPL-2.0
"""Reproduce bundled static Manrope faces (developer tool; requires fonttools).
Input: unmodified Google Fonts ofl/manrope/Manrope[wght].ttf, SIL OFL 1.1.
Static faces keep the upstream family name; upstream declares no Reserved Font Name.
"""
from pathlib import Path
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

ROOT = Path(__file__).resolve().parents[1] / 'rust/assets/fonts'
for weight, style in [(400, 'Regular'), (600, 'SemiBold')]:
    font = TTFont(ROOT / 'Manrope.ttf', recalcTimestamp=False)
    font = instantiateVariableFont(font, {'wght': weight}, inplace=True)
    font['OS/2'].usWeightClass = weight
    names = {1: 'Manrope', 2: style, 3: f'Manrope-{style}',
             4: f'Manrope {style}', 6: f'Manrope-{style}', 16: 'Manrope', 17: style}
    for record in font['name'].names:
        if record.nameID in names:
            record.string = names[record.nameID].encode(record.getEncoding())
    font.save(ROOT / f'Manrope-{style}.ttf')
