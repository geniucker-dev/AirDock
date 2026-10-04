//! Audio device ownership and sample conversion live on the output control thread.
pub mod ring;
use crate::{playback::AudioClock, state::Shared};
use anyhow::{Context, Result, bail};
use cpal::{
    SampleFormat, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};

enum Command {
    Pcm(Arc<Vec<i16>>, Option<i64>, u64, f32),
    Drain(mpsc::Sender<()>),
}
enum Control {
    Flush(u64),
    Stop,
    Hls,
}
pub struct Sink {
    control: mpsc::Sender<Control>,
    send: SyncSender<Command>,
    worker: Option<thread::JoinHandle<()>>,
    gain: f32,
    epoch: u64,
    clock: Arc<AudioClock>,
    stop: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}
impl Sink {
    pub fn for_session(rate: u32, channels: u8, shared: &Arc<Shared>) -> Result<Self> {
        let settings = shared.settings.read().unwrap().clone();
        Self::open(
            rate,
            channels,
            settings.audio_device,
            shared.media.audio_clock.clone(),
            shared.sessions.epoch.clone(),
            Some(shared.settings.clone()),
            Some(shared.ui.clone()),
        )
    }
    pub fn for_hls(rate: u32, channels: u8, shared: &Arc<Shared>) -> Result<Self> {
        let mut sink = Self::for_session(rate, channels, shared)?;
        let _ = sink.control.send(Control::Hls);
        sink.gain = 1.;
        Ok(sink)
    }
    pub fn new(rate: u32, channels: u8) -> Result<Self> {
        Self::open(
            rate,
            channels,
            "default".into(),
            Arc::new(AudioClock::default()),
            Arc::new(AtomicU64::new(1)),
            None,
            None,
        )
    }
    fn open(
        rate: u32,
        channels: u8,
        device: String,
        clock: Arc<AudioClock>,
        generation: Arc<AtomicU64>,
        settings: Option<Arc<std::sync::RwLock<crate::config::Settings>>>,
        status: Option<Arc<crate::status::Status>>,
    ) -> Result<Self> {
        anyhow::ensure!(rate > 0 && matches!(channels, 1 | 2), "Invalid PCM format");
        let epoch = generation.load(Ordering::Acquire);
        let current = generation.clone();
        let (send, recv) = mpsc::sync_channel(16);
        let (control, controls) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let c = clock.clone();
        let s = stop.clone();
        let null = cfg!(test) || std::env::var_os("AIRPLAY_AUDIO_NULL").is_some();
        let worker = thread::Builder::new()
            .name("audio-output".into())
            .spawn(move || {
                let mut device = device;
                let mut output = None::<Output>;
                let mut retry = std::time::Instant::now();
                let mut media_next = 0i64;
                let mut hls = false;
                let mut draining = None::<(mpsc::Sender<()>, Option<std::time::Instant>)>;
                while !s.load(Ordering::Acquire) {
                    for command in controls.try_iter() {
                        match command {
                            Control::Hls => {
                                hls = true;
                                if let Some(o) = output.as_mut() {
                                    o.low_latency = false;
                                }
                            }
                            Control::Stop => {
                                s.store(true, Ordering::Release);
                            }
                            Control::Flush(e) => {
                                output.take();
                                c.invalidate();
                                c.epoch.store(e, Ordering::Release);
                                retry = std::time::Instant::now();
                                media_next = 0;
                                draining.take();
                            }
                        }
                    }
                    if let Some(settings) = &settings {
                        let selected = settings.read().unwrap().audio_device.clone();
                        if selected != device {
                            device = selected;
                            output.take();
                            c.invalidate();
                            retry = std::time::Instant::now();
                        }
                    }
                    if !null
                        && (output
                            .as_ref()
                            .is_some_and(|o| o.failed.load(Ordering::Acquire))
                            || output.is_none())
                        && std::time::Instant::now() >= retry
                    {
                        output.take();
                        c.invalidate();
                        match Output::open(&device, c.clone(), current.clone()) {
                            Ok(o) => {
                                if let Some(status) = &status {
                                    status.lock().unwrap().audio_status = format!(
                                        "{} Hz · {} channels",
                                        o.config.sample_rate, o.config.channels
                                    );
                                }
                                tracing::info!(
                                    "Audio output opened: {} Hz, {} channels",
                                    o.config.sample_rate,
                                    o.config.channels
                                );
                                let mut o = o;
                                o.low_latency = !hls;
                                output = Some(o)
                            }
                            Err(e) => {
                                if let Some(status) = &status {
                                    status.lock().unwrap().audio_status =
                                        "Output device unavailable; retrying".into();
                                }
                                tracing::warn!("Audio output unavailable; retrying: {e:#}");
                                retry = std::time::Instant::now() + Duration::from_secs(1);
                            }
                        }
                    }
                    let pending = output.as_mut().is_some_and(|o| {
                        o.pump();
                        !o.pending.is_empty()
                    });
                    if let Some((ack, empty_at)) = draining.as_mut() {
                        if let Some(o) = output.as_mut() {
                            o.prefilled.store(true, Ordering::Release);
                            if !pending && o.producer.free() == o.capacity {
                                let at = empty_at.get_or_insert_with(std::time::Instant::now);
                                if at.elapsed().as_micros()
                                    >= c.output_latency_us.load(Ordering::Relaxed) as u128
                                {
                                    let _ = ack.send(());
                                    draining.take();
                                }
                            }
                        } else {
                            let _ = ack.send(());
                            draining.take();
                        }
                    }
                    if pending {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    match recv.recv_timeout(Duration::from_millis(if draining.is_some() {
                        20
                    } else {
                        100
                    })) {
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(Command::Pcm(pcm, pts, e, gain)) => {
                            if null || e != current.load(Ordering::Acquire) {
                                continue;
                            }
                            if let Some(o) = output.as_mut() {
                                if pts.is_some() {
                                    o.clocked = true;
                                }
                                let start = pts.unwrap_or(media_next);
                                media_next = start
                                    + (pcm.len() / channels as usize) as i64 * 1_000_000
                                        / rate as i64;
                                if let Err(err) = o.push(&pcm, rate, channels, start, e, gain) {
                                    tracing::warn!("Audio conversion: {err:#}");
                                    o.failed.store(true, Ordering::Release);
                                }
                            }
                        }
                        Ok(Command::Drain(ack)) => {
                            if let Some(o) = output.as_mut()
                                && let Err(e) = o.finish()
                            {
                                tracing::warn!("Audio resampler drain: {e:#}");
                            }
                            draining = Some((ack, None));
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                drop(output);
                c.invalidate();
            })?;
        Ok(Self {
            control,
            send,
            worker: Some(worker),
            gain: 1.,
            epoch,
            clock,
            stop,
            generation,
        })
    }
    pub fn set_epoch(&mut self, e: u64) {
        self.epoch = e;
    }
    pub fn volume(&mut self, db: f32) {
        self.gain = if db <= -100. {
            0.
        } else {
            10f32.powf(db.min(0.) / 20.)
        };
    }
    pub fn flush(&mut self) {
        self.clock.invalidate();
        let _ = self.control.send(Control::Flush(self.epoch));
    }
    /// Natural EOF drains queued PCM. Cancellation/seek still uses immediate Drop/FLUSH.
    pub fn drain(&mut self, cancel: &AtomicBool) -> Result<()> {
        let (ack, done) = mpsc::channel();
        let mut command = Command::Drain(ack);
        let start = std::time::Instant::now();
        loop {
            if cancel.load(Ordering::Acquire)
                || self.epoch != self.generation.load(Ordering::Acquire)
            {
                return Ok(());
            }
            match self.send.try_send(command) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(c)) => command = c,
                Err(mpsc::TrySendError::Disconnected(_)) => return Ok(()),
            }
            thread::sleep(Duration::from_millis(2));
        }
        loop {
            if cancel.load(Ordering::Acquire)
                || self.epoch != self.generation.load(Ordering::Acquire)
            {
                return Ok(());
            }
            match done.recv_timeout(Duration::from_millis(20)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            anyhow::ensure!(
                start.elapsed() < Duration::from_secs(10),
                "Audio drain timed out"
            );
        }
    }
    pub fn push(&mut self, pcm: &[i16]) -> Result<()> {
        self.push_at(pcm, None)
    }
    pub fn push_at(&mut self, pcm: &[i16], pts: Option<i64>) -> Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        self.push_shared(Arc::new(pcm.to_vec()), pts)
    }
    pub fn push_shared(&mut self, pcm: Arc<Vec<i16>>, pts: Option<i64>) -> Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        let mut command = Command::Pcm(pcm, pts, self.epoch, self.gain);
        loop {
            match self.send.try_send(command) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(anyhow::anyhow!("Audio output thread stopped"));
                }
                Err(mpsc::TrySendError::Full(c)) => {
                    command = c;
                    if self.epoch != self.generation.load(Ordering::Acquire)
                        || self.stop.load(Ordering::Acquire)
                    {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.control.send(Control::Stop);
        if let Some(t) = self.worker.take() {
            let _ = t.join();
        }
    }
}

struct Output {
    _stream: cpal::Stream,
    config: StreamConfig,
    producer: ring::Producer,
    epoch: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    prefilled: Arc<AtomicBool>,
    resampler: Option<ffmpeg_next::software::resampling::Context>,
    clocked: bool,
    format: SampleFormat,
    pending: std::collections::VecDeque<ring::Sample>,
    low_latency: bool,
    occupancy: f64,
    capacity: usize,
    next_pts: i64,
    last_gain: f32,
    last_epoch: u64,
}
impl Output {
    fn open(name: &str, clock: Arc<AudioClock>, epoch: Arc<AtomicU64>) -> Result<Self> {
        let host = cpal::default_host();
        let device = if name == "default" {
            host.default_output_device()
                .context("No default audio device")?
        } else {
            host.output_devices()?
                .find(|d| d.id().is_ok_and(|id| id.to_string() == name))
                .context("Selected audio device is disconnected")?
        };
        let supported = device.default_output_config()?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let (producer, consumer) =
            ring::channel(config.sample_rate as usize * config.channels as usize * 160 / 1000);
        let failed = Arc::new(AtomicBool::new(false));
        let prefilled = Arc::new(AtomicBool::new(false));
        macro_rules! build {
            ($t:ty) => {
                build::<$t>(
                    &device,
                    &config,
                    consumer,
                    epoch.clone(),
                    failed.clone(),
                    prefilled.clone(),
                    clock.clone(),
                )?
            };
        }
        let stream = match format {
            SampleFormat::F32 => build!(f32),
            SampleFormat::F64 => build!(f64),
            SampleFormat::I16 => build!(i16),
            SampleFormat::U16 => build!(u16),
            SampleFormat::I32 => build!(i32),
            SampleFormat::U32 => build!(u32),
            SampleFormat::I8 => build!(i8),
            SampleFormat::U8 => build!(u8),
            _ => bail!("Unsupported audio device sample format {format:?}"),
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            config,
            producer,
            epoch,
            failed,
            prefilled,
            resampler: None,
            clocked: false,
            format,
            pending: std::collections::VecDeque::new(),
            low_latency: true,
            occupancy: 0.04,
            capacity: config.sample_rate as usize * config.channels as usize * 160 / 1000,
            next_pts: 0,
            last_gain: 1.,
            last_epoch: 0,
        })
    }
    fn push(
        &mut self,
        pcm: &[i16],
        rate: u32,
        channels: u8,
        pts: i64,
        epoch: u64,
        gain: f32,
    ) -> Result<()> {
        use ffmpeg_next::{
            ChannelLayout,
            format::{Sample, sample::Type},
            frame,
        };
        let layout = ChannelLayout::default(channels as i32);
        let definition = ffmpeg_next::software::resampling::context::Definition {
            format: Sample::I16(Type::Packed),
            channel_layout: layout,
            rate,
        };
        if self
            .resampler
            .as_ref()
            .is_none_or(|r| r.input() != &definition)
        {
            self.resampler = Some(ffmpeg_next::software::resampling::Context::get(
                definition.format,
                layout,
                rate,
                Sample::F32(Type::Packed),
                ChannelLayout::default(self.config.channels as i32),
                self.config.sample_rate,
            )?);
        }
        let mut input = frame::Audio::new(definition.format, pcm.len() / channels as usize, layout);
        input.set_rate(rate);
        unsafe {
            std::ptr::copy_nonoverlapping(
                pcm.as_ptr(),
                (*input.as_mut_ptr()).data[0].cast(),
                pcm.len(),
            );
        }
        if self.low_latency {
            let queued = self.config.sample_rate as usize * self.config.channels as usize * 160
                / 1000
                - self.producer.free();
            let seconds =
                queued as f64 / self.config.sample_rate as f64 / self.config.channels as f64;
            let weight = (pcm.len() as f64 / channels as f64 / rate as f64 / 2.).clamp(0., 0.1);
            self.occupancy += (seconds - self.occupancy) * weight;
            let ppm = ((self.occupancy - 0.04) * 10_000.).clamp(-300., 300.);
            let distance = self.config.sample_rate as i32 * 10;
            let delta = (-ppm * distance as f64 / 1_000_000.).round() as i32;
            let result = unsafe {
                ffmpeg_next::ffi::swr_set_compensation(
                    self.resampler.as_mut().unwrap().as_mut_ptr(),
                    delta,
                    distance,
                )
            };
            anyhow::ensure!(result >= 0, "Audio drift compensation rejected");
        }
        let delay = unsafe {
            ffmpeg_next::ffi::swr_get_delay(
                self.resampler.as_mut().unwrap().as_mut_ptr(),
                rate as i64,
            )
        };
        self.next_pts = pts - delay * 1_000_000 / rate as i64;
        self.last_gain = gain;
        self.last_epoch = epoch;
        let output = resample(self.resampler.as_mut().unwrap(), &input)?;
        self.enqueue(&output);
        self.pump();
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        if self.resampler.is_some() {
            for _ in 0..8 {
                let mut output = ffmpeg_next::frame::Audio::new(
                    ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Packed),
                    4096,
                    ffmpeg_next::ChannelLayout::default(self.config.channels as i32),
                );
                output.set_rate(self.config.sample_rate);
                let delayed = self.resampler.as_mut().unwrap().flush(&mut output)?;
                self.enqueue(&output);
                if delayed.is_none() || output.samples() == 0 {
                    break;
                }
            }
        }
        self.pump();
        Ok(())
    }
    fn enqueue(&mut self, output: &ffmpeg_next::frame::Audio) {
        let count = output.samples() * self.config.channels as usize;
        if count == 0 {
            return;
        }
        let values =
            unsafe { std::slice::from_raw_parts((*output.as_ptr()).data[0].cast::<f32>(), count) };
        for (i, value) in values.iter().enumerate() {
            let sample = ring::Sample {
                value: encode((*value * self.last_gain).clamp(-1., 1.), self.format),
                epoch: self.last_epoch,
                pts_us: if self.clocked {
                    self.next_pts
                        + (i / self.config.channels as usize) as i64 * 1_000_000
                            / self.config.sample_rate as i64
                } else {
                    i64::MIN
                },
            };
            self.pending.push_back(sample);
        }
        self.next_pts += output.samples() as i64 * 1_000_000 / self.config.sample_rate as i64;
    }
    fn pump(&mut self) {
        while let Some(sample) = self.pending.front().copied() {
            if sample.epoch != self.epoch.load(Ordering::Acquire) {
                self.pending.pop_front();
                continue;
            }
            if !self.producer.push(sample) {
                break;
            }
            self.pending.pop_front();
        }
        let queued = self.config.sample_rate as usize * self.config.channels as usize * 160 / 1000
            - self.producer.free();
        if queued >= self.config.sample_rate as usize * self.config.channels as usize * 40 / 1000 {
            self.prefilled.store(true, Ordering::Release);
        }
    }
}
/// swr_convert_frame needs explicit output capacity when the device rate is higher
/// than the source rate. Allocating only input.samples() would accumulate delay.
pub(crate) fn resample(
    context: &mut ffmpeg_next::software::resampling::Context,
    input: &ffmpeg_next::frame::Audio,
) -> Result<ffmpeg_next::frame::Audio> {
    let capacity = unsafe {
        ffmpeg_next::ffi::swr_get_out_samples(context.as_mut_ptr(), input.samples() as i32)
    };
    anyhow::ensure!(capacity >= 0, "Invalid resampler output capacity");
    let definition = *context.output();
    let mut output = ffmpeg_next::frame::Audio::new(
        definition.format,
        capacity.max(1) as usize,
        definition.channel_layout,
    );
    output.set_rate(definition.rate);
    context.run(input, &mut output)?;
    Ok(output)
}
// Convert to the negotiated device format on the control thread, before publication.
fn encode(value: f32, format: SampleFormat) -> u64 {
    use cpal::Sample;
    match format {
        SampleFormat::F32 => value.to_bits() as u64,
        SampleFormat::F64 => (value as f64).to_bits(),
        SampleFormat::I16 => i16::from_sample(value) as u16 as u64,
        SampleFormat::U16 => u16::from_sample(value) as u64,
        SampleFormat::I32 => i32::from_sample(value) as u32 as u64,
        SampleFormat::U32 => u32::from_sample(value) as u64,
        SampleFormat::I8 => i8::from_sample(value) as u8 as u64,
        SampleFormat::U8 => u8::from_sample(value) as u64,
        _ => unreachable!("format is checked when opening the device"),
    }
}
struct Callback {
    consumer: ring::Consumer,
    epoch: Arc<AtomicU64>,
    prefilled: Arc<AtomicBool>,
    clock: Arc<AudioClock>,
    channels: usize,
    rate: u64,
}
impl Callback {
    fn fill<T: cpal::SizedSample>(&mut self, data: &mut [T], latency: u64) {
        let generation = self.epoch.load(Ordering::Acquire);
        let mut first = None;
        let mut horizon = None;
        let mut empty = false;
        let mut stale_budget = 16_384usize;
        for (index, out) in data.iter_mut().enumerate() {
            let sample = if self.prefilled.load(Ordering::Acquire)
                && !self.clock.paused.load(Ordering::Acquire)
            {
                loop {
                    match self.consumer.pop() {
                        Some(s) if s.epoch == generation => break Some(s),
                        Some(_) => {
                            if stale_budget == 0 {
                                break None;
                            }
                            stale_budget -= 1;
                        }
                        None => break None,
                    }
                }
            } else {
                None
            };
            if let Some(s) = sample {
                horizon = Some(s.pts_us.saturating_add(1_000_000 / self.rate as i64));
                if first.is_none() {
                    first = Some((s.pts_us, index / self.channels));
                }
                // Supported device formats are primitive samples, preconverted by encode().
                // Windows x64 and the Linux acceptance target are little-endian.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (&s.value as *const u64).cast::<u8>(),
                        (out as *mut T).cast::<u8>(),
                        std::mem::size_of::<T>(),
                    );
                }
            } else {
                *out = T::EQUILIBRIUM;
                empty = true;
            }
        }
        if generation != self.epoch.load(Ordering::Acquire) {
            data.fill(T::EQUILIBRIUM);
            self.clock.invalidate();
            return;
        }
        if let Some((pts, offset)) = first
            && pts != i64::MIN
        {
            self.clock.update(
                generation,
                pts,
                self.clock.now_us() + latency + offset as u64 * 1_000_000 / self.rate,
                latency,
                horizon.unwrap_or(pts),
            );
        }
        if empty
            && !self.clock.paused.load(Ordering::Acquire)
            && self.prefilled.swap(false, Ordering::AcqRel)
        {
            self.clock.underruns.fetch_add(1, Ordering::Relaxed);
            // Keep the last audible horizon: HLS freezes rather than advancing through silence.
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &StreamConfig,
    consumer: ring::Consumer,
    epoch: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    prefilled: Arc<AtomicBool>,
    clock: Arc<AudioClock>,
) -> Result<cpal::Stream> {
    let error = failed.clone();
    let error_clock = clock.clone();
    let mut callback = Callback {
        consumer,
        epoch,
        prefilled,
        clock,
        channels: config.channels as usize,
        rate: config.sample_rate as u64,
    };
    Ok(device.build_output_stream(
        *config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let timestamp = info.timestamp();
            let latency = timestamp
                .playback
                .duration_since(timestamp.callback)
                .as_micros() as u64;
            callback.fill(data, latency);
        },
        move |_| {
            error.store(true, Ordering::Release);
            error_clock.invalidate();
        },
        None,
    )?)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceChoice {
    pub id: String,
    pub label: String,
}
impl std::fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.label.fmt(f)
    }
}
impl DeviceChoice {
    pub fn default_output() -> Self {
        Self {
            id: "default".into(),
            label: "Follow Windows default output".into(),
        }
    }
}
pub fn devices() -> Result<Vec<DeviceChoice>> {
    let mut names = vec![DeviceChoice::default_output()];
    for d in cpal::default_host().output_devices()? {
        names.push(DeviceChoice {
            id: d.id()?.to_string(),
            label: d.description()?.name().into(),
        });
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };
    thread_local! {static REALTIME:Cell<bool>=const{Cell::new(false)};static ALLOCS:Cell<u32>=const{Cell::new(0)};}
    struct Allocator;
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            REALTIME.with(|active| {
                if active.get() {
                    ALLOCS.with(|n| n.set(n.get() + 1));
                }
            });
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            unsafe { System.dealloc(p, layout) }
        }
        unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            REALTIME.with(|active| {
                if active.get() {
                    ALLOCS.with(|n| n.set(n.get() + 1));
                }
            });
            unsafe { System.realloc(p, layout, size) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: Allocator = Allocator;
    fn callback() -> (ring::Producer, Callback) {
        let (p, c) = ring::channel(32);
        (
            p,
            Callback {
                consumer: c,
                epoch: Arc::new(AtomicU64::new(2)),
                prefilled: Arc::new(AtomicBool::new(true)),
                clock: Arc::new(AudioClock::default()),
                channels: 2,
                rate: 44100,
            },
        )
    }
    #[test]
    fn callback_copies_preconverted_pcm_and_zero_fills_without_allocations() {
        let (mut p, mut c) = callback();
        for i in 0..4 {
            assert!(p.push(ring::Sample {
                value: encode(0.25, SampleFormat::I16),
                epoch: 2,
                pts_us: 1_000_000 + i / 2
            }));
        }
        let mut data = [-1i16; 8];
        ALLOCS.with(|n| n.set(0));
        REALTIME.with(|b| b.set(true));
        c.fill(&mut data, 20_000);
        REALTIME.with(|b| b.set(false));
        assert_eq!(ALLOCS.with(Cell::get), 0);
        assert_eq!(&data[..4], &[8192; 4]);
        assert_eq!(&data[4..], &[0; 4]);
        assert_eq!(c.clock.underruns.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn callback_fences_old_pcm_and_honours_pause() {
        let (mut p, mut c) = callback();
        p.push(ring::Sample {
            value: encode(0.75, SampleFormat::F32),
            epoch: 1,
            pts_us: 0,
        });
        for _ in 0..4 {
            p.push(ring::Sample {
                value: encode(0.25, SampleFormat::F32),
                epoch: 2,
                pts_us: 1_000_000,
            });
        }
        let mut data = [0f32; 4];
        c.clock.paused.store(true, Ordering::Relaxed);
        c.fill(&mut data, 10_000);
        assert_eq!(data, [0.; 4]);
        c.clock.paused.store(false, Ordering::Relaxed);
        c.fill(&mut data, 10_000);
        assert_eq!(data, [0.25; 4]);
        assert!(c.clock.position(2).is_some());
        assert!(c.clock.position(1).is_none());
    }
    #[test]
    fn higher_device_rate_does_not_accumulate_resampling_delay() {
        use ffmpeg_next::{
            ChannelLayout,
            format::{Sample, sample::Type},
            frame::Audio,
            software::resampling::Context,
        };
        let mut context = Context::get(
            Sample::I16(Type::Packed),
            ChannelLayout::STEREO,
            44100,
            Sample::F32(Type::Packed),
            ChannelLayout::STEREO,
            48000,
        )
        .unwrap();
        let mut total = 0;
        for _ in 0..100 {
            let mut input = Audio::new(Sample::I16(Type::Packed), 441, ChannelLayout::STEREO);
            input.set_rate(44100);
            input.data_mut(0).fill(0);
            total += resample(&mut context, &input).unwrap().samples();
            let delay = unsafe { ffmpeg_next::ffi::swr_get_delay(context.as_mut_ptr(), 44100) };
            assert!(
                delay < 64,
                "Delay grew instead of remaining a fixed filter delay: {delay}"
            );
        }
        let mut tail = Audio::new(Sample::F32(Type::Packed), 4096, ChannelLayout::STEREO);
        tail.set_rate(48000);
        context.flush(&mut tail).unwrap();
        total += tail.samples();
        assert_eq!(total, 48000);
    }
    #[test]
    fn endpoint_format_conversion_includes_unsigned_silence() {
        assert_eq!(encode(0., SampleFormat::U16), 32768);
        assert_eq!(encode(0., SampleFormat::U8), 128);
    }
}
