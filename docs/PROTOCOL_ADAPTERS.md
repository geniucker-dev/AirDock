# Protocol adapter preparation

AirPlay is the only production adapter in this change. Google Cast Streaming and
Miracast are design targets, not advertised or implemented features. An Android
sender is not a protocol identifier: different Android devices expose different
casting protocols.

## Requirements checked before extraction

| Protocol / path | Adapter responsibilities | Shared playback boundary |
| --- | --- | --- |
| AirPlay mirror and RAOP | Pairing, TXT records, mirror framing/decryption, RTP recovery, source clock synchronization, SETUP/FLUSH/TEARDOWN | Decoded YUV and processed PCM, media generation, `Live` video policy; explicit mirror/RAOP clock relationship |
| AirPlay HLS | AirPlay reverse HTTP requests, playlist proxy, transport-specific playback controls | Existing bounded demux/decode queues, rational PTS, `Timed` video policy, audible audio master |
| Google Cast Streaming (future) | Cast discovery, TLS/device authentication, application/session control, OFFER/ANSWER, encrypted media and RTP/RTCP reconstruction | Negotiated codec decoding and normalized presentation timestamps; select live or timed policy explicitly |
| Miracast (future) | Platform Wi-Fi Direct establishment and teardown, WFD/RTSP negotiation, RTP reconstruction and MPEG-TS demux | Elementary stream decoding and normalized timestamps; low-latency scheduling policy verified against sender behavior |

Wi-Fi Direct setup is not a requirement of the common receiver API. A Miracast
adapter will request OS-specific connectivity services; infrastructure transport
(MS-MICE) is a separate capability, not something every Android source supports.
Discovery record formats belong to each adapter, while the DNS/Wi-Fi facilities
used to publish/discover them may be platform services.

Screen mirroring and handing a media URL to a player are different Cast paths.
A Cast flinging receiver is not evidence of screen-mirroring compatibility.
Likewise, a Cast sender library is not a desktop receiver implementation. Native
Android certificate acceptance must be verified before claiming interoperability.

## Implemented boundaries

`receiver::ReceiverBackend` contains only lifecycle/control operations: protocol
identity, settings refresh, disconnect notification, timestamp-playback activity
and idempotent shutdown. `receiver::Backends` runs these operations across
registered adapters, contains refresh failures to the affected adapter, rejects
duplicate registrations and shuts down in reverse registration order. Runtime
registers AirPlay and keeps adapters alive independently of presentation windows.
Adding a protocol is a compile-time registration change, not a dynamic plugin ABI.
Thread-affine OS objects belong on adapter/platform workers; the `Send` backend
handle is the control surface and need not contain those objects directly.

`receiver::airplay::AirPlay` owns the existing listener/discovery objects and HLS
activity query. AirPlay transport workers now live under this adapter rather than
under common media. AirPlay wire parsing, cryptography and discovery modules
remain protocol-specific; they are not interfaces another adapter should reuse.
The listener's own cancellation flag allows stopping this adapter without setting
the whole receiver's running flag. Discovery is withdrawn before listener and
session teardown. Auxiliary AirPlay controls cannot pause another protocol's
playback. Auxiliary control checks and mutations run under the ownership gate,
so another adapter cannot claim the output in between.

`session::Sessions` allocates IDs across all adapters and arbitrates one active
sender for the shared audio/video outputs. Wire IDs are adapter-local metadata.
A lease records protocol, peer and cancellation state. Same-peer replacement is
an explicit adapter policy and cannot replace a different protocol's lease. A
replacement waits for the previous workers and playback cleanup; timeout means
busy, not concurrent playback. Stale teardown cannot clear another lease or its
video activity. Global disconnect/exit cancels the owner and invalidates media.
Concurrent senders/mixing are outside this preparation.

Ownership grants and auxiliary mutations use the ownership gate. Read-only
protocol status uses a separate short snapshot lock; the mirror worker's
per-packet ownership check reads an atomic session ID. These observations remain
published until cleanup completes and never authorize a claim or a control.
Slow control processing must not block UI observations or mirror reception.

The media handoff retains `VideoFrame`'s owned FFmpeg reference, rational PTS,
receive instant and generation. `PlaybackMode::Live` releases decoded video
immediately; `Timed` waits for PTS against audible PCM or a monotonic anchor.
`ClockRelation` states whether audio/video share a timeline, need a known epoch
alignment, or remain unmapped. Unknown clock relationships yield no AV estimate.
AirPlay alone supplies its Unix/NTP alignment rule and translates wire codec IDs
into the common audio decoder configuration. RAOP synchronization state is
local to its audio worker and invalidated by media generation, rather than stored
in common playback. New adapters must normalize wrapping RTP clocks before this
handoff and supply codec configuration to appropriate decoders. New codecs,
TS transport ingestion and platform connectivity still require implementation.

No per-frame adapter dispatch, extra queue, image conversion, GPU allocation or
audio callback work is introduced. Video retains the bounded scheduler followed
by the final latest-frame mailbox; audio retains its existing actor and ring.
Adapter settings refresh must not restart live media for unrelated UI preferences.

## Reference material

- [Google Open Screen / libcast](https://chromium.googlesource.com/openscreen/+/refs/heads/main/cast/README.md)
  and its [Streaming Session Protocol](https://chromium.googlesource.com/openscreen/+/refs/heads/main/cast/protocol/streaming_session_protocol.md):
  receiver negotiation, media streaming and presentation/control separation.
- [Chromium mirroring tests](https://chromium.googlesource.com/chromium/src/+/HEAD/components/mirroring/service/README.md):
  standalone receiver testing requires explicitly trusting a developer certificate.
- [MiracleCast](https://github.com/albfan/miraclecast), especially
  [sink control](https://github.com/albfan/miraclecast/blob/master/src/ctl/ctl-sink.c):
  separates Wi-Fi management from RTSP/media negotiation; platform setup is not
  portable application playback code.
- [Lazycast](https://github.com/homeworkc/lazycast) and
  [its RTSP controller](https://github.com/homeworkc/lazycast/blob/master/d2.py):
  a second sink reference, with distinct infrastructure and Android compatibility
  considerations. Its legacy Pi players are not a performance baseline for AirDock.
- [GNOME Network Displays](https://github.com/GNOME/gnome-network-displays):
  useful platform/negotiation reference, but its display-source path is not proof
  of sink support.
- [openchromecast](https://github.com/yukarikaname/openchromecast): explicitly a
  URL/flinging receiver; its advertised Cast support is not a mirroring solution.
- [UxPlay](https://github.com/FDH2/UxPlay), including its mirror worker shutdown,
  and [Shairport Sync](https://github.com/mikebrady/shairport-sync), including its
  RTP timestamp and FLUSH handling: references for keeping presentation lifetime,
  worker retirement and source-clock invalidation distinct. Their scheduling
  policies are not copied indiscriminately into interactive mirroring.

## Validation scope

Automated regressions cover multi-adapter lifecycle/failure isolation, duplicate
registration, cross-protocol busy handling, same-peer reconnect, worker retirement,
stale teardown, local AirPlay shutdown and explicit clock mapping. Existing
encrypted mirror, RAOP recovery/FLUSH, reconnect, HLS TS/fMP4, rendering/color and
desktop lifecycle checks remain applicable and run in existing CI jobs.
Windows/iPhone/Android hardware compatibility and actual display latency/FPS
remain physical-device acceptance work; synthetic protocol and software-GPU tests
must not be reported as that evidence.
