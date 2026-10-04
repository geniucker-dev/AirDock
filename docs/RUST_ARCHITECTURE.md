# Iced / wgpu / cpal migration

The receiver is owned by `runtime::Runtime`, outside the Iced daemon. Closing a
presentation window does not stop the listener. Only explicit Quit / runtime
cancellation shuts down discovery, sessions, audio actors and media scheduling.

## C++ comparison baseline 562120e4

```text
mDNS / RTSP sessions -> FFmpeg decoder -> SDL video renderer
                   |-> RTP audio decoder -> SDL output
                   `-> recording workers -> MP4
ImGui controls / settings / tray -> SDL window and application loop
```

This source remains in Git and the independent C++ checkout. The migration
compares against its actual release playback performance, not just the former
Rust CPU conversion path.

## Before (Rust / Slint baseline 43bcb0c)

```text
protocol + sessions + HLS -> Shared
                            |-- recorder queue -> encoders -> MP4
                            |-- latest video -> CPU swscale BGRA -> SDL texture
                            `-- PCM -> SDL queue
Slint software pixels -> SDL UI texture -> SDL window / main loop
Shared / desktop.rs own settings, lifecycle, UI, tray and playback together
```

## After

```text
Runtime -> Server / discovery / platform monitoring
              |              |
          protocol       Sessions (ownership, cancellation, generation)
              `------ media decode / demux ------'
                       |                 |
                audio processing    PTS video scheduler
                       |                 ^ audio playhead
                 preallocated ring ------'
                       |                 |
                 cpal / WASAPI     latest display mailbox
                                         |
Commands -> runtime      Status -> Iced daemon -> YUV Primitive::draw
                                                  |
                               one Iced GPU device and window target
```

The `Shared` type is a protocol-facing handle, not a desktop object. Settings,
`session::Sessions`, `status::Status`, `playback::Playback` and telemetry have
separate ownership. Command, status and media channels are separate. Device
objects are confined to their control/output threads; GPU objects to the
compositor/renderer. Status messages never contain video planes or PCM.

## Clocks and scheduling

Media timestamps retain rational timebases. Mirroring records preserve source
NTP timestamps through decoder packet PTS and frame PTS; RTP synchronization
records establish the audio source clock. A zero / absent timestamp remains
missing, not a timestamp at the start of the application.

HLS has bounded independent compressed video/audio queues and separate demux,
audio decode and video scheduling. The demuxer never sleeps until a video's
presentation deadline. When available, estimated audible PCM position is the
video master; otherwise playback uses a monotonic PTS anchor that stops during HLS pause. The display slot
contains only the newest frame released by scheduling; future frames remain in
a bounded scheduling queue. Already-due frames are coalesced to the newest due
frame; future frames are not presented early. Mirroring does not build a large
queue to chase a source clock more than 100 ms ahead. The mirrored audio producer uses a slow occupancy
servo limited to 300 ppm, outside the real-time callback. HLS does not adjust
rate based on how quickly its network source downloads. The clock clamps to
the last submitted PCM horizon during underflow. Natural HLS EOF drains both
resampler/audio output and the video scheduling queue; cancellation/seek/FLUSH
remain immediate generation changes.

Reconnect, stream replacement, seek and FLUSH advance the media generation.
Each packet/frame/PCM contribution carries its producer's generation. Audio
control messages have priority over the bounded PCM channel; FLUSH rebuilds the
output stream and ring on the control thread. Producer/consumer ring indices are
never reset concurrently. A default-device change or device error triggers
stream reconstruction and format renegotiation without tearing down receiving.

The application callback copies already converted samples, supplies equilibrium
silence on underflow, and updates atomic clock/counter snapshots. It contains no
allocation, mutex, logging, decoding or format conversion. CPAL timestamps are
predictions of output delivery, not a measurement of the final speaker path.

## Video and GPU support

The production shader accepts validated 8-bit NV12 and YUV420P/YUVJ420P. It
handles padded and signed strides, odd chroma sizes, explicit full/limited range,
BT.709, BT.601, FCC, SMPTE240M and BT.2020 NCL. Missing mirroring range retains
the prior full-range convention; HLS supplies limited range when unspecified.
Matrices are not guessed from resolution. R8 / RG8 plane textures and bindings
are reused; no per-frame CPU RGBA conversion or image handle is created.

P010 / other 10-bit formats, BT.2020 CL and PQ/HLG are explicitly rejected with
an error rather than silently shown with an 8-bit or SDR matrix. This is an
intentional first-version support boundary, not HDR support. Native D3D11/CUDA
frames are downloaded to software YUV first. This is not zero-copy.

The owned compositor uses Iced's public Engine/Renderer interfaces. It chooses
compatible adapters with a balanced integrated-GPU preference, exposes an
explicit high-performance override, reuses the UI device/target, and attempts
bounded device reconstruction after device loss. Surface reconfiguration is
separate from device reconstruction. D3D11VA attempts the DXGI adapter matching
the render PCI vendor/device identity; NVDEC/CPU remain decode fallbacks. This
is best-effort adapter matching, not a native GPU frame sharing interface. Software GPU adapters remain distinguishable
from hardware. If GPU initialization fails, the software UI remains available
with a clear video-support message; software UI does not secretly render the YUV
primitive. Driver failures and cross-adapter cost still require physical Windows
validation.

Minimized/hidden windows suppress media/UI frame notifications and video uploads.
One latest CPU frame is retained for restoration. Settings/platform status updates
are coalesced; the daemon's low-frequency housekeeping is not an animation loop.

## Scope and licensing

Recording, encoders, MP4 output, SDL and Slint are removed. The legacy C++
application is preserved in Git at baseline `562120e4`, not built by this branch.
The former Rust/Slint baseline remains at `43bcb0c`. FFmpeg uses a pinned dynamic
LGPL playback-only overlay, with exact patched sources included in distributions.
DLL configuration, license, ABI, loaded path, codecs and dependency closure are
checked again after packaging, including cache restores.

The project's own license is unchanged. Replacing dependencies does not establish
that existing Rust protocol implementation is independent of GPL C++ sources;
that source relationship must be audited separately before any relicensing.
