pub mod audio;
pub mod display;
pub mod recorder;
pub mod transport;
pub mod video;
use ffmpeg_next as ffmpeg;
use std::time::{Duration, Instant};
pub struct VideoFrame {
    pub frame: ffmpeg::frame::Video,
    pub received: Instant,
    pub timeline: Duration,
}
impl VideoFrame {
    pub fn shared(&self) -> anyhow::Result<Self> {
        // ffmpeg-next's Video::clone() copies all pixels. AVFrame references do not.
        let pointer = unsafe { ffmpeg::ffi::av_frame_clone(self.frame.as_ptr()) };
        anyhow::ensure!(!pointer.is_null(), "Unable to retain video frame");
        Ok(Self {
            frame: unsafe { ffmpeg::frame::Video::wrap(pointer) },
            received: self.received,
            timeline: self.timeline,
        })
    }
}
