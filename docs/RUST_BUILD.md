# Building AirDock

Rust 1.99.0 is pinned. The application uses Iced 0.14 and its matching wgpu API,
cpal 0.18.2 (WASAPI on Windows), and playback-only FFmpeg. The default supported Windows target is x64 MSVC on
Windows 10/11 with a compatible DX12/Vulkan adapter. Software UI fallback cannot
display the custom video shader; its support message is explicit.

## Windows

Use a dedicated vcpkg checkout at the `builtin-baseline` from `rust/vcpkg.json`.
The embedded overlay builds dynamic LGPL FFmpeg 8.1 with all encoders/muxers and
NVENC disabled, retaining HLS/MOV/MPEG-TS/HTTPS (Schannel), AAC/ALAC/H.264/HEVC,
D3D11VA and NVDEC/CUVID. Do not substitute a GPL FFmpeg installation.

```powershell
$env:VCPKG_ROOT = 'C:\src\vcpkg'
$env:VCPKGRS_DYNAMIC = '1'
$env:VCPKG_DEFAULT_TRIPLET = 'x64-windows'
& "$env:VCPKG_ROOT/vcpkg.exe" install --triplet x64-windows "--x-manifest-root=$pwd/rust" "--x-install-root=$env:VCPKG_ROOT/installed"
$env:FFMPEG_DIR = "$env:VCPKG_ROOT/installed/x64-windows"
$env:PATH = "$env:FFMPEG_DIR/bin;$env:PATH"
cargo build --locked --release --bins --examples
cargo test --locked --all-targets
```

The overlay ships the exact patched FFmpeg source archive inside the native
installation, so a native cache restore includes corresponding source too.
Cache keys include overlay contents, manifest, compiler and SDK. Packaging uses
`tools/rust_package.py` to copy only runtime imports and audit actual bundled
DLLs; do not copy the entire vcpkg bin directory.

## Linux regression host

Install libavcodec/libavformat/libavutil/libswresample development files,
libasound2-dev, libfontconfig1-dev, libxkbcommon-dev, libxkbcommon-x11-0,
libvulkan1, mesa-vulkan-drivers, clang/libclang, pkg-config, xvfb/xauth and
xdotool and openbox. Linux system FFmpeg is a development/test dependency, not the Windows
LGPL distribution. `AIRDOCK_AUDIO_NULL=1` explicitly selects deterministic
no-device testing and never counts as WASAPI or speaker validation.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --bins --examples
python tools/rust_dependency_audit.py
target/debug/examples/gpu_verify
python tools/rust_integration.py --binary target/debug/airdock
python tools/rust_reconnect_integration.py --binary target/debug/airdock
python tools/rust_hls_integration.py --binary target/debug/airdock
python tools/rust_hls_integration.py --fmp4 --binary target/debug/airdock
xvfb-run -a python tools/rust_ui_smoke.py --binary target/debug/airdock
```

Integration uses checked-in synthetic media and packet metadata. Encoders and
ffmpeg/ffprobe executables are not needed for runtime tests or included in the
package. Independent protocol oracles remain separate test executables.

## Release

Rust CI runs debug/release Windows regressions, GPU readback checks, dependency
checks and playback from a clean packaged PATH. Release dispatch builds this
branch's Cargo application and only publishes after those gates pass. The
workflow uses the checked-out Cargo implementation. Physical Windows/iPhone
feature and performance acceptance remains separately recorded and cannot be
inferred from runner/WARP/llvmpipe checks.

`windows-build.yml` keeps the raw payload artifact and adds
`airdock-distributions-x64-<commit>` containing the Inno Setup
per-user installer, portable ZIP and SHA-256 checksums. `release.yml`
verifies and publishes those exact files, rather than repackaging a different
payload. Dispatch the `Release` workflow (`release.yml`) on `main`
with `publish=false` to verify release assembly without publishing.
Inno Setup 6 is used only for distribution building; its notice is included as
`INSTALLER_LICENSE.txt`. No Rust/native runtime dependency is added.

Install defaults to `%LOCALAPPDATA%\Programs\AirDock`. Start Menu
shortcuts are installed; a desktop shortcut is optional. Configuration remains
in `%APPDATA%\AirDock\config`. Upgrades remove obsolete files tracked by the
previous installer manifest; untracked files are preserved. Uninstall removes an
autostart entry only if it points to this installation. CI installs in a path
containing spaces and Unicode, checks hashes/DLLs, upgrades, plays from the
installed clean PATH, and verifies uninstall/data preservation. Installers are
currently unsigned.


The manifest's overlay triplet builds release DLLs only; Rust debug and release
use those same playback DLLs through FFMPEG_DIR. The native cache is saved after
actual DLL ABI/license/encoder/source checks, so later Rust lint/test failures do
not force a second cold FFmpeg build. A restored cache must pass both that check
and the independent final package audit. Cargo build caches are saved on failure;
Cargo fingerprints and successful final builds remain mandatory before packaging.

Corresponding build material includes `sources/native-build` (vcpkg ports),
`sources/native-triplets`, the original manifest/toolchain and the FFmpeg source
archive. To rebuild from a distribution, pass explicit overlay port/triplet paths
matching those directory names when invoking the pinned vcpkg baseline.
