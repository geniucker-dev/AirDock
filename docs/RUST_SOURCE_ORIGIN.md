# Rust source relationship review and license scope

Reviewed references: C++ `562120e4a6c85da1ec33bca88a6f438db91b8545`,
Rust/Slint `43bcb0ca8a7d659159290ff81bfa65ba3d627b9d`, Iced baseline
`cb11f48c29bb1993b0d5a84170f96053fe3d87f4`, and nored/airfry checkout
`4b0673041a3093105fc608d325ae508d45c25ef1`.

The copyright holder requested MPL-2.0 for project-owned Rust code. Git history
of the Rust production files identifies `geniucker-dev <me@geniucker.com>` as
the contributor. This grant applies to project-owned Rust work; it does not
relicense contributors' historical C++ work or upstream third-party material.
The old C++ GPL grant and attribution are retained, including the separate test
oracle. No C++ production source or GPL C implementation is linked into the Rust
receiver. FairPlay authorization-chain review is explicitly excluded by the owner;
PlayFair uses the MIT license as published by airfry, with its notices preserved.

## Reviewed relationships

- `crypto.rs`: RustCrypto, ed25519-dalek and x25519-dalek implement cryptographic
  primitives. Pairing transcript order, salt bytes and mirror derivation are
  interoperability requirements. The project implements framing/identity handling
  with Rust crate APIs; PlayFair code and protocol reply bytes are explicit exceptions.
- `protocol.rs` / `server.rs`: owned Rust request/parser/plist/session handlers.
  Route names, feature masks, plist keys and wire formats were checked against the
  C++ receiver. These shared protocol facts are not claimed as original inventions.
  The retained C++ protocol notes themselves are not relicensed.
- `rtp.rs` / `media/transport.rs`: Rust maps, polling, cancellation and generation
  ownership implement the tested ordering/loss/wrap/FLUSH contract. C++ recovery
  behavior was used for regression comparison, including the owner's audio fixes;
  the GPL C++ implementation remains a separate historical reference.
- `hls.rs`: Rust-owned proxy/cache/reverse-channel lifecycle uses reqwest/FFmpeg,
  HashMap/Condvar and bounded media scheduling. Condensed URL expansion and
  rendition selection were checked as protocol behavior; legacy C++/UxPlay source
  and documentation retain their original licenses.
- Media decoder calls and native adapter selection use public FFmpeg/DXGI APIs.
  Slice threading, LOW_DELAY and hardware frame-pool settings preserve the latency
  contract. Shared API identifiers/options do not relicense FFmpeg itself.
- Desktop, shared-device GPU primitive, color conversion, CPAL ring, scheduler,
  session/status/runtime separation and regression tools were authored in Rust
  for this receiver. Iced/wgpu/cpal retain their own dependency licenses.

The review identifies the third-party port and unchanged reply data separately;
MPL headers were added only to project-owned implementation files. Language
change alone was not used as the reason for relicensing. Third-party notices,
separate GPL reference license, exact application source archive and native LGPL
source/build material remain in distributions. CI checks metadata, source headers
and actual package/source license consistency.

## Production source inventory

| File | License | Source relationship |
|---|---|---|
| `rust/src/audio/mod.rs` | MPL-2.0 | Migration-authored lifecycle, scheduling, CPAL and metrics implementation |
| `rust/src/audio/ring.rs` | MPL-2.0 | Migration-authored lifecycle, scheduling, CPAL and metrics implementation |
| `rust/src/config.rs` | MPL-2.0 | Project-authored Rust UI, settings, discovery or platform integration |
| `rust/src/crypto.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/desktop/appearance.rs` | MPL-2.0 | Project-authored Iced visual tokens and styles |
| `rust/src/desktop/view.rs` | MPL-2.0 | Project-authored receiver and settings layout |
| `rust/src/desktop/form.rs` | MPL-2.0 | Project-authored Rust UI, settings, discovery or platform integration |
| `rust/src/desktop.rs` | MPL-2.0 | Project-authored Rust UI, settings, discovery or platform integration |
| `rust/src/discovery.rs` | MPL-2.0 | Project-authored Rust UI, settings, discovery or platform integration |
| `rust/src/i18n.rs` | MPL-2.0 | Project-authored locale selection and Chinese/English UI translations |
| `rust/src/hls.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/lib.rs` | MPL-2.0 | Project-authored Rust UI, settings, discovery or platform integration |
| `rust/src/main.rs` | MPL-2.0 | Project-authored Rust UI, settings, discovery or platform integration |
| `rust/src/media/audio.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/media/color.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/media/display.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/media/mod.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/media/transport.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/media/video.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/platform/mod.rs` | MPL-2.0 | Project-authored Rust UI, settings, discovery or platform integration |
| `rust/src/playback.rs` | MPL-2.0 | Migration-authored lifecycle, scheduling, CPAL and metrics implementation |
| `rust/src/playfair.rs` | MIT | nored/airfry MIT port; retained upstream license and attribution |
| `rust/src/protocol.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/render/compositor.rs` | MPL-2.0 | Migration-authored Iced/wgpu public API and color math |
| `rust/src/render/mod.rs` | MPL-2.0 | Migration-authored Iced/wgpu public API and color math |
| `rust/src/render/native.rs` | MPL-2.0 | Migration-authored Iced/wgpu public API and color math |
| `rust/src/render/sample_10bit.wgsl` | MPL-2.0 | Migration-authored Iced/wgpu public API and color math |
| `rust/src/render/sample_8bit.wgsl` | MPL-2.0 | Migration-authored Iced/wgpu public API and color math |
| `rust/src/render/yuv.wgsl` | MPL-2.0 | Migration-authored Iced/wgpu public API and color math |
| `rust/src/rtp.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/runtime.rs` | MPL-2.0 | Migration-authored lifecycle, scheduling, CPAL and metrics implementation |
| `rust/src/server.rs` | MPL-2.0 | Project-authored Rust protocol/media behavior; C++ contract comparison, Rust crate APIs |
| `rust/src/session.rs` | MPL-2.0 | Migration-authored lifecycle, scheduling, CPAL and metrics implementation |
| `rust/src/state.rs` | MPL-2.0 | Project-authored Rust UI, settings, discovery or platform integration |
| `rust/src/status.rs` | MPL-2.0 | Migration-authored lifecycle, scheduling, CPAL and metrics implementation |
| `rust/src/telemetry.rs` | MPL-2.0 | Migration-authored lifecycle, scheduling, CPAL and metrics implementation |

## Non-source exceptions

`rust/src/playfair_data/*` retains airfry's MIT notice and upstream attribution.
`rust/src/fairplay-replies.bin` is byte-identical historical protocol response
data; no new license is granted for those bytes. `third_party/playfair/*` and
`rust/tests/playfair_reference.c` remain separate GPL test inputs. The Windows
runtime dependency closure is audited from actual PE imports, not inferred from
the presence/absence of files in a source archive. Native vcpkg/FFmpeg patches,
Inno Setup, fixture notices and historical tools/docs retain their notices as
listed in LICENSES.md. This review does not claim a new grant for third-party
sources or protocol data.

Manrope fonts in `rust/assets/fonts/` retain SIL OFL 1.1. The original Google
Fonts variable face and deterministic static-face reproduction tool are included;
these third-party font files are not relicensed MPL.

The AirPlay UI CJK fonts are renamed static Noto Sans SC subsets under SIL OFL
1.1. Reproduction code records the upstream input SHA256; all translated UI
characters and the common GB2312 set are retained. The font license accompanies
the source and binary packages.
