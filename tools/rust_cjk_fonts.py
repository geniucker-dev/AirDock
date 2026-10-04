# SPDX-License-Identifier: MPL-2.0
"""Reproduce OFL UI fonts: fonttools required, original supplied using --input.
Input: Google Fonts ofl/notosanssc/NotoSansSC[wght].ttf
SHA256 a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da
Includes GB2312 plus all translated interface characters and common punctuation.
Renamed AirPlay UI CJK; not an upstream unmodified font. No Reserved Font Name used.
"""
import argparse, hashlib
from pathlib import Path
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from fontTools import subset
root=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--input',type=Path,required=True);args=parser.parse_args()
assert hashlib.sha256(args.input.read_bytes()).hexdigest()=='a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da'
chars=set(range(32,127))|set(range(0x2000,0x2070))|set(range(0x3000,0x3040))
for lead in range(0xa1,0xf8):
    for tail in range(0xa1,0xff):
        try: chars.update(map(ord,bytes([lead,tail]).decode('gb2312')))
        except UnicodeDecodeError: pass
chars.update(map(ord,(root/'rust/src/i18n.rs').read_text()))
for weight,style in [(400,'Regular'),(600,'SemiBold')]:
    font=TTFont(args.input,recalcTimestamp=False)
    options=subset.Options();options.name_IDs=['*'];options.name_legacy=True;options.name_languages=['*']
    sub=subset.Subsetter(options=options);sub.populate(unicodes=chars);sub.subset(font)
    font=instantiateVariableFont(font,{'wght':weight},inplace=True)
    font['OS/2'].usWeightClass=weight
    names={1:'AirPlay UI CJK',2:style,3:f'AirPlayUICJK-{style}',4:f'AirPlay UI CJK {style}',6:f'AirPlayUICJK-{style}',16:'AirPlay UI CJK',17:style}
    for record in font['name'].names:
        if record.nameID in names:record.string=names[record.nameID].encode(record.getEncoding())
    font.save(root/f'rust/assets/fonts/AirPlayUICJK-{style}.ttf')
