use crate::{config::Settings, media::VideoFrame};
#[cfg(test)]
use std::sync::Mutex;
use std::{
    sync::{Arc, RwLock, atomic::Ordering},
    time::Instant,
};

pub use crate::{status::UiState, telemetry::Metrics};
/// Protocol-facing handle. Window, GPU and audio device ownership live elsewhere.
pub struct Shared {
    pub settings: Arc<RwLock<Settings>>,
    pub ui: Arc<crate::status::Status>,
    pub metrics: Arc<Metrics>,
    pub sessions: crate::session::Sessions,
    pub media: crate::playback::Playback,
    pub started: Instant,
    #[cfg(test)]
    pub test_pcm: Mutex<Vec<i16>>,
}
pub use crate::session::SessionOwner;
impl std::ops::Deref for Shared {
    type Target = crate::session::Sessions;
    fn deref(&self) -> &Self::Target {
        &self.sessions
    }
}
impl Shared {
    pub fn new(settings: Settings) -> Arc<Self> {
        let sessions = crate::session::Sessions::default();
        let metrics = Arc::new(Metrics::default());
        let media = crate::playback::Playback::new(sessions.epoch.clone(), metrics.clone());
        Arc::new(Self {
            settings: Arc::new(RwLock::new(settings)),
            ui: Arc::new(crate::status::Status::default()),
            metrics,
            sessions,
            media,
            started: Instant::now(),
            #[cfg(test)]
            test_pcm: Mutex::new(Vec::new()),
        })
    }
    pub fn publish(&self, frame: VideoFrame) {
        self.media.submit(frame)
    }
    pub fn pcm(&self, samples: Arc<Vec<i16>>, _rate: u32) {
        #[cfg(test)]
        self.test_pcm.lock().unwrap().extend_from_slice(&samples);
        #[cfg(not(test))]
        let _ = samples;
    }
    pub fn reset_media(&self) -> u64 {
        self.metrics.video_depth.store(0, Ordering::Relaxed);
        self.metrics.video_colour_mode.store(0, Ordering::Relaxed);
        self.metrics
            .estimated_av_available
            .store(false, Ordering::Release);
        let epoch = self.advance();
        self.media.reset();
        epoch
    }
    pub fn request_disconnect(&self) {
        self.disconnect.fetch_add(1, Ordering::Relaxed);
        self.reset_media();
    }
    pub fn report(&self, error: String) {
        let mut status = self.ui.lock().unwrap();
        if status.error != error {
            tracing::error!("{error}");
            status.error = error;
        }
    }
    pub fn snapshot(&self) -> serde_json::Value {
        let m = &self.metrics;
        let mut latency = m
            .latency_samples
            .lock()
            .unwrap()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        latency.sort_unstable();
        let p95 = latency
            .get((latency.len() * 95).div_ceil(100).saturating_sub(1))
            .copied()
            .unwrap_or(0);
        let mut intervals = m
            .present_intervals_us
            .lock()
            .unwrap()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        intervals.sort_unstable();
        serde_json::json!({"elapsed_seconds":self.started.elapsed().as_secs_f64(),
            "decoded_frames":m.decoded.load(Ordering::Relaxed),"presented_frames":m.presented.load(Ordering::Relaxed),
            "replaced_frames":m.replaced.load(Ordering::Relaxed),"last_receive_to_present_us":m.latency_us.load(Ordering::Relaxed),
            "audio_packets":m.audio_packets.load(Ordering::Relaxed),"audio_recovered":m.audio_recovered.load(Ordering::Relaxed),
            "audio_errors":m.audio_errors.load(Ordering::Relaxed),"schedule_dropped":m.schedule_dropped.load(Ordering::Relaxed),"stale_dropped":m.stale_dropped.load(Ordering::Relaxed),
            "p95_receive_to_present_us":p95, "p99_receive_to_present_us":latency.get((latency.len()*99).div_ceil(100).saturating_sub(1)).copied().unwrap_or(0),
            "p95_new_submission_interval_us":intervals.get((intervals.len()*95).div_ceil(100).saturating_sub(1)).copied(),
            "p99_new_submission_interval_us":intervals.get((intervals.len()*99).div_ceil(100).saturating_sub(1)).copied(),
            "submission_interval_sample_count":intervals.len(),
            "estimated_av_offset_us":m.estimated_av_available.load(Ordering::Acquire).then(||m.estimated_av_offset_us.load(Ordering::Relaxed)),
            "av_measurement":"Predicted CPAL audible clock versus frame PTS, not measured speaker/display offset",
            "presentation_measurement":"GPU render submission, not physical display time",
            "uploaded_frames":m.uploaded.load(Ordering::Relaxed),
            "ten_bit_uploaded_frames":m.ten_bit_uploaded.load(Ordering::Relaxed),
            "hdr_uploaded_frames":m.hdr_uploaded.load(Ordering::Relaxed),
            "video_colour_mapping":m.video_colour_label(),
            "pending_scheduled_frames":self.media.pending_frames(),
            "audio_underruns":self.media.audio_clock.underruns.load(Ordering::Relaxed),
            "audio_output_latency_us":self.media.audio_clock.output_latency_us.load(Ordering::Relaxed),"latency_sample_count":latency.len(),"latency_sample_window":4096,
            "hls_audio_samples_per_channel":m.hls_audio_samples.load(Ordering::Relaxed)})
    }
}
