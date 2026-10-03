use super::VideoFrame;
use crate::{config::Settings, state::Shared};
use anyhow::{Context, Result, bail, ensure};
use ffmpeg_next::{
    self as ffmpeg, ChannelLayout, Dictionary, Packet, Rational, codec, encoder, format, frame,
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

enum Message {
    Video(VideoFrame),
    Audio(Arc<Vec<i16>>, u32, Duration),
}
#[derive(Default)]
struct Pending {
    video: VecDeque<VideoFrame>,
    audio: VecDeque<(Arc<Vec<i16>>, u32, Duration)>,
    samples: usize,
    closed: bool,
}
#[derive(Default)]
struct Queue {
    pending: Mutex<Pending>,
    ready: Condvar,
}
impl Queue {
    fn next(&self) -> Option<Message> {
        let p = self.pending.lock().unwrap();
        let mut p = self
            .ready
            .wait_while(p, |p| !p.closed && p.video.is_empty() && p.audio.is_empty())
            .unwrap();
        let video = match (p.video.front(), p.audio.front()) {
            (Some(v), Some((_, _, time))) => v.timeline <= *time,
            (Some(_), None) => true,
            _ => false,
        };
        if video {
            p.video.pop_front().map(Message::Video)
        } else {
            p.audio.pop_front().map(|(pcm, rate, time)| {
                p.samples -= pcm.len();
                Message::Audio(pcm, rate, time)
            })
        }
    }
    fn close(&self) {
        self.pending.lock().unwrap().closed = true;
        self.ready.notify_all();
    }
}
pub struct Recorder {
    queue: Arc<Queue>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.queue.close();
    }
}
impl Recorder {
    pub fn start(shared: &Shared, first: &VideoFrame) -> Result<Self> {
        let settings = shared.settings.read().unwrap().clone();
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        static NEXT_RECORDING: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT_RECORDING.fetch_add(1, Ordering::Relaxed);
        let path = settings
            .recording_directory
            .join(format!("AirPlay-{stamp}-{serial}.mp4"));
        let width = first.frame.width() & !1;
        let height = first.frame.height() & !1;
        let origin = first.timeline;
        let started = first.received;
        let queue = Arc::new(Queue::default());
        let receiver = queue.clone();
        let weak = shared.weak.clone();
        let output_path = path.clone();
        let worker = thread::Builder::new()
            .name("recording".into())
            .spawn(move || {
                let result = (|| {
                    std::fs::create_dir_all(&settings.recording_directory)?;
                    let mut mux = Mux::new(&settings, output_path, width, height, origin, started)?;
                    while let Some(message) = receiver.next() {
                        match message {
                            Message::Video(f) => mux.video(f)?,
                            Message::Audio(p, r, t) => mux.audio(&p, r, t)?,
                        }
                    }
                    mux.finish()
                })();
                if result.is_err() {
                    let mut pending = receiver.pending.lock().unwrap();
                    pending.closed = true;
                    pending.video.clear();
                    pending.audio.clear();
                    pending.samples = 0;
                }
                if let Err(e) = &result
                    && let Some(shared) = weak.upgrade()
                {
                    shared.report(format!("Recording: {e:#}"));
                    shared
                        .recording
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
                result
            })?;
        shared.ui.lock().unwrap().recording_path = path.to_string_lossy().into_owned();
        Ok(Self {
            queue,
            worker: Some(worker),
        })
    }
    pub fn video(&self, f: &VideoFrame) -> bool {
        let Ok(frame) = f.shared() else {
            return false;
        };
        let mut p = self.queue.pending.lock().unwrap();
        if p.closed {
            return false;
        }
        // Match the reference's separate 90-frame video queue. Dropping the
        // oldest frame keeps an overloaded encoder near the current timeline.
        let intact = p.video.len() < 90;
        if !intact {
            p.video.pop_front();
        }
        p.video.push_back(frame);
        drop(p);
        self.queue.ready.notify_one();
        intact
    }
    pub fn audio(&self, pcm: Arc<Vec<i16>>, rate: u32, time: Duration) -> bool {
        let mut p = self.queue.pending.lock().unwrap();
        if p.closed || rate == 0 {
            return false;
        }
        let limit = rate as usize * 2 * 10;
        if pcm.len() > limit {
            return false;
        }
        let mut intact = true;
        while p.samples + pcm.len() > limit {
            if let Some((old, _, _)) = p.audio.pop_front() {
                p.samples -= old.len();
                intact = false;
            } else {
                break;
            }
        }
        p.samples += pcm.len();
        p.audio.push_back((pcm, rate, time));
        drop(p);
        self.queue.ready.notify_one();
        intact
    }
    pub fn stop(mut self) -> Result<()> {
        self.queue.close();
        self.worker
            .take()
            .unwrap()
            .join()
            .map_err(|_| anyhow::anyhow!("Recording worker panicked"))?
    }
}

fn video_encoder(settings: &Settings, width: u32, height: u32) -> Result<encoder::video::Encoder> {
    let hevc = settings.recording_codec == "hevc";
    let gpu = if hevc {
        vec!["hevc_nvenc", "hevc_qsv", "hevc_amf", "hevc_mf"]
    } else {
        vec!["h264_nvenc", "h264_qsv", "h264_amf", "h264_mf"]
    };
    let cpu = if hevc { "libx265" } else { "libx264" };
    let mut candidates = if settings.recording_encoder == "cpu" {
        vec![]
    } else {
        gpu
    };
    if settings.recording_encoder != "gpu" {
        candidates.push(cpu);
    }
    let mut errors = Vec::new();
    for name in candidates {
        let Some(codec) = encoder::find_by_name(name) else {
            continue;
        };
        let attempt = (|| -> Result<_> {
            let formats = codec.video()?.formats().map(|f| f.collect::<Vec<_>>());
            let pixel = if formats
                .as_ref()
                .is_none_or(|f| f.contains(&ffmpeg::format::Pixel::YUV420P))
            {
                ffmpeg::format::Pixel::YUV420P
            } else if formats
                .as_ref()
                .is_some_and(|f| f.contains(&ffmpeg::format::Pixel::NV12))
            {
                ffmpeg::format::Pixel::NV12
            } else {
                bail!("Encoder has no supported CPU input format");
            };
            let mut e = codec::Context::new_with_codec(codec).encoder().video()?;
            e.set_width(width);
            e.set_height(height);
            e.set_format(pixel);
            e.set_time_base((1, 1000));
            e.set_frame_rate(Some((settings.max_fps as i32, 1)));
            e.set_bit_rate(settings.recording_mbps as usize * 1_000_000);
            e.set_gop(settings.max_fps * 2);
            e.set_max_b_frames(0);
            e.set_flags(codec::Flags::GLOBAL_HEADER);
            unsafe {
                (*e.as_mut_ptr()).color_range = ffmpeg::ffi::AVColorRange::AVCOL_RANGE_JPEG;
                (*e.as_mut_ptr()).colorspace = ffmpeg::ffi::AVColorSpace::AVCOL_SPC_BT709;
            }
            let mut options = Dictionary::new();
            if name == cpu {
                options.set("preset", "veryfast");
                options.set("tune", "zerolatency");
                if hevc {
                    options.set("x265-params", "log-level=error:pools=2");
                }
            }
            if name.ends_with("nvenc") {
                options.set("preset", "p4");
                options.set("tune", "ll");
            }
            Ok(e.open_with(options)?)
        })();
        match attempt {
            Ok(e) => return Ok(e),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    bail!(
        "No {} recording encoder is usable: {}",
        settings.recording_encoder,
        errors.join("; ")
    )
}
struct Mux {
    output: format::context::Output,
    video: encoder::video::Encoder,
    audio: encoder::audio::Encoder,
    width: u32,
    height: u32,
    origin: Duration,
    last_video: i64,
    video_duration: i64,
    started: Instant,
    last_frame: Option<frame::Video>,
    audio_next: i64,
    audio_started: bool,
    pcm: VecDeque<i16>,
    scaler: Option<ffmpeg::software::scaling::Context>,
    resampler: Option<ffmpeg::software::resampling::Context>,
}
impl Mux {
    fn new(
        settings: &Settings,
        path: PathBuf,
        width: u32,
        height: u32,
        origin: Duration,
        started: Instant,
    ) -> Result<Self> {
        let video = video_encoder(settings, width, height)?;
        let audio_codec =
            encoder::find(codec::Id::AAC).ok_or_else(|| anyhow::anyhow!("AAC encoder missing"))?;
        let mut audio = codec::Context::new_with_codec(audio_codec)
            .encoder()
            .audio()?;
        audio.set_rate(44100);
        audio.set_channel_layout(ChannelLayout::STEREO);
        audio.set_format(ffmpeg::format::Sample::F32(
            ffmpeg::format::sample::Type::Planar,
        ));
        audio.set_bit_rate(192000);
        audio.set_time_base((1, 44100));
        audio.set_flags(codec::Flags::GLOBAL_HEADER);
        let audio = audio.open_as(audio_codec)?;
        let mut output = format::output(&path).context("Open recording file")?;
        {
            let mut stream = output.add_stream(video.codec())?;
            stream.set_time_base((1, 1000));
            stream.set_parameters(&video);
        }
        {
            let mut stream = output.add_stream(audio_codec)?;
            stream.set_time_base((1, 44100));
            stream.set_parameters(&audio);
        }
        output.write_header()?;
        Ok(Self {
            output,
            video,
            audio,
            width,
            height,
            origin,
            last_video: -1,
            video_duration: (1000 / settings.max_fps as i64).max(1),
            started,
            last_frame: None,
            audio_next: 0,
            audio_started: false,
            pcm: VecDeque::new(),
            scaler: None,
            resampler: None,
        })
    }
    fn video(&mut self, input: VideoFrame) -> Result<()> {
        let f = &input.frame;
        let aspect =
            (self.width as f64 / f.width() as f64).min(self.height as f64 / f.height() as f64);
        let w = ((f.width() as f64 * aspect).round() as u32 & !1).max(2);
        let h = ((f.height() as f64 * aspect).round() as u32 & !1).max(2);
        let def = ffmpeg::software::scaling::context::Definition {
            format: f.format(),
            width: f.width(),
            height: f.height(),
        };
        if self
            .scaler
            .as_ref()
            .is_none_or(|s| s.input() != &def || s.output().width != w || s.output().height != h)
        {
            self.scaler = Some(ffmpeg::software::scaling::Context::get(
                f.format(),
                f.width(),
                f.height(),
                self.video.format(),
                w,
                h,
                ffmpeg::software::scaling::Flags::FAST_BILINEAR,
            )?);
        }
        let scaler = self.scaler.as_mut().unwrap();
        unsafe {
            let src = ffmpeg::ffi::sws_getCoefficients(super::display::colorspace(f));
            let dst = ffmpeg::ffi::sws_getCoefficients(ffmpeg::ffi::SWS_CS_ITU709);
            let range =
                i32::from((*f.as_ptr()).color_range != ffmpeg::ffi::AVColorRange::AVCOL_RANGE_MPEG);
            ensure!(
                ffmpeg::ffi::sws_setColorspaceDetails(
                    scaler.as_mut_ptr(),
                    src,
                    range,
                    dst,
                    1,
                    0,
                    1 << 16,
                    1 << 16
                ) >= 0,
                "Recording color conversion failed"
            );
        }
        let mut scaled = frame::Video::empty();
        scaler.run(f, &mut scaled)?;
        let mut target = if w == self.width && h == self.height {
            scaled
        } else {
            let mut target = frame::Video::new(self.video.format(), self.width, self.height);
            let nv12 = self.video.format() == ffmpeg::format::Pixel::NV12;
            for plane in 0..if nv12 { 2 } else { 3 } {
                let vertical = if plane == 0 { 1 } else { 2 };
                let horizontal = if nv12 { 1 } else { vertical };
                let pitch = target.stride(plane);
                let src_pitch = scaled.stride(plane);
                let (pw, ph) = (w as usize / horizontal, h as usize / vertical);
                let x = (((self.width - w) / 2) & !1) as usize / horizontal;
                let y = (((self.height - h) / 2) & !1) as usize / vertical;
                target
                    .data_mut(plane)
                    .fill(if plane == 0 { 0 } else { 128 });
                for row in 0..ph {
                    target.data_mut(plane)[(y + row) * pitch + x..(y + row) * pitch + x + pw]
                        .copy_from_slice(
                            &scaled.data(plane)[row * src_pitch..row * src_pitch + pw],
                        );
                }
            }
            target
        };
        let pts = (input.timeline.saturating_sub(self.origin).as_millis() as i64)
            .max(self.last_video + 1);
        if self.last_video >= 0 {
            self.video_duration = (pts - self.last_video).max(1);
        }
        self.last_video = pts;
        target.set_pts(Some(pts));
        unsafe {
            (*target.as_mut_ptr()).color_range = ffmpeg::ffi::AVColorRange::AVCOL_RANGE_JPEG;
            (*target.as_mut_ptr()).colorspace = ffmpeg::ffi::AVColorSpace::AVCOL_SPC_BT709;
        }
        self.video.send_frame(&target)?;
        let retained = unsafe { ffmpeg::ffi::av_frame_clone(target.as_ptr()) };
        ensure!(!retained.is_null(), "Retain final recording frame");
        self.last_frame = Some(unsafe { frame::Video::wrap(retained) });
        self.drain_video()
    }
    fn drain_video(&mut self) -> Result<()> {
        drain(
            &mut self.video,
            &mut self.output,
            0,
            Rational(1, 1000),
            self.video_duration,
        )
    }
    fn audio(&mut self, pcm: &[i16], rate: u32, time: Duration) -> Result<()> {
        ensure!(
            (8000..=192000).contains(&rate) && pcm.len().is_multiple_of(2),
            "Invalid recording PCM"
        );
        let converted;
        let pcm = if rate != 44100 {
            let format = ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed);
            if self
                .resampler
                .as_ref()
                .is_none_or(|r| r.input().rate != rate)
            {
                self.resampler = Some(ffmpeg::software::resampling::Context::get(
                    format,
                    ChannelLayout::STEREO,
                    rate,
                    format,
                    ChannelLayout::STEREO,
                    44100,
                )?);
            }
            let mut input = frame::Audio::new(format, pcm.len() / 2, ChannelLayout::STEREO);
            input.set_rate(rate);
            let bytes =
                unsafe { std::slice::from_raw_parts(pcm.as_ptr().cast::<u8>(), pcm.len() * 2) };
            input.data_mut(0)[..bytes.len()].copy_from_slice(bytes);
            let mut output = frame::Audio::new(
                format,
                (pcm.len() / 2 * 44100).div_ceil(rate as usize) + 256,
                ChannelLayout::STEREO,
            );
            self.resampler.as_mut().unwrap().run(&input, &mut output)?;
            converted = unsafe {
                std::slice::from_raw_parts(
                    (*output.as_ptr()).data[0].cast::<i16>(),
                    output.samples() * 2,
                )
            }
            .to_vec();
            converted.as_slice()
        } else {
            pcm
        };
        let end = (time.saturating_sub(self.origin).as_secs_f64() * 44100.) as i64;
        let start = (end - pcm.len() as i64 / 2).max(0);
        if !self.audio_started {
            self.audio_next = start;
            self.audio_started = true;
        }
        let buffered = self.pcm.len() as i64 / 2;
        let gap = start - self.audio_next - buffered;
        // Small scheduling differences do not become discontinuities. Larger
        // pauses retain the common video clock, with bounded silence insertion.
        if gap > 4410 {
            if gap > 22050 {
                self.encode_audio(true)?;
                self.audio_next = start;
            } else {
                self.pcm.extend(std::iter::repeat_n(0, gap as usize * 2));
            }
        }
        self.pcm.extend(pcm.iter().copied());
        self.encode_audio(false)
    }
    fn encode_audio(&mut self, finish: bool) -> Result<()> {
        let n = self.audio.frame_size() as usize;
        while self.pcm.len() >= n * 2 || finish && !self.pcm.is_empty() {
            let mut frame = frame::Audio::new(self.audio.format(), n, ChannelLayout::STEREO);
            frame.set_rate(44100);
            frame.set_pts(Some(self.audio_next));
            for i in 0..n {
                for ch in 0..2 {
                    frame.plane_mut::<f32>(ch)[i] =
                        self.pcm.pop_front().unwrap_or(0) as f32 / 32768.;
                }
            }
            self.audio_next += n as i64;
            self.audio.send_frame(&frame)?;
            drain(&mut self.audio, &mut self.output, 1, Rational(1, 44100), 0)?;
        }
        Ok(())
    }
    fn finish(mut self) -> Result<()> {
        if let Some(resampler) = &mut self.resampler
            && let Some(delay) = resampler.delay()
        {
            let format = ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed);
            let mut output =
                frame::Audio::new(format, delay.output as usize + 32, ChannelLayout::STEREO);
            resampler.flush(&mut output)?;
            let samples = unsafe {
                std::slice::from_raw_parts(
                    (*output.as_ptr()).data[0].cast::<i16>(),
                    output.samples() * 2,
                )
            };
            self.pcm.extend(samples.iter().copied());
        }
        self.encode_audio(true)?;
        // A sender can stop sending video while audio continues, or while the
        // receiver is paused. Preserve the held image until recording stops.
        let final_pts = self.started.elapsed().as_millis() as i64;
        if final_pts > self.last_video + self.video_duration
            && let Some(mut frame) = self.last_frame.take()
        {
            frame.set_pts(Some(final_pts));
            self.video.send_frame(&frame)?;
            self.drain_video()?;
        }
        self.video.send_eof()?;
        self.drain_video()?;
        self.audio.send_eof()?;
        drain(&mut self.audio, &mut self.output, 1, Rational(1, 44100), 0)?;
        self.output.write_trailer()?;
        Ok(())
    }
}
fn drain(
    encoder: &mut encoder::Encoder,
    output: &mut format::context::Output,
    index: usize,
    timebase: Rational,
    duration: i64,
) -> Result<()> {
    loop {
        let mut packet = Packet::empty();
        match encoder.receive_packet(&mut packet) {
            Ok(()) => {}
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(e) => return Err(e.into()),
        }
        packet.set_stream(index);
        if packet.duration() <= 0 && duration > 0 {
            packet.set_duration(duration);
        }
        packet.rescale_ts(timebase, output.stream(index).unwrap().time_base());
        packet.write_interleaved(output)?;
    }
    Ok(())
}
