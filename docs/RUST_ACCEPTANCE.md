# Rust / Slint acceptance

Reference: C++ commit `562120e4a6c85da1ec33bca88a6f438db91b8545`.
The Rust branch is accepted only when functionality and performance match the
reference. Compilation or a successful connection alone is not acceptance.

## Required functionality

- Native Windows discovery, persistent identity, pair-setup/pair-verify and
  FairPlay key recovery; IPv4, IPv6 and Windows Mobile Hotspot connectivity.
- H.264 / HEVC mirroring, orientation and resolution changes, software decode
  and hardware decode with fallback; all supported range/matrix combinations.
- AAC-ELD 480/512-frame variants and ALAC, loss recovery, reordering, sequence
  wrap, FLUSH fencing, volume, pause/resume, cover art and track metadata.
- Synchronized MP4 video/audio recording, H.264/HEVC, automatic/GPU/CPU encoder
  selection, bitrate and output folder, with bounded recording queues.
- Fullscreen, aspect-preserving resizing, UI hiding, keyboard shortcuts,
  disconnect, persistent settings, Unicode paths and high-DPI behavior.
- Close/minimize to tray, restore, tray recording/disconnect/quit commands,
  optional start with Windows and start hidden. Closing to tray keeps sessions
  alive; Quit flushes recordings and stops networking.
- USB arrival/removal information and the existing Personal Hotspot path.
- Existing experimental HLS behavior remains opt-in; unsupported DRM playback
  is not advertised as supported.
- Windows debug/release CI, core/integration tests, protocol conformance,
  dependency-complete downloadable builds and a separate Rust release workflow.

## Automated gates

Use independent inputs and the C++ reference, not only Rust round trips:
pair-verify Python harness, FairPlay differential vectors, actual AAC/ALAC
packets, captured/generated H.264/HEVC, RTP loss/wrap/FLUSH cases, malformed
request bounds, recordings read back with ffprobe, and reference YUV values.
Maximum RGB error against the established reference must stay below 3/255.
Benchmarks report actual decoded/presented new frames, queue sizes and latency;
UI redraw frequency is not reported as video frame rate.

## Windows / iPhone performance gate

Run both release builds on the same PC, display, iPhone, network and settings.
Alternate builds, at least five paired runs, after equal warm-up. Include
1080p, 1440p and 4K where the phone supports them; 60/120 Hz where supported;
H.264/HEVC; software/hardware decode; Wi-Fi/hotspot/USB network; recording
on/off; fullscreen and tray restore. Record the exact hardware and OS versions.

- Actual new-frame presentation rate: Rust median at least 95% of reference;
  no new recurring stalls. Investigate differences within measurement noise.
- Local pipeline latency: Rust p95 no more than one display refresh interval
  above reference; report absolute and paired differences.
- CPU and steady-state memory: no unexplained regression above 10%.
- At least 30 minutes of continuous video plus audio per principal mode,
  including imposed loss/jitter, pause/resume, repeated reconnects, rollover,
  orientation changes and tray hide/restore; no new audio interruptions.
- Color test patterns and real image comparisons pass the automated error
  bound; recording duration, audio/video sync and timestamps remain correct.

Hardware-dependent gates must be recorded as **pending** until measured on real
Windows/iPhone hardware. CI artifacts are experimental until those gates pass.
