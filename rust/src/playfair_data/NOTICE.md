# PlayFair lookup tables and port

The Rust cipher and these expanded little-endian lookup tables were adapted
from nored/airfry, which ports doubletake's Go PlayFair implementation:

- https://github.com/nored/airfry — MIT, copyright 2026 nored;
  the license is reproduced in `AIRFRY-LICENSE.txt`.
- https://github.com/omarroth/doubletake — original compact lookup table
  expansion and reverse-engineered algorithm, credit omarroth and upstream
  contributors.

The application embeds these table bytes directly. It does not run a FairPlay
binary emulator or download binaries at build time. The protocol reply data in
`../fairplay-replies.bin` is the unchanged reply table from this repository's
C++ reference; its existing ownership/provenance notices still apply.

The Rust port's promoted byte-rotation arithmetic was corrected after comparison
with the bundled C implementation. `tools/rust_fairplay_differential.py` checks
all four modes using independent deterministic inputs, and the first failing
case is retained as a regression fixture. That C oracle is test-only and is
never linked into the Rust application.
