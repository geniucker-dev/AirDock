# AirPlay Windows — Rust / Slint edition

This branch (`feat/rust-slint`) implements the receiver and desktop application
in Rust, with a Slint interface. It does not link or launch the old C++ receiver.
FFmpeg and SDL2 remain native media dependencies; Windows integration uses the
Windows APIs directly. The C++ sources are retained as a comparison reference.

**Experimental until feature and performance parity is measured on real
Windows/iPhone hardware.** The reference is C++ commit
`562120e4a6c85da1ec33bca88a6f438db91b8545`. Passing CI alone does not establish
hardware parity. See [acceptance criteria](docs/RUST_ACCEPTANCE.md),
[build instructions](docs/RUST_BUILD.md) and [validation evidence](docs/RUST_VALIDATION.md).

![Rust / Slint receiver](docs/validation/receiver.png)

## Features

- Native Windows discovery, persistent identity, pair verification and Rust
  FairPlay key derivation; IPv4/IPv6, normal LAN and Windows Mobile Hotspot.
- H.264/HEVC mirroring, aspect-preserving resizing and orientation changes.
  Optional NVDEC → D3D11VA → CPU decoding fallback. Explicit color range/matrix
  conversion keeps the established color behavior.
- AAC-ELD 480/512, AAC-LC and ALAC audio; bounded RTP reordering, retransmission,
  sequence rollover, FLUSH, pause/resume, volume, cover art and track metadata.
- MP4 recording with audio, H.264/HEVC, automatic/GPU/CPU encoder selection,
  bitrate and folder settings. Recording uses a separate bounded worker queue.
- Slint receiver, recording, settings and about pages; persistent settings,
  fullscreen, borderless UI hiding, shortcuts and device/network status.
- Windows tray menu: restore/hide, recording, disconnect and quit. Close and
  minimize to tray are configurable; optional start hidden and Windows autostart.
  Tray hiding keeps networking, audio and recording active.
- USB arrival/removal detection and the existing USB Personal Hotspot network
  path. This uses AirPlay over the tethered network.
- Opt-in experimental HLS/FCUP playback, playlist/segment proxying and controls.
  FairPlay-protected streaming remains unsupported, as in the reference.

## Run

Download a Rust Windows CI artifact, extract the entire directory and run
`airplay-windows.exe`. Keep the included DLLs beside the executable. Allow the
receiver through Windows Firewall on the network you use.

Connect the iPhone and PC to the same reachable network, then choose
**AirPlay-Windows** in iOS Screen Mirroring. A home router is optional: enable
**Settings → Network & internet → Mobile hotspot** on Windows and join that
hotspot from the iPhone. The app includes a button to open those settings.
USB Personal Hotspot is another network path where the relevant Apple drivers
and tethering are available.

Settings and receiver identity are kept in the user's application data directory,
separately from the executable. Default recordings go into `Videos/AirPlay-Windows`.
To run an isolated instance: `airplay-windows.exe --config-dir <directory>`.

| Shortcut | Action |
| --- | --- |
| F11 | Toggle fullscreen |
| Ctrl+H | Show/hide UI; hidden UI uses a draggable, resizable borderless window |
| Ctrl+R | Start/stop recording |
| Ctrl+D | Disconnect |
| Escape | Leave fullscreen or restore UI |

`--help` lists command-line options, including headless mode, port, receiver
name, resolution/FPS hints, hardware decoding, logs and JSON metrics. The metrics
count newly presented video frames, separately from decoding and UI repainting.

## Build and CI

Rust 1.99.0 is pinned. Windows builds use the separate `rust/vcpkg.json` native
dependency manifest. See [RUST_BUILD.md](docs/RUST_BUILD.md) for reproducible commands.

- `rust-ci.yml`: Windows debug/release verification and Linux protocol/media,
  Slint and conversion performance checks.
- `rust-windows-build.yml`: reusable build, media tests and dependency-complete
  Windows artifact packaging, including dependency license texts.
- `Release` (`release.yml`): manual dispatch selects the implementation from
  the chosen commit. This Rust branch builds Cargo/Slint and publishes a clearly
  marked Rust prerelease; C++ sources use the original CMake release job.
- `rust-release.yml`: reusable experimental Rust publication, also triggered by
  `rust-v*` tags. New Rust tags point at the exact build commit.

The [original C++ README](docs/CPP_REFERENCE_README.md) describes the reference
implementation and its existing CMake workflows.

## License

GPL-3.0. See [LICENSE](LICENSE),
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and the notices shipped in build
artifacts. Slint is used under its GPL option. The independently compared Rust
PlayFair port retains its upstream MIT attribution.
