pub mod audio;
pub mod color;
pub mod display;
pub mod transport;
pub mod video;
use crate::playback::MediaTime;
use ffmpeg_next as ffmpeg;
use std::time::Instant;

/// Owned FFmpeg reference; no pixel copy at the final UI handoff.
pub struct VideoFrame {
    pub frame: ffmpeg::frame::Video,
    pub received: Instant,
    pub pts: Option<MediaTime>,
    pub epoch: u64,
    pub sequence: u64,
    pub hls: bool,
}
impl VideoFrame {
    pub fn shared(&self) -> anyhow::Result<Self> {
        let pointer = unsafe { ffmpeg::ffi::av_frame_clone(self.frame.as_ptr()) };
        anyhow::ensure!(!pointer.is_null(), "Unable to retain video frame");
        Ok(Self {
            frame: unsafe { ffmpeg::frame::Video::wrap(pointer) },
            received: self.received,
            pts: self.pts,
            epoch: self.epoch,
            sequence: self.sequence,
            hls: self.hls,
        })
    }
}
