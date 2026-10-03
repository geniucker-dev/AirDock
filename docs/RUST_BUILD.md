# Building the Rust / Slint edition

The application build is `cargo`, not the reference CMake target. No C/C++
receiver, OpenSSL, libplist, FDK-AAC, ImGui or SDL_ttf is linked into this target.
FFmpeg and SDL2 are native libraries called from Rust. The C/C++ test oracles
and AAC fixture generator are independent validation tools only.

## Windows x64

Install Visual Studio 2022 C++ build tools with the Windows SDK, LLVM/Clang,
Python and Rust. Use an x64 developer PowerShell so the linker can find the
Windows libraries. Use a dedicated vcpkg checkout for the Rust edition,
bootstrap it and select the `builtin-baseline` revision from `rust/vcpkg.json`.

```powershell
rustup toolchain install 1.99.0 --component rustfmt --component clippy
$env:VCPKG_ROOT = 'C:\src\vcpkg'
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
$rustNativeRoot = Join-Path $env:VCPKG_ROOT 'installed'
& "$env:VCPKG_ROOT\vcpkg.exe" install --triplet x64-windows `
  "--x-manifest-root=$pwd\rust" "--x-install-root=$rustNativeRoot"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force "$rustNativeRoot\vcpkg\updates" | Out-Null
$env:FFMPEG_DIR = "$rustNativeRoot\x64-windows"
$env:VCPKGRS_DYNAMIC = '1'
$env:VCPKG_DEFAULT_TRIPLET = 'x64-windows'
$env:PATH = "$env:FFMPEG_DIR\bin;$env:FFMPEG_DIR\debug\bin;$env:FFMPEG_DIR\tools\ffmpeg;$env:PATH"
cargo build --locked --release --bins --examples
```

The pinned Rust `vcpkg` crate reads `VCPKG_ROOT/installed`; it does not honor
`VCPKG_INSTALLED_ROOT`. Keep the install root, Cargo library lookup and DLL
packaging pointed at that same directory. The manifest remains `rust/vcpkg.json`.

The FFmpeg manifest includes NVDEC, AMF and x264/x265 support. Usable hardware
encoders depend on the installed driver. Automatic recording selection tries
available hardware encoders and then CPU; GPU-only mode reports a failure when
no hardware encoder can open. Hardware decoder selection tries NVDEC, D3D11VA
and CPU, including recovery from hardware initialization/decode failures.

For distribution, copy the release executable and all release native DLLs,
plus the matching Visual C++ runtime. The reusable Windows workflow does this
and tests `--version` with the build dependency directories removed from PATH.
Do not distribute just the executable.

## Linux development and automated checks

Linux supports the receiver and Slint/SDL window for protocol/media validation.
Windows tray, autostart, native DNS registration and Apple USB device detection
are Windows-specific.

```bash
sudo apt-get install libavcodec-dev libavformat-dev libavutil-dev libswscale-dev \
  libswresample-dev libsdl2-dev libfontconfig1-dev clang libclang-dev pkg-config \
  ffmpeg xvfb xauth xdotool python3-cryptography python3-pil
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --bins --examples
python3 tools/rust_integration.py --binary target/debug/airplay-windows
xvfb-run -a python3 tools/rust_integration.py --gui --binary target/debug/airplay-windows
xvfb-run -a python3 tools/rust_ui_smoke.py --binary target/debug/airplay-windows
```

The audio unit test explicitly selects SDL's dummy driver. Integration tests
select the dummy driver only for their child process. The production receiver
uses the default audio device.

### Independent FairPlay comparison

```bash
cc -O2 -Ithird_party/playfair rust/tests/playfair_reference.c \
  third_party/playfair/playfair.c third_party/playfair/omg_hax.c \
  third_party/playfair/hand_garble.c third_party/playfair/sap_hash.c \
  third_party/playfair/modified_md5.c -lm -o target/playfair-reference
python3 tools/rust_fairplay_differential.py --reference target/playfair-reference \
  --rust target/debug/examples/fairplay_vectors
```

The Windows workflow uses `cl` for the same test-only reference.

### Conversion performance gate

```bash
cargo build --locked --release --example core_bench
c++ -O3 rust/tests/conversion_reference.cpp -lavutil -lswscale -o target/conversion-reference
python3 tools/rust_performance.py --reference target/conversion-reference \
  --rust target/release/examples/core_bench --output rust-validation/conversion-performance.json
```

Five paired runs alternate order at 1080p, 1440p and 4K, with the same FFmpeg
and warm-up. Each case must retain at least 95% of the reference conversion
throughput. This is a conversion microbenchmark, not a measurement of actual
AirPlay presentation FPS. Real Windows/iPhone acceptance is separate.

## Diagnostics and desktop behavior

`--metrics path.json` writes a final snapshot on graceful exit. Counters include
decoded/presented/replaced frames, audio errors/recovery and recording queue
losses. Latency is measured from complete encrypted mirror-payload receipt to SDL
presentation; p95 uses the last 4,096 newly presented frames. It includes decryption,
decoding and rendering, and excludes sender/network time before the complete
body arrives, matching the C++ latency origin. Use external presentation/CPU/memory traces for the paired
hardware gate; the C++ UI's decode FPS is not a substitute for presentation FPS.

`--log path` records diagnostics. `--exit-after seconds` and `--screenshot path`
allow repeatable smoke checks. Headless mode receives/decodes media without a
window, so its presented-frame counter is zero.

Close-to-tray defaults on; minimize-to-tray, start hidden and autostart are
optional. Quit shuts down sessions and joins recording finalizers. Restore via
the tray or launch the same application again. A per-config-directory instance
lock prevents duplicate receivers; the second launch requests restoration.

Identity/settings are written separately from executable files; malformed
identity data is rejected instead of silently replacing a paired identity.
Settings validate input before saving and use atomic replacement. Existing
connections keep their current media configuration; updated display hints take
effect when the sender reconnects. Changing name/HEVC/HLS advertisement settings
re-registers discovery. Network and USB enumeration runs outside the UI thread.

## CI caches and manual releases

The Windows CI and release jobs share Cargo dependency caches. Native installed
packages are cached separately by vcpkg baseline, manifest, architecture, MSVC
and Windows SDK versions. An unrelated runner image update does not invalidate
that native cache. The previous image-based key is accepted for migration on
that same image/compiler combination. Installation still runs to validate the
manifest and required files; cache hits should not rebuild unchanged packages.

Native dependencies are saved immediately after successful installation, before
Rust lint/tests, so a later Rust failure does not discard them. Cargo dependency
caches include the native manifest and compiler/SDK versions. Once Debug and
Release compilation both finish, they can be saved even if a later integration
or packaging check fails; an early compile failure cannot freeze incomplete
profiles under an immutable key. A cold build can still take around 30 minutes;
Rust CI lets it finish instead of cancelling it with every new push. GitHub
coalesces pending runs to the newest queued commit.

For a manual Rust release, select the existing **Release** workflow and choose
`feat/rust-slint` in **Run workflow**. The workflow reads the selected commit's
sources and calls the Rust build/release jobs; the CMake job is skipped. Rust
packages and tags use the `rust-dev-*` naming and are always prereleases. Their
tags point to the build's commit, not the repository's default branch. A C++
branch continues to use the CMake job.
