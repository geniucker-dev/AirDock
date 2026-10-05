// SPDX-License-Identifier: MPL-2.0
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering},
    },
};
#[derive(Default)]
pub struct Metrics {
    pub decoded: AtomicU64,
    pub receiver_view_builds: AtomicU64,
    pub ui_frame_events: AtomicU64,
    pub video_depth: AtomicU32,
    pub video_colour_mode: AtomicU32,
    pub hdr_uploaded: AtomicU64,
    pub ten_bit_uploaded: AtomicU64,
    pub estimated_av_offset_us: AtomicI64,
    pub estimated_av_available: AtomicBool,
    pub presented: AtomicU64,
    pub last_submitted_sequence: AtomicU64,
    pub last_submission_epoch: AtomicU64,
    pub last_submission_us: AtomicU64,
    pub replaced: AtomicU64,
    pub hidden_replaced: AtomicU64,
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
impl Metrics {
    pub fn video_colour_label(&self) -> &'static str {
        match self.video_colour_mode.load(Ordering::Relaxed) {
            1 => "PQ HDR → SDR",
            2 => "HLG HDR → SDR",
            3 => "Wide-gamut SDR → sRGB",
            _ => match self.video_depth.load(Ordering::Relaxed) {
                10 => "10-bit SDR",
                8 => "8-bit SDR",
                _ => "No displayed video",
            },
        }
    }
}
