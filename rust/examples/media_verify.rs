use airplay_windows::{
    config::Settings,
    media::{
        VideoFrame,
        display::Converter,
        recorder::Recorder,
        video::{Configuration, Decoder},
    },
    state::Shared,
};
use anyhow::{Context, Result, ensure};
use clap::Parser;
use ffmpeg_next::{self as ffmpeg, media};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Parser)]
struct Arguments {
    output_directory: PathBuf,
    inputs: Vec<PathBuf>,
    #[arg(long, default_value_t = 44100)]
    audio_rate: u32,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = 0)]
    hold_ms: u64,
}
fn main() -> Result<()> {
    ffmpeg::init()?;
    let args = Arguments::parse();
    ensure!(
        !args.inputs.is_empty() && args.audio_rate >= 8000 && args.audio_rate <= 192000,
        "Invalid test inputs"
    );
    let settings = Settings {
        recording_directory: args.output_directory,
        recording_encoder: "cpu".into(),
        recording_codec: args.codec,
        ..Default::default()
    };
    let shared = Shared::new(settings);
    let mut recorder = None;
    let mut decoder = None::<Decoder>;
    let mut total = 0u64;
    let mut converter = Converter::default();
    let mut output_path = String::new();
    for path in &args.inputs {
        let mut input = ffmpeg::format::input(path)?;
        let stream = input
            .streams()
            .best(media::Type::Video)
            .context("Missing video stream")?;
        let index = stream.index();
        let parameters = stream.parameters();
        let extra = unsafe {
            let p = parameters.as_ptr();
            ensure!((*p).extradata_size > 0, "Missing codec configuration");
            std::slice::from_raw_parts((*p).extradata, (*p).extradata_size as usize)
        };
        let configuration = Configuration::parse(extra)?;
        if let Some(d) = &mut decoder {
            d.reconfigure(configuration, false)?;
        } else {
            decoder = Some(Decoder::new(configuration, false)?);
        }
        let decoder = decoder.as_mut().unwrap();
        let mut count = 0;
        for (stream, packet) in input.packets() {
            if stream.index() != index {
                continue;
            }
            let mut bytes = packet.data().context("Empty video packet")?.to_vec();
            for frame in decoder.decode(&mut bytes)? {
                ensure!(
                    frame.width() > 0 && frame.height() > 0,
                    "Invalid decoded dimensions"
                );
                converter.bgra(&frame)?;
                let timeline = Duration::from_micros(total * 1_000_000 / 30);
                let frame = VideoFrame {
                    frame,
                    received: Instant::now(),
                    timeline,
                };
                if recorder.is_none() {
                    let r = Recorder::start(&shared, &frame)?;
                    output_path = shared.ui.lock().unwrap().recording_path.clone();
                    recorder = Some(r);
                }
                let r = recorder.as_ref().unwrap();
                ensure!(r.video(&frame), "Recording video queue full");
                let samples = args.audio_rate as u64 / 30;
                let mut pcm = Vec::with_capacity(samples as usize * 2);
                for i in 0..samples {
                    let phase = (total * samples + i) as f64 * 2. * std::f64::consts::PI * 440.
                        / args.audio_rate as f64;
                    let value = (phase.sin() * 8000.) as i16;
                    pcm.extend_from_slice(&[value, value]);
                }
                ensure!(
                    r.audio(
                        Arc::new(pcm),
                        args.audio_rate,
                        Duration::from_micros((total + 1) * 1_000_000 / 30)
                    ),
                    "Recording audio queue full"
                );
                total += 1;
                count += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        ensure!(
            count == 30,
            "Expected 30 decoded frames from {}, got {count}",
            path.display()
        );
    }
    std::thread::sleep(Duration::from_millis(args.hold_ms));
    recorder.context("No decoded frames")?.stop()?;
    println!(
        "{}",
        serde_json::json!({"decoded_frames":total,"recording":output_path})
    );
    Ok(())
}
