use crate::{
    crypto::{FairPlay, PairVerify},
    media::transport::{self, AudioCommand, AudioOptions, AudioStream, Worker},
    protocol::{self, Reader, Request, Response, dict, integer, string, uint},
    state::{SessionOwner, Shared, UiState},
};
use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::SigningKey;
use plist::Value;
use sha2::{Digest, Sha512};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::BTreeMap,
    io::Cursor,
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{Arc, Mutex, atomic::Ordering},
    thread,
    time::Duration,
};

pub struct Device {
    pub identity: SigningKey,
    pub mac: String,
    pub port: u16,
    pub shared: Arc<Shared>,
    pub hls: crate::hls::Hls,
}
impl Device {
    pub fn new(identity: SigningKey, port: u16, shared: Arc<Shared>) -> Result<Self> {
        // Stable locally administered ID, independent of adapter/hotspot changes.
        let mut id = identity.verifying_key().to_bytes()[..6].to_vec();
        id[0] = (id[0] | 2) & !1;
        let mac = id
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        let hls = crate::hls::Hls::new(shared.clone())?;
        Ok(Self {
            identity,
            mac,
            port,
            shared,
            hls,
        })
    }
    pub fn features(&self) -> u64 {
        let s = self.shared.settings.read().unwrap();
        let low = if s.hls_enabled {
            0x5a7ffef7
        } else {
            0x5a7ffee6
        };
        low | if s.hevc_enabled { 0x400u64 << 32 } else { 0 }
    }
    pub fn info(&self) -> Value {
        let s = self.shared.settings.read().unwrap();
        let display = dict([
            ("features", uint(14)),
            ("width", uint(s.mirror_width as u64)),
            ("height", uint(s.mirror_height as u64)),
            ("widthPixels", uint(s.mirror_width as u64)),
            ("heightPixels", uint(s.mirror_height as u64)),
            ("widthPhysical", uint(0)),
            ("heightPhysical", uint(0)),
            ("refreshRate", uint(s.refresh_rate as u64)),
            ("maxFPS", uint(s.max_fps as u64)),
            ("rotation", uint(0)),
            ("overscanned", Value::Boolean(false)),
            (
                "uuid",
                Value::String("e5f7a168-f1f9-4f51-9f9e-6a9cc0c8cc9f".into()),
            ),
        ]);
        let low = if s.hls_enabled {
            0x5a7ffef7
        } else {
            0x5a7ffee6
        };
        let features = low | if s.hevc_enabled { 0x400u64 << 32 } else { 0 };
        dict([
            ("deviceid", Value::String(self.mac.clone())),
            ("macAddress", Value::String(self.mac.clone())),
            ("model", Value::String("AppleTV3,2".into())),
            ("name", Value::String(s.name.clone())),
            (
                "pi",
                Value::String("b08f5a79-db29-4384-b456-a4784d9e6055".into()),
            ),
            ("sourceVersion", Value::String("220.68".into())),
            ("features", uint(features)),
            ("statusFlags", uint(68)),
            ("vv", uint(2)),
            ("keepAliveLowPower", Value::Boolean(true)),
            ("keepAliveSendStatsAsBody", Value::Boolean(true)),
            (
                "pk",
                Value::Data(self.identity.verifying_key().to_bytes().to_vec()),
            ),
            ("displays", Value::Array(vec![display])),
            (
                "audioFormats",
                Value::Array(
                    [100, 101]
                        .iter()
                        .map(|t| {
                            dict([
                                ("type", uint(*t)),
                                ("audioInputFormats", uint(0x03fffffc)),
                                ("audioOutputFormats", uint(0x03fffffc)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "audioLatencies",
                Value::Array(vec![dict([
                    ("audioType", Value::String("default".into())),
                    ("type", uint(100)),
                    ("inputLatencyMicros", uint(3000)),
                    ("outputLatencyMicros", uint(90000)),
                ])]),
            ),
        ])
    }
}

pub struct Server {
    device: Arc<Device>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn start(device: Device) -> Result<Self> {
        let listener = listen(device.port)?;
        let device = Arc::new(device);
        let d = device.clone();
        let thread = thread::Builder::new()
            .name("airplay-listener".into())
            .spawn(move || {
                let mut children: Vec<thread::JoinHandle<()>> = Vec::new();
                let mut id = 0u64;
                while d.shared.running.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            children.retain(|child| !child.is_finished());
                            if children.len() >= 32 {
                                continue;
                            }
                            id = id.wrapping_add(1);
                            let device = d.clone();
                            if let Ok(child) = thread::Builder::new()
                                .name("airplay-session".into())
                                .spawn(move || {
                                    if let Err(e) = connection(stream, peer, id, device) {
                                        tracing::debug!("Session ended: {e:#}");
                                    }
                                })
                            {
                                children.push(child);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(e) => {
                            d.shared.report(format!("AirPlay listener: {e}"));
                            break;
                        }
                    }
                }
                for child in children {
                    let _ = child.join();
                }
            })?;
        Ok(Self {
            device,
            thread: Some(thread),
        })
    }
    pub fn device(&self) -> &Arc<Device> {
        &self.device
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.device.shared.running.store(false, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn listen(port: u16) -> Result<TcpListener> {
    let attempt = (|| -> Result<_> {
        let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_only_v6(false)?;
        socket.set_reuse_address(true)?;
        socket.bind(&format!("[::]:{port}").parse::<SocketAddr>()?.into())?;
        socket.listen(16)?;
        socket.set_nonblocking(true)?;
        Ok(TcpListener::from(socket))
    })();
    match attempt {
        Ok(l) => Ok(l),
        Err(_) => {
            let l = TcpListener::bind(("0.0.0.0", port))?;
            l.set_nonblocking(true)?;
            Ok(l)
        }
    }
}

struct Session {
    id: u64,
    peer: SocketAddr,
    device: Arc<Device>,
    pair: Option<PairVerify>,
    fairplay: FairPlay,
    key: Option<[u8; 16]>,
    iv: [u8; 16],
    timing: Option<Worker>,
    audio: Option<AudioStream>,
    mirror: Option<Worker>,
    legacy: Vec<UdpSocket>,
    sdp: Option<String>,
    claimed: bool,
    lease: Option<Arc<SessionOwner>>,
}
impl Session {
    fn claim(&mut self) -> Result<bool> {
        let previous = self.device.shared.owner.lock().unwrap().clone();
        if let Some(previous) = previous {
            if previous.id == self.id {
                return Ok(true);
            }
            if previous.peer != self.peer.ip() {
                return Ok(false);
            }
            // iOS reconnects with a fresh TCP session before closing the old
            // one. Retire its media workers completely before accepting new
            // packets; unrelated senders still receive a busy response.
            previous.stop.store(true, Ordering::Release);
            let done = previous.done.lock().unwrap();
            let (done, _) = previous
                .ready
                .wait_timeout_while(done, Duration::from_secs(2), |d| !*d)
                .unwrap();
            if !*done {
                return Ok(false);
            }
        }
        let mut owner = self.device.shared.owner.lock().unwrap();
        if owner.is_some() {
            return Ok(false);
        }
        let lease = Arc::new(SessionOwner {
            id: self.id,
            peer: self.peer.ip(),
            stop: Default::default(),
            done: Mutex::new(false),
            ready: Default::default(),
        });
        *owner = Some(lease.clone());
        self.device.shared.reset_media();
        self.lease = Some(lease);
        self.claimed = true;
        Ok(true)
    }
    fn clear(&mut self) {
        self.audio = None;
        self.mirror = None;
        self.timing = None;
        self.legacy.clear();
        let mut owner = self.device.shared.owner.lock().unwrap();
        if owner.as_ref().is_some_and(|owner| owner.id == self.id) {
            *owner = None;
            {
                let mut ui = self.device.shared.ui.lock().unwrap();
                let usb = ui.usb;
                let addresses = ui.addresses.clone();
                *ui = UiState {
                    usb,
                    addresses,
                    ..Default::default()
                };
            }
            self.device.shared.reset_media();
            drop(owner);
            self.device.hls.stop();
        }
        self.claimed = false;
        if let Some(lease) = self.lease.take() {
            *lease.done.lock().unwrap() = true;
            lease.ready.notify_all();
        }
    }
    fn route(&mut self, req: &Request) -> Result<Response> {
        let path = req.path();
        let method = req.method.as_str();
        match (method,path) {
            ("OPTIONS",_)=>return Ok(Response::ok().header("Public","ANNOUNCE, SETUP, RECORD, PAUSE, FLUSH, TEARDOWN, OPTIONS, GET_PARAMETER, SET_PARAMETER, POST, GET")),
            ("GET","/info")=>return Response::ok().plist(self.device.info()),
            ("POST","/pair-setup")=>return Ok(Response::ok().bytes("application/octet-stream",self.device.identity.verifying_key().to_bytes().to_vec())),
            ("POST","/pair-verify")=>{
                let result=if req.body.first()==Some(&1) {PairVerify::start(&self.device.identity,&req.body).map(|(p,r)|{self.pair=Some(p);r})}
                    else {self.pair.as_mut().ok_or_else(||anyhow::anyhow!("Missing pair round one")).and_then(|p|p.finish(&req.body)).map(|_|Vec::new())};
                return Ok(match result {Ok(bytes)=>Response::ok().bytes("application/octet-stream",bytes),Err(_)=>{self.pair=None;Response {status:403,..Response::ok()}}});
            },
            ("POST","/fp-setup")=>return Ok(match self.fairplay.process(&req.body) {Ok(bytes)=>Response::ok().bytes("application/octet-stream",bytes),Err(_)=>Response {status:400,..Response::ok()}}),
            ("POST","/feedback")=>return Ok(Response::ok()),
            ("POST","/audioMode")=>{self.device.shared.ui.lock().unwrap().paused=false;return Ok(Response::ok());},
            ("POST","/command")=>return Ok(Response::ok()),
            ("POST","/auth-setup")=>return Ok(Response {status:501,..Response::ok()}),
            ("GET","/server-info")=>{
                let value=dict([("features",uint(if self.device.shared.settings.read().unwrap().hls_enabled {0x27f}else{0x26f})),
                    ("macAddress",Value::String(self.device.mac.clone())),("deviceid",Value::String(self.device.mac.clone())),
                    ("model",Value::String("AppleTV3,2".into())),("osBuildVersion",Value::String("12B435".into())),("protovers",Value::String("1.0".into())),
                    ("srcvers",Value::String("220.68".into())),("vv",uint(2))]);let mut bytes=Vec::new();value.to_writer_xml(&mut bytes)?;
                return Ok(Response::ok().bytes("text/x-apple-plist+xml",bytes));
            },
            ("POST","/play")=>{
                if !self.device.shared.settings.read().unwrap().hls_enabled {return Ok(Response {status:501,..Response::ok()})}
                if !self.claim()? {return Ok(Response {status:503,..Response::ok()})}
                let (url,position)=if req.body.starts_with(b"bplist00") {
                    let value=Value::from_reader(Cursor::new(&req.body))?;let d=value.as_dictionary().context("Invalid playback request")?;
                    ensure!(!d.contains_key("FairPlay")&&!d.contains_key("FPS"),"Protected HLS playback is unsupported");
                    (string(d,"Content-Location").context("Missing playback URL")?.to_owned(),d.get("Start-Position-Seconds").and_then(Value::as_real).unwrap_or(0.))
                }else{let body=String::from_utf8_lossy(&req.body);(body.lines().find_map(|s|s.strip_prefix("Content-Location:")).context("Missing playback URL")?.trim().into(),0.)};
                self.device.hls.play(req.header("x-apple-session-id"),&url,position)?;
                {let mut ui=self.device.shared.ui.lock().unwrap();ui.peer=self.peer.ip().to_string();ui.kind="HLS playback".into();}
                return Ok(Response::ok());
            },
            ("POST","/action")=>{self.device.hls.action(req.header("x-apple-session-id"),Value::from_reader(Cursor::new(&req.body))?)?;return Ok(Response::ok());},
            ("POST","/stop")=>{self.device.hls.stop();return Ok(Response::ok());},
            ("POST","/rate")=>{if let Some(rate)=req.query("value").and_then(|v|v.parse::<f64>().ok()).filter(|v|v.is_finite()){self.device.hls.rate(rate)}return Ok(Response::ok());},
            ("POST","/scrub")=>{if let Some(position)=req.query("position").and_then(|v|v.parse::<f64>().ok()).filter(|p|p.is_finite()){self.device.hls.seek(position)}return Ok(Response::ok());},
            ("GET","/scrub")=>return Ok(Response::ok().bytes("text/parameters",format!("position: {}\r\n",self.device.hls.position()).into_bytes())),
            ("GET","/playback-info")=>{let mut bytes=Vec::new();self.device.hls.info().to_writer_xml(&mut bytes)?;return Ok(Response::ok().bytes("text/x-apple-plist+xml",bytes));},
            ("POST"|"PUT","/getProperty"|"/setProperty")=>{
                let value=Value::from_reader(Cursor::new(&req.body)).unwrap_or_else(|_|dict([] as [(&str,Value);0]));
                let key=req.uri.split_once('?').map(|(_,q)|q.trim_end_matches('&')).or_else(||value.as_dictionary().and_then(|d|string(d,"property"))).unwrap_or("default");
                let bytes=self.device.hls.property(req.header("x-apple-session-id"),key,(path=="/setProperty").then(||req.body.clone()))?;
                return Ok(Response::ok().bytes(if bytes.starts_with(b"bplist00"){"application/x-apple-binary-plist"}else{"application/octet-stream"},bytes));
            },
            _=>{},
        }
        match method {
            "ANNOUNCE" => {
                let text = std::str::from_utf8(&req.body)?;
                ensure!(
                    text.starts_with("v=0") && text.lines().any(|s| s.starts_with("m=")),
                    "Invalid SDP"
                );
                self.sdp = Some(text.into());
                Ok(Response::ok())
            }
            "SETUP" => self.setup(req),
            "RECORD" => {
                if !self.claimed {
                    return Ok(Response {
                        status: 455,
                        ..Response::ok()
                    });
                }
                self.device.shared.ui.lock().unwrap().paused = false;
                Ok(Response::ok()
                    .header("Session", self.id)
                    .header("Audio-Latency", 11025))
            }
            "FLUSH" | "PAUSE" => {
                if let Some(audio) = &self.audio {
                    let seq = req
                        .header("rtp-info")
                        .split(';')
                        .find_map(|s| s.trim().strip_prefix("seq=").and_then(|s| s.parse().ok()));
                    audio.command(AudioCommand::Flush(seq));
                }
                if self.claimed {
                    self.device.shared.ui.lock().unwrap().paused = true;
                }
                Ok(Response::ok())
            }
            "TEARDOWN" => {
                let value = Value::from_reader(Cursor::new(&req.body)).ok();
                let streams = value
                    .as_ref()
                    .and_then(|p| p.as_dictionary())
                    .and_then(|d| d.get("streams"))
                    .and_then(|v| v.as_array());
                if let Some(streams) = streams {
                    for s in streams {
                        match s.as_dictionary().and_then(|d| integer(d, "type")) {
                            Some(96) => self.audio = None,
                            Some(110) => self.mirror = None,
                            _ => {}
                        }
                    }
                } else {
                    self.clear();
                }
                Ok(Response::ok())
            }
            "GET_PARAMETER" => Ok(Response::ok().bytes(
                "text/parameters",
                format!(
                    "volume: {:.6}\r\n",
                    self.device.shared.ui.lock().unwrap().volume_db
                )
                .into_bytes(),
            )),
            "SET_PARAMETER" => {
                if !self.claimed {
                    return Ok(Response::ok());
                }
                let content = req.header("content-type");
                if content.is_empty() || content.starts_with("text/parameters") {
                    for line in String::from_utf8_lossy(&req.body).lines() {
                        if let Some(value) = line.strip_prefix("progress:") {
                            let numbers = value
                                .trim()
                                .split('/')
                                .map(str::parse::<u64>)
                                .collect::<std::result::Result<Vec<_>, _>>();
                            if let Ok(p) = numbers
                                && p.len() == 3
                                && p[2] >= p[0]
                            {
                                let mut ui = self.device.shared.ui.lock().unwrap();
                                ui.duration_seconds = (p[2] - p[0]) as f64 / 44100.;
                                ui.progress_seconds = (p[1].saturating_sub(p[0]) as f64 / 44100.)
                                    .min(ui.duration_seconds);
                                ui.progress_at = Some(std::time::Instant::now());
                            }
                        }
                        if let Some(db) = line
                            .strip_prefix("volume:")
                            .and_then(|v| v.trim().parse::<f32>().ok())
                            .filter(|v| v.is_finite())
                        {
                            self.device.shared.ui.lock().unwrap().volume_db = db;
                            if let Some(audio) = &self.audio {
                                audio.command(AudioCommand::Volume(db));
                            }
                        }
                    }
                } else if content.starts_with("image/") {
                    self.device.shared.ui.lock().unwrap().cover = Arc::new(req.body.clone());
                } else if content == "application/x-dmap-tagged" {
                    let mut m = BTreeMap::new();
                    protocol::metadata(&req.body, 0, &mut m);
                    let mut ui = self.device.shared.ui.lock().unwrap();
                    if let Some(v) = m.remove("minm") {
                        ui.title = v
                    }
                    if let Some(v) = m.remove("asar") {
                        ui.artist = v
                    }
                    if let Some(v) = m.remove("asal") {
                        ui.album = v
                    }
                    if let Some(ms) = m.remove("astm").and_then(|s| s.parse::<u32>().ok()) {
                        ui.duration_seconds = ms as f64 / 1000.;
                    }
                }
                Ok(Response::ok())
            }
            _ => Ok(Response {
                status: 404,
                ..Response::ok()
            }),
        }
    }
    fn setup(&mut self, req: &Request) -> Result<Response> {
        if !self.claim()? {
            return Ok(Response {
                status: 503,
                ..Response::ok()
            });
        }
        if req.body.is_empty() {
            ensure!(
                !req.header("transport").is_empty(),
                "Missing SETUP transport"
            );
            self.legacy.clear();
            for _ in 0..3 {
                self.legacy.push(transport::udp(self.peer)?);
            }
            let p = self
                .legacy
                .iter()
                .map(|s| s.local_addr().map(|a| a.port()))
                .collect::<std::io::Result<Vec<_>>>()?;
            return Ok(Response::ok().header("Session", self.id).header(
                "Transport",
                format!(
                    "RTP/AVP/UDP;unicast;mode=record;server_port={};control_port={};timing_port={}",
                    p[0], p[1], p[2]
                ),
            ));
        }
        let value = Value::from_reader(Cursor::new(&req.body))?;
        let d = value
            .as_dictionary()
            .ok_or_else(|| anyhow::anyhow!("SETUP is not a dictionary"))?;
        let mut response = plist::Dictionary::new();
        if let (Some(ekey), Some(eiv)) = (
            d.get("ekey").and_then(Value::as_data),
            d.get("eiv").and_then(Value::as_data),
        ) {
            ensure!(eiv.len() == 16, "Invalid stream IV");
            ensure!(
                d.get("isRemoteControlOnly").and_then(Value::as_boolean) != Some(true),
                "Remote control timing is unsupported"
            );
            let mut key = self.fairplay.decrypt(ekey)?;
            if let Some(p) = &self.pair {
                let mut digest = Sha512::new();
                digest.update(key);
                digest.update(p.secret);
                key.copy_from_slice(&digest.finalize()[..16]);
            }
            self.key = Some(key);
            self.iv.copy_from_slice(eiv);
            let remote = integer(d, "timingPort").unwrap_or(0);
            ensure!(remote <= 65535, "Invalid timing port");
            self.timing = None;
            let (port, worker) =
                transport::timing(self.peer, remote as u16, self.device.shared.clone())?;
            self.timing = Some(worker);
            response.insert("eventPort".into(), uint(0));
            response.insert("timingPort".into(), uint(port as u64));
        }
        if let Some(streams) = d.get("streams").and_then(Value::as_array) {
            ensure!(streams.len() <= 8, "Too many streams");
            let mut allocated = Vec::new();
            for stream in streams {
                let s = stream
                    .as_dictionary()
                    .ok_or_else(|| anyhow::anyhow!("Invalid stream dictionary"))?;
                let kind = integer(s, "type").unwrap_or(0);
                let key = self
                    .key
                    .ok_or_else(|| anyhow::anyhow!("Missing encryption key"))?;
                let allocation = match kind {
                    110 => {
                        self.mirror = None;
                        let (port, worker) = transport::mirror(
                            self.peer,
                            key,
                            protocol::stream_connection_id(s).unwrap_or(0),
                            self.device.shared.clone(),
                        )?;
                        self.mirror = Some(worker);
                        self.device.shared.ui.lock().unwrap().kind = "Screen mirroring".into();
                        dict([("type", uint(110)), ("dataPort", uint(port as u64))])
                    }
                    96 => {
                        self.audio = None;
                        let ct = integer(s, "ct").unwrap_or(8);
                        let rate = integer(s, "sampleRate").unwrap_or(44100);
                        ensure!((7350..=96000).contains(&rate), "Invalid sample rate");
                        let channels = integer(s, "ch").unwrap_or(2);
                        ensure!(matches!(channels, 1 | 2), "Invalid channels");
                        let control = integer(s, "controlPort").unwrap_or(0);
                        ensure!(control <= 65535, "Invalid control port");
                        let spf = integer(s, "spf").unwrap_or(if ct == 2 { 352 } else { 480 });
                        ensure!(spf <= 4096 && spf > 0, "Invalid frame length");
                        let opts = AudioOptions {
                            ct,
                            rate: rate as u32,
                            channels: channels as u8,
                            spf: spf as u32,
                            key,
                            iv: self.iv,
                            control_port: control as u16,
                        };
                        let (data, control, audio) =
                            transport::audio(self.peer, opts, self.device.shared.clone())?;
                        audio.command(AudioCommand::Volume(
                            self.device.shared.ui.lock().unwrap().volume_db,
                        ));
                        self.audio = Some(audio);
                        {
                            let mut ui = self.device.shared.ui.lock().unwrap();
                            if ui.kind.is_empty() {
                                ui.kind = "Audio".into();
                            }
                        }
                        dict([
                            ("type", uint(96)),
                            ("dataPort", uint(data as u64)),
                            ("controlPort", uint(control as u64)),
                        ])
                    }
                    _ => bail!("Unsupported stream type {kind}"),
                };
                allocated.push(allocation);
            }
            response.insert("streams".into(), Value::Array(allocated));
        }
        {
            let mut ui = self.device.shared.ui.lock().unwrap();
            ui.peer = self.peer.ip().to_string();
            if let Some(name) = string(d, "name") {
                ui.device = name.into()
            }
            if let Some(model) = string(d, "model") {
                ui.model = model.into()
            }
        }
        Response::ok()
            .header("Session", self.id)
            .plist(Value::Dictionary(response))
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.clear();
        self.device.hls.remove_reverse(self.id);
    }
}
fn connection(mut stream: TcpStream, peer: SocketAddr, id: u64, device: Arc<Device>) -> Result<()> {
    // Accept inherits nonblocking mode on Windows; timeout-based reads must
    // block between requests instead of spinning on WouldBlock.
    stream.set_nonblocking(false)?;
    let peer = match peer {
        SocketAddr::V6(v6) => v6
            .ip()
            .to_ipv4_mapped()
            .map(|v4| SocketAddr::new(v4.into(), v6.port()))
            .unwrap_or(peer),
        _ => peer,
    };
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_nodelay(true)?;
    let mut reader = Reader::default();
    let generation = device.shared.disconnect.load(Ordering::Acquire);
    let mut session = Session {
        id,
        peer,
        device: device.clone(),
        pair: None,
        fairplay: FairPlay::default(),
        key: None,
        iv: [0; 16],
        timing: None,
        audio: None,
        mirror: None,
        legacy: vec![],
        sdp: None,
        claimed: false,
        lease: None,
    };
    while device.shared.running.load(Ordering::Acquire)
        && generation == device.shared.disconnect.load(Ordering::Acquire)
        && !session
            .lease
            .as_ref()
            .is_some_and(|l| l.stop.load(Ordering::Acquire))
    {
        let Some(request) = reader.next(&mut stream)? else {
            continue;
        };
        if request.method == "REVERSE_RESPONSE" {
            continue;
        }
        if request.method == "POST" && request.path() == "/reverse" {
            Response {
                status: 101,
                ..Response::ok()
            }
            .header("Upgrade", "PTTH/1.0")
            .header("Connection", "Upgrade")
            .write(&request, &mut stream)?;
            device.hls.reverse(
                request.header("x-apple-session-id"),
                id,
                stream.try_clone()?,
            )?;
            continue;
        }
        let response = match session.route(&request) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("{} {}: {e:#}", request.method, request.path());
                Response {
                    status: 400,
                    ..Response::ok()
                }
            }
        };
        response.write(&request, &mut stream)?;
        if request.header("connection").eq_ignore_ascii_case("close") {
            break;
        }
    }
    Ok(())
}
