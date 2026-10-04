//! Desktop presentation only. Receiver lifetime is owned by Runtime, outside Iced.
use crate::{
    config::{self, Settings},
    media::VideoFrame,
    render::{self, compositor::Renderer},
    runtime::{self, Client, Runtime},
    state::UiState,
};
use anyhow::{Context, Result};
use clap::Parser;
use fs2::FileExt;
use futures::{StreamExt, channel::mpsc};
use iced::{
    Element, Length, Subscription, Task, Theme,
    widget::{
        self, button, checkbox, column, container, pick_list, row, scrollable, slider, text,
        text_input,
    },
    window,
};
use std::{
    fs::OpenOptions,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
#[derive(Parser, Clone, Debug)]
#[command(version, about = "AirPlay receiver — Iced + wgpu / cpal")]
struct Arguments {
    #[arg(long)]
    headless: bool,
    #[arg(long, default_value_t = 7000)]
    port: u16,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    config_dir: Option<PathBuf>,
    #[arg(long)]
    mirror_res: Option<String>,
    #[arg(long)]
    fps: Option<u32>,
    #[arg(long)]
    no_hevc: bool,
    #[arg(long)]
    hwaccel: bool,
    #[arg(long)]
    start_hidden: bool,
    #[arg(long)]
    hls_proxy_playback: bool,
    #[arg(long)]
    metrics: Option<PathBuf>,
    #[arg(long)]
    exit_after: Option<u64>,
    #[arg(long)]
    screenshot: Option<PathBuf>,
    #[arg(long)]
    log: Option<PathBuf>,
    /// Explicit deterministic backend for CI; never implies a real speaker test.
    #[arg(long)]
    audio_null: bool,
    #[arg(long)]
    native_report: bool,
}
pub fn run() -> Result<()> {
    let args = Arguments::parse();
    if args.native_report {
        println!("{}", crate::render::native_report()?);
        return Ok(());
    }
    // Set once, before any threads exist; runtime selection does not mutate the process environment.
    if args.audio_null {
        unsafe {
            std::env::set_var("AIRPLAY_AUDIO_NULL", "1");
        }
    }
    ffmpeg_next::init()?;
    if let Some(path) = &args.log {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(Mutex::new(file))
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();
    }
    let directory = args.config_dir.clone().unwrap_or_else(config::data_dir);
    std::fs::create_dir_all(&directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("receiver.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        std::fs::write(directory.join("restore-window"), b"show")?;
        return Ok(());
    }
    let mut settings = Settings::load(&directory.join("settings.json"))?;
    if let Some(name) = &args.name {
        settings.name = name.clone()
    }
    if let Some(res) = &args.mirror_res {
        let (w, h) = res
            .split_once('x')
            .context("Resolution must be WIDTHxHEIGHT")?;
        settings.mirror_width = w.parse()?;
        settings.mirror_height = h.parse()?;
    }
    if let Some(fps) = args.fps {
        settings.max_fps = fps;
        settings.refresh_rate = fps;
    }
    if args.no_hevc {
        settings.hevc_enabled = false
    }
    if args.hwaccel {
        settings.hardware_decode = true
    }
    if args.hls_proxy_playback {
        settings.hls_enabled = true
    }
    settings.validate()?;
    render::compositor::PREFERENCE.store(
        match settings.gpu_preference.as_str() {
            "high-performance" => 2,
            "low-power" => 1,
            _ => 0,
        },
        Ordering::Relaxed,
    );
    crate::platform::install_exit_handlers()?;
    let mut runtime = Runtime::start(directory.clone(), args.port, settings.clone())?;
    tracing::info!(
        "AirPlay listening on port {} as {}",
        args.port,
        settings.name
    );
    let result = if args.headless {
        while runtime.client.shared.running.load(Ordering::Acquire) {
            if args
                .exit_after
                .is_some_and(|s| runtime.client.shared.started.elapsed() >= Duration::from_secs(s))
            {
                break;
            }
            let guard = runtime.client.shared.media.display.latest.lock().unwrap();
            let _ = runtime
                .client
                .shared
                .media
                .display
                .ready
                .wait_timeout(guard, Duration::from_millis(100));
        }
        Ok(())
    } else {
        let client = runtime.client.clone();
        let a = args.clone();
        iced::daemon(
            move || {
                App::boot(
                    client.clone(),
                    settings.clone(),
                    directory.clone(),
                    a.clone(),
                )
            },
            App::update,
            App::view,
        )
        .title(|_: &App, _| "AirPlay-Windows".to_owned())
        .theme(|_: &App, _| desktop_theme())
        .subscription(App::subscription)
        .run()
        .map_err(Into::into)
    };
    runtime.stop();
    if let Some(path) = args.metrics {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&runtime.client.shared.snapshot())?,
        )?;
    }
    result
}
#[derive(Clone, Debug)]
enum Message {
    Frame,
    Status,
    Tick,
    Opened,
    Window(window::Id, window::Event),
    Minimized(bool),
    Page(u8),
    Fullscreen,
    Crop,
    Focus,
    Escape,
    Hide,
    Quit,
    Disconnect,
    ClearError,
    Save,
    Reset,
    Name(String),
    Width(String),
    Height(String),
    Fps(String),
    Gpu(String),
    Audio(crate::audio::DeviceChoice),
    Hardware(bool),
    Hevc(bool),
    Vsync(bool),
    Hls(bool),
    MinimizeTray(bool),
    Autostart(bool),
    StartHidden(bool),
    Volume(f32),
    Devices(Result<Vec<crate::audio::DeviceChoice>, String>),
    Screenshot(window::Screenshot),
    #[cfg(windows)]
    Tray(crate::platform::tray::Command),
}
#[derive(Clone)]
struct FrameEvents(Arc<Mutex<Option<mpsc::Receiver<()>>>>, bool);
impl Hash for FrameEvents {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(h)
    }
}
impl PartialEq for FrameEvents {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for FrameEvents {}
fn frame_events(events: &FrameEvents) -> impl futures::Stream<Item = Message> + use<> {
    {
        let status = events.1;
        events
            .0
            .lock()
            .unwrap()
            .take()
            .expect("subscription starts once")
            .map(move |_| {
                if status {
                    Message::Status
                } else {
                    Message::Frame
                }
            })
    }
}
#[cfg(windows)]
#[derive(Clone)]
struct TrayEvents(Arc<Mutex<Option<mpsc::UnboundedReceiver<crate::platform::tray::Command>>>>);
#[cfg(windows)]
impl Hash for TrayEvents {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(h)
    }
}
#[cfg(windows)]
impl PartialEq for TrayEvents {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
#[cfg(windows)]
impl Eq for TrayEvents {}
#[cfg(windows)]
fn tray_events(events: &TrayEvents) -> impl futures::Stream<Item = Message> + use<> {
    events
        .0
        .lock()
        .unwrap()
        .take()
        .expect("tray events start once")
        .map(Message::Tray)
}
struct App {
    client: Client,
    settings: Settings,
    directory: PathBuf,
    args: Arguments,
    status: UiState,
    frame: Option<Arc<VideoFrame>>,
    events: FrameEvents,
    status_events: FrameEvents,
    window: Option<window::Id>,
    page: u8,
    fullscreen: bool,
    crop: bool,
    focus: bool,
    minimized: bool,
    gpu_warning: bool,
    width: String,
    height: String,
    fps: String,
    devices: Vec<crate::audio::DeviceChoice>,
    cover: Option<widget::image::Handle>,
    cover_source: Arc<Vec<u8>>,
    last_capture: bool,
    last_metrics: u64,
    metrics_text: String,
    #[cfg(windows)]
    tray: Option<crate::platform::tray::Tray>,
    #[cfg(windows)]
    tray_events: TrayEvents,
}
impl App {
    fn boot(
        client: Client,
        settings: Settings,
        directory: PathBuf,
        args: Arguments,
    ) -> (Self, Task<Message>) {
        let (sender, receiver) = mpsc::channel(1);
        *client.shared.media.display.wake.lock().unwrap() = Some(sender);
        let (status_sender, status_receiver) = mpsc::channel(1);
        *client.shared.ui.wake.lock().unwrap() = Some(status_sender);
        #[cfg(windows)]
        let mut tray = crate::platform::tray::Tray::new()
            .map_err(|e| client.shared.report(format!("Tray unavailable: {e:#}")))
            .ok();
        #[cfg(windows)]
        let tray_events = TrayEvents(Arc::new(Mutex::new(tray.as_mut().map(|t| t.take_events()))));
        let hidden = (args.start_hidden || settings.start_hidden) && cfg!(windows);
        let mut app = Self {
            client,
            events: FrameEvents(Arc::new(Mutex::new(Some(receiver))), false),
            status_events: FrameEvents(Arc::new(Mutex::new(Some(status_receiver))), true),
            status: UiState::default(),
            frame: None,
            window: None,
            page: 0,
            fullscreen: settings.fullscreen,
            crop: false,
            focus: settings.hide_ui,
            minimized: false,
            gpu_warning: false,
            width: settings.mirror_width.to_string(),
            height: settings.mirror_height.to_string(),
            fps: settings.max_fps.to_string(),
            devices: vec![crate::audio::DeviceChoice::default_output()],
            cover: None,
            cover_source: Arc::new(Vec::new()),
            last_capture: false,
            last_metrics: 0,
            metrics_text: String::new(),
            #[cfg(windows)]
            tray,
            #[cfg(windows)]
            tray_events,
            settings,
            directory,
            args,
        };
        let task = if hidden && app.tray_available() {
            Task::none()
        } else {
            app.open_window()
        };
        (app, task)
    }
    fn tray_available(&self) -> bool {
        #[cfg(windows)]
        {
            self.tray.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    fn open_window(&mut self) -> Task<Message> {
        if let Some(id) = self.window {
            self.minimized = false;
            self.client
                .shared
                .media
                .display
                .visible
                .store(self.page == 0 && !self.gpu_warning, Ordering::Release);
            self.client.shared.ui.visible.store(true, Ordering::Release);
            self.client
                .shared
                .metrics
                .last_submission_us
                .store(0, Ordering::Relaxed);
            self.refresh();
            return Task::batch([window::minimize(id, false), window::gain_focus(id)]);
        }
        let (id, task) = window::open(window::Settings {
            size: iced::Size::new(
                self.settings.window_width as f32,
                self.settings.window_height as f32,
            ),
            min_size: Some(iced::Size::new(760., 520.)),
            exit_on_close_request: false,
            fullscreen: self.fullscreen,
            ..Default::default()
        });
        self.window = Some(id);
        self.client
            .shared
            .media
            .display
            .visible
            .store(self.page == 0, Ordering::Release);
        self.minimized = false;
        self.client
            .shared
            .metrics
            .last_submission_us
            .store(0, Ordering::Relaxed);
        self.client.shared.ui.visible.store(true, Ordering::Release);
        task.map(|_| Message::Opened)
    }
    fn subscription(&self) -> Subscription<Message> {
        #[allow(unused_mut)]
        let mut subscriptions = vec![
            Subscription::run_with(self.events.clone(), frame_events),
            Subscription::run_with(self.status_events.clone(), frame_events),
            window::events().filter_map(|(id, event)| {
                (!matches!(event, window::Event::RedrawRequested(_)))
                    .then_some(Message::Window(id, event))
            }),
            iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key, modifiers, ..
                }) => match key {
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::F11) => {
                        Some(Message::Fullscreen)
                    }
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => {
                        Some(Message::Escape)
                    }
                    iced::keyboard::Key::Character(c) if modifiers.control() => match c.as_str() {
                        "h" | "H" => Some(Message::Focus),
                        "d" | "D" => Some(Message::Disconnect),
                        "q" | "Q" => Some(Message::Quit),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            }),
        ];
        #[cfg(windows)]
        if self.tray.is_some() {
            subscriptions.push(Subscription::run_with(
                self.tray_events.clone(),
                tray_events,
            ));
        }
        Subscription::batch(subscriptions)
    }
    fn refresh(&mut self) {
        let paused = self.status.paused;
        self.status = self.client.shared.ui.lock().unwrap().clone();
        if paused != self.status.paused {
            self.client
                .shared
                .metrics
                .last_submission_us
                .store(0, Ordering::Relaxed);
        }
        let epoch = self.client.shared.generation();
        self.frame = self
            .client
            .shared
            .media
            .display
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .filter(|f| f.epoch == epoch)
            .cloned();
        if !Arc::ptr_eq(&self.status.cover, &self.cover_source) {
            self.cover_source = self.status.cover.clone();
            self.cover = (!self.cover_source.is_empty())
                .then(|| widget::image::Handle::from_bytes((*self.cover_source).clone()));
        }
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Opened | Message::Frame | Message::Status => self.refresh(),
            Message::Tick => {
                if crate::platform::exit_requested()
                    || !self.client.shared.running.load(Ordering::Acquire)
                {
                    return Task::done(Message::Quit);
                }
                self.refresh();
                if self.directory.join("restore-window").exists() {
                    let _ = std::fs::remove_file(self.directory.join("restore-window"));
                    return self.open_window();
                }
                let gpu = render::compositor::STATUS.load(Ordering::Acquire);
                if gpu == 3 {
                    self.client
                        .shared
                        .media
                        .display
                        .visible
                        .store(false, Ordering::Release);
                    self.client.shared.report("Video unavailable: no compatible GPU. Software fallback supports controls and audio only.".into());
                    self.status.error = self.client.shared.ui.lock().unwrap().error.clone();
                }
                if gpu == 5 {
                    self.status.error = "GPU device lost; recovery failed. The receiver and audio remain active. Restart the application to restore video.".into();
                    if !self.gpu_warning {
                        self.gpu_warning = true;
                        self.client
                            .shared
                            .media
                            .display
                            .visible
                            .store(false, Ordering::Release);
                        crate::platform::startup_error(&self.status.error);
                    }
                }
                let n = self.client.shared.metrics.presented.load(Ordering::Relaxed);
                self.metrics_text = format!(
                    "{} new video submissions/s · {} decoded · {:.1} ms processing",
                    n.saturating_sub(self.last_metrics),
                    self.client.shared.metrics.decoded.load(Ordering::Relaxed),
                    self.client
                        .shared
                        .metrics
                        .latency_us
                        .load(Ordering::Relaxed) as f64
                        / 1000.
                );
                self.last_metrics = n;
                if self
                    .args
                    .exit_after
                    .is_some_and(|s| self.client.shared.started.elapsed() >= Duration::from_secs(s))
                {
                    return Task::done(Message::Quit);
                }
                if !self.last_capture
                    && self.args.screenshot.is_some()
                    && self.client.shared.started.elapsed() > Duration::from_secs(1)
                {
                    self.last_capture = true;
                    if let Some(id) = self.window {
                        return window::screenshot(id).map(Message::Screenshot);
                    }
                }
                if let Some(id) = self.window {
                    return window::is_minimized(id)
                        .map(|v| Message::Minimized(v.unwrap_or(false)));
                }
            }
            Message::Window(id, window::Event::CloseRequested) => {
                self.client
                    .shared
                    .media
                    .display
                    .visible
                    .store(false, Ordering::Release);
                self.client
                    .shared
                    .ui
                    .visible
                    .store(false, Ordering::Release);
                if self.tray_available() {
                    self.window = None;
                    return window::close(id);
                } else {
                    return window::minimize(id, true);
                }
            }
            Message::Window(_, window::Event::Resized(size)) => {
                if !self.fullscreen {
                    self.settings.window_width = size.width as u32;
                    self.settings.window_height = size.height as u32;
                }
            }
            Message::Window(_, _) => {}
            Message::Minimized(minimized) => {
                if self.minimized != minimized {
                    self.client
                        .shared
                        .metrics
                        .last_submission_us
                        .store(0, Ordering::Relaxed);
                }
                self.minimized = minimized;
                self.client.shared.media.display.visible.store(
                    !minimized && self.page == 0 && !self.gpu_warning,
                    Ordering::Release,
                );
                self.client
                    .shared
                    .ui
                    .visible
                    .store(!minimized, Ordering::Release);
                if minimized && self.settings.minimize_to_tray && self.tray_available() {
                    return Task::done(Message::Hide);
                }
            }
            Message::Page(page) => {
                self.page = page;
                self.client.shared.media.display.visible.store(
                    page == 0 && self.window.is_some() && !self.minimized && !self.gpu_warning,
                    Ordering::Release,
                );
                self.client
                    .shared
                    .metrics
                    .last_submission_us
                    .store(0, Ordering::Relaxed);
                self.refresh();
                if page == 1 {
                    return Task::perform(
                        async { crate::audio::devices().map_err(|e| format!("{e:#}")) },
                        Message::Devices,
                    );
                }
            }
            Message::Devices(result) => match result {
                Ok(devices) => self.devices = devices,
                Err(e) => self.client.shared.report(e),
            },
            Message::Fullscreen => {
                self.fullscreen = !self.fullscreen;
                self.settings.fullscreen = self.fullscreen;
                if let Some(id) = self.window {
                    return window::set_mode(
                        id,
                        if self.fullscreen {
                            window::Mode::Fullscreen
                        } else {
                            window::Mode::Windowed
                        },
                    );
                }
            }
            Message::Crop => self.crop = !self.crop,
            Message::Focus => {
                self.focus = !self.focus;
                self.settings.hide_ui = self.focus;
            }
            Message::Escape => {
                if self.fullscreen {
                    return Task::done(Message::Fullscreen);
                }
                if self.focus {
                    return Task::done(Message::Focus);
                }
            }
            Message::Hide => {
                if self.tray_available() {
                    self.client
                        .shared
                        .media
                        .display
                        .visible
                        .store(false, Ordering::Release);
                    self.client
                        .shared
                        .ui
                        .visible
                        .store(false, Ordering::Release);
                    if let Some(id) = self.window.take() {
                        return window::close(id);
                    }
                }
            }

            Message::Quit => return iced::exit(),
            Message::Disconnect => {
                let _ = self
                    .client
                    .commands
                    .try_send(runtime::Command::Disconnect)
                    .map_err(|e| {
                        self.client
                            .shared
                            .report(format!("Could not disconnect: {e}"))
                    });
            }
            Message::ClearError => self.client.shared.ui.lock().unwrap().error.clear(),
            Message::Save => {
                let result = (|| -> Result<()> {
                    self.settings.mirror_width = self.width.parse()?;
                    self.settings.mirror_height = self.height.parse()?;
                    self.settings.max_fps = self.fps.parse()?;
                    self.settings.refresh_rate = self.settings.max_fps;
                    self.settings.validate()?;
                    Ok(())
                })();
                if let Err(e) = result {
                    self.client.shared.report(format!("{e:#}"))
                } else {
                    let _ = self
                        .client
                        .commands
                        .try_send(runtime::Command::Settings(self.settings.clone()))
                        .map_err(|e| {
                            self.client
                                .shared
                                .report(format!("Could not apply settings: {e}"))
                        });
                }
            }
            Message::Reset => {
                self.settings = Settings::default();
                self.width = self.settings.mirror_width.to_string();
                self.height = self.settings.mirror_height.to_string();
                self.fps = self.settings.max_fps.to_string();
            }
            Message::Name(v) => self.settings.name = v,
            Message::Width(v) => self.width = v,
            Message::Height(v) => self.height = v,
            Message::Fps(v) => self.fps = v,
            Message::Gpu(v) => self.settings.gpu_preference = v,
            Message::Audio(v) => self.settings.audio_device = v.id,
            Message::Hardware(v) => self.settings.hardware_decode = v,
            Message::Hevc(v) => self.settings.hevc_enabled = v,
            Message::Vsync(v) => self.settings.vsync = v,
            Message::Hls(v) => self.settings.hls_enabled = v,
            Message::MinimizeTray(v) => self.settings.minimize_to_tray = v,
            Message::Autostart(v) => self.settings.autostart = v,
            Message::StartHidden(v) => self.settings.start_hidden = v,
            Message::Volume(v) => {
                self.client.shared.ui.lock().unwrap().volume_db = v;
            }
            Message::Screenshot(capture) => {
                if let Some(path) = &self.args.screenshot
                    && let Err(e) = image::save_buffer(
                        path,
                        &capture.rgba,
                        capture.size.width,
                        capture.size.height,
                        image::ColorType::Rgba8,
                    )
                {
                    self.client.shared.report(format!("Screenshot: {e}"));
                }
            }
            #[cfg(windows)]
            Message::Tray(command) => {
                return Task::done(match command {
                    crate::platform::tray::Command::Show => return self.open_window(),
                    crate::platform::tray::Command::Hide => Message::Hide,
                    crate::platform::tray::Command::Disconnect => Message::Disconnect,
                    crate::platform::tray::Command::Quit => Message::Quit,
                });
            }
        }
        Task::none()
    }
    fn view(&self, _: window::Id) -> Element<'_, Message, Theme, Renderer> {
        let navigation = column![
            text("AIRPLAY").size(13),
            text("Windows").size(25),
            text("RECEIVER").size(11),
            widget::space().height(24),
            button("Receive")
                .on_press(Message::Page(0))
                .style(if self.page == 0 {
                    button::primary
                } else {
                    button::text
                })
                .width(Length::Fill),
            button("Settings")
                .on_press(Message::Page(1))
                .style(if self.page == 1 {
                    button::primary
                } else {
                    button::text
                })
                .width(Length::Fill),
            button("Diagnostics")
                .on_press(Message::Page(2))
                .style(if self.page == 2 {
                    button::primary
                } else {
                    button::text
                })
                .width(Length::Fill),
            widget::space().height(Length::Fill),
            text("Service stays active\nin the system tray").size(12),
            button("Hide to tray").on_press_maybe(self.tray_available().then_some(Message::Hide)),
            button("Quit").on_press(Message::Quit)
        ]
        .spacing(12)
        .padding(22)
        .width(190);
        let content:Element<'_,Message,Theme,Renderer>=match self.page {
            1=>scrollable(column![text("Receiver settings").size(28),text("Network and decoder settings apply to the next connection. GPU selection applies on restart. Audio output changes immediately.").size(13),text_input("Receiver name",&self.settings.name).on_input(Message::Name),row![text_input("Width",&self.width).on_input(Message::Width),text_input("Height",&self.height).on_input(Message::Height),text_input("FPS",&self.fps).on_input(Message::Fps)].spacing(10),checkbox(self.settings.hardware_decode).label("Hardware decoding").on_toggle(Message::Hardware),checkbox(self.settings.hevc_enabled).label("Advertise HEVC").on_toggle(Message::Hevc),checkbox(self.settings.vsync).label("VSync (restart)").on_toggle(Message::Vsync),checkbox(self.settings.hls_enabled).label("HLS / FCUP playback").on_toggle(Message::Hls),text("Render adapter preference"),pick_list(vec!["balanced".to_string(),"low-power".into(),"high-performance".into()],Some(self.settings.gpu_preference.clone()),Message::Gpu),text("Audio output"),pick_list(self.devices.clone(),self.devices.iter().find(|d|d.id==self.settings.audio_device).cloned(),Message::Audio),checkbox(self.settings.minimize_to_tray).label("Minimize to tray").on_toggle(Message::MinimizeTray),checkbox(self.settings.start_hidden).label("Start in tray").on_toggle(Message::StartHidden),checkbox(self.settings.autostart).label("Start with Windows").on_toggle(Message::Autostart),row![button("Save settings").on_press(Message::Save),button("Reset form").on_press(Message::Reset)].spacing(12)].spacing(16).padding(28)).into(),
            2=>column![text("Diagnostics").size(28),text(&self.metrics_text),text(format!("Source: {}\nCodec: {}\nDecoder: {}\nDimensions: {}\nAudio: {}",self.status.peer,self.status.codec,self.status.decoder,self.status.dimensions,self.status.audio_status)),text(&self.status.addresses).size(13),text("Iced + wgpu · cpal / WASAPI · FFmpeg LGPL DLL profile"),text("Counters describe new GPU submissions, not measured screen scanout. Hardware performance and end-to-end AV latency require Windows/iPhone acceptance measurements.").size(13),text(format!("Version {}",env!("CARGO_PKG_VERSION")))].spacing(22).padding(28).into(),
            _=>{
                let video:Element<'_,Message,Theme,Renderer>=if let Some(frame)=&self.frame {
                    if render::compositor::STATUS.load(Ordering::Acquire)==3{container(text("Video requires a compatible GPU").size(22)).center(Length::Fill).into()}
                    else{widget::shader(render::Video{frame:frame.clone(),shared:self.client.shared.clone(),crop:self.crop}).width(Length::Fill).height(Length::Fill).into()}
                }else{
                    let title=if self.status.peer.is_empty(){"Ready to receive"}else if self.status.kind=="Audio"{"Audio playback"}else{"Waiting for video"};
                    let mut ready=column![text(title).size(29),text(&self.settings.name).size(20),text(if self.status.peer.is_empty(){"Open Screen Mirroring on your iPhone or iPad.\nConnect through your LAN or Windows mobile hotspot."}else{&self.status.title}).size(14)].spacing(16).align_x(iced::Alignment::Center);
                    if let Some(cover)=&self.cover{ready=ready.push(widget::image(cover.clone()).width(180).height(180));}
                    if !self.status.artist.is_empty(){ready=ready.push(text(&self.status.artist));}
                    container(ready).center(Length::Fill).into()
                };
                column![row![text(if self.status.device.is_empty(){"Your screen, here"}else{&self.status.device}).size(24),widget::space().width(Length::Fill),button("Disconnect").on_press_maybe((!self.status.peer.is_empty()).then_some(Message::Disconnect))].spacing(12),container(video).width(Length::Fill).height(Length::Fill).style(|_|container::Style{background:Some(iced::Color::from_rgb8(9,12,20).into()),border:iced::Border{radius:12.into(),..Default::default()},..Default::default()}),row![button("Fullscreen · F11").style(button::secondary).on_press(Message::Fullscreen),button(if self.crop{"Fit"}else{"Fill / crop"}).style(button::secondary).on_press(Message::Crop),widget::space().width(Length::Fill),text("Volume"),slider(-60.0..=0.,self.status.volume_db,Message::Volume).width(140)].spacing(14).align_y(iced::Alignment::Center)].spacing(20).padding(24).into()
            }
        };
        let mut body = column![content].height(Length::Fill).width(Length::Fill);
        if !self.status.error.is_empty() {
            body = body.push(
                container(
                    row![
                        text(&self.status.error).size(13),
                        button("Dismiss").on_press(Message::ClearError)
                    ]
                    .spacing(12),
                )
                .padding(12)
                .style(container::rounded_box),
            );
        }
        if self.focus
            && self.status.error.is_empty()
            && self.page == 0
            && matches!(render::compositor::STATUS.load(Ordering::Acquire), 1 | 2)
        {
            return container(if let Some(frame) = &self.frame {
                Element::from(
                    widget::shader(render::Video {
                        frame: frame.clone(),
                        shared: self.client.shared.clone(),
                        crop: self.crop,
                    })
                    .width(Length::Fill)
                    .height(Length::Fill),
                )
            } else {
                Element::from(
                    container(text("Waiting for video · Ctrl+H to show controls"))
                        .center(Length::Fill),
                )
            })
            .height(Length::Fill)
            .width(Length::Fill)
            .into();
        }
        container(
            row![navigation, body]
                .height(Length::Fill)
                .width(Length::Fill),
        )
        .height(Length::Fill)
        .into()
    }
}

fn desktop_theme() -> Theme {
    static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
    THEME
        .get_or_init(|| {
            Theme::custom(
                "AirPlay",
                iced::theme::Palette {
                    background: iced::Color::from_rgb8(17, 21, 31),
                    text: iced::Color::from_rgb8(221, 228, 244),
                    primary: iced::Color::from_rgb8(116, 145, 250),
                    success: iced::Color::from_rgb8(72, 197, 144),
                    warning: iced::Color::from_rgb8(228, 180, 92),
                    danger: iced::Color::from_rgb8(233, 121, 144),
                },
            )
        })
        .clone()
}
