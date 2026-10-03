use crate::{
    media::{VideoFrame, audio},
    protocol::{Reader, Response, dict, integer, string, uint},
    state::Shared,
};
use anyhow::{Context, Result, ensure};
use ffmpeg_next::{self as ffmpeg, ChannelLayout, codec, format, frame, media};
use plist::Value;
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Reply {
    bytes: Vec<u8>,
    redirect: String,
    content_range: String,
}
#[derive(Default)]
struct Session {
    generation: u64,
    master: String,
    pending: HashMap<u64, (String, u64)>,
    next_request: u64,
    replies: HashMap<String, Reply>,
    order: VecDeque<String>,
    cache_bytes: usize,
    urls: HashMap<u64, String>,
    ids: HashMap<String, u64>,
    next_url: u64,
    rate: f64,
    seek: Option<f64>,
    position: f64,
    duration: f64,
    started: bool,
    properties: HashMap<String, Vec<u8>>,
}
type ReverseChannels = HashMap<String, (u64, Arc<Mutex<TcpStream>>)>;
struct Inner {
    shared: Arc<Shared>,
    reverse: Mutex<ReverseChannels>,
    session: Mutex<(String, Session)>,
    ready: Condvar,
    stop: AtomicBool,
    port: u16,
}
pub struct Hls {
    inner: Arc<Inner>,
    proxy: Mutex<Option<thread::JoinHandle<()>>>,
    player: Mutex<Option<(Arc<AtomicBool>, thread::JoinHandle<()>)>>,
}
impl Hls {
    pub fn new(shared: Arc<Shared>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            shared,
            reverse: Mutex::new(HashMap::new()),
            session: Mutex::new((String::new(), Session::default())),
            ready: Condvar::new(),
            stop: AtomicBool::new(false),
            port,
        });
        let i = inner.clone();
        let proxy = thread::Builder::new()
            .name("hls-proxy".into())
            .spawn(move || {
                let mut children: Vec<thread::JoinHandle<()>> = Vec::new();
                while !i.stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            children.retain(|h| !h.is_finished());
                            if children.len() >= 16 {
                                continue;
                            }
                            let inner = i.clone();
                            if let Ok(t) =
                                thread::Builder::new()
                                    .name("hls-fetch".into())
                                    .spawn(move || {
                                        if let Err(e) = serve(stream, &inner) {
                                            tracing::debug!("HLS proxy: {e:#}");
                                        }
                                    })
                            {
                                children.push(t)
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
                for t in children {
                    let _ = t.join();
                }
            })?;
        Ok(Self {
            inner,
            proxy: Mutex::new(Some(proxy)),
            player: Mutex::new(None),
        })
    }
    pub fn reverse(&self, id: &str, owner: u64, stream: TcpStream) -> Result<()> {
        ensure!(
            !id.is_empty() && id.len() <= 128,
            "Missing or invalid reverse session ID"
        );
        let mut reverse = self.inner.reverse.lock().unwrap();
        ensure!(
            reverse.len() < 32 || reverse.contains_key(id),
            "Too many reverse sessions"
        );
        reverse.insert(id.into(), (owner, Arc::new(Mutex::new(stream))));
        Ok(())
    }
    pub fn remove_reverse(&self, owner: u64) {
        self.inner
            .reverse
            .lock()
            .unwrap()
            .retain(|_, (id, _)| *id != owner);
    }
    pub fn play(&self, id: &str, url: &str, position: f64) -> Result<()> {
        ensure!(
            url.len() <= 8192
                && matches!(url::Url::parse(url)?.scheme(), "mlhls" | "http" | "https"),
            "Invalid HLS URL"
        );
        self.stop();
        {
            let mut state = self.inner.session.lock().unwrap();
            let generation = state.1.generation + 1;
            let properties = if state.0 == id {
                std::mem::take(&mut state.1.properties)
            } else {
                HashMap::new()
            };
            state.0 = id.into();
            state.1 = Session {
                generation,
                master: url.into(),
                rate: 1.,
                seek: (position > 0.).then_some(position),
                properties,
                ..Default::default()
            };
        }
        if url.starts_with("mlhls:") {
            self.inner.request(url)?;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let inner = self.inner.clone();
        let input = if url.starts_with("mlhls:") {
            format!("http://127.0.0.1:{}/master.m3u8", self.inner.port)
        } else {
            url.into()
        };
        let worker = thread::Builder::new()
            .name("hls-playback".into())
            .spawn(move || {
                if let Err(e) = playback(&input, &inner, &flag)
                    && !flag.load(Ordering::Acquire)
                {
                    inner.shared.report(format!("HLS playback: {e:#}"));
                }
                let mut state = inner.session.lock().unwrap();
                state.1.rate = 0.;
                state.1.started = false;
                drop(state);
                inner.shared.ui.lock().unwrap().paused = true;
            })?;
        *self.player.lock().unwrap() = Some((stop, worker));
        Ok(())
    }
    pub fn stop(&self) {
        {
            let mut s = self.inner.session.lock().unwrap();
            s.1.generation += 1;
        }
        self.inner.ready.notify_all();
        if let Some((stop, worker)) = self.player.lock().unwrap().take() {
            stop.store(true, Ordering::Release);
            self.inner.ready.notify_all();
            let _ = worker.join();
        }
        let mut session = self.inner.session.lock().unwrap();
        session.1.rate = 0.;
        session.1.started = false;
    }
    pub fn action(&self, id: &str, value: Value) -> Result<()> {
        let d = value.as_dictionary().context("Invalid HLS action")?;
        if string(d, "type") != Some("unhandledURLResponse") {
            return Ok(());
        }
        let params = d
            .get("params")
            .and_then(Value::as_dictionary)
            .context("Missing HLS action parameters")?;
        let url = string(params, "FCUP_Response_URL").context("Missing response URL")?;
        let bytes = params
            .get("FCUP_Response_Data")
            .and_then(Value::as_data)
            .unwrap_or(&[])
            .to_vec();
        let redirect = params
            .get("FCUP_Response_Headers")
            .and_then(Value::as_dictionary)
            .and_then(|d| string(d, "Location"))
            .unwrap_or("")
            .to_owned();
        let request = integer(params, "FCUP_Response_RequestID");
        let mut state = self.inner.session.lock().unwrap();
        if state.0 != id {
            return Ok(());
        }
        if let Some(request) = request.filter(|n| *n != 0) {
            if state
                .1
                .pending
                .remove(&request)
                .is_none_or(|(u, g)| u != url || g != state.1.generation)
            {
                return Ok(());
            }
        } else if !url.starts_with("mlhls:") {
            return Ok(());
        }
        ensure!(bytes.len() <= 32 * 1024 * 1024, "HLS response too large");
        state.1.put(
            url.into(),
            Reply {
                bytes,
                redirect,
                content_range: String::new(),
            },
        );
        drop(state);
        self.inner.ready.notify_all();
        Ok(())
    }
    pub fn rate(&self, rate: f64) {
        let mut s = self.inner.session.lock().unwrap();
        s.1.rate = if rate > 0. { 1. } else { 0. };
        self.inner.shared.ui.lock().unwrap().paused = rate <= 0.;
        self.inner.ready.notify_all();
    }
    pub fn seek(&self, position: f64) {
        self.inner.session.lock().unwrap().1.seek = Some(position.max(0.));
        self.inner.ready.notify_all();
    }
    pub fn position(&self) -> f64 {
        self.inner.session.lock().unwrap().1.position
    }
    pub fn info(&self) -> Value {
        let s = self.inner.session.lock().unwrap();
        dict([
            ("duration", Value::Real(s.1.duration)),
            ("position", Value::Real(s.1.position)),
            ("rate", Value::Real(s.1.rate)),
            ("readyToPlay", Value::Boolean(s.1.started)),
            ("playbackBufferEmpty", Value::Boolean(!s.1.started)),
            ("playbackBufferFull", Value::Boolean(s.1.started)),
            ("playbackLikelyToKeepUp", Value::Boolean(s.1.started)),
            (
                "loadedTimeRanges",
                Value::Array(vec![dict([
                    ("start", Value::Real(0.)),
                    ("duration", Value::Real(s.1.duration)),
                ])]),
            ),
        ])
    }
    pub fn property(&self, id: &str, key: &str, value: Option<Vec<u8>>) -> Result<Vec<u8>> {
        ensure!(id.len() <= 128 && key.len() <= 256, "Invalid property key");
        let mut s = self.inner.session.lock().unwrap();
        if s.0.is_empty() && value.is_some() {
            s.0 = id.into();
        }
        if s.0 != id {
            return Ok(Vec::new());
        }
        if let Some(v) = value {
            let size = s.1.properties.values().map(Vec::len).sum::<usize>()
                - s.1.properties.get(key).map_or(0, Vec::len)
                + v.len();
            ensure!(
                size <= 1024 * 1024
                    && (s.1.properties.len() < 128 || s.1.properties.contains_key(key)),
                "Property cache full"
            );
            s.1.properties.insert(key.into(), v);
            return Ok(Vec::new());
        }
        Ok(s.1.properties.get(key).cloned().unwrap_or_default())
    }
}
impl Drop for Hls {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
        self.inner.ready.notify_all();
        self.stop();
        if let Some(t) = self.proxy.lock().unwrap().take() {
            let _ = t.join();
        }
    }
}
impl Session {
    fn put(&mut self, url: String, reply: Reply) {
        if let Some(old) = self.replies.remove(&url) {
            self.cache_bytes -= old.bytes.len();
            self.order.retain(|u| u != &url);
        }
        self.cache_bytes += reply.bytes.len();
        self.order.push_back(url.clone());
        self.replies.insert(url, reply);
        while self.cache_bytes > 64 * 1024 * 1024 || self.order.len() > 64 {
            if let Some(url) = self.order.pop_front() {
                if let Some(old) = self.replies.remove(&url) {
                    self.cache_bytes -= old.bytes.len();
                }
            } else {
                break;
            }
        }
    }
    fn local(&mut self, url: &str, port: u16) -> String {
        let id = if let Some(id) = self.ids.get(url) {
            *id
        } else {
            self.next_url += 1;
            let id = self.next_url;
            self.ids.insert(url.into(), id);
            self.urls.insert(id, url.into());
            if self.urls.len() > 8192 {
                let expired = id - 8192;
                if let Some(url) = self.urls.remove(&expired) {
                    self.ids.remove(&url);
                }
            }
            id
        };
        format!("http://127.0.0.1:{port}/resource/{id}")
    }
}
impl Inner {
    fn request(&self, url: &str) -> Result<()> {
        let (id, request, generation) = {
            let mut s = self.session.lock().unwrap();
            s.1.next_request += 1;
            (s.0.clone(), s.1.next_request, s.1.generation)
        };
        let stream = self
            .reverse
            .lock()
            .unwrap()
            .get(&id)
            .map(|(_, s)| s.clone())
            .context("No reverse channel for HLS session")?;
        let value=dict([("sessionID",uint(1)),("type",Value::String("unhandledURLRequest".into())),("request",dict([
            ("FCUP_Response_ClientInfo",uint(1)),("FCUP_Response_ClientRef",uint(40030004)),("FCUP_Response_RequestID",uint(request)),
            ("FCUP_Response_URL",Value::String(url.into())),("sessionID",uint(1)),("FCUP_Response_Headers",dict([
                ("X-Playback-Session-Id",Value::String(id.clone())),("User-Agent",Value::String("AppleCoreMedia/1.0.0.11B554a (Apple TV; U; CPU OS 7_0_4 like Mac OS X; en_us)".into())),("Accept-Language",Value::String("en-US,en;q=0.9".into()))]))]))]);
        let mut bytes = Vec::new();
        value.to_writer_xml(&mut bytes)?;
        {
            let mut s = self.session.lock().unwrap();
            ensure!(s.1.pending.len() < 256, "Too many outstanding HLS requests");
            s.1.pending.insert(request, (url.into(), generation));
        }
        let mut stream = stream.lock().unwrap();
        write!(
            stream,
            "POST /event HTTP/1.1\r\nX-Apple-Session-ID: {id}\r\nContent-Type: text/x-apple-plist+xml\r\nContent-Length: {}\r\n\r\n",
            bytes.len()
        )?;
        stream.write_all(&bytes)?;
        Ok(())
    }
    fn fetch(&self, url: &str, refresh: bool) -> Result<Reply> {
        if !refresh && let Some(r) = self.session.lock().unwrap().1.replies.get(url) {
            return Ok(r.clone());
        }
        if refresh {
            let mut s = self.session.lock().unwrap();
            if let Some(old) = s.1.replies.remove(url) {
                s.1.cache_bytes -= old.bytes.len();
                s.1.order.retain(|u| u != url);
            }
        }
        self.request(url)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut s = self.session.lock().unwrap();
        let generation = s.1.generation;
        loop {
            if let Some(r) = s.1.replies.get(url) {
                return Ok(r.clone());
            }
            ensure!(
                !self.stop.load(Ordering::Acquire) && s.1.generation == generation,
                "Playback cancelled"
            );
            let timeout = deadline.saturating_duration_since(Instant::now());
            ensure!(!timeout.is_zero(), "HLS response timed out");
            s = self
                .ready
                .wait_timeout(s, timeout.min(Duration::from_millis(100)))
                .unwrap()
                .0;
        }
    }
}
fn serve(mut stream: TcpStream, inner: &Inner) -> Result<()> {
    // Windows accepted sockets inherit the nonblocking listener's mode.
    // Wait for the request (including fragments) instead of treating WouldBlock
    // as an absent request and closing the connection.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let req = Reader::default()
        .next(&mut stream)?
        .context("Missing local HLS request")?;
    ensure!(req.method == "GET", "Only GET is supported");
    let (url, master) = {
        let s = inner.session.lock().unwrap();
        if req.path() == "/master.m3u8" {
            (s.1.master.clone(), true)
        } else {
            let id = req
                .path()
                .strip_prefix("/resource/")
                .and_then(|s| s.parse::<u64>().ok())
                .context("Unknown HLS resource")?;
            (
                s.1.urls.get(&id).context("Expired HLS resource")?.clone(),
                false,
            )
        }
    };
    let mut reply = if url.starts_with("mlhls:") {
        inner.fetch(&url, false)?
    } else {
        http_fetch(&url, req.header("range"))?
    };
    if !reply.redirect.is_empty() {
        reply = http_fetch(&reply.redirect, req.header("range"))?;
    }
    let playlist = reply.bytes.starts_with(b"#EXTM3U");
    if playlist {
        let text = std::str::from_utf8(&reply.bytes)?;
        // Live playlists are refetched; VOD playlists retain their immutable cache.
        if !master
            && url.starts_with("mlhls:")
            && !text.contains("#EXT-X-ENDLIST")
            && let Ok(fresh) = inner.fetch(&url, true)
        {
            reply = fresh;
        }
        let text = std::str::from_utf8(&reply.bytes)?;
        let filtered = if master {
            filter_master(text)
        } else {
            expand_condensed(text)
        };
        let mut s = inner.session.lock().unwrap();
        reply.bytes = rewrite(&filtered, &url, |u| s.1.local(u, inner.port))?.into_bytes();
    }
    let mut response = Response::ok();
    if !reply.content_range.is_empty() && !playlist {
        response.status = 206;
        response = response.header("Content-Range", &reply.content_range);
    }
    response
        .header("Connection", "close")
        .bytes(
            if playlist {
                "application/vnd.apple.mpegurl"
            } else {
                "application/octet-stream"
            },
            reply.bytes,
        )
        .write(&req, &mut stream)
}
fn http_fetch(url: &str, range: &str) -> Result<Reply> {
    ensure!(
        matches!(url::Url::parse(url)?.scheme(), "http" | "https"),
        "Unsupported upstream URL"
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut request = client.get(url);
    if !range.is_empty() {
        request = request.header("Range", range)
    }
    let response = request.send()?.error_for_status()?;
    let content_range = response
        .headers()
        .get("content-range")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_owned();
    ensure!(
        response.status().as_u16() != 206 || !content_range.is_empty(),
        "Partial resource lacks Content-Range"
    );
    let mut bytes = Vec::new();
    response
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 64 * 1024 * 1024, "HLS resource too large");
    Ok(Reply {
        bytes,
        redirect: String::new(),
        content_range,
    })
}
fn quoted<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let after = line.split_once(&format!("{key}=\""))?.1;
    Some(after.split_once('"')?.0)
}
pub fn expand_condensed(playlist: &str) -> String {
    let Some(header) = playlist
        .lines()
        .find(|s| s.starts_with("#YT-EXT-CONDENSED-URL"))
    else {
        return playlist.into();
    };
    let (Some(base), Some(params), Some(prefix)) = (
        quoted(header, "BASE-URI"),
        quoted(header, "PARAMS"),
        quoted(header, "PREFIX"),
    ) else {
        return playlist.into();
    };
    let keys = params.split(',').collect::<Vec<_>>();
    let mut output = String::new();
    for line in playlist.lines() {
        if let Some(tail) = line.strip_prefix(prefix) {
            let values = tail
                .split('/')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            output.push_str(base.trim_end_matches('/'));
            if values.iter().any(|v| Some(v) == keys.first()) {
                for v in values {
                    output.push('/');
                    output.push_str(v)
                }
                if tail.ends_with('/') {
                    output.push('/')
                }
            } else {
                for (i, key) in keys.iter().enumerate() {
                    output.push('/');
                    output.push_str(key);
                    output.push('/');
                    output.push_str(values.get(i).unwrap_or(&""));
                }
            }
        } else {
            output.push_str(line)
        }
        output.push('\n');
    }
    output
}
pub fn filter_master(text: &str) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let first = lines
        .iter()
        .position(|l| l.starts_with("#EXT-X-STREAM-INF:"));
    let Some(first) = first else {
        return text.into();
    };
    let group = quoted(lines[first], "AUDIO");
    let audio = lines
        .iter()
        .position(|l| {
            l.starts_with("#EXT-X-MEDIA:TYPE=AUDIO")
                && quoted(l, "GROUP-ID") == group
                && l.contains("DEFAULT=YES")
        })
        .or_else(|| {
            lines.iter().position(|l| {
                l.starts_with("#EXT-X-MEDIA:TYPE=AUDIO") && quoted(l, "GROUP-ID") == group
            })
        });
    let mut output = String::new();
    let mut skip_uri = false;
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with("#EXT-X-MEDIA:") && audio != Some(index) {
            continue;
        }
        if line.starts_with("#EXT-X-STREAM-INF:") && index != first {
            skip_uri = true;
            continue;
        }
        if !line.starts_with('#') && !line.is_empty() && skip_uri {
            skip_uri = false;
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    output
}
pub fn rewrite(text: &str, base: &str, mut local: impl FnMut(&str) -> String) -> Result<String> {
    let base = url::Url::parse(base)?;
    let mut result = String::new();
    for line in text.lines() {
        if line.starts_with('#') {
            let mut rest = line;
            while let Some(pos) = rest.find("URI=\"") {
                result.push_str(&rest[..pos + 5]);
                let tail = &rest[pos + 5..];
                let (uri, after) = tail.split_once('"').context("Malformed playlist URI")?;
                result.push_str(&local(base.join(uri)?.as_str()));
                result.push('"');
                rest = after;
            }
            result.push_str(rest);
        } else if !line.is_empty() {
            result.push_str(&local(base.join(line.trim())?.as_str()));
        }
        result.push('\n');
    }
    Ok(result)
}

