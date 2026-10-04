# License scope

The current Iced Rust receiver's project-owned source is licensed under
[Mozilla Public License 2.0](LICENSE), including `rust/src/` (except the MIT port
below), `rust/examples/`, project-owned `tools/rust_*.py`, the installer script,
Rust workflows, and Rust documentation. Existing source grants remain valid;
this change offers MPL-2.0 for the project-owned Rust implementation.

Exceptions retain their existing license and attribution:

- `rust/src/playfair.rs` and `rust/src/playfair_data/`: MIT as published by
  nored/airfry; see `rust/src/playfair_data/AIRFRY-LICENSE.txt` and `NOTICE.md`.
  FairPlay authorization-chain analysis is outside this change.
- `rust/src/fairplay-replies.bin`: unchanged protocol reply data. Historical
  ownership/provenance notices apply; no new license grant over these bytes.
- `third_party/playfair/`, `third_party/fairplay_blobs_*.cpp`,
  `rust/tests/playfair_reference.c`, `tools/test_pair_verify.py`,
  `tools/generate_eld_fixtures.cpp`, retained C++ documentation/workflows and
  historical C++ sources: existing GPL-3.0 terms; [preserved text](licenses/GPL-3.0.txt).
  The GPL C oracle is a separate test executable and is never linked into the receiver.
- `rust/native/ports/`: vcpkg MIT attribution in its LICENSE.txt; bundled FFmpeg
  patches/source keep their upstream notices. Runtime FFmpeg DLLs are LGPL.
- `rust/assets/fonts/`: Manrope, SIL Open Font License 1.1. Original variable
  font and derived static Regular/SemiBold faces retain OFL; see `OFL.txt`.
  `tools/rust_ui_fonts.py` reproduces the static faces.
- `tools/installer/INNO-LICENSE.txt`, `tools/licenses/` and Cargo dependencies:
  their own license texts. Screenshots and synthetic fixtures keep their notices.

Distribution includes exact Rust application source, native LGPL source/build
materials and third-party notices. MPL does not require unrelated files to be
MPL; modified covered files must remain available under MPL when distributed.
See [source relationship review](docs/RUST_SOURCE_ORIGIN.md).
