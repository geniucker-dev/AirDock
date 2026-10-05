# Release performance comparison

Measured on 2026-10-05 (Asia/Taipei). The latest stable release was [v0.1.2](https://github.com/geniucker-dev/AirDock/releases/tag/v0.1.2),
source commit `216443d2745515ce67fe173db7ee6b96f1beb5cf`.

The comparison found no material loss of new GPU submission throughput in this
software environment. Some HEVC tail timings were higher; these are retained
below, not interpreted as proof of identical hardware performance. An independent
concurrency regression exposed a UI-blocking status lookup introduced by the
control gate. That lookup was fixed and the final candidate was measured again.
Physical Windows/iPhone performance acceptance remains **pending**.

## Method and scope

- Rebuilt the release source and current receiver with Rust 1.99.0 and the same
  locked dependencies, native libraries and unmodified release profile:
  `cargo build --release --locked --bin airdock`, thin LTO, one codegen unit.
  These are optimized Linux source builds, not the official Windows executables.
- Linux/Xvfb, Openbox, Vulkan llvmpipe 19.1.7; `LP_NUM_THREADS=2`, four-core CPU
  quota, 16 GiB memory quota. Both binaries used the same driver, 1000 x 760
  window, disabled VSync, software decoding and explicit null audio.
- Animated 1920 x 1080 H.264/HEVC 60 fps fixtures, no reordered frames, sent over
  the encrypted AirPlay wire path. AAC-ELD packets accompanied mirror playback.
  HLS used 1080p30 fMP4, AAC, the real FCUP `/reverse` and `/action` proxy path,
  demuxing, decoding and timestamp scheduling.
- Alternated release/current execution order by paired round. CPU/RSS sampling
  excludes process startup and video warmup. Frame counters and per-run timing
  percentiles include the video warmup and final drain; they are not steady-state
  percentiles obtained by resetting counters. Sender scheduling P99 lateness and
  per-second process resource samples are included in the evidence.
- CPU is receiver process time, including software GPU workers; 100% means one
  occupied core. The sender is excluded. Memory is private resident memory from
  `smaps_rollup`, not Windows private committed bytes or dedicated GPU memory.
- Only newly submitted video frames count. Repeated UI redraws do not count as
  new frames. Submission time is not physical scanout time or display latency.

## Initial comparison: protocol-adapter receiver

Candidate: `f19721811be4c10925ed82f455705c93a9577021`.
Five paired runs per scenario: 30 seconds measured playback plus 3 seconds video
warmup; idle and hidden playback each measured for 15 seconds. Mirror scenarios
therefore received 9,900 frames per version. All input mirror frames decoded.

| Scenario / version | Submitted (total) | Receive to submit P95 / P99 (ms) | New-submit interval P95 / P99 (ms) | CPU (%) | Private RSS (MiB) |
|---|---:|---:|---:|---:|---:|
| H.264 1080p60 / release | 9872 | 5.793 / 11.263 | 18.024 / 24.261 | 144.96 | 116.65 |
| H.264 1080p60 / current | 9866 | 5.682 / 11.887 | 18.592 / 24.345 | 146.23 | 116.77 |
| HEVC 1080p60 / release | 9864 | 8.969 / 13.942 | 18.942 / 25.701 | 167.23 | 121.89 |
| HEVC 1080p60 / current | 9851 | 10.141 / 17.339 | 19.225 / 27.223 | 170.66 | 122.13 |
| HLS fMP4 1080p30 / release | 5062 | 1466.780 / 1469.635 | 33.821 / 37.987 | 78.57 | 196.71 |
| HLS fMP4 1080p30 / current | 5063 | 1466.941 / 1472.968 | 34.340 / 41.098 | 76.47 | 198.00 |
| Visible idle / release | 0 | 0.000 / 0.000 | n/a / n/a | 2.60 | 90.16 |
| Visible idle / current | 0 | 0.000 / 0.000 | n/a / n/a | 2.53 | 89.95 |
| Hidden H.264 1080p60 / release | 893 | 6.910 / 13.464 | 18.959 / 26.819 | 16.53 | 116.66 |
| Hidden H.264 1080p60 / current | 895 | 7.088 / 13.738 | 17.841 / 26.367 | 17.00 | 116.68 |

Timings and resource columns are medians of per-run measurements; submitted
frames are totals. Hidden submissions belong to the initial visible warmup.
Continuing hidden frames decoded without restoring the window or continuing
plane uploads. Latest-frame replacements while hidden are intentional.

H.264 submissions changed by -0.061% and HEVC by -0.132%. HEVC median internal
P99 increased from 13.942 to 17.339 ms and interval P99 from 25.701 to 27.223 ms.
Paired round differences changed sign: the mean HEVC interval-P99 difference was
+0.052 ms, with a descriptive paired t interval of -2.196 to +2.300 ms. The mean
internal-P99 difference was +0.927 ms (-3.037 to +4.891 ms). With only five noisy
software-rendered pairs, this does not establish a repeatable slowdown or rule
out a small cost; it is not a hardware acceptance test or an equivalence proof.

HLS receive-to-submit P95 near 1.467 seconds includes timestamp-scheduled
waiting and demux/decode prefetch. It must not be described as CPU decode cost
or interactive mirror latency. Its extra decoded frames at exit are queued
future frames, not necessarily lost display frames. End-of-run late/stale
counters include shutdown generation fencing; see the raw counters rather
than assuming decoded-minus-submitted is HLS loss.

## Receiver/decode isolation

Three further paired headless HEVC 1080p60 runs, 30 seconds measured plus
3 seconds warmup, used the same binaries and bitstream. Each version decoded
5,940 video frames and processed 9,903 AAC-ELD packets without packet errors.
No GPU uploads occurred.

| Version | Mean CPU (%) | Median private RSS (MiB) |
|---|---:|---:|
| Release | 45.14 | 44.95 |
| Initial candidate | 45.62 | 44.74 |

The mean difference was 0.48 percentage points of one core, about 1.1% relative,
with direction changing between rounds. This gives no clear evidence of added
protocol/decode cost in the measured path. It does not measure a physical audio
callback, WASAPI latency, audible AV offset or drift.

## Status/control isolation fix and final rerun

Final receiver code: `c2f0fe26e64546a5715c1318749b1b8b6c353aca`.
`Shared::snapshot()` previously queried active protocol through the same mutex
held by auxiliary controls and session cleanup. UI metric updates could therefore
wait for HLS cancellation or resource-response processing. A controlled test held
that gate open until after the status read: the old implementation could not
answer; the independent observer snapshot answers before control completion.

Protocol identity is now published during successful claim and final release
using a short separate observer lock. Ownership authorization, handover and stale
cleanup guards retain their control gate. Media queue bounds, decode behavior,
texture/pipeline reuse, shader conversion and frame scheduling are unchanged.
No dependencies were added.

Validation passed: 82 library tests, 2 updater tests, formatting and warning-free
Clippy. Encrypted GUI reconnection passed all 13 cases with 390/390 frames
submitted; fMP4 GUI playback submitted 30/30 frames.

Three new paired runs against the same v0.1.2 baseline measured 15 seconds of
playback plus 3 seconds video warmup. H.264/HEVC therefore each received 3,240
frames per version. Mirror bitstreams and SHA-256 hashes matched the original
comparison. HLS playlists were regenerated for the shorter run, identically for
both versions; the two datasets should not be used to claim a before/after gain
from the status fix.

| Scenario / version | Submitted (total) | Receive to submit P95 / P99 (ms) | New-submit interval P95 / P99 (ms) | CPU (%) | Private RSS (MiB) |
|---|---:|---:|---:|---:|---:|
| H.264 1080p60 / release | 3231 | 5.511 / 11.614 | 17.829 / 25.859 | 145.19 | 116.65 |
| H.264 1080p60 / current | 3229 | 5.920 / 12.252 | 18.711 / 24.399 | 142.06 | 116.56 |
| HEVC 1080p60 / release | 3234 | 10.409 / 15.728 | 19.129 / 26.504 | 173.66 | 121.92 |
| HEVC 1080p60 / current | 3227 | 11.778 / 17.737 | 19.263 / 26.634 | 172.53 | 121.77 |
| HLS fMP4 1080p30 / release | 1687 | 1466.806 / 1470.968 | 33.901 / 39.033 | 77.13 | 182.27 |
| HLS fMP4 1080p30 / current | 1689 | 1466.705 / 1469.363 | 33.764 / 37.373 | 76.60 | 186.62 |

Final H.264 submissions differed by -0.062% and HEVC by -0.216%. Per-run CPU
medians were slightly lower for the final receiver in all three scenarios.
Private RSS was essentially equal for mirror playback; HLS increased by 4.35 MiB
(about 2.4%). HEVC internal P99 remained 2.009 ms higher, while interval P99
differed by only 0.130 ms. This focused rerun found no material throughput or
resource regression from the status fix; it does not erase the observed tails.

## Additional review: mirror/control isolation

Receiver code: `dce6cbadd0c0e21b3d6bad414c3482404febcb94`.
Rechecking all 74 original runs reproduced their totals, resource medians and
binary hashes. Linux and Windows CI for `24cb1900fc4a0cebcba20e25fd1cb4588218ca55`
[passed](https://github.com/geniucker-dev/AirDock/actions/runs/37329532670), including
Windows packaged playback, installer upgrade/uninstall and updater checks.

The mirror transport still read `Sessions::owner_id()` through the ownership
gate before each packet. An independent control holding that gate prevented the
observer from answering within its one-second test deadline. This can stall
mirror reception even after UI protocol status was isolated. The ownership ID
now uses an atomic observer, published only by successful claim/final cleanup
under the original gate. Controls still authorize against the gate. Worker joins,
handover and stale teardown protection are unchanged; no media queue, decoder,
texture upload or audio callback changes were needed.

The reproduction now answers before the control finishes. A second regression
holds cleanup open and verifies both observers remain readable, then clear after
cleanup. All 83 library tests and 2 updater tests passed, as did formatting,
Clippy and the dependency audit. Optimized GUI reconnect submitted 390/390 frames
across 13 cases; fMP4 playback submitted 30/30. Active-session SIGINT/SIGTERM
cleanup took 0.767/0.766 seconds and released the listener.

Twelve additional measured processes used three paired rounds per mirror codec,
15 seconds playback plus 3 seconds video warmup, with the same release baseline,
bitstreams and software environment. Each codec/version decoded all 3,240 input
frames. This is ordinary playback coverage; deliberate control contention is
covered by the deterministic concurrency regressions rather than these timings.

| Scenario / version | Submitted (total) | Receive to submit P95 / P99 (ms) | New-submit interval P95 / P99 (ms) | CPU (%) | Private RSS (MiB) |
|---|---:|---:|---:|---:|---:|
| H.264 1080p60 / release | 3234 | 4.843 / 11.535 | 18.098 / 25.276 | 144.99 | 116.65 |
| H.264 1080p60 / current | 3233 | 5.798 / 12.013 | 18.234 / 24.823 | 144.26 | 116.29 |
| HEVC 1080p60 / release | 3227 | 10.357 / 16.560 | 19.187 / 26.479 | 165.53 | 121.80 |
| HEVC 1080p60 / current | 3227 | 10.170 / 16.495 | 19.447 / 27.869 | 169.33 | 121.43 |

H.264 throughput differed by -0.031%; HEVC submission totals were identical.
Mirror private RSS did not increase. HEVC median CPU was 3.80 percentage points
of one core higher (2.3% relative); interval P99 was 1.390 ms higher. Paired CPU
differences were -0.867, +7.796 and -2.534 points, so the direction was not
consistent. HEVC internal P99 differences were -3.321, +2.806 and +0.799 ms.
These small samples show no material submission-throughput loss but cannot
establish equivalent CPU/tail performance. The earlier higher HEVC tails remain
part of the evidence; changing percentile medians across runs is not proof that
this fix removed them. Windows/iPhone physical acceptance is still pending.

[Additional raw runs, binary/input/source hashes and regression results](benchmarks/2026-10-05-control-isolation.json)
are separate from the original data. To reproduce this candidate, substitute
`dce6cbadd0c0e21b3d6bad414c3482404febcb94` for the candidate worktree and
`--current-commit` below, and use `--cases h264 hevc --runs 3 --seconds 15`.
Across both evidence files there are 86 measured process runs. HLS, idle and
hidden performance measurements remain those of the preceding comparison;
only HLS functional playback was repeated for this additional fix.

## Reproduction and evidence

[Machine-readable runs, hashes, input manifests, resource samples and regressions](benchmarks/2026-10-05-release-comparison.json)
include all 74 measured process runs across the three stages. The report does not
feed synthetic observations into `tools/rust_performance.py` as physical results.

On Linux, install development FFmpeg/ffprobe with fixture encoders, Vulkan Mesa,
Xvfb, Openbox, xdotool and the Python `cryptography` package. Fixture encoders are
development tools, not application/package dependencies. Build both snapshots
with the same target directory/toolchain/native libraries and copy each finished
binary before rebuilding the next snapshot:

```sh
git worktree add --detach ../airdock-release v0.1.2
git worktree add --detach ../airdock-candidate c2f0fe26e64546a5715c1318749b1b8b6c353aca
cargo build --release --locked --bin airdock --manifest-path ../airdock-release/Cargo.toml --target-dir ./target/performance
cp target/performance/release/airdock /tmp/airdock-release
cargo build --release --locked --bin airdock --manifest-path ../airdock-candidate/Cargo.toml --target-dir ./target/performance
cp target/performance/release/airdock /tmp/airdock-current

WGPU_BACKEND=vulkan LP_NUM_THREADS=2 xvfb-run -a -s '-screen 0 1280x1024x24' \
  python tools/rust_release_benchmark.py \
  --release /tmp/airdock-release --current /tmp/airdock-current \
  --release-commit 216443d2745515ce67fe173db7ee6b96f1beb5cf \
  --current-commit c2f0fe26e64546a5715c1318749b1b8b6c353aca \
  --output /tmp/airdock-comparison --runs 5 --seconds 30 --idle-seconds 15 --warmup 3

python tools/rust_core_release_benchmark.py \
  --release /tmp/airdock-release --current /tmp/airdock-current \
  --release-commit 216443d2745515ce67fe173db7ee6b96f1beb5cf \
  --current-commit c2f0fe26e64546a5715c1318749b1b8b6c353aca \
  --fixture /tmp/airdock-comparison/input/hevc.mp4 \
  --output /tmp/airdock-core-comparison --runs 3 --seconds 30 --warmup 3
```

Openbox must actually start; the benchmark rejects failed initialization. A
rootless extracted install needs its theme directory in `XDG_DATA_DIRS`, library
paths, and a valid Vulkan ICD path in `VK_DRIVER_FILES`. Initial pilot runs with
an unconfigured window manager were rejected and excluded from these results.

## Remaining limits

No Windows/iPhone or physical display/audio device is attached. Still pending:
real display FPS and latency, actual dropped display frames, hardware decoder
throughput and cross-GPU transfers, GPU utilization/VRAM/power, WASAPI callbacks
and device changes, AV offset/drift, Wi-Fi losses, Windows tray/DPI/fullscreen
performance and long-duration memory/resource behavior. Inputs here cover 8-bit
SDR software YUV420P; native hardware NV12, 10-bit/HDR and other resolutions were
not performance-benchmarked. Functional coverage of other formats is separate.
This task compared the latest release, not the original C++ or retired Slint
receiver. Same-device physical acceptance remains necessary.
