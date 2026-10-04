use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64},
    },
};
#[derive(Default)]
pub struct Metrics {
    pub decoded: AtomicU64,
    pub estimated_av_offset_us: AtomicI64,
    pub estimated_av_available: AtomicBool,
    pub presented: AtomicU64,
    pub last_submitted_sequence: AtomicU64,
    pub last_submission_epoch: AtomicU64,
    pub last_submission_us: AtomicU64,
    pub replaced: AtomicU64,
    pub bytes: AtomicU64,
    pub latency_us: AtomicU64,
    pub latency_samples: Mutex<VecDeque<u64>>,
    pub audio_packets: AtomicU64,
    pub audio_recovered: AtomicU64,
    pub audio_errors: AtomicU64,
    pub hls_audio_samples: AtomicU64,
    pub schedule_dropped: AtomicU64,
    pub stale_dropped: AtomicU64,
    pub uploaded: AtomicU64,
    pub present_intervals_us: Mutex<VecDeque<u64>>,
}
