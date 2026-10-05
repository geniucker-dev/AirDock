// SPDX-License-Identifier: MPL-2.0
use ffmpeg_next::{format::Pixel, frame::Video};
fn main() -> anyhow::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let width = args.get(1).map(String::as_str).unwrap_or("1920").parse()?;
    let height = args.get(2).map(String::as_str).unwrap_or("1080").parse()?;
    let count: u64 = args.get(3).map(String::as_str).unwrap_or("1200").parse()?;
    let frame = Video::new(Pixel::NV12, width, height);
    let start = std::time::Instant::now();
    for _ in 0..count {
        std::hint::black_box(airdock::media::display::describe(&frame)?);
    }
    println!(
        "{}",
        serde_json::json!({"width":width,"height":height,"frames":count,"validation_ns_per_frame":start.elapsed().as_secs_f64()*1e9/count as f64,"measurement":"plane validation only; not GPU upload, display FPS or conversion benchmark"})
    );
    Ok(())
}
