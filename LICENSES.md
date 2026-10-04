# Licenses

Project-owned receiver source, examples, build tools and documentation are
licensed under [Mozilla Public License 2.0](LICENSE).

The following files and dependencies keep their own licenses:

- `rust/src/playfair.rs` and `rust/src/playfair_data/`: MIT, copyright 2026 nored;
  see `AIRFRY-LICENSE.txt` and `NOTICE.md` in that directory.
- `rust/src/fairplay-replies.bin`: protocol reply data; existing ownership notices
  apply, with no additional rights granted by this project's MPL license.
- `tools/test_pair_verify.py` and `tools/generate_eld_fixtures.cpp`: GPL-3.0-only;
  license text in `licenses/GPL-3.0.txt`.
- `rust/native/ports/`: MIT; see its `LICENSE.txt`. FFmpeg source and patches
  retain their notices; bundled playback DLLs use LGPL-2.1-or-later.
- `rust/assets/fonts/`: SIL OFL 1.1; see `OFL.txt`, `NOTO-OFL.txt` and `README.md`.
- `tools/installer/INNO-LICENSE.txt`, `tools/licenses/` and Cargo dependencies:
  their accompanying license texts.

Distributions include application source, corresponding FFmpeg source/build
materials, dependency license texts and notices. Modified MPL-covered files
must remain available under MPL-2.0 when distributed.