fn playback(url: &str, inner: &Arc<Inner>, stop: &Arc<AtomicBool>) -> Result<()> {
    let mut options = ffmpeg::Dictionary::new();
    options.set("rw_timeout", "2000000");
    options.set("protocol_whitelist", "http,https,tcp,tls,crypto");
    // Local resource URLs deliberately carry opaque IDs, including signed
    // condensed URLs with no extension. The proxy validates upstream schemes.
    options.set("allowed_extensions", "ALL");
    options.set("allowed_segment_extensions", "ALL");
    options.set("extension_picky", "0");
    let cancel = stop.clone();
    let global = inner.clone();
    let mut input = format::input_with_interrupt_and_dictionary(
        url,
        move || cancel.load(Ordering::Acquire) || global.stop.load(Ordering::Acquire),
        options,
    )?;
    let video_stream = input
        .streams()
        .best(media::Type::Video)
        .context("HLS video stream missing")?;
    let vi = video_stream.index();
    let vt = video_stream.time_base();
    let mut video = codec::Context::from_parameters(video_stream.parameters())?
        .decoder()
        .video()?;
    let audio_stream = input.streams().best(media::Type::Audio);
    let ai = audio_stream.as_ref().map(|s| s.index());
    let mut decoder = audio_stream
        .map(|s| codec::Context::from_parameters(s.parameters()).and_then(|c| c.decoder().audio()))
        .transpose()?;
    let mut resampler = None::<ffmpeg::software::resampling::Context>;
    let mut sink = if decoder.is_some() {
        Some(audio::Sink::new(44100, 2)?)
    } else {
        None
    };
    let mut clock = None::<(Instant, f64)>;
    let start_time = unsafe { (*input.as_ptr()).start_time };
    let start_seconds = if start_time == ffmpeg::ffi::AV_NOPTS_VALUE {
        0.
    } else {
        start_time as f64 / 1_000_000.
    };
    {
        let mut s = inner.session.lock().unwrap();
        s.1.duration = input.duration().max(0) as f64 / 1_000_000.;
        s.1.started = true;
    }
    {
        let mut ui = inner.shared.ui.lock().unwrap();
        ui.codec = format!("{:?}", video.id());
        ui.kind = "HLS playback".into();
        ui.decoder = "CPU".into();
    }
    loop {
        if stop.load(Ordering::Acquire) || inner.stop.load(Ordering::Acquire) {
            break;
        }
        let seek = inner.session.lock().unwrap().1.seek.take();
        if let Some(position) = seek {
            input.seek(((position + start_seconds) * 1_000_000.) as i64, ..)?;
            video.flush();
            if let Some(d) = &mut decoder {
                d.flush()
            }
            if let Some(s) = &mut sink {
                s.flush()
            }
            clock = None;
        }
        while inner.session.lock().unwrap().1.rate <= 0. {
            if stop.load(Ordering::Acquire) || inner.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
            clock = None;
        }
        let mut packet = ffmpeg::Packet::empty();
        let eof = match packet.read(&mut input) {
            Ok(()) => false,
            Err(ffmpeg::Error::Eof) => {
                video.send_eof()?;
                true
            }
            Err(e) => return Err(e.into()),
        };
        if eof || packet.stream() == vi {
            if !eof {
                video.send_packet(&packet)?;
            }
            loop {
                let mut frame = frame::Video::empty();
                match video.receive_frame(&mut frame) {
                    Ok(()) => {}
                    Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
                    Err(ffmpeg::Error::Eof) => break,
                    Err(e) => return Err(e.into()),
                }
                let position =
                    (frame.timestamp().unwrap_or(0) as f64 * f64::from(vt) - start_seconds).max(0.);
                let (start, base) = *clock.get_or_insert((Instant::now(), position));
                let target = start + Duration::from_secs_f64((position - base).max(0.));
                while Instant::now() < target {
                    if stop.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    thread::sleep(
                        target
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(10)),
                    );
                }
                inner.session.lock().unwrap().1.position = position;
                inner.shared.ui.lock().unwrap().dimensions =
                    format!("{} × {}", frame.width(), frame.height());
                // HLS frames use codec defaults; unlike mirroring, unspecified
                // YUV range is conventionally limited, and is made explicit.
                unsafe {
                    if (*frame.as_ptr()).color_range
                        == ffmpeg::ffi::AVColorRange::AVCOL_RANGE_UNSPECIFIED
                    {
                        (*frame.as_mut_ptr()).color_range =
                            ffmpeg::ffi::AVColorRange::AVCOL_RANGE_MPEG;
                    }
                }
                inner.shared.publish(VideoFrame {
                    frame,
                    received: Instant::now(),
                    timeline: inner.shared.started.elapsed(),
                });
            }
        } else if Some(packet.stream()) == ai
            && let Some(decoder) = &mut decoder
        {
            decoder.send_packet(&packet)?;
            loop {
                let mut frame = frame::Audio::empty();
                if decoder.receive_frame(&mut frame).is_err() {
                    break;
                }
                if frame.channel_layout().is_empty() {
                    frame.set_channel_layout(ChannelLayout::STEREO)
                }
                let def = ffmpeg::software::resampling::context::Definition {
                    format: frame.format(),
                    channel_layout: frame.channel_layout(),
                    rate: frame.rate(),
                };
                if resampler.as_ref().is_none_or(|r| r.input() != &def) {
                    resampler = Some(ffmpeg::software::resampling::Context::get(
                        frame.format(),
                        frame.channel_layout(),
                        frame.rate(),
                        ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed),
                        ChannelLayout::STEREO,
                        44100,
                    )?);
                }
                let mut output = frame::Audio::empty();
                resampler.as_mut().unwrap().run(&frame, &mut output)?;
                let pcm = unsafe {
                    std::slice::from_raw_parts(
                        (*output.as_ptr()).data[0] as *const i16,
                        output.samples() * 2,
                    )
                }
                .to_vec();
                sink.as_mut().unwrap().push(&pcm)?;
                inner
                    .shared
                    .metrics
                    .hls_audio_samples
                    .fetch_add((pcm.len() / 2) as u64, Ordering::Relaxed);
                inner.shared.pcm(Arc::new(pcm), 44100);
            }
        }
        if eof {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_accepts_delayed_fragmented_request_on_inherited_nonblocking_socket() {
        let hls = Hls::new(Shared::new(Default::default())).unwrap();
        let url = "mlhls://fixture/blob";
        {
            let mut s = hls.inner.session.lock().unwrap();
            s.1.master = url.into();
            s.1.put(
                url.into(),
                Reply {
                    bytes: b"fixture data".to_vec(),
                    redirect: String::new(),
                    content_range: String::new(),
                },
            );
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let (accepted, _) = listener.accept().unwrap();
        // Windows accept inherits the listener's nonblocking mode. Force that
        // mode here too, so Linux tests cover arrival after accept and fragments.
        accepted.set_nonblocking(true).unwrap();
        let inner = hls.inner.clone();
        let server = thread::spawn(move || serve(accepted, &inner));
        thread::sleep(Duration::from_millis(50));
        client.write_all(b"GET /master.m3u8 HTTP/1.1\r\n").unwrap();
        thread::sleep(Duration::from_millis(30));
        client.write_all(b"Host: localhost\r\n\r\n").unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        server.join().unwrap().unwrap();
        assert!(response.ends_with("\r\n\r\nfixture data"));
    }
    #[test]
    fn proxy_preserves_http_byte_ranges() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/segment", upstream.local_addr().unwrap());
        let remote = thread::spawn(move || {
            let (mut socket, _) = upstream.accept().unwrap();
            let request = Reader::default().next(&mut socket).unwrap().unwrap();
            assert_eq!(request.header("range"), "bytes=1-3");
            socket.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 1-3/8\r\nConnection: close\r\n\r\nBCD").unwrap();
        });
        let hls = Hls::new(Shared::new(Default::default())).unwrap();
        hls.inner.session.lock().unwrap().1.master = url;
        let mut socket = TcpStream::connect(("127.0.0.1", hls.inner.port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .write_all(b"GET /master.m3u8 HTTP/1.1\r\nRange: bytes=1-3\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(response.contains("Content-Range: bytes 1-3/8\r\n"));
        assert!(response.ends_with("\r\n\r\nBCD"));
        remote.join().unwrap();
    }
    #[test]
    fn condensed_shapes_preserve_signed_paths() {
        let p = "#EXTM3U\n#YT-EXT-CONDENSED-URL:BASE-URI=\"https://cdn/x\",PARAMS=\"begin,len,gosq\",PREFIX=\"mlhls://localhost/\"\n#EXTINF:2,\nmlhls://localhost/begin/0/len/2/gosq/\n";
        assert!(expand_condensed(p).contains("https://cdn/x/begin/0/len/2/gosq/\n"));
        assert!(
            expand_condensed(&p.replace("begin/0/len/2/gosq/", "0/2/"))
                .contains("https://cdn/x/begin/0/len/2/gosq/\n")
        );
    }
    #[test]
    fn localises_tag_and_segment_uris() {
        let p = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\nchunk.m4s\n";
        let mut seen = Vec::new();
        let r = rewrite(p, "https://cdn/a/master.m3u8", |u| {
            seen.push(u.to_owned());
            format!("/resource/{}", seen.len())
        })
        .unwrap();
        assert_eq!(seen, ["https://cdn/a/init.mp4", "https://cdn/a/chunk.m4s"]);
        assert!(r.contains("URI=\"/resource/1\""));
    }
    #[test]
    fn restricts_master_to_matching_audio_group() {
        let p = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",DEFAULT=YES,URI=\"a.m3u8\"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"b\",URI=\"b.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1,AUDIO=\"a\"\nv1.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2,AUDIO=\"b\"\nv2.m3u8\n";
        let r = filter_master(p);
        assert!(r.contains("a.m3u8"));
        assert!(r.contains("v1.m3u8"));
        assert!(!r.contains("b.m3u8"));
        assert!(!r.contains("v2.m3u8"));
    }
}
