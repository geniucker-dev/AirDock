# Migration delivery

Implementation branch: `feat/rust-iced-wgpu`. The C++ and Rust/Slint reference
commits are frozen in [acceptance](RUST_ACCEPTANCE.md). Their history is preserved.
Old/new character diagrams and ownership are in [architecture](RUST_ARCHITECTURE.md).

## Changes

- Replaced Slint/SDL desktop rendering with Iced and one shared wgpu compositor.
  NV12/YUV420P planes upload to reusable textures; a custom primitive draws into
  Iced's current render pass. Stale frames are replaced immediately before upload.
- Added receiver/settings pages with inline reception diagnostics, fullscreen/focus, Windows tray
  restoration, startup/device/GPU options, cover art and compatible configuration.
- Added validated settings drafts/save feedback, independently saved audio output
  and geometry, close-to-tray policy and automatically hidden mouse playback controls.
- Split runtime, sessions, status, settings, platform services, audio and playback.
  Window close does not stop receiving; Quit cancels and joins workers.
- Replaced SDL audio with cpal/WASAPI and a preallocated, generation-tagged SPSC
  ring. Decode/resampling/conversion remain outside the callback. Added device
  reconstruction, underflow handling, audio-clock scheduling and mirror drift servo.
- Added bounded independent HLS queues, timestamp scheduling, pause/seek and EOF
  draining. The latest-frame slot serves only the final display handoff.
- Removed recording UI/configuration/workers/MP4 writers and encoder dependencies.
  Legacy recording fields are ignored, and existing files are untouched.
- Added pinned playback-only LGPL FFmpeg DLLs, early audited native caching,
  recursive runtime packaging, exact corresponding sources and verified notices.
  Debug/release use the same native release DLL profile. Release builds this Rust
  branch and requires Linux/Windows regression gates.

## Executed evidence

The first complete automatic run is
[37176872603](https://github.com/geniucker-dev/AirPlay-Windows/actions/runs/37176872603),
on `1ac2d47`. Final artifacts from subsequent successful runs are authoritative.

| Check | Result |
|---|---|
| Format / Clippy | Passed, warnings denied |
| Unit regressions | 38 passed on Linux and Windows debug/release |
| Production shader | Initial 18,816 SDR comparisons passed on Vulkan and DX12 software; current oracle adds 96,800 8/10-bit/HDR checks |
| Encrypted mirroring | 26 protocol checks, 120 decoded frames, 64 audio packets |
| Reconnect / replacement / FLUSH | 13 scenarios, 390 decoded frames |
| HLS TS and fMP4 | 30 decoded frames and 46,080 PCM samples/channel each |
| CPAL conversion | Virtual ALSA 48 kHz stereo; 44.1→48 kHz conversion and draining passed |
| Desktop lifecycle | Settings/hidden stop uploads; restore, fullscreen, resize, config upgrade and clean shutdown passed under Xvfb |
| GPU reconstruction | Synthetic device destruction with continued encrypted playback passed |
| Windows package | Actual loaded DLL license/version/configuration/import audit and clean-PATH packaged reconnect/fMP4 passed |

The local final normal GUI fixture submitted 120/120 unique video frames. The
synthetic device-loss fixture recovered and submitted 118/120; dropped frames
during reconstruction are counted. These are software-renderer submission counts,
not physical display FPS or C++/Slint performance parity measurements.

## Receiver UI and latency follow-up

The desktop now uses a compact top navigation, a black playback stage, Manrope
Regular/SemiBold and a quiet steel-blue shell. Receive includes live new-submission
FPS, internal processing time, bandwidth and skipped frames. Video, timing, audio
and transport diagnostics expand in the same page. Settings adapts from two
columns to one; video-only fullscreen and automatically hidden controls remain.
Layout/styles are separate from desktop lifecycle. No idle animation was added.

Mirror frames no longer wait behind a stalled or buffered audio clock. HLS retains
PTS scheduling. Status clones only on revision changes, the Receive tree is cached
between changes, and GPU prepare reads the latest display mailbox. VSync settings
now reach Iced, preferring Mailbox when supported. Balanced GPU selection follows
the monitor's adapter; hidden-start decoding obtains a matching adapter up front.
The explicit low-power/high-performance overrides remain available.

Project-owned Rust source/tools now carry MPL-2.0; upstream MIT/OFL/LGPL and
historical GPL material retain their notices. Binary packaging checks exact source
and payload license consistency, including the embedded Manrope font's OFL.

Local follow-up: 51 unit regressions, Clippy, 115,616 GPU colour comparisons
(maximum 1/255), encrypted mirroring, reconnect/FLUSH, fMP4 HLS and 11 desktop
scenes passed. Audio-selection/draft isolation, invalid setting rejection, window
geometry restart and fullscreen mute/auto-hide passed on virtual ALSA/Xvfb.
The 30-frame receive fixture submitted all frames; 32 video events produced only
13 receiver-tree rebuilds. This is software GPU evidence, not Windows hardware
latency/FPS parity. Hidden/settings presentation and video-only reveal were also
checked; physical tray, hybrid GPU and WASAPI acceptance remain pending.

## Distribution and limits

Download `airplay-windows-rust-x64-<commit>` from the final successful CI run.
It includes the executable, exact runtime DLL closure, `NATIVE_AUDIT.json`, Rust
notices, patched FFmpeg source/build materials, exact application source and docs.
Validation reports are separate Windows/Linux artifacts. CI artifact retention is
14 days. The additional `airplay-windows-rust-distributions-x64-<commit>`
artifact contains an unsigned per-user installer, portable ZIP and checksums.
The experimental release workflow publishes these exact files when invoked.
Installer CI verifies install/upgrade, obsolete payload removal, installed playback,
uninstall and preservation of configuration/identity/untracked user files.

No Windows/iPhone pair, physical speaker or hardware GPU is attached here.
Real mirroring/HLS interoperability, WASAPI default-device switching/hotplug,
speaker/display AV offset and long-term drift, Windows tray/DPI/autostart, real
driver reset/hybrid-GPU power, HTTPS origins and same-device performance against
both baselines remain **pending**. Detailed gates are in RUST_ACCEPTANCE.md.

Video needs a compatible wgpu adapter; software UI fallback clearly reports its
video limitation. P010/YUV420P10 and PQ/HLG are supported through GPU HDR-to-SDR
mapping. Native HDR output, dynamic HDR metadata and BT.2020 CL remain unsupported.
Hardware frames still download to software YUV before plane upload; this is not
zero-copy. Project-owned Rust code is MPL-2.0; third-party and historical GPL
reference licenses remain distinct (LICENSES.md / RUST_SOURCE_ORIGIN.md). FairPlay authorization is outside
this migration's scope.
