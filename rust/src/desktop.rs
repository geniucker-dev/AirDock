// SPDX-License-Identifier: MPL-2.0
//! Desktop presentation only. Receiver lifetime is owned by Runtime, outside Iced.
use crate::{
    config::{self, Settings, WindowPreferences},
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
    widget::{self, button, checkbox, container, pick_list, row, scrollable, slider, text_input},
    window,
};
use std::{
    fs::OpenOptions,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};
mod appearance;
mod form;
mod view;
use appearance::desktop_theme;
use form::Form;

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
        let graphics_settings = iced::Settings {
            default_font: appearance::font(settings.language.resolve()),
            default_text_size: iced::Pixels(14.),
            fonts: vec![
                include_bytes!("../assets/fonts/AirPlayUICJK-Regular.ttf")
                    .as_slice()
                    .into(),
                include_bytes!("../assets/fonts/AirPlayUICJK-SemiBold.ttf")
                    .as_slice()
                    .into(),
                include_bytes!("../assets/fonts/Manrope-Regular.ttf")
                    .as_slice()
                    .into(),
                include_bytes!("../assets/fonts/Manrope-SemiBold.ttf")
                    .as_slice()
                    .into(),
            ],
            vsync: settings.vsync,
            ..Default::default()
        };
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
        .settings(graphics_settings)
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
    Minimized(window::Id, bool),
    ModeApplied(window::Id, window::Mode, u64, u8),
    CheckedSize(window::Id, iced::Size, window::Mode, u64),
    SettingsSaved(Result<Settings, String>),
    AudioSaved(Result<Settings, String>),
    PreferencesSaved(WindowPreferences, Result<Settings, String>),
    Pointer,
    ControlsHovered(bool),
    Mute,
    LeavePresentation,
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
    Diagnostics,
    Language(crate::i18n::Language),
    LanguageSaved(Result<Settings, String>),
    Audio(crate::audio::DeviceChoice),
    Hardware(bool),
    Hevc(bool),
    Vsync(bool),
    Hls(bool),
    MinimizeTray(bool),
    CloseTray(bool),
    Autostart(bool),
    StartHidden(bool),
    Volume(f32),
    Devices(Result<Vec<crate::audio::DeviceChoice>, String>),
    Capture(window::Id),
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
/// One reveal per receiving session, not per media generation (FLUSH/seek).
#[derive(Default)]
struct ConnectionReveal {
    seen: Option<u64>,
}
impl ConnectionReveal {
    fn video(&mut self, session: Option<u64>, has_frame: bool, hidden: bool) -> bool {
        let Some(session) = session.filter(|_| has_frame) else {
            return false;
        };
        if self.seen == Some(session) {
            return false;
        }
        self.seen = Some(session);
        hidden
    }
    fn dismiss(&mut self, session: Option<u64>) {
        if session.is_some() {
            self.seen = session;
        }
    }
}
struct App {
    client: Client,
    form: Form,
    preferences: WindowPreferences,
    preferences_dirty: Option<Instant>,
    preferences_pending: bool,
    quit_requested: bool,
    window_mode_revision: u64,
    mode_transition: bool,
    restoring_size: Option<iced::Size>,
    locale: crate::i18n::Language,
    controls_visible: bool,
    controls_hovered: bool,
    controls_used: Instant,
    last_audible_db: f32,
    directory: PathBuf,
    args: Arguments,
    status: UiState,
    frame: Option<Arc<VideoFrame>>,
    events: FrameEvents,
    status_events: FrameEvents,
    window: Option<window::Id>,
    viewport: iced::Size,
    page: u8,
    fullscreen: bool,
    fullscreen_return_page: Option<u8>,
    crop: bool,
    focus: bool,
    minimized: bool,
    close_minimized: bool,
    connection_reveal: ConnectionReveal,
    gpu_warning: bool,

