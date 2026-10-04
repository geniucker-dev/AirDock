# Embedded desktop fonts

Manrope Regular/SemiBold are static faces of the included upstream variable font,
under SIL OFL 1.1 (`OFL.txt`). Reproduce with `tools/rust_ui_fonts.py`.

AirPlay UI CJK Regular/SemiBold are renamed, static subsets of Noto Sans SC,
Copyright 2014–2021 Adobe, under SIL OFL 1.1 (`NOTO-OFL.txt`). They include GB2312,
interface translations and common punctuation. Other sender metadata glyphs may
use the operating system's font fallback. The original reserved name `Source`
is not used for the derived faces.

Pinned source:
https://raw.githubusercontent.com/google/fonts/2894aab31764f10f29c421bdfd2340d3b382d384/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf

SHA-256: `a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da`

Install fonttools, download that input, then run:

```sh
python tools/rust_cjk_fonts.py --input /path/to/NotoSansSC.ttf
```

Both faces are embedded once, not uploaded again for each video frame. Packaged
`MANROPE_LICENSE.txt` and `NOTO_LICENSE.txt` must match their source licenses;
the Windows payload audit checks this before producing installer/portable assets.
