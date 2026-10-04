use anyhow::{Result, ensure};
use ffmpeg_next as ffmpeg;
fn main() -> Result<()> {
    ffmpeg::init()?;
    let mut total = 0;
    let mut formats = Vec::new();
    for path in std::env::args().skip(1) {
        let mut input = ffmpeg::format::input(&path)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| anyhow::anyhow!("Missing video stream"))?;
        let index = stream.index();
        let mut decoder = ffmpeg::codec::Context::from_parameters(stream.parameters())?
            .decoder()
            .video()?;
        let mut count = 0;
        for (stream, packet) in input.packets() {
            if stream.index() != index {
                continue;
            }
            decoder.send_packet(&packet)?;
            loop {
                let mut frame = ffmpeg::frame::Video::empty();
                match decoder.receive_frame(&mut frame) {
                    Ok(()) => {
                        let planes = airplay_windows::media::display::describe(&frame)?;
                        formats.push(format!("{:?}", planes.layout));
                        count += 1;
                    }
                    Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
                    Err(e) => return Err(e.into()),
                }
            }
        }
        decoder.send_eof()?;
        loop {
            let mut f = ffmpeg::frame::Video::empty();
            if decoder.receive_frame(&mut f).is_err() {
                break;
            }
            airplay_windows::media::display::describe(&f)?;
            count += 1;
        }
        ensure!(count > 0, "No decoded frames");
        total += count;
    }
    formats.sort();
    formats.dedup();
    println!(
        "{}",
        serde_json::json!({"decoded_frames":total,"validated_formats":formats})
    );
    Ok(())
}
