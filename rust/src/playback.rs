// SPDX-License-Identifier: MPL-2.0
//! Media scheduling precedes the one-frame display mailbox. No UI or device handles here.
use crate::{media::VideoFrame, state::Metrics};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

fn frame_bytes(frame: &VideoFrame) -> u64 {
    // Use backing allocations: word YUV and decoder stride padding cost more
    // than the old width*height*1.5 byte estimate.
    unsafe {
        (*frame.frame.as_ptr())
            .buf
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .map(|p| (*p).size as u64)
            .sum()
    }
}
fn intake_available(queue: &VecDeque<VideoFrame>) -> bool {
    queue.len() < 8 && queue.iter().map(frame_bytes).sum::<u64>() < 48 * 1024 * 1024
}

/// A playback policy, not a wire protocol or container format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PlaybackMode {
    /// Interactive mirror: release decoded frames immediately.
    Live,
    /// Buffered media: schedule by PTS against audible PCM or a monotonic anchor.
    Timed,
}

/// The adapter establishes the relationship between video and audio timestamps.
/// Unmapped source clocks must not be reported as an AV offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockRelation {
    SharedTimeline,
    Unmapped,
    EpochAligned {
        epoch_offset_us: i64,
        max_skew_us: u64,
    },
}
impl ClockRelation {
    pub fn offset(self, video_us: i64, audio_us: i64) -> Option<i64> {
        if self == Self::Unmapped {
            return None;
        }
        let delta = video_us.checked_sub(audio_us)?;
        match self {
            Self::SharedTimeline => Some(delta),
            Self::Unmapped => None,
            Self::EpochAligned {
                epoch_offset_us,
                max_skew_us,
            } => [
                Some(0),
                Some(epoch_offset_us),
                epoch_offset_us.checked_neg(),
            ]
            .into_iter()
            .flatten()
            .find_map(|adjustment| {
                delta
                    .checked_add(adjustment)
                    .filter(|offset| offset.unsigned_abs() <= max_skew_us)
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaTime {
    pub ticks: i64,
    pub numerator: i32,
    pub denominator: i32,
}
impl MediaTime {
    pub fn micros(self) -> i64 {
        ((self.ticks as i128 * self.numerator as i128 * 1_000_000)
            / self.denominator.max(1) as i128)
            .clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }
    pub fn microseconds(us: i64) -> Self {
        Self {
            ticks: us,
            numerator: 1,
            denominator: 1_000_000,
        }
    }
    pub fn ntp(raw: u64) -> Option<Self> {
        (raw != 0).then(|| {
            Self::microseconds(
                ((raw >> 32) as i64) * 1_000_000
                    + (((raw & 0xffff_ffff) as u128 * 1_000_000) >> 32) as i64,
            )
        })
    }
}

/// Snapshot of the estimated audible PCM position, never the ring write position.
pub struct AudioClock {
    origin: Instant,
    pub revision: AtomicU64,
    pub epoch: AtomicU64,
    pub media_us: AtomicI64,
    pub horizon_us: AtomicI64,
    pub audible_at_us: AtomicU64,
    pub valid: AtomicBool,
    pub paused: AtomicBool,
    pub underruns: AtomicU64,
    pub output_latency_us: AtomicU64,
}
impl Default for AudioClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            revision: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            media_us: AtomicI64::new(0),
            horizon_us: AtomicI64::new(0),
            audible_at_us: AtomicU64::new(0),
            valid: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            output_latency_us: AtomicU64::new(0),
        }
    }
}
impl AudioClock {
    pub fn now_us(&self) -> u64 {
        self.origin.elapsed().as_micros() as u64
    }
    pub fn position(&self, epoch: u64) -> Option<i64> {
        if !self.valid.load(Ordering::Acquire) || self.epoch.load(Ordering::Relaxed) != epoch {
            return None;
        }
        for _ in 0..3 {
            let before = self.revision.load(Ordering::Acquire);
            if before & 1 != 0 {
                continue;
            }
            let media = self.media_us.load(Ordering::Relaxed);
            let at = self.audible_at_us.load(Ordering::Relaxed);
            let horizon = self.horizon_us.load(Ordering::Relaxed);
            if before == self.revision.load(Ordering::Acquire)
                && self.epoch.load(Ordering::Relaxed) == epoch
            {
                return Some(
                    media
                        .saturating_add(self.now_us() as i64 - at as i64)
                        .min(horizon),
                );
            }
        }
        None
    }
    pub fn update(&self, epoch: u64, media: i64, audible: u64, latency: u64, horizon: i64) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.media_us.store(media, Ordering::Relaxed);
        self.horizon_us.store(horizon, Ordering::Relaxed);
        self.audible_at_us.store(audible, Ordering::Relaxed);
        self.output_latency_us.store(latency, Ordering::Relaxed);
        self.epoch.store(epoch, Ordering::Relaxed);
        self.revision.fetch_add(1, Ordering::Release);
        self.valid.store(true, Ordering::Release);
    }

    pub fn invalidate(&self) {
        self.valid.store(false, Ordering::Release);
    }
}

#[derive(Default)]
pub struct DisplayMailbox {
    pub latest: Mutex<Option<Arc<VideoFrame>>>,
    pub ready: Condvar,
    pub revision: AtomicU64,
    pub visible: AtomicBool,
    pub consumed: AtomicU64,
    pub wake: Mutex<Option<futures::channel::mpsc::Sender<()>>>,
}
impl DisplayMailbox {
    fn publish(&self, frame: VideoFrame, metrics: &Metrics) {
        let old = self.latest.lock().unwrap().replace(Arc::new(frame));
        let first = old.is_none();
        if let Some(old) = old
            && old.sequence > self.consumed.load(Ordering::Acquire)
        {
            metrics.replaced.fetch_add(1, Ordering::Relaxed);
            if !self.visible.load(Ordering::Acquire) {
                metrics.hidden_replaced.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.revision.fetch_add(1, Ordering::Release);
        self.ready.notify_all();
        // Notify a hidden desktop once when video starts, so a new session can
        // reveal its player promptly. Continuing hidden frames never wake it.
        if (first || self.visible.load(Ordering::Relaxed))
            && let Some(wake) = self.wake.lock().unwrap().as_mut()
        {
            let _ = wake.try_send(());
        }
    }
    pub fn clear(&self) {
        self.latest.lock().unwrap().take();
        self.revision.fetch_add(1, Ordering::Release);
        self.ready.notify_all();
        if let Some(wake) = self.wake.lock().unwrap().as_mut() {
            let _ = wake.try_send(());
        }
    }
}

pub struct Playback {
    sender: SyncSender<VideoFrame>,
    pub display: Arc<DisplayMailbox>,
    pub audio_clock: Arc<AudioClock>,
    pub metrics: Arc<Metrics>,
    epoch: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    sequence: AtomicU64,
    pending: Arc<AtomicUsize>,
}
impl Playback {
    pub fn new(epoch: Arc<AtomicU64>, metrics: Arc<Metrics>) -> Self {
        let (sender, recv) = mpsc::sync_channel::<VideoFrame>(2);
        let display = Arc::new(DisplayMailbox::default());
        let audio_clock = Arc::new(AudioClock::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (d, c, e, s, m) = (
            display.clone(),
            audio_clock.clone(),
            epoch.clone(),
            stop.clone(),
            metrics.clone(),
        );
        let pending = Arc::new(AtomicUsize::new(0));
        let queued = pending.clone();
        let worker = thread::Builder::new()
            .name("media-scheduler".into())
            .spawn(move || {
                let mut queue = VecDeque::<VideoFrame>::new();
                let mut anchor = None::<(u64, PlaybackMode, i64, Instant)>;
                let mut paused_at = None;
                while !s.load(Ordering::Acquire) {
                    let wait = if c.paused.load(Ordering::Acquire) || queue.is_empty() {
                        Duration::from_millis(100)
                    } else {
                        let delta = queue
                            .front()
                            .and_then(|f| {
                                (f.playback == PlaybackMode::Timed)
                                    .then_some(f.pts)
                                    .flatten()
                            })
                            .map(|pts| {
                                let media_now =
                                    c.position(e.load(Ordering::Acquire)).or_else(|| {
                                        anchor.map(|(_, _, base, wall)| {
                                            base + wall.elapsed().as_micros() as i64
                                        })
                                    });
                                pts.micros()
                                    .saturating_sub(media_now.unwrap_or(pts.micros()))
                            })
                            .unwrap_or(0);
                        Duration::from_micros(delta.clamp(1_000, 20_000) as u64)
                    };
                    if intake_available(&queue) {
                        match recv.recv_timeout(wait) {
                            Ok(f) => queue.push_back(f),
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(_) => {}
                        }
                    } else {
                        // Do not receive another future/paused frame every loop
                        // after the queue fills; keep producer backpressure.
                        thread::park_timeout(wait);
                    }
                    while intake_available(&queue) {
                        match recv.try_recv() {
                            Ok(f) => queue.push_back(f),
                            Err(_) => break,
                        }
                    }
                    let epoch = e.load(Ordering::Acquire);
                    if c.paused.load(Ordering::Acquire) {
                        paused_at.get_or_insert_with(Instant::now);
                        continue;
                    }
                    if let Some(paused) = paused_at.take()
                        && let Some((_, _, _, wall)) = anchor.as_mut()
                    {
                        *wall += paused.elapsed();
                    }
                    let before = queue.len();
                    queue.retain(|f| f.epoch == epoch);
                    queued.fetch_sub(before - queue.len(), Ordering::Release);
                    let mut due = None;
                    while let Some(f) = queue.front() {
                        let pts = f.pts.map(MediaTime::micros);
                        let now = Instant::now();
                        // Mirror frames are interactive: sender NTP is telemetry,
                        // not permission to wait for the buffered audio horizon.
                        // Timed playback uses PTS scheduling and audio as its master clock.
                        let target = if f.playback == PlaybackMode::Timed
                            && let Some(pts) = pts
                        {
                            let (ae, ah, base, wall) =
                                *anchor.get_or_insert((epoch, f.playback, pts, now));
                            if ae != epoch
                                || ah != f.playback
                                || (pts - base).unsigned_abs() > 3_600_000_000
                            {
                                anchor = Some((epoch, f.playback, pts, now));
                                continue;
                            }
                            let media_now = c
                                .position(epoch)
                                .unwrap_or(base + wall.elapsed().as_micros() as i64);
                            let delta = pts.saturating_sub(media_now);
                            now.checked_add(Duration::from_micros(delta.max(0) as u64))
                                .unwrap_or(now)
                        } else {
                            now
                        };
                        if target > now {
                            break;
                        }
                        if due.replace(queue.pop_front().unwrap()).is_some() {
                            queued.fetch_sub(1, Ordering::Release);
                            m.schedule_dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    if let Some(frame) = due {
                        d.publish(frame, &m);
                        queued.fetch_sub(1, Ordering::Release);
                    }
                }
            })
            .expect("media scheduler thread");
        Self {
            sender,
            display,
            audio_clock,
            metrics,
            epoch,
            stop,
            worker: Mutex::new(Some(worker)),
            sequence: AtomicU64::new(0),
            pending,
        }
    }
    pub fn sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub fn submit(&self, mut frame: VideoFrame) {
        if self.stop.load(Ordering::Acquire) {
            return;
        }
        if frame.epoch != self.epoch.load(Ordering::Acquire) {
            self.metrics.stale_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        frame.sequence = self.sequence();
        self.metrics.decoded.fetch_add(1, Ordering::Relaxed);
        self.pending.fetch_add(1, Ordering::Relaxed);
        loop {
            match self.sender.try_send(frame) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(f)) => {
                    frame = f;
                    if self.stop.load(Ordering::Acquire)
                        || frame.epoch != self.epoch.load(Ordering::Acquire)
                    {
                        self.pending.fetch_sub(1, Ordering::Release);
                        break;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => {
                    self.pending.fetch_sub(1, Ordering::Release);
                    break;
                }
            }
        }
    }
    pub fn pending_frames(&self) -> usize {
        if self.stop.load(Ordering::Acquire) {
            0
        } else {
            self.pending.load(Ordering::Acquire)
        }
    }
    pub fn drain(&self, epoch: u64, cancel: &AtomicBool) {
        while self.pending_frames() != 0
            && self.epoch.load(Ordering::Acquire) == epoch
            && !self.stop.load(Ordering::Acquire)
            && !cancel.load(Ordering::Acquire)
        {
            thread::sleep(Duration::from_millis(5));
        }
    }
    pub fn reset(&self) {
        self.audio_clock.invalidate();
        self.audio_clock.paused.store(false, Ordering::Release);
        self.display.clear();
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.worker.lock().unwrap().take() {
            let _ = t.join();
        }
    }
}
impl Drop for Playback {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_clock_mapping_is_explicit_and_unknown_clocks_have_no_estimate() {
        let unix = 1_800_000_000_000_000;
        let ntp = unix + 2_208_988_800_000_000;
        assert_eq!(
            ClockRelation::SharedTimeline.offset(170_000, 100_000),
            Some(70_000)
        );
        // A shared Cast/MPEG timeline must not receive AirPlay's epoch correction.
        assert_eq!(
            ClockRelation::SharedTimeline.offset(unix, ntp),
            Some(-2_208_988_800_000_000)
        );
        assert_eq!(ClockRelation::Unmapped.offset(170_000, 100_000), None);
        assert_eq!(
            ClockRelation::SharedTimeline.offset(i64::MAX, i64::MIN),
            None
        );
    }
    #[test]
    fn hidden_mailbox_wakes_only_for_the_first_frame_or_reset() {
        use futures::{FutureExt, StreamExt};
        let mailbox = DisplayMailbox::default();
        let metrics = Metrics::default();
        let (sender, mut receiver) = futures::channel::mpsc::channel(1);
        *mailbox.wake.lock().unwrap() = Some(sender);
        let frame = |sequence| VideoFrame {
            frame: ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV420P, 16, 16),
            received: Instant::now(),
            pts: None,
            epoch: 1,
            sequence,
            playback: crate::playback::PlaybackMode::Live,
            clock: crate::receiver::airplay::MIRROR_CLOCK,
        };
        mailbox.publish(frame(1), &metrics);
        assert_eq!(receiver.next().now_or_never(), Some(Some(())));
        for sequence in 2..=120 {
            mailbox.publish(frame(sequence), &metrics);
        }
        assert!(receiver.next().now_or_never().is_none());
        assert_eq!(metrics.hidden_replaced.load(Ordering::Relaxed), 119);
        mailbox.clear();
        assert_eq!(receiver.next().now_or_never(), Some(Some(())));
        mailbox.publish(frame(121), &metrics);
        assert_eq!(receiver.next().now_or_never(), Some(Some(())));
        mailbox.visible.store(true, Ordering::Release);
        mailbox.publish(frame(122), &metrics);
        assert_eq!(receiver.next().now_or_never(), Some(Some(())));
        assert_eq!(metrics.replaced.load(Ordering::Relaxed), 120);
        assert_eq!(metrics.hidden_replaced.load(Ordering::Relaxed), 119);
    }
    #[test]
    fn future_word_frames_keep_bounded_intake_and_shutdown_unblocks_producer() {
        let p = Arc::new(Playback::new(
            Arc::new(AtomicU64::new(2)),
            Arc::new(Metrics::default()),
        ));
        p.audio_clock.update(2, 0, p.audio_clock.now_us(), 0, 0);
        let completed = Arc::new(AtomicUsize::new(0));
        let producer = p.clone();
        let count = completed.clone();
        let t = thread::spawn(move || {
            for _ in 0..24 {
                producer.submit(VideoFrame {
                    frame: ffmpeg_next::frame::Video::new(
                        ffmpeg_next::format::Pixel::P010LE,
                        64,
                        64,
                    ),
                    received: Instant::now(),
                    pts: Some(MediaTime::microseconds(10_000_000)),
                    epoch: 2,
                    sequence: 0,
                    playback: crate::playback::PlaybackMode::Timed,
                    clock: crate::playback::ClockRelation::SharedTimeline,
                });
                count.fetch_add(1, Ordering::Release);
            }
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while completed.load(Ordering::Acquire) < 10 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        thread::sleep(Duration::from_millis(60));
        let accepted = completed.load(Ordering::Acquire);
        let pending = p.pending_frames();
        p.stop();
        t.join().unwrap();
        assert_eq!(
            accepted, 10,
            "Eight scheduled frames plus two in the channel"
        );
        assert!(
            pending <= 11,
            "At most one additional blocked producer frame"
        );
        assert_eq!(p.pending_frames(), 0);
    }
    #[test]
    fn mirror_av_epoch_normalization_and_unknown_clock() {
        let unix = 1_800_000_000_000_000;
        let ntp = unix + 2_208_988_800_000_000;
        assert_eq!(
            crate::receiver::airplay::MIRROR_CLOCK.offset(unix + 83_500, ntp),
            Some(83_500)
        );
        assert_eq!(
            crate::receiver::airplay::MIRROR_CLOCK.offset(ntp - 50_000, unix),
            Some(-50_000)
        );
        assert_eq!(
            crate::receiver::airplay::MIRROR_CLOCK.offset(ntp + 25_000, ntp),
            Some(25_000)
        );
        assert_eq!(
            crate::receiver::airplay::MIRROR_CLOCK.offset(123_000, ntp),
            None
        );
        assert_eq!(
            crate::receiver::airplay::MIRROR_CLOCK.offset(i64::MIN, i64::MAX),
            None
        );
        assert_eq!(
            crate::receiver::airplay::MIRROR_CLOCK.offset(i64::MIN, 0),
            None
        );
    }
    #[test]
    fn rational_pts_and_ntp_keep_precision() {
        assert_eq!(
            MediaTime {
                ticks: 90_001,
                numerator: 1,
                denominator: 90_000
            }
            .micros(),
            1_000_011
        );
        assert_eq!(
            MediaTime::ntp((2 << 32) | 0x8000_0000).unwrap().micros(),
            2_500_000
        );
        assert!(MediaTime::ntp(0).is_none());
    }
    #[test]
    fn audio_clock_cannot_run_past_submitted_pcm() {
        let c = AudioClock::default();
        c.update(
            3,
            1_000_000,
            c.now_us().saturating_sub(20_000),
            0,
            1_001_000,
        );
        assert!(c.position(3).unwrap() <= 1_001_000);
        assert!(c.position(4).is_none());
        c.invalidate();
        assert!(c.position(3).is_none());
    }
    #[test]
    fn hls_future_frame_waits_for_audio_horizon_before_display_handoff() {
        let epoch = Arc::new(AtomicU64::new(2));
        let p = Playback::new(epoch, Arc::new(Metrics::default()));
        p.audio_clock
            .update(2, 500_000, p.audio_clock.now_us(), 0, 500_000);
        p.submit(VideoFrame {
            frame: ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV420P, 16, 16),
            received: Instant::now(),
            pts: Some(MediaTime::microseconds(1_000_000)),
            epoch: 2,
            sequence: 0,
            playback: crate::playback::PlaybackMode::Timed,
            clock: crate::playback::ClockRelation::SharedTimeline,
        });
        thread::sleep(Duration::from_millis(35));
        assert!(p.display.latest.lock().unwrap().is_none());
        assert_eq!(p.pending_frames(), 1);
        p.audio_clock
            .update(2, 1_000_000, p.audio_clock.now_us(), 0, 1_000_000);
        let deadline = Instant::now() + Duration::from_secs(1);
        while p.pending_frames() != 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(p.pending_frames(), 0);
        assert_eq!(
            p.display
                .latest
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .pts
                .unwrap()
                .micros(),
            1_000_000
        );
    }
    #[test]
    fn video_only_hls_pause_does_not_advance_the_monotonic_anchor() {
        let p = Playback::new(Arc::new(AtomicU64::new(2)), Arc::new(Metrics::default()));
        let frame = |pts| VideoFrame {
            frame: ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV420P, 16, 16),
            received: Instant::now(),
            pts: Some(MediaTime::microseconds(pts)),
            epoch: 2,
            sequence: 0,
            playback: crate::playback::PlaybackMode::Timed,
            clock: crate::playback::ClockRelation::SharedTimeline,
        };
        p.submit(frame(0));
        let deadline = Instant::now() + Duration::from_secs(1);
        while p.display.latest.lock().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        p.audio_clock.paused.store(true, Ordering::Release);
        p.submit(frame(200_000));
        thread::sleep(Duration::from_millis(250));
        p.audio_clock.paused.store(false, Ordering::Release);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            p.display
                .latest
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .pts
                .unwrap()
                .micros(),
            0
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        while p.pending_frames() != 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(p.pending_frames(), 0);
    }
    #[test]
    fn mirror_does_not_wait_for_stalled_or_buffered_audio() {
        let p = Playback::new(Arc::new(AtomicU64::new(2)), Arc::new(Metrics::default()));
        // The old path waited forever for this horizon to reach the mirror PTS.
        p.audio_clock
            .update(2, 0, p.audio_clock.now_us(), 40_000, 0);
        p.submit(VideoFrame {
            frame: ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV420P, 16, 16),
            received: Instant::now(),
            pts: Some(MediaTime::microseconds(90_000)),
            epoch: 2,
            sequence: 0,
            playback: crate::playback::PlaybackMode::Live,
            clock: crate::receiver::airplay::MIRROR_CLOCK,
        });
        let mailbox = p.display.latest.lock().unwrap();
        let (mailbox, _) = p
            .display
            .ready
            .wait_timeout_while(mailbox, Duration::from_millis(80), |f| f.is_none())
            .unwrap();
        assert!(
            mailbox.is_some(),
            "Mirror stalled behind audio instead of displaying immediately"
        );
    }
    #[test]
    fn old_generation_cannot_enter_display() {
        let epoch = Arc::new(AtomicU64::new(2));
        let p = Playback::new(epoch, Arc::new(Metrics::default()));
        p.submit(VideoFrame {
            frame: ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV420P, 16, 16),
            received: Instant::now(),
            pts: None,
            epoch: 1,
            sequence: 0,
            playback: crate::playback::PlaybackMode::Live,
            clock: crate::receiver::airplay::MIRROR_CLOCK,
        });
        assert_eq!(p.metrics.stale_dropped.load(Ordering::Relaxed), 1);
        assert!(p.display.latest.lock().unwrap().is_none());
    }
}
