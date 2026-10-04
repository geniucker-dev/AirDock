# Third-party notices

Dependency copyright and license notices are retained below and in the accompanying
license texts. See [Credits](README.md#credits) for acknowledgements.

- **AirDock UI CJK**: static Regular/SemiBold subsets derived from Noto Sans SC,
  copyright 2014–2021 Adobe. SIL OFL 1.1; renamed to avoid upstream reserved names.
  https://github.com/google/fonts/tree/main/ofl/notosanssc. Includes GB2312 and all
  translated UI characters. Reproduction tool/input hash accompany source;
  NOTO_LICENSE.txt accompanies binary distributions.
- **Manrope**: Copyright 2018 The Manrope Project Authors, SIL OFL 1.1.
  https://github.com/sharanda/manrope. The application embeds static 400/600
  faces derived from the Google Fonts variable font. Original font, OFL and
  reproduction tool accompany source; MANROPE_LICENSE.txt accompanies binaries.
- **Iced** (0.14): MIT. https://github.com/iced-rs/iced
- **wgpu** (27): MIT OR Apache-2.0. https://github.com/gfx-rs/wgpu
- **cpal** (0.18): Apache-2.0. https://github.com/RustAudio/cpal
- **FFmpeg**: Windows DLLs use the pinned playback-only LGPL-2.1-or-later build,
  with GPL/nonfree features, all encoders and muxers disabled. No x264/x265/NVENC
  encoding libraries are included. D3D11VA/NVDEC/CUVID decoding remains enabled.
  The package includes license texts, patched corresponding FFmpeg source and
  exact vcpkg build materials. Actual DLL versions/configuration/hashes and
  runtime dependency closure are recorded in NATIVE_AUDIT.json. https://ffmpeg.org/
- **vcpkg FFmpeg build port**: Microsoft/vcpkg, MIT; copied from the pinned
  manifest baseline and modified to disable encoding and archive patched source.
  Its license accompanies the overlay. https://github.com/microsoft/vcpkg
- **Microsoft Visual C++ runtime**: Microsoft redistributable runtime DLLs are
  bundled when imported by the executable/native DLLs. Windows OS libraries and
  graphics drivers are system prerequisites and are not redistributed.
- **Inno Setup installer**: Copyright Jordan Russell and Martijn Laan.
  The installer incorporates Inno Setup; its license/acknowledgement accompanies
  distributions as INSTALLER_LICENSE.txt. https://jrsoftware.org/isinfo.php
- **Rust dependencies**: Cargo.lock pins versions. Distribution artifacts include
  source license texts and SPDX/repository metadata in rust-licenses/index.json.
- **Rust PlayFair implementation/data**: MIT as published by nored/airfry;
  its license and attribution remain in rust/src/playfair_data/. The port retains
  MIT and its copyright notice.
- **Test fixtures**: synthetic media, generated independently; no device captures
  or copyrighted media. See rust/tests/fixtures/README.md. External fixture
  generators/encoders are used only to regenerate test data.


- **Protocol reply data** (`rust/src/fairplay-replies.bin`): Apple Inc.;
  no additional license grant over these bytes.
- **Test utilities** (`tools/test_pair_verify.py`, `tools/generate_eld_fixtures.cpp`):
  GPL-3.0-only; license text accompanies source and distributions.
