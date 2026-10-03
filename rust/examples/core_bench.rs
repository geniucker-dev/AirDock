use airplay_windows::media::{VideoFrame, display::Converter};
use ffmpeg_next::{self as ffmpeg, format::Pixel, frame::Video};
use std::time::{Duration, Instant};
fn main() -> anyhow::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let width: u32 = args.get(1).map(String::as_str).unwrap_or("1920").parse()?;
    let height: u32 = args.get(2).map(String::as_str).unwrap_or("1080").parse()?;
    let count: u64 = args.get(3).map(String::as_str).unwrap_or("1200").parse()?;
    let mut frame = Video::new(Pixel::NV12, width, height);
    frame.data_mut(0).fill(100);
    frame.data_mut(1).fill(128);
    unsafe {
        (*frame.as_mut_ptr()).colorspace = ffmpeg::ffi::AVColorSpace::AVCOL_SPC_BT709;
        (*frame.as_mut_ptr()).color_range = ffmpeg::ffi::AVColorRange::AVCOL_RANGE_JPEG;
    }
    let mut converter = Converter::default();
    for _ in 0..60 {
        std::hint::black_box(converter.bgra(&frame)?);
    }
    let source = VideoFrame {
        frame,
        received: Instant::now(),
        timeline: Duration::ZERO,
    };
    let start = Instant::now();
    for _ in 0..count {
        let frame = source.shared()?;
        std::hint::black_box(converter.bgra(&frame.frame)?);
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "{}",
        serde_json::json!({"width":width,"height":height,"frames":count,"seconds":elapsed,"conversion_frames_per_second":count as f64/elapsed,"frame_reference_and_conversion_ns":elapsed*1e9/count as f64})
    );
    Ok(())
}
