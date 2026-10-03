# Rust / Slint validation evidence

Validated on 2026-10-03 in a Debian 13 Linux container, Rust 1.99.0, FFmpeg
7.1.5 and SDL2. The application is an experimental implementation. These
results establish automated protocol/media behavior, not Windows/iPhone
feature or end-to-end performance parity.

| Check | Result |
| --- | --- |
| Formatting and Clippy, all targets | Passed, warnings denied |
| Debug and release unit tests | 26 passed in each profile |
| Pair verification and wire protocol | 26 independent Python checks passed |
| PlayFair | 256 independent deterministic vectors, all four modes, identical to the bundled C oracle |
| Encrypted mirroring | 120 H.264/HEVC frames decoded, including changes between landscape/portrait and between codecs on the same connection |
| Mirroring reconnects | 13 independent encrypted connection scenarios, 390 frames decoded in both headless and Slint GUI runs |
| Slint presentation | 120 new frames presented; zero replaced frames and zero recording drops in the synthetic release run |
| Color conversion | 4,752 independently calculated vectors; maximum error 2.886/255, below the 3/255 bound |
| Audio | Encrypted ALAC over IPv4/IPv6 with exact PCM, AAC-ELD 480/512 and both supported content types; loss/retransmit/reorder/wrap/FLUSH/long-pause recovery passed |
| Sequence rollover | 140,000 consecutive packets checked across wraps |
| Recording | H.264 and HEVC MP4s decoded with ffprobe/FFmpeg; 120 frames, audio/video duration difference below 80 ms, 48 kHz input resampled to 44.1 kHz; held final image retained |
| Experimental HLS | Independent FCUP sender: 30 video frames including B-frame drain, 46,080 audio samples per channel; pause/resume, property echo and cancellation passed |
| Slint desktop | Receiver/settings/recording/about screenshots inspected; hidden UI has no remaining pixels, fullscreen/restore returns to the receiver image, text editing/HEVC toggle/save persists correctly |
| Windows API compile checks | DNS registration, tray, autostart, folder picker, USB detection, settings and console integration cross-compiled and linted for MSVC; hardware decoder API paths compile-checked separately |
| Workflows | All three Rust workflows pass actionlint; Python harnesses compile; locked dependency license packaging checked |

The synthetic release GUI run's receive-to-present p95 was 2.445 ms for 120
small generated frames under xvfb. This is diagnostic evidence only: it does
not measure real network/sender latency or establish iPhone frame-rate parity.
Subsequent CI runs save their own metrics rather than assuming this number.

[Receiver preview](validation/receiver.png) · [Settings preview](validation/settings.png) · [Recording preview](validation/recording.png)

## Conversion microbenchmark

Five paired, alternating runs at each resolution, 600 measured frames per run
and 60 warm-up frames. Rust and the small C++ oracle use the same FFmpeg
conversion settings and AVFrame reference operation. The reference program is
`rust/tests/conversion_reference.cpp`; it is not the complete C++ receiver.

| Resolution | Rust / C++ paired median throughput | 95% conversion gate |
| --- | --- | --- |
| 1920 × 1080 | 98.33% | Passed |
| 2560 × 1440 | 98.51% | Passed |
| 3840 × 2160 | 96.73% | Passed |

[Raw paired measurements and environment](validation/conversion-performance.json).
This comparison covers CPU frame references and NV12-to-BGRA conversion only.
It excludes decoding, transport, SDL presentation, Slint overlays and hardware
frame transfers. Actual AirPlay FPS, CPU and memory still require the paired
hardware measurements in [RUST_ACCEPTANCE.md](RUST_ACCEPTANCE.md).

The UI retains its previous image in an SDL texture and uploads dirty spans
with one scanline and at most 32 staged rows. At 4K, CPU pixel staging is about
0.5 MiB instead of two 31.6 MiB images. The UI texture is released while hidden
or minimized; hidden UI skips rendering and blending. On the active receiver
page, only the surrounding UI and any error banner are blended over the video.
On Windows, NV12 chroma is split with SSE2 into two cached planes before the
planar swscale conversion. Luma stays in its original frame. This avoids the
MSVC build's less accurate C conversion fallback without a full-frame copy;
the extra chroma storage is about 4 MiB at 4K and is released when the pixel
format changes. Both layout paths have independent color-vector coverage,
and unaligned SIMD tails, odd dimensions and size changes are tested. The
Windows workflow also enforces the same five-pair 95% conversion performance
gate with its actual FFmpeg DLLs.

Video uses a cached independent texture and the latest-frame handoff. Recording
has a separate bounded queue: 90 video frames and 10 seconds of stereo audio;
encoder initialization and MP4 finalization run outside presentation/transport.

## Mirroring reconnect regression

The previous receiver decoded only the first 30 of 60 independently encrypted
frames when a low-bit stream ID was followed by an eight-byte ID with its high
bit set. The plist reader interprets eight-byte integers as signed; the generic
unsigned accessor dropped these IDs, and SETUP substituted zero. Video key
derivation therefore used a different ID from the sender. SETUP now preserves
the opaque ID's 64-bit pattern, without changing port or other numeric parsing.

A separately reproduced TCP reset during a partial mirror header also closed
the advertised video listener. Connection errors now end only the accepted
socket, allowing a new data connection with fresh decoder and cipher state.

`tools/rust_reconnect_integration.py` exercises IDs around the signed boundary,
both eight-byte signed and wider positive plist encodings, full and stream-only
TEARDOWN on a reused control connection, replacement control connections while
the old connection remains open, clean data EOF, resets during headers/payloads,
and recovery after malformed configuration. All 13 successful connections must
decode their 30 H.264/HEVC frames. Windows and Linux CI run this harness; Linux
also runs it through Slint presentation. The local GUI run decoded 390 frames
and presented 386, with four latest-frame replacements under synthetic load.
These checks do not establish actual iPhone or high-resolution FPS parity.

Windows accepted TCP sockets inherit the nonblocking listener's mode. Control,
mirror and HLS proxy connections now explicitly use blocking reads with their
existing timeouts. This prevents idle control/video threads spinning on
`WouldBlock`, and prevents the proxy closing a connection before its HTTP
request arrives. A regression forces the inherited nonblocking mode on Linux
too, then sends a delayed, fragmented request; it fails before the correction.
The wire harness also samples receiver process CPU while idle control/video
connections remain open. Headless runs must use less than 0.35 CPU seconds per
one-second interval; the local run used 0.01 seconds in each idle scenario.

The complete native Windows build, pinned FFmpeg 8.1 linking, media checks,
conversion performance gate and packaged DLL startup already passed in the
[preceding CI run](https://github.com/geniucker-dev/AirPlay-Windows/actions/runs/37124317005).
Each subsequent run validates its own reconnect changes and saves its metrics
in the validation artifacts.

## Still pending

- Real iPhone discovery/pairing, mirroring/audio and reconnect acceptance on
  Windows, including firewall, hotspot and USB network configurations.
- Native tray hide/restore/quit, startup settings, Unicode folders and monitor
  DPI changes on Windows.
- Driver-dependent NVDEC/D3D11VA and GPU recording on actual NVIDIA/Intel/AMD
  hardware, including initialization failure and orientation changes.
- The full paired performance gate: actual newly presented FPS, p95 latency,
  CPU, memory and at least 30 minutes of continuous audio/video per principal
  mode. No claim that performance is identical is made before this passes.

The original C++ release workflow remains available. Rust packages are marked
experimental and the dedicated Rust release workflow always creates prereleases
until hardware evidence is reviewed.
