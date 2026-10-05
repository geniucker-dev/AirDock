# Receiver architecture

The receiver is owned by `runtime::Runtime`, outside the Iced daemon. Closing a
presentation window does not stop the listener. Only explicit Quit / runtime
cancellation shuts down discovery, sessions, audio actors and media scheduling.

## Runtime and media pipeline

```text
                      Runtime (outside windows)
                       |                    |
               Backends / lifecycle   Platform services
                       |
  AirPlay adapter -----+----- Cast / Miracast adapters (future)
  discovery, auth, transport   discovery, auth, transport
                       |
          Sessions (one owner, IDs, cancellation, generation)
                       |
               Media decode / demux
                 |             |
          audio processing  Live / Timed video scheduler
                 |             ^ audible audio clock
         preallocated ring ----'
                 |             |
           cpal / WASAPI   latest display mailbox
                               |
 Commands -> Runtime    Status -> Iced -> YUV Primitive::draw
                               |
                  shared UI/video GPU device and target
```

The `Shared` type is a protocol-facing handle, not a desktop object. Settings,
`session::Sessions`, `status::Status`, `playback::Playback` and telemetry have
separate ownership. Command, status and media channels are separate. Device
objects are confined to their control/output threads; GPU objects to the
compositor/renderer. Status messages never contain video planes or PCM.

`receiver::ReceiverBackend` is a control/lifecycle interface, not a per-frame
queue. The AirPlay adapter owns discovery and listener teardown; stopping it does
not stop the common runtime or another protocol. Common sessions allocate unique
IDs and enforce a single playback owner. Same-peer reconnect applies only within
one protocol, and old media workers/cleanup complete before a replacement owns
the outputs. Media scheduling uses `Live`/`Timed` policy and explicit clock
relationships instead of a protocol-specific HLS flag. See
[Protocol adapter preparation](PROTOCOL_ADAPTERS.md) for extension requirements
and the protocol references checked before extracting these boundaries.

## Discovery and receiver identity

AirPlay TXT records and `/info` share a receiver UUID derived from the persisted
pairing public key. The display UUID has a separate domain; neither depends on
the receiver name, port or network adapter. Existing pairing keys remain intact.
Name/HLS/HEVC changes restart discovery. Unrelated preference saves keep active
advertisements, while a failed asynchronous Windows registration is retried.

Windows shutdown waits for registration completion, cancels pending operations
and waits for `DnsServiceDeRegister` acknowledgement. Successful registrations
carry a withdrawal lease through the completion channel, so late success after
cancellation or an abandoned receiver still initiates withdrawal. Request,
cancellation and TXT backing storage survive until their native callbacks.
Waits are bounded and failures are logged. Other platforms acknowledge service
withdrawal and daemon shutdown before leaving the receiver runtime.

This lifecycle cannot reliably remove historical records registered by another
application or cached by an iPhone. Verifying disappearance from a physical
device's picker requires Windows/iPhone network testing.

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
The current HLS path selects `Timed`; mirror selects `Live`. Future adapters choose
the policy and normalize their own source clocks explicitly. The mirrored audio producer uses a slow occupancy
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
receiving session can reveal its player promptly. A separate video presentation
token changes when video stops and restarts, even on the same control connection.
Manual hiding, FLUSH, seek and repeated SETUP preserve its reveal latch; audio-only
sessions never reveal. Windows opened automatically from the tray return after
video teardown and a short reconnect grace period. A replacement presentation
cancels that return; manual restoration or leaving fullscreen keeps the window.
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

## In-application updates

`update` owns stable-release metadata, daily check state, cancellation and
downloads; `desktop/updates.rs` bridges typed completion messages to UI state.
These jobs run outside media and presentation callbacks. Metadata and package
SHA-256 come from the official GitHub API. Download attempts use GitHub first,
then probe enabled mirrors only upon failure. No mirror supplies authoritative
metadata. Background checks prompt without starting installation.

User-confirmed Windows x64 updates download the matching installer or portable
ZIP and validate its exact size and hash. A standalone `airdock-updater.exe`,
with an independently audited runtime closure and no FFmpeg import, waits for
graceful receiver exit before modifying files. Portable updates validate all
manifest files, preserve untracked user files and roll back ordinary installation
failures; installed updates invoke the per-user installer in the same directory.
Configuration and pairing identity remain outside the payload. The helper
restarts the receiver with its existing arguments and reports failures on restart.

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
