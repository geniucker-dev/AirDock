// SPDX-License-Identifier: MPL-2.0
use crate::media::{VideoFrame, audio, video};
use crate::{crypto, rtp, state::Shared};
use anyhow::{Result, ensure};
use ctr::cipher::StreamCipher;
use polling::{Event, Events, Poller};
use std::{
    io::Read,
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn udp(peer: SocketAddr) -> Result<UdpSocket> {
    let socket = UdpSocket::bind(if peer.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}
pub struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    wake: Option<Arc<Poller>>,
}
impl Worker {
    fn start(
        name: &str,
        shared: Arc<Shared>,
        wake: Option<Arc<Poller>>,
        work: impl FnOnce(Arc<AtomicBool>) -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::Builder::new().name(name.into()).spawn(move || {
            if let Err(e) = work(flag) {
                shared.report(format!("{e:#}"));
            }
        })?;
        Ok(Self {
            stop,
            thread: Some(thread),
            wake,
        })
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(p) = &self.wake {
            let _ = p.notify();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn timing(peer: SocketAddr, remote_port: u16, shared: Arc<Shared>) -> Result<(u16, Worker)> {
    let socket = udp(peer)?;
    let port = socket.local_addr()?.port();
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut endpoint = peer;
    endpoint.set_port(remote_port);
    let worker = Worker::start("airplay-clock", shared, None, move |stop| {
        let mut last = Instant::now() - Duration::from_secs(3);
        let mut last_ref = [0u8; 8];
        let mut last_recv = 0u64;
        let mut bytes = [0; 128];
        while !stop.load(Ordering::Acquire) {
            if remote_port != 0 && last.elapsed() >= Duration::from_secs(3) {
                let mut request = [0; 32];
                request[..4].copy_from_slice(&[0x80, 0xd2, 0, 7]);
                request[8..16].copy_from_slice(&last_ref);
                request[16..24].copy_from_slice(&last_recv.to_be_bytes());
                request[24..32].copy_from_slice(&ntp().to_be_bytes());
                socket.send_to(&request, endpoint)?;
                last = Instant::now();
            }
            match socket.recv_from(&mut bytes) {
                Ok((n, from)) if n >= 32 && from.ip() == peer.ip() => {
                    if bytes[1] & 0x7f == 0x52 {
                        let mut reply = [0; 32];
                        reply[..4].copy_from_slice(&[0x80, 0xd3, bytes[2], bytes[3]]);
                        reply[8..16].copy_from_slice(&bytes[24..32]);
                        reply[16..24].copy_from_slice(&ntp().to_be_bytes());
                        reply[24..32].copy_from_slice(&ntp().to_be_bytes());
                        socket.send_to(&reply, from)?;
                    } else {
                        last_ref.copy_from_slice(&bytes[24..32]);
                        last_recv = ntp();
                    }
                }
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    })?;
    Ok((port, worker))
}
fn ntp() -> u64 {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    ((t.as_secs() + 2_208_988_800) << 32) | (((t.subsec_nanos() as u64) << 32) / 1_000_000_000)
}

pub fn mirror(
    peer: SocketAddr,
    key: [u8; 16],
    connection: u64,
    shared: Arc<Shared>,
) -> Result<(u16, Worker)> {
    let listener = TcpListener::bind(if peer.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let state = shared.clone();
    let owner_id = shared.sessions.owner_id();
    let worker = Worker::start("mirror-video", shared, None, move |stop| {
        while !stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, addr)) if addr.ip() == peer.ip() => {
                    // A reset or malformed record ends this data connection,
                    // not the advertised listener. The sender can reconnect
                    // without another SETUP; each connection gets fresh CTR state.
                    let result = (|| -> Result<()> {
                        // Windows inherits the listener's nonblocking mode.
                        stream.set_nonblocking(false)?;
                        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
                        stream.set_nodelay(true)?;
                        let mut cipher = crypto::mirror_cipher(&key, connection);
                        let mut decoder: Option<video::Decoder> = None;
                        let mut decode_epoch = state.generation();
                        let mut announced_video = None;
                        loop {
                            if state.sessions.owner_id() != owner_id {
                                break;
                            }
                            let epoch = state.generation();
                            let mut header = [0; 128];
                            if !read_exact_cancel(&mut stream, &mut header, &stop)? {
                                break;
                            }
                            let len = u32::from_le_bytes(header[..4].try_into()?) as usize;
                            ensure!(len <= 2_000_000, "Mirror payload exceeds limit");
                            let kind = u16::from_be_bytes(header[4..6].try_into()?);
                            let mut payload = vec![0; len];
                            if !read_exact_cancel(&mut stream, &mut payload, &stop)? {
                                break;
                            }
                            let received = Instant::now();
                            state.metrics.bytes.fetch_add(len as u64, Ordering::Relaxed);
                            match kind {
                                0x0100 => {
                                    let config = video::Configuration::parse(&payload)?;
                                    let codec = if config.hevc { "HEVC" } else { "H.264" };
                                    let hardware = state.settings.read().unwrap().hardware_decode;
                                    if let Some(d) = &mut decoder {
                                        d.reconfigure(config, hardware)?;
                                    } else {
                                        decoder = Some(video::Decoder::new(config, hardware)?);
                                    }
                                    let d = decoder.as_ref().unwrap();
                                    {
                                        let mut ui = state.ui.lock().unwrap();
                                        ui.codec = codec.into();
                                        ui.decoder = d.backend().into();
                                    }
                                }
                                0x0000 | 0x0010 => {
                                    cipher.apply_keystream(&mut payload);
                                    if let Some(d) = decoder.as_mut() {
                                        if decode_epoch != epoch {
                                            d.flush();
                                            decode_epoch = epoch;
                                            announced_video = None;
                                        }
                                        let source_pts = crate::playback::MediaTime::ntp(
                                            u64::from_le_bytes(header[8..16].try_into()?),
                                        );
                                        match d.decode_mirror_at(
                                            &mut payload,
                                            kind == 0x0010,
                                            source_pts.map(|t| t.micros()),
                                        ) {
                                            Ok(frames) => {
                                                for frame in frames {
                                                    let description = (
                                                        frame.width(),
                                                        frame.height(),
                                                        d.backend(),
                                                    );
                                                    if announced_video != Some(description) {
                                                        let mut ui = state.ui.lock().unwrap();
                                                        ui.dimensions = format!(
                                                            "{} × {}",
                                                            description.0, description.1
                                                        );
                                                        ui.decoder = description.2.into();
                                                        announced_video = Some(description);
                                                    }
                                                    let pts=frame.timestamp().map(crate::playback::MediaTime::microseconds).or(source_pts);
                                                    state.publish(VideoFrame {
                                                        frame,
                                                        received,
                                                        pts,
                                                        epoch,
                                                        sequence: 0,
                                                        playback:
                                                            crate::playback::PlaybackMode::Live,
                                                        clock:
                                                            crate::receiver::airplay::MIRROR_CLOCK,
                                                    });
                                                }
                                            }
                                            Err(e) => tracing::warn!("Video packet rejected: {e}"),
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Ok(())
                    })();
                    if let Err(e) = result {
                        tracing::warn!("Mirror connection ended: {e:#}");
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    })?;
    Ok((port, worker))
}
fn read_exact_cancel(stream: &mut TcpStream, bytes: &mut [u8], stop: &AtomicBool) -> Result<bool> {
    let mut used = 0;
    while used < bytes.len() && !stop.load(Ordering::Acquire) {
        match stream.read(&mut bytes[used..]) {
            Ok(0) => return Ok(false),
            Ok(n) => used += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(used == bytes.len())
}

pub enum AudioCommand {
    Flush(Option<u16>),
    Volume(f32),
}
pub struct AudioStream {
    pub worker: Worker,
    pub commands: Sender<AudioCommand>,
    pub poller: Arc<Poller>,
}
impl AudioStream {
    pub fn command(&self, command: AudioCommand) {
        let _ = self.commands.send(command);
        let _ = self.poller.notify();
    }
}
pub struct AudioOptions {
    pub ct: u64,
    pub rate: u32,
    pub channels: u8,
    pub spf: u32,
    pub key: [u8; 16],
    pub iv: [u8; 16],
    pub control_port: u16,
}
pub fn audio(
    peer: SocketAddr,
    options: AudioOptions,
    shared: Arc<Shared>,
) -> Result<(u16, u16, AudioStream)> {
    let data = udp(peer)?;
    let control = udp(peer)?;
    let data_port = data.local_addr()?.port();
    let control_port = control.local_addr()?.port();
    let decoder = audio::Decoder::new(super::audio_configuration(
        options.ct,
        options.rate,
        options.channels,
        options.spf,
    )?)?;
    let sink = audio::Sink::for_session(options.rate, options.channels, &shared)?;
    let poller = Arc::new(Poller::new()?);
    let wake = poller.clone();
    let (send, recv) = mpsc::channel();
    let state = shared.clone();
    let worker = Worker::start("airplay-audio", shared, Some(wake), move |stop| {
        run_audio(
            data, control, peer, options, decoder, sink, poller, state, stop, recv,
        )
    })?;
    let poller = worker.wake.as_ref().unwrap().clone();
    Ok((
        data_port,
        control_port,
        AudioStream {
            worker,
            commands: send,
            poller,
        },
    ))
}
#[allow(clippy::too_many_arguments)]
fn run_audio(
    data: UdpSocket,
    control: UdpSocket,
    peer: SocketAddr,
    options: AudioOptions,
    mut decoder: audio::Decoder,
    mut sink: audio::Sink,
    poller: Arc<Poller>,
    state: Arc<Shared>,
    stop: Arc<AtomicBool>,
    commands: Receiver<AudioCommand>,
) -> Result<()> {
    // Sockets outlive their poll registrations. Delete registrations before returning.
    unsafe {
        poller.add(&data, Event::readable(1))?;
        poller.add(&control, Event::readable(2))?;
    }
    let result = (|| -> Result<()> {
        let epoch = Instant::now();
        let mut recovery = rtp::Recovery::default();
        let mut endpoint = peer;
        endpoint.set_port(options.control_port);
        let mut request_id = 0u16;
        let mut last_request = 0u64;
        let mut last_active = 0u64;
        let mut flush_until = 0u64;
        let mut timestamp: Option<u32> = None;
        let mut audio_sync: Option<(u64, u32, i64, u32)> = None;
        let mut last_normal = 0u64;
        let mut bytes = [0; 65536];
        let mut events = Events::new();
        while !stop.load(Ordering::Acquire) {
            let now = epoch.elapsed().as_millis() as u64;
            for command in commands.try_iter() {
                match command {
                    AudioCommand::Flush(sequence) => {
                        recovery.reset(sequence, now);
                        decoder.flush();
                        state.reset_media();
                        sink.set_epoch(state.generation());
                        sink.flush();
                        timestamp = None;
                        audio_sync = None;
                        flush_until = now + 500;
                        last_request = 0;
                    }
                    AudioCommand::Volume(db) => sink.volume(db),
                }
            }
            for (is_control, socket) in [(false, &data), (true, &control)] {
                // Bound one burst so control, retransmission and cancellation get serviced.
                for _ in 0..256 {
                    match socket.recv_from(&mut bytes) {
                        Ok((n, from)) if from.ip() == peer.ip() => {
                            if is_control {
                                endpoint = from;
                            }
                            if is_control && n >= 20 && bytes[1] & 0x7f == 0x54 {
                                let rtp = u32::from_be_bytes(bytes[4..8].try_into()?);
                                let ntp = u64::from_be_bytes(bytes[8..16].try_into()?);
                                if let Some(time) = crate::playback::MediaTime::ntp(ntp) {
                                    audio_sync = Some((
                                        state.generation(),
                                        rtp,
                                        time.micros(),
                                        options.rate,
                                    ));
                                }
                                continue;
                            }
                            let resent = n > 1 && bytes[1] & 0x7f == 0x56;
                            if let Some(mut packet) = rtp::parse(&bytes[..n]) {
                                let advance = timestamp
                                    .map(|t| packet.timestamp.wrapping_sub(t))
                                    .unwrap_or(0);
                                if !resent
                                    && now.saturating_sub(last_normal) >= 500
                                    && advance >= options.rate / 2
                                    && advance < 0x80000000
                                {
                                    recovery.reset(None, now);
                                    decoder.flush();
                                    sink.flush();
                                    timestamp = None;
                                }
                                if !resent {
                                    last_normal = now;
                                }
                                packet.arrival_ms = now;
                                if recovery.enqueue(packet, now) {
                                    state.metrics.audio_packets.fetch_add(1, Ordering::Relaxed);
                                    if resent {
                                        state
                                            .metrics
                                            .audio_recovered
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            if let Some((sequence, count)) = recovery.missing()
                && endpoint.port() != 0
                && now.saturating_sub(last_request) >= 20
            {
                let mut request = [0x80, 0xd5, 0, 0, 0, 0, 0, 0];
                request[2..4].copy_from_slice(&request_id.to_be_bytes());
                request[4..6].copy_from_slice(&sequence.to_be_bytes());
                request[6..8].copy_from_slice(&count.to_be_bytes());
                control.send_to(&request, endpoint)?;
                request_id = request_id.wrapping_add(1);
                last_request = now;
            }
            while let Some(mut packet) = recovery.pop(now) {
                let payload_size = packet.payload.len();
                crypto::decrypt_audio(&options.key, &options.iv, &mut packet.payload);
                match decoder.decode(&packet.payload) {
                    Ok(pcm) if !pcm.is_empty() => {
                        if let Some(expected) = timestamp {
                            let gap = packet.timestamp.wrapping_sub(expected);
                            if gap > 0 && gap < 0x80000000 {
                                if gap <= options.rate / 2 {
                                    let silence = vec![0i16; 2048 * options.channels as usize];
                                    let mut left = gap as usize;
                                    while left > 0 {
                                        let n = left.min(2048);
                                        sink.push(&silence[..n * options.channels as usize])?;
                                        let stereo = if options.channels == 2 {
                                            silence[..n * 2].to_vec()
                                        } else {
                                            vec![0; n * 2]
                                        };
                                        state.pcm(Arc::new(stereo), options.rate);
                                        left -= n;
                                    }
                                } else {
                                    sink.flush();
                                }
                            }
                        }
                        timestamp = Some(
                            packet
                                .timestamp
                                .wrapping_add((pcm.len() / options.channels as usize) as u32),
                        );
                        let active = payload_size >= 100 || pcm.iter().any(|s| *s != 0);
                        if active && now >= flush_until {
                            last_active = now;
                            let mut ui = state.ui.lock().unwrap();
                            if ui.paused {
                                ui.paused = false;
                            }
                        }
                        let pts = audio_sync
                            .filter(|(generation, _, _, _)| *generation == state.generation())
                            .map(|(_, rtp, us, rate)| {
                                us + (packet.timestamp.wrapping_sub(rtp) as i32 as i64) * 1_000_000
                                    / rate as i64
                            });
                        sink.volume(state.ui.lock().unwrap().volume_db);
                        sink.push_shared(pcm.clone(), pts)?;
                        let stereo = if options.channels == 2 {
                            pcm
                        } else {
                            Arc::new(pcm.iter().flat_map(|s| [*s, *s]).collect())
                        };
                        state.pcm(stereo, options.rate);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        state.metrics.audio_errors.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!("Audio decode: {e}");
                    }
                }
            }
            if last_active > 0 && now.saturating_sub(last_active) >= 500 {
                let mut ui = state.ui.lock().unwrap();
                if !ui.paused {
                    ui.paused = true;
                }
            }
            let wait = if recovery.missing().is_some() {
                10
            } else {
                100
            };
            poller.modify(&data, Event::readable(1))?;
            poller.modify(&control, Event::readable(2))?;
            events.clear();
            poller.wait(&mut events, Some(Duration::from_millis(wait)))?;
        }
        Ok(())
    })();
    let _ = poller.delete(&data);
    let _ = poller.delete(&control);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffmpeg_next as ffmpeg;
    fn encode_alac(value: i16) -> Vec<u8> {
        match value {
            10 => include_bytes!("../../../tests/fixtures/alac-10.bin").as_slice(),
            20 => include_bytes!("../../../tests/fixtures/alac-20.bin").as_slice(),
            30 => include_bytes!("../../../tests/fixtures/alac-30.bin").as_slice(),
            50 => include_bytes!("../../../tests/fixtures/alac-50.bin").as_slice(),
            55 => include_bytes!("../../../tests/fixtures/alac-55.bin").as_slice(),
            60 => include_bytes!("../../../tests/fixtures/alac-60.bin").as_slice(),
            99 => include_bytes!("../../../tests/fixtures/alac-99.bin").as_slice(),
            _ => panic!("Missing test fixture"),
        }
        .to_vec()
    }
    fn packet(sequence: u16, timestamp: u32, plain: &[u8]) -> Vec<u8> {
        use cbc::cipher::{BlockEncryptMut, KeyIvInit};
        let mut payload = plain.to_vec();
        let mut cipher = cbc::Encryptor::<aes::Aes128>::new(&[0; 16].into(), &[0; 16].into());
        for block in payload.as_chunks_mut::<16>().0 {
            cipher.encrypt_block_mut(block.into())
        }
        let mut bytes = vec![0x80, 0xe0];
        bytes.extend_from_slice(&sequence.to_be_bytes());
        bytes.extend_from_slice(&timestamp.to_be_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 1]);
        bytes.extend_from_slice(&payload);
        bytes
    }
    fn wait_pcm(shared: &Shared, count: usize, timeout_ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while Instant::now() < deadline {
            if shared.test_pcm.lock().unwrap().len() >= count {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!(
            "PCM deadline: expected {count}, got {}; error={}",
            shared.test_pcm.lock().unwrap().len(),
            shared.ui.lock().unwrap().error
        );
    }
    fn setup(
        ip: &str,
        ct: u64,
        spf: u32,
    ) -> (Arc<Shared>, UdpSocket, SocketAddr, SocketAddr, AudioStream) {
        let peer: SocketAddr = if ip.contains(':') {
            format!("[{ip}]:1")
        } else {
            format!("{ip}:1")
        }
        .parse()
        .unwrap();
        let sender = UdpSocket::bind(if peer.is_ipv6() {
            "[::1]:0"
        } else {
            "127.0.0.1:0"
        })
        .unwrap();
        sender
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let state = Shared::new(crate::config::Settings::default());
        let options = AudioOptions {
            ct,
            rate: 44100,
            channels: 2,
            spf,
            key: [0; 16],
            iv: [0; 16],
            control_port: sender.local_addr().unwrap().port(),
        };
        let (data, control, stream) = audio(peer, options, state.clone()).unwrap();
        let mut data_addr = peer;
        data_addr.set_port(data);
        let mut control_addr = peer;
        control_addr.set_port(control);
        (state, sender, data_addr, control_addr, stream)
    }
    #[test]
    fn encrypted_udp_audio_recovers_loss_wrap_flush_and_long_pause() {
        ffmpeg::init().unwrap();
        for ip in ["127.0.0.1", "::1"] {
            let (shared, sender, data, control, stream) = setup(ip, 2, 352);
            let first = packet(65534, 0xfffffe00, &encode_alac(10));
            let second = packet(65535, 0xfffffe00u32.wrapping_add(352), &encode_alac(20));
            sender.send_to(&first, data).unwrap();
            sender.send_to(&first, data).unwrap();
            wait_pcm(&shared, 704, 2000);
            sender
                .send_to(
                    &packet(0, 0xfffffe00u32.wrapping_add(704), &encode_alac(30)),
                    data,
                )
                .unwrap();
            let mut request = [0; 8];
            assert_eq!(sender.recv(&mut request).unwrap(), 8);
            assert_eq!(&request[4..], [0xff, 0xff, 0, 1]);
            let mut resend = vec![0x80, 0xd6, 0, 1];
            resend.extend_from_slice(&second);
            sender.send_to(&resend, control).unwrap();
            sender.send_to(&resend, control).unwrap();
            wait_pcm(&shared, 704 * 3, 2000);
            sender
                .send_to(
                    &packet(2, 0xfffffe00u32.wrapping_add(352 * 4), &encode_alac(50)),
                    data,
                )
                .unwrap();
            wait_pcm(&shared, 704 * 5, 400);
            thread::sleep(Duration::from_millis(520));
            sender
                .send_to(&packet(40000, 2_000_000, &encode_alac(55)), data)
                .unwrap();
            wait_pcm(&shared, 704 * 6, 2000);
            stream.command(AudioCommand::Flush(Some(20000)));
            thread::sleep(Duration::from_millis(5));
            sender
                .send_to(&packet(2, 500, &encode_alac(99)), data)
                .unwrap();
            sender
                .send_to(&packet(20000, 10000, &encode_alac(60)), data)
                .unwrap();
            wait_pcm(&shared, 704 * 7, 2000);
            drop(stream);
            let pcm = shared.test_pcm.lock().unwrap();
            assert_eq!(pcm.len(), 704 * 7);
            for (chunk, value) in pcm
                .as_chunks::<704>()
                .0
                .iter()
                .zip([10, 20, 30, 0, 50, 55, 60])
            {
                assert!(chunk.iter().all(|s| *s == value));
            }
            assert_eq!(shared.metrics.audio_recovered.load(Ordering::Relaxed), 1);
            assert_eq!(shared.metrics.audio_errors.load(Ordering::Relaxed), 0);
        }
        for (spf, fixture) in [
            (
                480,
                include_bytes!("../../../tests/fixtures/eld-480.bin").as_slice(),
            ),
            (
                512,
                include_bytes!("../../../tests/fixtures/eld-512.bin").as_slice(),
            ),
        ] {
            let mut rest = fixture;
            let mut packets = Vec::new();
            while !rest.is_empty() {
                let n = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
                packets.push(&rest[4..4 + n]);
                rest = &rest[4 + n..];
            }
            for ct in [4, 8] {
                let (shared, sender, data, _, stream) = setup("127.0.0.1", ct, spf);
                let mut reference = audio::Decoder::new(
                    super::super::audio_configuration(ct, 44100, 2, spf).unwrap(),
                )
                .unwrap();
                let mut pcm = Vec::new();
                for (i, payload) in packets.iter().enumerate() {
                    let decoded = reference.decode(payload).unwrap();
                    assert_eq!(decoded.len(), spf as usize * 2);
                    pcm.extend_from_slice(&decoded);
                    let bytes = packet(
                        65530u16.wrapping_add(i as u16),
                        0xffffff00u32.wrapping_add(i as u32 * spf),
                        payload,
                    );
                    sender.send_to(&bytes, data).unwrap();
                    sender.send_to(&bytes, data).unwrap();
                    if i % 8 == 7 {
                        wait_pcm(&shared, (i + 1) * spf as usize * 2, 2000)
                    }
                }
                wait_pcm(&shared, 64 * spf as usize * 2, 2000);
                drop(stream);
                assert_eq!(*shared.test_pcm.lock().unwrap(), pcm);
                assert!(pcm.iter().any(|v| v.abs() > 1000));
                assert_eq!(shared.metrics.audio_errors.load(Ordering::Relaxed), 0);
            }
        }
    }
}