    devices: Vec<crate::audio::DeviceChoice>,
    cover: Option<widget::image::Handle>,
    cover_source: Arc<Vec<u8>>,
    last_capture: bool,
    last_metrics: u64,
    stats: view::Stats,
    diagnostics_expanded: bool,
    ui_revision: u64,
    sampled_at: Instant,
    last_decoded: u64,
    last_bytes: u64,
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
        let mut tray = crate::platform::tray::Tray::new(settings.language.resolve())
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
            viewport: iced::Size::new(settings.window_width as f32, settings.window_height as f32),
            page: 0,
            fullscreen: settings.fullscreen,
            fullscreen_return_page: None,
            crop: false,
            focus: settings.hide_ui,
            minimized: false,
            close_minimized: false,
            connection_reveal: ConnectionReveal::default(),
            gpu_warning: false,
            form: Form::new(settings.clone()),
            preferences: WindowPreferences::from_settings(&settings),
            preferences_dirty: None,
            preferences_pending: false,
            quit_requested: false,
            window_mode_revision: 0,
            mode_transition: false,
            restoring_size: None,
            locale: settings.language.resolve(),
            controls_visible: false,
            controls_hovered: false,
            controls_used: Instant::now(),
            last_audible_db: 0.,
            devices: vec![crate::audio::DeviceChoice::default_output()],
            cover: None,
            cover_source: Arc::new(Vec::new()),
            last_capture: false,
            last_metrics: 0,
            stats: view::Stats::default(),
            diagnostics_expanded: false,
            ui_revision: u64::MAX,
            sampled_at: Instant::now(),
            last_decoded: 0,
            last_bytes: 0,
            #[cfg(windows)]
            tray,
            #[cfg(windows)]
            tray_events,
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
    fn language(&self) -> crate::i18n::Language {
        self.locale
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
        self.close_minimized = false;
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
        let restored = crate::config::restored_window_size(
            self.preferences.width,
            self.preferences.height,
            crate::platform::desktop_work_area(),
        );
        self.preferences.width = restored.0;
        self.preferences.height = restored.1;
        self.mode_transition = self.fullscreen;
        let (id, task) = window::open(window::Settings {
            size: iced::Size::new(
                self.preferences.width as f32,
                self.preferences.height as f32,
            ),
            min_size: Some(iced::Size::new(
                760.0_f32.min(restored.0 as f32),
                520.0_f32.min(restored.1 as f32),
            )),
            exit_on_close_request: false,
            position: window::Position::Centered,
            fullscreen: false,
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
    fn apply_window_mode(&mut self) -> Task<Message> {
        let Some(id) = self.window else {
            return Task::none();
        };
        self.mode_transition = true;
        self.restoring_size = (!self.fullscreen).then_some(iced::Size::new(
            self.preferences.width as f32,
            self.preferences.height as f32,
        ));
        let revision = self.window_mode_revision;
        let mut task = window::set_mode(
            id,
            if self.fullscreen {
                window::Mode::Fullscreen
            } else {
                window::Mode::Windowed
            },
        );
        if !self.fullscreen {
            task = task.chain(window::resize(
                id,
                iced::Size::new(
                    self.preferences.width as f32,
                    self.preferences.height as f32,
                ),
            ));
        }
        task.chain(Self::check_window_mode(id, revision, 0))
    }
    fn check_window_mode(id: window::Id, revision: u64, attempt: u8) -> Task<Message> {
        // X11/Windows mode changes settle asynchronously; an immediate query
        // can still return the preceding mode even though the change succeeds.
        Task::perform(
            async { tokio::time::sleep(Duration::from_millis(100)).await },
            |_| (),
        )
        .then(move |_| {
            window::mode(id).map(move |mode| Message::ModeApplied(id, mode, revision, attempt))
        })
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
        let revision = self.client.shared.ui.revision.load(Ordering::Acquire);
        if self.ui_revision != revision {
            self.status = self.client.shared.ui.lock().unwrap().clone();
            self.ui_revision = revision;
        }
        if self.status.volume_db > -100. {
            self.last_audible_db = self.status.volume_db;
        }
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
    fn active_session(&self) -> Option<u64> {
        self.client
            .shared
            .sessions
            .owner
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.id)
    }
    fn active_video_session(&self) -> Option<u64> {
        let owner = self.client.shared.sessions.owner.lock().unwrap();
        self.frame
            .as_ref()
            .filter(|f| f.epoch == self.client.shared.generation())
            .and_then(|_| owner.as_ref().map(|s| s.id))
    }
    fn reveal_video(&mut self) -> Option<Task<Message>> {
        if self.connection_reveal.video(
            self.active_video_session(),
            self.frame.is_some(),
            self.window.is_none() || self.minimized,
        ) {
            self.page = 0;
            return Some(self.open_window());
        }
        None
    }
    fn save_preferences(&mut self) -> Task<Message> {
        if self.preferences_dirty.is_none() || self.preferences_pending {
            return Task::none();
        }
        let values = self.preferences.clone();
        let (reply, receive) = futures::channel::oneshot::channel();
        match self
            .client
            .commands
            .try_send(runtime::Command::WindowPreferences(values.clone(), reply))
        {
            Ok(()) => {
                self.preferences_pending = true;
                Task::perform(
                    async move {
                        receive.await.unwrap_or_else(|_| {
                            Err("Receiver stopped before preferences were saved".into())
                        })
                    },
                    move |r| Message::PreferencesSaved(values.clone(), r),
                )
            }
            Err(e) => {
                self.client
                    .shared
                    .report(format!("Could not remember window preferences: {e}"));
                if self.quit_requested {
                    iced::exit()
                } else {
                    Task::none()
                }
            }
        }
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        if matches!(
            &message,
            Message::Name(_)
                | Message::Width(_)
                | Message::Height(_)
                | Message::Fps(_)
                | Message::Gpu(_)
                | Message::Hardware(_)
                | Message::Hevc(_)
                | Message::Vsync(_)
                | Message::Hls(_)
                | Message::MinimizeTray(_)
                | Message::CloseTray(_)
                | Message::Autostart(_)
                | Message::StartHidden(_)
        ) {
            self.form.changed();
        }
        match message {
            Message::Opened | Message::Frame | Message::Status => {
                if matches!(message, Message::Frame) {
                    self.client
                        .shared
                        .metrics
                        .ui_frame_events
                        .fetch_add(1, Ordering::Relaxed);
                }
                self.refresh();
                if matches!(message, Message::Opened) && self.fullscreen {
                    return self.apply_window_mode();
                }
                if let Some(task) = self.reveal_video() {
                    return task;
                }
            }
            Message::Tick => {
                if crate::platform::exit_requested()
                    || !self.client.shared.running.load(Ordering::Acquire)
                {
                    return Task::done(Message::Quit);
                }
                self.refresh();
                let controls_just_hidden = self.controls_visible
                    && !self.controls_hovered
                    && self.controls_used.elapsed() >= Duration::from_secs(2);
                if controls_just_hidden {
                    self.controls_visible = false;
                }

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
                self.sample_metrics();
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
                    && !controls_just_hidden
                    && (!self.fullscreen || !self.controls_visible)
                    && let Some(id) = self.window
                {
                    self.last_capture = true;
                    // Let this tick's rebuilt layout reach the renderer first.
                    // A same-update capture can consume the previous layer batch.
                    return Task::future(async move {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        Message::Capture(id)
                    });
                }
                if let Some(task) = self.reveal_video() {
                    return task;
                }
                if self
                    .preferences_dirty
                    .is_some_and(|t| t.elapsed() >= Duration::from_millis(500))
                    && !self.preferences_pending
                {
                    return self.save_preferences();
                }
                if let Some(id) = self.window {
                    return window::is_minimized(id)
                        .map(move |v| Message::Minimized(id, v.unwrap_or(false)));
                }
            }
            Message::Window(id, window::Event::CloseRequested) => {
                if self.window != Some(id) {
                    return Task::none();
                }
                let session = self.active_session();
                self.controls_visible = false;
                self.controls_hovered = false;
                self.connection_reveal.dismiss(session);
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
                let preferences = self.save_preferences();
                if self.tray_available() && self.form.saved.close_to_tray {
                    self.window = None;
                    return Task::batch([window::close(id), preferences]);
                } else {
                    self.close_minimized = true;
                    return Task::batch([window::minimize(id, true), preferences]);
                }
            }
            Message::Window(id, window::Event::Resized(size)) => {
                if self.window == Some(id) {
                    self.viewport = size;
                    if !self.fullscreen
                        && self
                            .restoring_size
                            .is_some_and(|target| size_matches(target, size))
                    {
                        self.restoring_size = None;
                        self.mode_transition = false;
                    }
                }
                if self.window == Some(id) && !self.fullscreen && !self.mode_transition {
                    let revision = self.window_mode_revision;
                    return window::mode(id)
                        .map(move |mode| Message::CheckedSize(id, size, mode, revision));
                }
            }
            Message::ModeApplied(id, mode, revision, attempt) => {
                if self.window == Some(id) && revision == self.window_mode_revision {
                    if self.fullscreen
                        || self.restoring_size.is_none()
                        || self
                            .restoring_size
                            .is_some_and(|target| size_matches(target, self.viewport))
                    {
                        self.mode_transition = false;
                        self.restoring_size = None;
                    }
                    if (mode == window::Mode::Fullscreen) != self.fullscreen {
                        if attempt < 9 {
                            return Self::check_window_mode(id, revision, attempt + 1);
                        }
                        self.client
                            .shared
                            .report("Window mode could not be applied".into());
                    }
                }
            }
            Message::CheckedSize(id, size, mode, revision) => {
                if self.window == Some(id)
                    && !self.fullscreen
                    && !self.mode_transition
                    && mode == window::Mode::Windowed
                    && revision == self.window_mode_revision
                    && size.width >= 760.
                    && size.height >= 520.
                {
                    let (w, h) = (size.width as u32, size.height as u32);
                    if (w, h) != (self.preferences.width, self.preferences.height) {
                        self.preferences.width = w;
                        self.preferences.height = h;
                        self.preferences_dirty = Some(Instant::now());
                    }
                }
            }
            Message::Window(_, _) => {}
            Message::Minimized(id, minimized) => {
                if self.window != Some(id) {
                    return Task::none();
                }
                if self.minimized != minimized {
                    if minimized {
                        let session = self.active_session();
                        self.connection_reveal.dismiss(session);
                    }
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
                if !minimized {
                    self.close_minimized = false;
                }
                if minimized
                    && !self.close_minimized
                    && self.form.saved.minimize_to_tray
                    && self.tray_available()
                {
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
                self.window_mode_revision += 1;
                self.preferences.fullscreen = self.fullscreen;
                self.preferences_dirty = Some(Instant::now());
                self.controls_visible = false;
                self.controls_hovered = false;
                if self.fullscreen {
                    self.fullscreen_return_page = Some(self.page);
                    self.page = 0;
                } else if let Some(page) = self.fullscreen_return_page.take() {
                    self.page = page;
                }
                self.client.shared.media.display.visible.store(
                    self.page == 0 && self.window.is_some() && !self.minimized && !self.gpu_warning,
                    Ordering::Release,
                );
                self.client
                    .shared
                    .metrics
                    .last_submission_us
                    .store(0, Ordering::Relaxed);
                self.refresh();
                return Task::batch([self.save_preferences(), self.apply_window_mode()]);
            }
            Message::Crop => self.crop = !self.crop,
            Message::Focus => {
                self.focus = !self.focus;
                self.preferences.hide_ui = self.focus;
                self.preferences_dirty = Some(Instant::now());
                self.controls_visible = false;
                self.controls_hovered = false;
                return self.save_preferences();
            }
            Message::Pointer => {
                if self.fullscreen || self.focus {
                    self.controls_visible = true;
                    self.controls_used = Instant::now();
                }
            }
            Message::ControlsHovered(v) => {
                self.controls_hovered = v;
                self.controls_used = Instant::now();
            }
            Message::LeavePresentation => {
                if self.fullscreen {
                    return Task::done(Message::Fullscreen);
                }
                if self.focus {
                    return Task::done(Message::Focus);
                }
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
                    self.controls_visible = false;
                    self.controls_hovered = false;
                    let session = self.active_session();
                    self.connection_reveal.dismiss(session);
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
                        return Task::batch([window::close(id), self.save_preferences()]);
                    }
                }
            }

            Message::Quit => {
                self.quit_requested = true;
                if self.preferences_pending {
                    return Task::none();
                }
                if self.preferences_dirty.is_some() {
                    return self.save_preferences();
                }
                return iced::exit();
            }
            Message::PreferencesSaved(values, result) => {
                self.preferences_pending = false;
                match result {
                    Ok(saved) => {
                        WindowPreferences::from_settings(&saved).apply(&mut self.form.saved);
                        if self.preferences == values {
                            self.preferences_dirty = None;
                        }
                    }
                    Err(error) => {
                        self.client
                            .shared
                            .report(format!("Could not remember window preferences: {error}"));
                        self.preferences_dirty = Some(Instant::now());
                        if self.quit_requested {
                            crate::platform::startup_error(&error);
                            return iced::exit();
                        }
                    }
                }
                if self.quit_requested {
                    return Task::done(Message::Quit);
                }
            }
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
                if self.form.pending.is_some() || self.form.audio_pending {
                    return Task::none();
                }
                let candidate = match self.form.validate() {
                    Ok(s) => s,
                    Err(e) => {
                        self.form.feedback = e;
                        return Task::none();
                    }
                };
                let (reply, receive) = futures::channel::oneshot::channel();
                if let Err(e) = self
                    .client
                    .commands
                    .try_send(runtime::Command::Settings(candidate, reply))
                {
                    self.form.committed(Err(e.to_string()));
                    return Task::none();
                }
                return Task::perform(
                    async move {
                        receive.await.unwrap_or_else(|_| {
                            Err("Receiver stopped before settings were saved".into())
                        })
                    },
                    Message::SettingsSaved,
                );
            }
            Message::SettingsSaved(result) => self.form.committed(result),
            Message::Reset => self.form.reset(),
            Message::Name(v) => self.form.draft.name = v,
            Message::Width(v) => self.form.width = v,
            Message::Height(v) => self.form.height = v,
            Message::Fps(v) => self.form.fps = v,
            Message::Diagnostics => {
                self.diagnostics_expanded = !self.diagnostics_expanded;
                if self.diagnostics_expanded {
                    self.sample_metrics();
                }
            }
            Message::Language(language) => {
                if self.form.pending.is_some() || self.form.audio_pending {
                    return Task::none();
                }
                self.form.audio_pending = true;
                let (reply, receive) = futures::channel::oneshot::channel();
                if let Err(error) = self
                    .client
                    .commands
                    .try_send(runtime::Command::Language(language, reply))
                {
                    return Task::done(Message::LanguageSaved(Err(error.to_string())));
                }
                return Task::perform(
                    async move {
                        receive
                            .await
                            .unwrap_or_else(|_| Err("Receiver stopped".into()))
                    },
                    Message::LanguageSaved,
                );
            }
            Message::LanguageSaved(result) => {
                self.form.audio_pending = false;
                match result {
                    Ok(saved) => {
                        self.locale = saved.language.resolve();
                        self.form.saved.language = saved.language;
                        self.form.draft.language = saved.language;
                        self.form.feedback = "Settings saved".into();
                        self.sample_metrics();
                        #[cfg(windows)]
                        if let Some(tray) = self.tray.as_mut() {
                            tray.set_language(saved.language.resolve());
                        }
                    }
                    Err(error) => self.form.feedback = format!("Could not save: {error}"),
                }
            }
            Message::Gpu(v) => self.form.draft.gpu_preference = v,
            Message::Audio(v) => {
                if self.form.audio_pending
                    || self.form.pending.is_some()
                    || v.id == self.form.saved.audio_device
                {
                    return Task::none();
                }
                self.form.draft.audio_device = v.id.clone();
                self.form.audio_pending = true;
                self.form.feedback = "Saving audio output preference…".into();
                let (reply, receive) = futures::channel::oneshot::channel();
                if let Err(e) = self
                    .client
                    .commands
                    .try_send(runtime::Command::AudioOutput(v.id, reply))
                {
                    return Task::done(Message::AudioSaved(Err(e.to_string())));
                }
                return Task::perform(
                    async move {
                        receive.await.unwrap_or_else(|_| {
                            Err("Receiver stopped before audio preference was saved".into())
                        })
                    },
                    Message::AudioSaved,
                );
            }
            Message::AudioSaved(result) => {
                self.form.audio_pending = false;
                match result {
                    Ok(saved) => {
                        self.form.saved.audio_device = saved.audio_device.clone();
                        self.form.draft.audio_device = saved.audio_device;
                        self.form.feedback =
                            "Audio output saved; active output switches automatically".into();
                    }
                    Err(e) => {
                        self.form
                            .draft
                            .audio_device
                            .clone_from(&self.form.saved.audio_device);
                        self.form.feedback = format!("Could not change audio output: {e}");
                    }
                }
            }
            Message::Hardware(v) => self.form.draft.hardware_decode = v,
            Message::Hevc(v) => self.form.draft.hevc_enabled = v,
            Message::Vsync(v) => self.form.draft.vsync = v,
            Message::Hls(v) => self.form.draft.hls_enabled = v,
            Message::MinimizeTray(v) => self.form.draft.minimize_to_tray = v,
            Message::CloseTray(v) => self.form.draft.close_to_tray = v,
            Message::Autostart(v) => self.form.draft.autostart = v,
            Message::StartHidden(v) => self.form.draft.start_hidden = v,
            Message::Volume(v) => {
                self.client.shared.ui.lock().unwrap().volume_db = v;
                self.status.volume_db = v;
                self.last_audible_db = v;
                self.controls_used = Instant::now();
            }
            Message::Mute => {
                let db = if self.status.volume_db <= -100. {
                    self.last_audible_db
                } else {
                    self.last_audible_db = self.status.volume_db;
                    -144.
                };
                self.client.shared.ui.lock().unwrap().volume_db = db;
                self.status.volume_db = db;
                self.controls_used = Instant::now();
            }
            Message::Capture(id) => {
                if self.window == Some(id) {
                    return window::screenshot(id).map(Message::Screenshot);
                }
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
    fn sample_metrics(&mut self) {
        let shared = &self.client.shared;
        let m = &shared.metrics;
        let seconds = self.sampled_at.elapsed().as_secs_f64().max(0.001);
        let submitted = m.presented.load(Ordering::Relaxed);
        let decoded = m.decoded.load(Ordering::Relaxed);
        let bytes = m.bytes.load(Ordering::Relaxed);
        if self.window.is_some()
            && !self.minimized
            && self.page == 0
            && !self.fullscreen
            && !self.focus
        {
            let report = shared.snapshot();
            let ms = |key: &str| {
                report[key]
                    .as_u64()
                    .map(|v| format!("{:.1}", v as f64 / 1000.))
                    .unwrap_or_else(|| "—".into())
            };
            let av = report["estimated_av_offset_us"]
                .as_i64()
                .map(|v| format!("{:+.1} ms (estimate)", v as f64 / 1000.))
                .unwrap_or_else(|| "unavailable".into());
            let gpu = render::compositor::ADAPTER_NAME.lock().unwrap().clone();
            self.stats = view::Stats {
                fps: submitted.saturating_sub(self.last_metrics) as f64 / seconds,
                processing_ms: (submitted != self.last_metrics)
                    .then(|| m.latency_us.load(Ordering::Relaxed) as f64 / 1000.),
                mbps: bytes.saturating_sub(self.last_bytes) as f64 * 8. / seconds / 1_000_000.,
                dropped: m
                    .replaced
                    .load(Ordering::Relaxed)
                    .saturating_sub(m.hidden_replaced.load(Ordering::Relaxed))
                    + m.schedule_dropped.load(Ordering::Relaxed),
                video: format!(
                    "Source: {}\nCodec: {}\nResolution: {}\nDecoder: {}\nRender GPU: {}\nColour: {}\nDecoded: {} ({:.1} fps)",
                    self.status.peer,
                    self.status.codec,
                    self.status.dimensions,
                    self.status.decoder,
                    gpu,
                    m.video_colour_label(),
                    decoded,
                    decoded.saturating_sub(self.last_decoded) as f64 / seconds
                ),
                timing: format!(
                    "Submitted frames: {submitted} ({:.1} fps)\nProcessing P95 / P99: {} / {} ms\nFrame interval P95 / P99: {} / {} ms\nAV offset: {}\nPending video frames: {}\nReplaced / late / stale: {} / {} / {}\nHidden replacements: {}\nTotal data: {:.1} MiB",
                    submitted.saturating_sub(self.last_metrics) as f64 / seconds,
                    ms("p95_receive_to_present_us"),
                    ms("p99_receive_to_present_us"),
                    ms("p95_new_submission_interval_us"),
                    ms("p99_new_submission_interval_us"),
                    av,
                    report["pending_scheduled_frames"],
                    m.replaced.load(Ordering::Relaxed),
                    m.schedule_dropped.load(Ordering::Relaxed),
                    m.stale_dropped.load(Ordering::Relaxed),
                    m.hidden_replaced.load(Ordering::Relaxed),
                    bytes as f64 / 1_048_576.
                ),
                audio: format!(
                    "{}\nOutput latency: {} ms\nUnderruns: {}\nRecovered packets: {}\nPacket errors: {}",
                    if self.status.audio_status.is_empty() {
                        "Waiting for audio"
                    } else {
                        &self.status.audio_status
                    },
                    ms("audio_output_latency_us"),
                    report["audio_underruns"],
                    m.audio_recovered.load(Ordering::Relaxed),
                    m.audio_errors.load(Ordering::Relaxed)
                ),
                network: format!(
                    "{}\nModel: {}. Stage: {:.0} × {:.0}. HLS: FFmpeg. Version {}.",
                    self.status.addresses,
                    self.status.model,
                    self.viewport.width,
                    self.viewport.height,
                    env!("CARGO_PKG_VERSION")
                ),
            };
        }
        self.last_metrics = submitted;
        self.last_decoded = decoded;
        self.last_bytes = bytes;
        self.sampled_at = Instant::now();
    }
}

// Mixed-DPI window managers may round a logical resize by a physical pixel.
fn size_matches(target: iced::Size, actual: iced::Size) -> bool {
    (target.width - actual.width).abs() <= 1. && (target.height - actual.height).abs() <= 1.
}

#[cfg(test)]
mod tests {
    use super::ConnectionReveal;

    #[test]
    fn hidden_video_reveals_once_and_audio_or_flush_do_not_reopen() {
        let mut reveal = ConnectionReveal::default();
        assert!(!reveal.video(None, false, true));
        assert!(!reveal.video(Some(1), false, true)); // Audio/handshake only.
        assert!(reveal.video(Some(1), true, true));
        assert!(!reveal.video(Some(1), true, true)); // Later frame or FLUSH.
        assert!(reveal.video(Some(2), true, true)); // Same peer, new session.
        assert!(!reveal.video(Some(3), true, false)); // Already showing.
        assert!(!reveal.video(Some(3), true, true)); // Subsequently hidden.
    }

    #[test]
    fn explicit_hide_suppresses_an_active_session_even_before_first_frame() {
        let mut reveal = ConnectionReveal::default();
        reveal.dismiss(None);
        assert!(reveal.video(Some(1), true, true));
        reveal.dismiss(Some(2));
        assert!(!reveal.video(Some(2), true, true));
        assert!(reveal.video(Some(3), true, true));
    }
}
