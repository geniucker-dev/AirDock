# Receiver acceptance

## Automatic gates

- Rust format, warning-free Clippy, unit regressions and debug/release builds.
- Independent protocol vectors, encrypted H.264/HEVC streams, resolution changes,
  AAC-ELD/ALAC packets, RTP loss/reordering/wrap, FLUSH and repeated reconnects.
- HLS TS and fMP4 proxy/demux/decode, cancellation and audio/video sample counts.
- Production WGSL shader readback for NV12/YUV420P, six SDR matrices,
  limited/full range and linear/sRGB render targets. RGB error at most 2/255.
- P010/YUV420P10 limited/full range, PQ/HLG transfer and HDR-to-SDR/gamut mapping,
  metadata fallback, static peak changes and 8/10-bit/HDR stream transitions.
  Invalid stride/buffer extents and unsupported 12-bit/non-420/BT.2020 CL are rejected.
- Audio callback allocation guard, negotiated sample conversion, ring publication,
  generation isolation, pause and starvation; legacy configuration compatibility.
- Iced pages, fullscreen/focus restoration, resizing and settings save under Xvfb.
- Locked dependency audit and actual packaged Windows DLL license/configuration,
  decoder/demuxer/protocol availability, recursive normal/delayed PE imports,
  hashes, corresponding patched FFmpeg source and build materials.
- Playback tests using only the packaged DLL directory and Windows system paths.

CI uses deterministic null audio where explicitly configured. It verifies decoded
PCM and callback logic, not physical WASAPI output, hotplug or audible synchronization.
Software Vulkan shader readback verifies arithmetic, not hardware playback performance.

## Same-device physical comparison

Test the current and two reference builds on the same Windows PC, monitor, iPhone, network,
resolution, codec, volume and decoder preference. Record GPU/driver, power mode,
Windows/iOS versions and exact binary hashes. Use at least five alternating runs
per case after identical warm-up. Include 1080p/1440p/4K and 60/120 Hz where the
source/device support them, H.264/HEVC, hardware/software decode, LAN/hotspot,
windowed/fullscreen and tray restore. Distinguish playing, idle and hidden states.

Capture actual unique displayed frames and dropped source frames independently
(e.g. synchronized frame-number test patterns and high-speed capture); distinguish
ETW/PresentMon present events from new video frames and from physical scanout.
Collect P95/P99 new-frame intervals, local processing delay, observed end-to-end
AV offset and 30-minute drift, CPU/GPU load, private memory and dedicated/shared
GPU memory. `present()` return duration is never an end-to-end latency measurement.

The current build must retain at least 95% of each reference's median new-frame rate,
with no recurring stall regression; P95/P99 intervals and processing latency must
not worsen by more than one display period. Investigate CPU/GPU/power or memory
regressions above 10%, rather than accepting them on average FPS alone. State
absolute AV offset/drift and test error bounds; do not infer synchronization from
shared timestamps. Set any device-specific AV tolerance before taking measurements.

Run continuous playback for at least 30 minutes per principal mode, repeat
reconnects/orientation/FLUSH, inject loss/jitter, pause/seek HLS, switch and unplug
speakers/default devices, move between DPI scales, hide/restore the tray and force
a GPU reset. Window close/minimize must keep receiving; Quit must release sockets,
workers, CPAL streams and GPU resources. Check Windows autostart and Unicode paths.

These physical gates stay **pending** until measured. CI packages are experimental
until functionality and reference performance comparisons pass. Use
`tools/rust_performance.py` to validate supplied measurement sets; it does not
create missing measurements or substitute internal submissions for display frames.
