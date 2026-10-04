# AirDock

<img src="rust/assets/icons/airdock-128.png" width="64" height="64" alt="AirDock icon">

A Rust AirPlay receiver with an Iced desktop interface, GPU YUV video rendering
and cpal/WASAPI audio. Receiving continues while its presentation window is
minimized, closed or hidden in the Windows tray. Explicit Quit shuts down the
service and its media workers.

![Iced receiver](docs/validation/receiver.png)

## Features

- AirPlay pairing, encrypted mirroring, RTP audio recovery and session
  replacement behavior, with generation fencing across reconnects and FLUSH.
- H.264 / HEVC decoding, including D3D11VA and NVDEC fallback paths.
- Reusable NV12 / YUV420P plane textures, GPU colour conversion, aspect fit/crop
  and full-screen presentation. No CPU YUV-to-RGBA video conversion.
- P010 / YUV420P10 Main10 input, PQ/HLG HDR to SDR tone mapping and BT.2020/P3
  gamut conversion. HDR playback uses the same shared GPU render pass.
- cpal audio output with negotiated device formats, an allocation-free application
  callback, underflow recovery and default/selected-device reconstruction.
- Separate HLS demux/audio/video scheduling, TS/fMP4 and FCUP resource delivery.
- Receiver/settings pages with inline playback diagnostics, live frame rate,
  processing time, bandwidth, cover art, volume, Windows tray restoration, startup
  options and persistent settings.

Video requires a compatible GPU backend. Software UI fallback supports controls
and audio but displays an explicit video limitation. 10-bit PQ/HLG video is
mapped to the current SDR window; native Windows HDR output and Dolby Vision /
HDR10+ dynamic metadata are not implemented. BT.2020 CL, 12-bit and non-420
formats remain unsupported. See [HDR rendering](docs/RUST_HDR.md). Hardware decode
still downloads to software YUV before upload: **this is not zero-copy**.

## Run

```text
airdock.exe
airdock.exe --hwaccel
airdock.exe --start-hidden
airdock.exe --headless --port 7000
```

Use the same LAN or Windows mobile hotspot, then select the receiver in your
iPhone/iPad's Screen Mirroring menu. HLS/FCUP playback can be enabled in settings.
Settings and pairing identity are stored in AirDock’s own application configuration directory.

Double-click the video area or press F11 to show video alone in fullscreen.
Double-click again or press Escape to return to the previous page.
Moving the mouse shows playback controls, which hide after inactivity unless hovered.
Video keeps its aspect ratio unless Fill/crop was selected. Ctrl+H hides/restores
controls, Ctrl+D disconnects and Ctrl+Q quits. The Windows tray restores/hides the presentation window,
disconnects the sender, or quits the whole application. Closing the window does
not mean quitting the receiver. With Close window to tray disabled, closing
minimizes to the taskbar instead. Normal window size and true fullscreen mode save
automatically and restore separately; oversized old bounds are limited to the
Windows logical work area.
English and Simplified Chinese are available in Settings → Language, with system
language detection by default. Language and audio-device selection apply and save
immediately without committing other unsaved edits; other settings use Save,
with field validation and explicit unsaved/saved feedback.
New video sessions automatically show a hidden/minimized player. Audio-only
connections remain in the tray. Hiding an ongoing session is respected until a
new connection; FLUSH and format changes do not repeatedly reopen the window.

## Build, verification and delivery

See [build instructions](docs/RUST_BUILD.md), [architecture](docs/RUST_ARCHITECTURE.md),
[validation status](docs/RUST_VALIDATION.md) and [hardware acceptance](docs/RUST_ACCEPTANCE.md).
The Windows package uses audited LGPL FFmpeg DLLs, includes corresponding patched
source/build material and required notices, and is tested again with a clean PATH.
CI and manual release dispatch build the checked-out application. Both
provide a per-user Windows installer (`*-setup.exe`), the portable ZIP
(`*-portable.zip`) and `SHA256SUMS.txt`. The installer needs no administrator
rights; upgrades and uninstall preserve configuration, pairing identity and
untracked user files. The current installer is unsigned.

GPU readback/protocol tests do not establish real Windows/iPhone FPS, latency,
power, device hotplug or long-term AV drift. Those measurements must compare
reference builds on identical hardware.

## License

Project-owned receiver source and build tools are licensed under **MPL-2.0**.
Modified MPL-covered files must remain available as source when distributed.
Dependency licenses, required notices and source/build materials accompany
releases. See [LICENSES.md](LICENSES.md) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Credits

Thanks to the authors and contributors of:

- [AirPlay-Windows](https://github.com/moieric11/AirPlay-Windows)
- [UxPlay](https://github.com/FDH2/UxPlay)
- [RPiPlay](https://github.com/FD-/RPiPlay)
- [shairport-sync](https://github.com/mikebrady/shairport-sync)
- [OpenAirPlay / airplay-spec](https://github.com/openairplay/airplay-spec)
- [airfry](https://github.com/nored/airfry) — nored
- [doubletake](https://github.com/omarroth/doubletake) — omarroth and contributors
- [Iced](https://github.com/iced-rs/iced)
- [wgpu](https://github.com/gfx-rs/wgpu)
- [cpal](https://github.com/RustAudio/cpal)
- [FFmpeg](https://ffmpeg.org/)
- [vcpkg](https://github.com/microsoft/vcpkg)
- [Manrope](https://github.com/sharanda/manrope)
- [Noto Sans SC](https://github.com/google/fonts/tree/main/ofl/notosanssc)
- [Inno Setup](https://jrsoftware.org/isinfo.php)
- [CairoSVG](https://cairosvg.org/), [Pillow](https://python-pillow.github.io/) and
  [fontTools](https://github.com/fonttools/fonttools) for reproducible design assets

Dependency licenses and required notices accompany distributions; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and [LICENSES.md](LICENSES.md).
