# Receiver architecture

The receiver is owned by `runtime::Runtime`, outside the Iced daemon. Closing a
presentation window does not stop the listener. Only explicit Quit / runtime
cancellation shuts down discovery, sessions, audio actors and media scheduling.

## Runtime and media pipeline

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
frame; future frames are not presented early. Mirroring releases decoded frames immediately, without waiting for an audio-clock
horizon that can stall or lag behind live video. Source PTS remains telemetry;
HLS alone uses timestamp scheduling. The mirrored audio producer uses a slow occupancy
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

The production shader accepts 8-bit NV12/YUV420P/YUVJ420P and 10-bit
P010LE/YUV420P10LE. It
handles padded and signed strides, odd chroma sizes, explicit full/limited range,
BT.709, BT.601, FCC, SMPTE240M and BT.2020 NCL. Missing mirroring range retains
the prior full-range convention; HLS supplies limited range when unspecified.
Matrices are not guessed from resolution. R8/RG8 and R16Uint/RG16Uint plane
textures and bindings are reused. The integer shader masks unused bits before
bilinear scaling, without optional GPU format features. No per-frame CPU RGBA
conversion or image handle is created.

PQ/HLG transfer functions, static HDR peak metadata, a BT.2390 highlight shoulder
and BT.2020/P3 to BT.709 gamut conversion map HDR to the SDR window. Cached LUTs
change only when colour metadata changes; the SDR path skips them. Native Windows
HDR output and dynamic HDR metadata processing are outside this path. BT.2020 CL,
12-bit and non-420 formats remain explicitly rejected. See RUST_HDR.md. Native D3D11/CUDA
frames are downloaded to software YUV first. This is not zero-copy.

The owned compositor uses Iced's public Engine/Renderer interfaces. It chooses
compatible adapters with a balanced preference for the GPU driving the window's
monitor (integrated GPU as fallback), exposes an
explicit high-performance override, reuses the UI device/target, and attempts
bounded device reconstruction after device loss. Surface reconfiguration is
separate from device reconstruction. Hidden startup obtains the primary monitor's adapter before decoding starts.
D3D11VA attempts the DXGI adapter matching
the render PCI vendor/device identity; NVDEC/CPU remain decode fallbacks. This
is best-effort adapter matching at device creation, not a native GPU frame sharing
interface. Moving the window to another GPU's monitor does not migrate a live
device; restarting reselects the adapter. Software GPU adapters remain distinguishable
from hardware. If GPU initialization fails, the software UI remains available
with a clear video-support message; software UI does not secretly render the YUV
primitive. Driver failures and cross-adapter cost still require physical Windows
validation.

Minimized/hidden windows suppress continuing media/UI frame notifications and video uploads.
The first scheduled video after an empty mailbox sends one notification so a new
receiving session can reveal its player promptly. The reveal latch uses the
network session ID, not the media generation: manual hiding, FLUSH, seek and
format switches cannot repeatedly steal focus. Audio-only sessions never reveal.
One latest CPU frame is retained for restoration. Settings/platform status updates
are coalesced; the daemon's low-frequency housekeeping is not an animation loop.
Fullscreen removes navigation, title and control rows and uses the same video
Primitive over the entire surface. Errors appear as an overlay. Exiting returns
to the page from which fullscreen was entered; aspect ratio/crop remain explicit.
Mouse movement reveals a bounded floating playback bar, with mute, volume, fit/crop
and exit. Existing low-frequency housekeeping hides it after inactivity; no new
animation timer is added.

The desktop form holds saved settings, a separate draft and validation/save state.
Runtime serializes form commits, immediate audio and language choices,
and window preferences. Each merges unrelated values from the latest saved state,
persists before applying, and replies with success/error. Unsaved form edits cannot
be committed by resizing or selecting an audio device. Geometry changes are
debounced; exit flushes pending preferences. Close-to-tray policy is distinct
from minimize-to-tray; disabling close-to-tray minimizes to the taskbar while
receiving continues.

## Distribution

FFmpeg uses a pinned dynamic LGPL playback-only build, with corresponding
patched source and build material included in distributions. DLL configuration,
license, ABI, loaded path, codecs and dependency closure are checked after
packaging, including cache restores. Project and dependency licenses accompany
source and binary packages.

## Desktop presentation

The Receive page gives the video most of the window, with a compact top navigation
and a compact one-line live statistics strip. Detailed video, clock, audio and transport status
open over the video without shrinking its viewport. Settings uses one scrollable form, two columns on wide
windows and one on narrow windows. `desktop/view.rs` owns layout;
`desktop/appearance.rs` owns visual tokens/styles; `desktop/form.rs` owns drafts
and validation. The UI does not animate while idle. Status is cloned on revision
changes, the receiver layout is cached between changes, and the video primitive
consumes the newest display frame at prepare time. Fullscreen remains video only,
with mouse controls that disappear when unused. VSync prefers Mailbox when
supported, otherwise AutoVsync; disabling it requests AutoNoVsync.
