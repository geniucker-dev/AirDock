use crate::{config::Settings, media::VideoFrame};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, RwLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct UiState {
    pub peer: String,
    pub device: String,
    pub model: String,
    pub kind: String,
    pub codec: String,
    pub dimensions: String,
    pub decoder: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Arc<Vec<u8>>,
    pub paused: bool,
    pub volume_db: f32,
    pub progress_seconds: f64,
    pub duration_seconds: f64,
    pub progress_at: Option<Instant>,
    pub error: String,
    pub usb: bool,
    pub addresses: String,
    pub recording_path: String,
}
#[derive(Default)]
pub struct Metrics {
    pub decoded: AtomicU64,
    pub presented: AtomicU64,
    pub replaced: AtomicU64,
    pub bytes: AtomicU64,
    pub latency_us: AtomicU64,
    pub latency_samples: Mutex<VecDeque<u64>>,
    pub audio_packets: AtomicU64,
    pub audio_recovered: AtomicU64,
    pub audio_errors: AtomicU64,
    pub hls_audio_samples: AtomicU64,
    pub recording_dropped: AtomicU64,
}
pub struct Shared {
    pub settings: RwLock<Settings>,
    pub ui: Mutex<UiState>,
    pub frame: Mutex<Option<VideoFrame>>,
    pub metrics: Metrics,
    pub running: AtomicBool,
    pub disconnect: AtomicU64,
    pub recording: AtomicBool,
    pub recorder: Mutex<Option<crate::media::recorder::Recorder>>,
    pub owner: Mutex<Option<Arc<SessionOwner>>>,
    pub started: Instant,
    pub frame_ready: Condvar,
    pub weak: Weak<Self>,
    finalizers: Mutex<Vec<thread::JoinHandle<()>>>,
    #[cfg(test)]
    pub test_pcm: Mutex<Vec<i16>>,
}
pub struct SessionOwner {
    pub id: u64,
    pub peer: std::net::IpAddr,
    pub stop: AtomicBool,
    pub done: Mutex<bool>,
    pub ready: Condvar,
}
impl Shared {
    pub fn new(settings: Settings) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            settings: RwLock::new(settings),
            ui: Mutex::new(UiState::default()),
            frame: Mutex::new(None),
            metrics: Metrics::default(),
            running: AtomicBool::new(true),
            disconnect: AtomicU64::new(0),
            recording: AtomicBool::new(false),
            recorder: Mutex::new(None),
            owner: Mutex::new(None),
            started: Instant::now(),
            frame_ready: Condvar::new(),
            weak: weak.clone(),
            finalizers: Mutex::new(Vec::new()),
            #[cfg(test)]
            test_pcm: Mutex::new(Vec::new()),
        })
    }
    pub fn publish(&self, frame: VideoFrame) {
        self.metrics.decoded.fetch_add(1, Ordering::Relaxed);
        if self.recording.load(Ordering::Relaxed) {
            let mut recorder = self.recorder.lock().unwrap();
            if recorder.is_none() {
                match crate::media::recorder::Recorder::start(self, &frame) {
                    Ok(r) => *recorder = Some(r),
                    Err(e) => {
                        self.report(format!("Recording: {e:#}"));
                        self.recording.store(false, Ordering::Relaxed);
                    }
                }
            }
            if let Some(r) = recorder.as_ref()
                && !r.video(&frame)
            {
                self.metrics
                    .recording_dropped
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        if self.frame.lock().unwrap().replace(frame).is_some() {
            self.metrics.replaced.fetch_add(1, Ordering::Relaxed);
        }
        self.frame_ready.notify_one();
    }
    pub fn pcm(&self, samples: Arc<Vec<i16>>, rate: u32) {
        #[cfg(test)]
        self.test_pcm.lock().unwrap().extend_from_slice(&samples);
        if let Some(r) = self.recorder.lock().unwrap().as_ref()
            && !r.audio(samples, rate, self.started.elapsed())
        {
            self.metrics
                .recording_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn stop_recording(&self) {
        self.recording.store(false, Ordering::Relaxed);
        let recorder = self.recorder.lock().unwrap().take();
        if let Some(r) = recorder {
            let weak = self.weak.clone();
            let job = thread::spawn(move || {
                if let Err(e) = r.stop()
                    && let Some(shared) = weak.upgrade()
                {
                    shared.report(format!("Recording: {e:#}"));
                }
            });
            let mut jobs = self.finalizers.lock().unwrap();
            let mut i = 0;
            while i < jobs.len() {
                if jobs[i].is_finished() {
                    let job = jobs.swap_remove(i);
                    let _ = job.join();
                } else {
                    i += 1
                }
            }
            jobs.push(job);
        }
    }
    pub fn finish_recordings(&self) {
        self.stop_recording();
        for job in self.finalizers.lock().unwrap().drain(..) {
            let _ = job.join();
        }
    }
    pub fn request_disconnect(&self) {
        self.disconnect.fetch_add(1, Ordering::Relaxed);
    }
    pub fn wait_for_frame(&self, timeout: Duration) {
        let frame = self.frame.lock().unwrap();
        if frame.is_none() {
            let _ = self.frame_ready.wait_timeout(frame, timeout).unwrap();
        }
    }
    pub fn wait_for_activity(&self, timeout: Duration) {
        // Hidden windows retain their latest frame for restoration. Waiting
        // must still block when that frame slot is occupied.
        let frame = self.frame.lock().unwrap();
        let _ = self.frame_ready.wait_timeout(frame, timeout).unwrap();
    }
    pub fn report(&self, error: String) {
        tracing::error!("{error}");
        self.ui.lock().unwrap().error = error;
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
        serde_json::json!({"elapsed_seconds":self.started.elapsed().as_secs_f64(),
            "decoded_frames":m.decoded.load(Ordering::Relaxed),"presented_frames":m.presented.load(Ordering::Relaxed),
            "replaced_frames":m.replaced.load(Ordering::Relaxed),"last_receive_to_present_us":m.latency_us.load(Ordering::Relaxed),
            "audio_packets":m.audio_packets.load(Ordering::Relaxed),"audio_recovered":m.audio_recovered.load(Ordering::Relaxed),
            "audio_errors":m.audio_errors.load(Ordering::Relaxed),"recording_dropped":m.recording_dropped.load(Ordering::Relaxed),
            "p95_receive_to_present_us":p95,"latency_sample_count":latency.len(),"latency_sample_window":4096,
            "hls_audio_samples_per_channel":m.hls_audio_samples.load(Ordering::Relaxed)})
    }
}
