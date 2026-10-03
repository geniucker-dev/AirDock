use crate::{
    config::{self, Settings},
    crypto,
    discovery::Discovery,
    integration,
    media::display::Display,
    server::{Device, Server},
    state::Shared,
};
use anyhow::{Context, Result, ensure};
use clap::Parser;
use fs2::FileExt;
use sdl2::{
    event::{Event, WindowEvent as SdlWindowEvent},
    keyboard::{Keycode, Mod},
    mouse::MouseButton,
    pixels::{Color, PixelFormatEnum},
    rect::Rect,
    video::FullscreenType,
};
use slint::{
    ComponentHandle,
    platform::{
        self, Key, PointerEventButton, WindowEvent,
        software_renderer::{
            LineBufferProvider, MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
        },
    },
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    fs::OpenOptions,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
slint::include_modules!();
unsafe extern "C" {
    fn SDL_RenderSetVSync(renderer: *mut sdl2::sys::SDL_Renderer, vsync: i32) -> i32;
}

// Retain previous UI pixels in the SDL texture, not two full CPU images.
// Slint recomposites dirty spans; stage one scanline and at most 32 upload rows.
struct UiUpload<'a, 'texture> {
    texture: &'a mut sdl2::render::Texture<'texture>,
    row: &'a mut Vec<PremultipliedRgbaColor>,
    stripe: &'a mut Vec<u8>,
    origin: (usize, usize),
    width: usize,
    rows: usize,
    error: Option<String>,
}
impl UiUpload<'_, '_> {
    fn flush(&mut self) {
        if self.rows == 0 {
            return;
        }
        if self.error.is_none()
            && let Err(error) = self.texture.update(
                Rect::new(
                    self.origin.0 as i32,
                    self.origin.1 as i32,
                    self.width as u32,
                    self.rows as u32,
                ),
                self.stripe,
                self.width * 4,
            )
        {
            self.error = Some(error.to_string());
        }
        self.stripe.clear();
        self.rows = 0;
    }
}
impl LineBufferProvider for &mut UiUpload<'_, '_> {
    type TargetPixel = PremultipliedRgbaColor;
    fn process_line(
        &mut self,
        line: usize,
        range: std::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        if self.rows != 0
            && (self.origin.0 != range.start
                || self.width != range.len()
                || line != self.origin.1 + self.rows
                || self.rows == 32)
        {
            self.flush();
        }
        if self.rows == 0 {
            self.origin = (range.start, line);
            self.width = range.len();
        }
        self.row
            .resize(range.len(), PremultipliedRgbaColor::default());
        render_fn(self.row);
        // SDL expects straight alpha. This conversion never touches video pixels.
        for p in self.row.iter() {
            let a = u32::from(p.alpha);
            self.stripe.extend_from_slice(&[
                (u32::from(p.red) * 255)
                    .checked_div(a)
                    .unwrap_or(0)
                    .min(255) as u8,
                (u32::from(p.green) * 255)
                    .checked_div(a)
                    .unwrap_or(0)
                    .min(255) as u8,
                (u32::from(p.blue) * 255)
                    .checked_div(a)
                    .unwrap_or(0)
                    .min(255) as u8,
                p.alpha,
            ]);
        }
        self.rows += 1;
    }
}

#[derive(Parser, Debug)]
#[command(version, about = "AirPlay receiver — native Rust + Slint edition")]
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
}
enum Command {
    Fullscreen,
    HideUi,
    HideTray,
    Quit,
    Save,
    Reset,
    Browse,
    OpenFolder,
    Hotspot,
}
struct SdlPlatform {
    window: Rc<MinimalSoftwareWindow>,
    epoch: Instant,
    clipboard: sdl2::clipboard::ClipboardUtil,
}
impl platform::Platform for SdlPlatform {
    fn create_window_adapter(
        &self,
    ) -> std::result::Result<Rc<dyn platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.epoch.elapsed()
    }
    fn set_clipboard_text(&self, text: &str, clipboard: platform::Clipboard) {
        if clipboard == platform::Clipboard::DefaultClipboard {
            let _ = self.clipboard.set_clipboard_text(text);
        }
    }
    fn clipboard_text(&self, clipboard: platform::Clipboard) -> Option<String> {
        if clipboard == platform::Clipboard::DefaultClipboard {
            self.clipboard.clipboard_text().ok()
        } else {
            None
        }
    }
}

unsafe extern "C" fn borderless_hit_test(
    window: *mut sdl2::sys::SDL_Window,
    area: *const sdl2::sys::SDL_Point,
    _: *mut libc::c_void,
) -> sdl2::sys::SDL_HitTestResult {
    use sdl2::sys::SDL_HitTestResult::*;
    if window.is_null() || area.is_null() {
        return SDL_HITTEST_NORMAL;
    }
    if unsafe { sdl2::sys::SDL_GetWindowFlags(window) }
        & sdl2::sys::SDL_WindowFlags::SDL_WINDOW_FULLSCREEN as u32
        != 0
    {
        return SDL_HITTEST_NORMAL;
    }
    let (mut width, mut height) = (0, 0);
    unsafe {
        sdl2::sys::SDL_GetWindowSize(window, &mut width, &mut height);
    }
    let point = unsafe { *area };
    match (
        point.x < 8,
        point.x >= width - 8,
        point.y < 8,
        point.y >= height - 8,
    ) {
        (true, _, true, _) => SDL_HITTEST_RESIZE_TOPLEFT,
        (_, true, true, _) => SDL_HITTEST_RESIZE_TOPRIGHT,
        (true, _, _, true) => SDL_HITTEST_RESIZE_BOTTOMLEFT,
        (_, true, _, true) => SDL_HITTEST_RESIZE_BOTTOMRIGHT,
        (true, _, _, _) => SDL_HITTEST_RESIZE_LEFT,
        (_, true, _, _) => SDL_HITTEST_RESIZE_RIGHT,
        (_, _, true, _) => SDL_HITTEST_RESIZE_TOP,
        (_, _, _, true) => SDL_HITTEST_RESIZE_BOTTOM,
        _ => SDL_HITTEST_DRAGGABLE,
    }
}
fn set_ui_hidden(window: &mut sdl2::video::Window, ui: &AppWindow, hidden: bool) {
    ui.set_ui_hidden(hidden);
    window.set_bordered(!hidden);
    unsafe {
        sdl2::sys::SDL_SetWindowHitTest(
            window.raw(),
            if hidden {
                Some(borderless_hit_test)
            } else {
                None
            },
            std::ptr::null_mut(),
        );
    }
}

pub fn run() -> Result<()> {
    let args = Arguments::parse();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "airplay_windows=info".into());
    if let Some(path) = &args.log {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    ffmpeg_next::init()?;
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
    let settings_path = directory.join("settings.json");
    let mut settings = Settings::load(&settings_path)?;
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
    let identity = crypto::load_identity(&directory.join("identity.key"))?;
    let shared = Shared::new(settings.clone());
    #[cfg(windows)]
    {
        // SDL 2.24+ keeps window/input coordinates in logical points and
        // renderer output in pixels, including per-monitor DPI changes.
        sdl2::hint::set("SDL_WINDOWS_DPI_AWARENESS", "permonitorv2");
        sdl2::hint::set("SDL_WINDOWS_DPI_SCALING", "1");
    }
    let monitor_shared = shared.clone();
    let monitor = std::thread::Builder::new()
        .name("desktop-devices".into())
        .spawn(move || {
            while monitor_shared.running.load(Ordering::Acquire) {
                let usb = integration::usb_present();
                let addresses = if_addrs::get_if_addrs()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|a| !a.is_loopback())
                    .map(|a| format!("{}: {}", a.name, a.ip()))
                    .collect::<Vec<_>>()
                    .join("  ·  ");
                {
                    let mut state = monitor_shared.ui.lock().unwrap();
                    if usb != state.usb {
                        tracing::info!(
                            "Apple USB device {}",
                            if usb { "connected" } else { "removed" }
                        );
                    }
                    state.usb = usb;
                    state.addresses = addresses;
                }
                for _ in 0..10 {
                    if !monitor_shared.running.load(Ordering::Acquire) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })?;
    let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
    let _audio = sdl.audio().map_err(anyhow::Error::msg)?;
    let server = Server::start(Device::new(identity, args.port, shared.clone())?)?;
    let mut discovery = match Discovery::start(server.device()) {
        Ok(d) => Some(d),
        Err(e) => {
            shared.report(format!("Discovery unavailable: {e:#}"));
            None
        }
    };
    tracing::info!(
        "AirPlay listening on port {} as {}",
        args.port,
        settings.name
    );
    let result = if args.headless {
        headless(&args, &shared, &sdl)
    } else {
        gui(
            &args,
            &shared,
            &sdl,
            &directory,
            &settings_path,
            server.device(),
            &mut discovery,
        )
    };
    shared.running.store(false, Ordering::Release);
    let _ = monitor.join();
    drop(discovery);
    drop(server);
    shared.finish_recordings();
    if let Some(path) = args.metrics {
        std::fs::write(path, serde_json::to_vec_pretty(&shared.snapshot())?)?;
    }
    result
}
fn headless(args: &Arguments, shared: &Arc<Shared>, sdl: &sdl2::Sdl) -> Result<()> {
    let mut pump = sdl.event_pump().map_err(anyhow::Error::msg)?;
    while shared.running.load(Ordering::Acquire) {
        if args
            .exit_after
            .is_some_and(|s| shared.started.elapsed() >= Duration::from_secs(s))
        {
            break;
        }
        if pump.poll_iter().any(|e| matches!(e, Event::Quit { .. })) {
            break;
        }
        shared.frame.lock().unwrap().take();
        shared.wait_for_frame(Duration::from_millis(50));
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)] // The UI owns these process resources on its main thread.
fn gui(
    args: &Arguments,
    shared: &Arc<Shared>,
    sdl: &sdl2::Sdl,
    directory: &std::path::Path,
    settings_path: &std::path::Path,
    device: &Device,
    discovery: &mut Option<Discovery>,
) -> Result<()> {
    let settings = shared.settings.read().unwrap().clone();
    let video = sdl.video().map_err(anyhow::Error::msg)?;
    let mut builder = video.window(
        "AirPlay-Windows",
        settings.window_width,
        settings.window_height,
    );
    builder.position_centered().resizable().allow_highdpi();
    let mut window = builder.build()?;
    window.set_minimum_size(900, 620)?;
    let mut canvas = window.into_canvas().build()?;
    unsafe {
        SDL_RenderSetVSync(canvas.raw(), i32::from(settings.vsync));
    }
    let creator = canvas.texture_creator();
    let mut display = Display::default();
    let adapter = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    platform::set_platform(Box::new(SdlPlatform {
        window: adapter.clone(),
        epoch: Instant::now(),
        clipboard: video.clipboard(),
    }))
    .map_err(|e| anyhow::anyhow!("Slint platform: {e}"))?;
    let ui = AppWindow::new()?;
    load_settings(&ui, &settings);
    set_ui_hidden(canvas.window_mut(), &ui, settings.hide_ui);
    let actions = Rc::new(RefCell::new(VecDeque::new()));
    macro_rules! action {
        ($callback:ident,$command:expr) => {
            let q = actions.clone();
            ui.$callback(move || q.borrow_mut().push_back($command));
        };
    }
    action!(on_fullscreen, Command::Fullscreen);
    action!(on_hide_ui, Command::HideUi);
    action!(on_hide_tray, Command::HideTray);
    action!(on_quit, Command::Quit);
    action!(on_save_settings, Command::Save);
    action!(on_reset_settings, Command::Reset);
    action!(on_browse_folder, Command::Browse);
    action!(on_open_folder, Command::OpenFolder);
    action!(on_open_hotspot, Command::Hotspot);
    {
        let s = shared.clone();
        ui.on_disconnect(move || s.request_disconnect());
    }
    {
        let s = shared.clone();
        ui.on_toggle_recording(move || toggle_recording(&s));
    }
    {
        let s = shared.clone();
        ui.on_clear_error(move || s.ui.lock().unwrap().error.clear());
    }
    #[cfg(windows)]
    let tray = match integration::tray::Tray::new() {
        Ok(t) => Some(t),
        Err(e) => {
            shared.report(format!("System tray unavailable: {e}"));
            None
        }
    };
    #[cfg(windows)]
    let tray_available = tray.is_some();
    #[cfg(not(windows))]
    let tray_available = false;
    ui.set_tray_available(tray_available);
    let mut visible = !(tray_available && (args.start_hidden || settings.start_hidden));
    if !visible {
        canvas.window_mut().hide();
    }
    if settings.fullscreen {
        canvas
            .window_mut()
            .set_fullscreen(FullscreenType::Desktop)
            .map_err(anyhow::Error::msg)?;
    }
    ui.show()?;
    video.text_input().start();
    let mut pump = sdl.event_pump().map_err(anyhow::Error::msg)?;
    let mut dimensions = (0, 0);
    let mut ui_texture = None;
    let mut ui_full_repaint = true;
    let mut ui_row = Vec::<PremultipliedRgbaColor>::new();
    let mut ui_stripe = Vec::<u8>::new();
    let mut scale = 1f32;
    let mut mouse = (0, 0);
    let mut last_status = Instant::now() - Duration::from_secs(1);
    let mut last_rate = Instant::now();
    let mut last_presented = 0;
    let mut last_decoded = 0;
    let mut fps = 0.;
    let mut decoded_fps = 0.;
    let mut last_cover = Arc::new(Vec::new());
    let mut video_active = false;
    let mut redraw = true;
    let mut screenshot_written = false;
    let _ = std::fs::remove_file(directory.join("restore-window"));
    while shared.running.load(Ordering::Acquire) {
        if args
            .exit_after
            .is_some_and(|s| shared.started.elapsed() >= Duration::from_secs(s))
        {
            break;
        }
        for event in pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::Window {
                    win_event: SdlWindowEvent::Close,
                    ..
                } => {
                    if tray_available && shared.settings.read().unwrap().close_to_tray {
                        actions.borrow_mut().push_back(Command::HideTray)
                    } else {
                        actions.borrow_mut().push_back(Command::Quit)
                    }
                }
                Event::Window {
                    win_event: SdlWindowEvent::Minimized,
                    ..
                } => {
                    if tray_available && shared.settings.read().unwrap().minimize_to_tray {
                        actions.borrow_mut().push_back(Command::HideTray)
                    }
                }
                Event::Window {
                    win_event:
                        SdlWindowEvent::Shown | SdlWindowEvent::Restored | SdlWindowEvent::Exposed,
                    ..
                } => {
                    redraw = true;
                }
                Event::Window {
                    win_event: SdlWindowEvent::Hidden,
                    ..
                } => redraw = true,
                Event::Window {
                    win_event: SdlWindowEvent::FocusGained,
                    ..
                } => adapter.dispatch_event(WindowEvent::WindowActiveChanged(true)),
                Event::Window {
                    win_event: SdlWindowEvent::FocusLost,
                    ..
                } => adapter.dispatch_event(WindowEvent::WindowActiveChanged(false)),
                Event::MouseMotion { x, y, .. } => {
                    mouse = (x, y);
                    adapter.dispatch_event(WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(x as f32, y as f32),
                    });
                }
                Event::MouseButtonDown {
                    x, y, mouse_btn, ..
                }
                | Event::MouseButtonUp {
                    x, y, mouse_btn, ..
                } => {
                    let position = slint::LogicalPosition::new(x as f32, y as f32);
                    let button = pointer_button(mouse_btn);
                    if matches!(event, Event::MouseButtonDown { .. }) {
                        adapter.dispatch_event(WindowEvent::PointerPressed { position, button })
                    } else {
                        adapter.dispatch_event(WindowEvent::PointerReleased { position, button })
                    }
                }
                Event::MouseWheel {
                    precise_x,
                    precise_y,
                    ..
                } => adapter.dispatch_event(WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(mouse.0 as f32, mouse.1 as f32),
                    delta_x: precise_x * 32.,
                    delta_y: precise_y * 32.,
                }),
                Event::TextInput { text, .. } => {
                    for c in text.chars() {
                        adapter.dispatch_event(WindowEvent::KeyPressed {
                            text: c.to_string().into(),
                        });
                        adapter.dispatch_event(WindowEvent::KeyReleased {
                            text: c.to_string().into(),
                        });
                    }
                }
                Event::KeyDown {
                    keycode: Some(key),
                    keymod,
                    repeat,
                    ..
                } => {
                    let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD);
                    let command = match key {
                        Keycode::F11 => Some(Command::Fullscreen),
                        Keycode::H if ctrl => Some(Command::HideUi),
                        Keycode::R if ctrl => {
                            if !repeat {
                                toggle_recording(shared)
                            }
                            None
                        }
                        Keycode::D if ctrl => {
                            if !repeat {
                                shared.request_disconnect()
                            }
                            None
                        }
                        Keycode::Escape => {
                            if canvas.window().fullscreen_state() != FullscreenType::Off {
                                Some(Command::Fullscreen)
                            } else {
                                set_ui_hidden(canvas.window_mut(), &ui, false);
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(command) = command {
                        if !repeat {
                            actions.borrow_mut().push_back(command)
                        }
                    } else if let Some(text) = key_text(key, ctrl) {
                        adapter.dispatch_event(if repeat {
                            WindowEvent::KeyPressRepeated { text }
                        } else {
                            WindowEvent::KeyPressed { text }
                        })
                    }
                }
                Event::KeyUp {
                    keycode: Some(key),
                    keymod,
                    ..
                } => {
                    if let Some(text) =
                        key_text(key, keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD))
                    {
                        adapter.dispatch_event(WindowEvent::KeyReleased { text })
                    }
                }
                _ => {}
            }
        }
        #[cfg(windows)]
        if let Some(tray) = &tray {
            for command in tray.commands() {
                use integration::tray::Command as T;
                match command {
                    T::Show => {
                        canvas.window_mut().show();
                        canvas.window_mut().raise();
                        redraw = true;
                    }
                    T::Hide => actions.borrow_mut().push_back(Command::HideTray),
                    T::Record => toggle_recording(shared),
                    T::Disconnect => shared.request_disconnect(),
                    T::Quit => actions.borrow_mut().push_back(Command::Quit),
                }
            }
        }
        while let Some(command) = actions.borrow_mut().pop_front() {
            let result: Result<()> = (|| {
                match command {
                    Command::Fullscreen => {
                        let mode = if canvas.window().fullscreen_state() == FullscreenType::Off {
                            FullscreenType::Desktop
                        } else {
                            FullscreenType::Off
                        };
                        canvas
                            .window_mut()
                            .set_fullscreen(mode)
                            .map_err(anyhow::Error::msg)?;
                    }
                    Command::HideUi => set_ui_hidden(canvas.window_mut(), &ui, !ui.get_ui_hidden()),
                    Command::HideTray => {
                        if tray_available {
                            canvas.window_mut().hide();
                        }
                    }
                    Command::Quit => shared.running.store(false, Ordering::Release),
                    Command::Save => {
                        let previous = shared.settings.read().unwrap().clone();
                        let mut next = read_settings(&ui, previous.clone())?;
                        next.hide_ui = ui.get_ui_hidden();
                        next.fullscreen = canvas.window().fullscreen_state() != FullscreenType::Off;
                        if next.autostart != previous.autostart {
                            integration::autostart(next.autostart)?;
                        }
                        next.save(settings_path)?;
                        *shared.settings.write().unwrap() = next.clone();
                        unsafe {
                            SDL_RenderSetVSync(canvas.raw(), i32::from(next.vsync));
                        }
                        if next.name != previous.name
                            || next.hevc_enabled != previous.hevc_enabled
                            || next.hls_enabled != previous.hls_enabled
                        {
                            *discovery = None;
                            *discovery = Some(Discovery::start(device)?);
                        }
                        ui.set_status("Settings saved".into());
                    }
                    Command::Reset => load_settings(&ui, &Settings::default()),
                    Command::Browse => {
                        if let Some(folder) = integration::choose_folder()? {
                            ui.set_recording_directory(folder.to_string_lossy().as_ref().into());
                        }
                    }
                    Command::OpenFolder => {
                        integration::reveal(&PathBuf::from(ui.get_recording_directory().as_str()))?
                    }
                    Command::Hotspot => integration::open("ms-settings:network-mobilehotspot")?,
                }
                Ok(())
            })();
            if let Err(e) = result {
                shared.report(format!("{e:#}"));
            }
            redraw = true;
        }
        let flags = canvas.window().window_flags();
        visible = flags & sdl2::sys::SDL_WindowFlags::SDL_WINDOW_SHOWN as u32 != 0
            && flags & sdl2::sys::SDL_WindowFlags::SDL_WINDOW_MINIMIZED as u32 == 0;
        platform::update_timers_and_animations();
        let new_dimensions = canvas.output_size().map_err(anyhow::Error::msg)?;
        let logical = canvas.window().size();
        let new_scale = new_dimensions.0 as f32 / logical.0.max(1) as f32;
        if dimensions != new_dimensions || (scale - new_scale).abs() > f32::EPSILON {
            dimensions = new_dimensions;
            scale = new_scale;
            adapter.dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
            adapter.set_size(slint::PhysicalSize::new(dimensions.0, dimensions.1));
            adapter.dispatch_event(WindowEvent::Resized {
                size: slint::LogicalSize::new(logical.0 as f32, logical.1 as f32),
            });
            ui_texture = None;
            redraw = true;
        }
        if !visible || ui.get_ui_hidden() {
            ui_texture = None;
        } else if ui_texture.is_none() {
            let mut texture = creator.create_texture_streaming(
                PixelFormatEnum::RGBA32,
                dimensions.0,
                dimensions.1,
            )?;
            texture.set_blend_mode(sdl2::render::BlendMode::Blend);
            ui_texture = Some(texture);
            ui_full_repaint = true;
            adapter.request_redraw();
            redraw = true;
        }
        if last_status.elapsed() >= Duration::from_millis(200) {
            let state = shared.ui.lock().unwrap().clone();
            let connected = !state.peer.is_empty();
            ui.set_connected(connected);
            ui.set_device_name(
                if connected {
                    if state.device.is_empty() {
                        state.peer.clone()
                    } else {
                        state.device.clone()
                    }
                } else {
                    "No device connected".into()
                }
                .into(),
            );
            ui.set_peer(state.peer.into());
            ui.set_codec(state.codec.into());
            ui.set_dimensions(state.dimensions.into());
            ui.set_decoder(state.decoder.into());
            ui.set_paused(state.paused);
            ui.set_error(state.error.into());
            ui.set_recording(shared.recording.load(Ordering::Relaxed));
            ui.set_recording_path(state.recording_path.into());
            let progress = (state.progress_seconds
                + if state.paused {
                    0.
                } else {
                    state.progress_at.map_or(0., |t| t.elapsed().as_secs_f64())
                })
            .min(state.duration_seconds);
            ui.set_progress_ratio(if state.duration_seconds > 0. {
                (progress / state.duration_seconds) as f32
            } else {
                0.
            });
            let elapsed = progress as u64;
            let total = state.duration_seconds as u64;
            ui.set_progress_text(if total > 0 {
                format!(
                    "{}:{:02} / {}:{:02}",
                    elapsed / 60,
                    elapsed % 60,
                    total / 60,
                    total % 60
                )
                .into()
            } else {
                "".into()
            });
            ui.set_track_title(if state.title.is_empty() {
                if state.kind == "Screen mirroring" {
                    "Waiting for video…".into()
                } else {
                    "Audio streaming".into()
                }
            } else {
                state.title.into()
            });
            ui.set_track_artist(state.artist.into());
            ui.set_track_album(state.album.into());
            ui.set_status(
                if connected {
                    "Connected · ".to_owned() + &state.kind
                } else {
                    "Ready to connect".into()
                }
                .into(),
            );
            if !Arc::ptr_eq(&state.cover, &last_cover) {
                ui.set_cover(cover_image(&state.cover).unwrap_or_default());
                last_cover = state.cover;
            }
            if !connected {
                display.clear();
                video_active = false;
                ui.set_video_active(false);
            }
            if last_rate.elapsed() >= Duration::from_secs(1) {
                let p = shared.metrics.presented.load(Ordering::Relaxed);
                let d = shared.metrics.decoded.load(Ordering::Relaxed);
                let t = last_rate.elapsed().as_secs_f64();
                fps = (p - last_presented) as f64 / t;
                decoded_fps = (d - last_decoded) as f64 / t;
                last_presented = p;
                last_decoded = d;
                last_rate = Instant::now();
                let devices = shared.ui.lock().unwrap();
                ui.set_addresses(devices.addresses.as_str().into());
                ui.set_usb_status(
                    if devices.usb {
                        "Apple USB device detected · Trust this computer / enable Personal Hotspot"
                    } else {
                        "No USB device detected"
                    }
                    .into(),
                );
            }
            ui.set_stats(format!("{fps:.1} FPS presented · {decoded_fps:.1} decoded · {:.1} ms · {} capture drops",shared.metrics.latency_us.load(Ordering::Relaxed) as f64/1000.,shared.metrics.recording_dropped.load(Ordering::Relaxed)).into());
            last_status = Instant::now();
            if directory.join("restore-window").exists() {
                let _ = std::fs::remove_file(directory.join("restore-window"));
                canvas.window_mut().show();
                canvas.window_mut().raise();
                redraw = true;
            }
        }
        let frame = if visible {
            shared.frame.lock().unwrap().take()
        } else {
            None
        };
        let mut presented_origin = None;
        if let Some(frame) = frame
            && visible
        {
            display.upload(&creator, &frame.frame)?;
            video_active = true;
            ui.set_video_active(true);
            redraw = true;
            presented_origin = Some(frame.received);
        }
        let mut upload_error = None;
        let dirty = if visible && !ui.get_ui_hidden() {
            adapter.draw_if_needed(|renderer| {
                if ui_full_repaint {
                    renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
                }
                let mut upload = UiUpload {
                    texture: ui_texture.as_mut().unwrap(),
                    row: &mut ui_row,
                    stripe: &mut ui_stripe,
                    origin: (0, 0),
                    width: 0,
                    rows: 0,
                    error: None,
                };
                renderer.render_by_line(&mut upload);
                renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
                ui_full_repaint = false;
                upload.flush();
                upload_error = upload.error;
            })
        } else {
            false
        };
        if let Some(e) = upload_error {
            return Err(anyhow::anyhow!(e));
        }
        if !screenshot_written
            && args.screenshot.is_some()
            && shared.started.elapsed() > Duration::from_secs(1)
        {
            redraw = true;
        }
        if visible && (redraw || dirty) {
            canvas.set_draw_color(Color::RGB(10, 13, 19));
            canvas.clear();
            let viewport = Rect::new(
                (ui.get_viewport_x() * scale) as i32,
                (ui.get_viewport_y() * scale) as i32,
                (ui.get_viewport_width() * scale).max(1.) as u32,
                (ui.get_viewport_height() * scale).max(1.) as u32,
            );
            if video_active && (ui.get_page() == 0 || ui.get_ui_hidden()) {
                display.draw(&mut canvas, viewport)?;
            }
            if let Some(texture) = ui_texture.as_ref() {
                if video_active && ui.get_page() == 0 {
                    // The active receiver page has UI only around the video.
                    // Avoid blending a full-window transparent image every frame.
                    let side = (230. * scale).ceil() as u32;
                    let header = (76. * scale).ceil() as u32;
                    let footer = (52. * scale).ceil() as u32;
                    for rect in [
                        Rect::new(0, 0, side, dimensions.1),
                        Rect::new(side as i32, 0, dimensions.0 - side, header),
                        Rect::new(
                            side as i32,
                            (dimensions.1 - footer) as i32,
                            dimensions.0 - side,
                            footer,
                        ),
                    ] {
                        canvas
                            .copy(texture, rect, rect)
                            .map_err(anyhow::Error::msg)?;
                    }
                    if !ui.get_error().is_empty() {
                        let rect = Rect::new(
                            (246. * scale) as i32,
                            (84. * scale) as i32,
                            ((logical.0 as f32 - 262.) * scale).max(1.) as u32,
                            (64. * scale).ceil() as u32,
                        );
                        canvas
                            .copy(texture, rect, rect)
                            .map_err(anyhow::Error::msg)?;
                    }
                } else {
                    canvas
                        .copy(texture, None, None)
                        .map_err(anyhow::Error::msg)?;
                }
            }
            if !screenshot_written
                && shared.started.elapsed() > Duration::from_secs(1)
                && let Some(path) = &args.screenshot
            {
                let buffer = canvas
                    .read_pixels(None, PixelFormatEnum::RGBA32)
                    .map_err(anyhow::Error::msg)?;
                image::save_buffer(
                    path,
                    &buffer,
                    dimensions.0,
                    dimensions.1,
                    image::ColorType::Rgba8,
                )?;
                screenshot_written = true;
            }
            canvas.present();
            if let Some(origin) = presented_origin
                && (ui.get_page() == 0 || ui.get_ui_hidden())
            {
                shared.metrics.presented.fetch_add(1, Ordering::Relaxed);
                let latency = origin.elapsed().as_micros() as u64;
                shared.metrics.latency_us.store(latency, Ordering::Relaxed);
                let mut samples = shared.metrics.latency_samples.lock().unwrap();
                if samples.len() == 4096 {
                    samples.pop_front();
                }
                samples.push_back(latency);
            }
            redraw = false;
        }
        if !visible {
            shared.wait_for_activity(Duration::from_millis(20));
        } else if !redraw {
            shared.wait_for_frame(Duration::from_millis(if visible && video_active {
                8
            } else {
                20
            }));
        }
    }
    let mut settings = shared.settings.read().unwrap().clone();
    let (w, h) = canvas.window().size();
    if canvas.window().fullscreen_state() == FullscreenType::Off {
        settings.window_width = w;
        settings.window_height = h;
    }
    settings.fullscreen = canvas.window().fullscreen_state() != FullscreenType::Off;
    settings.hide_ui = ui.get_ui_hidden();
    settings.save(settings_path)?;
    ui.hide()?;
    Ok(())
}
fn toggle_recording(shared: &Shared) {
    if shared.recording.load(Ordering::Relaxed) {
        shared.stop_recording()
    } else {
        // An encoder may have failed asynchronously; retire its old worker
        // before retrying so the next frame creates a fresh recorder.
        shared.stop_recording();
        shared.recording.store(true, Ordering::Relaxed)
    }
}
fn pointer_button(b: MouseButton) -> PointerEventButton {
    match b {
        MouseButton::Left => PointerEventButton::Left,
        MouseButton::Right => PointerEventButton::Right,
        MouseButton::Middle => PointerEventButton::Middle,
        _ => PointerEventButton::Other,
    }
}
fn key_text(key: Keycode, ctrl: bool) -> Option<slint::SharedString> {
    let special = match key {
        Keycode::Backspace => Key::Backspace,
        Keycode::Delete => Key::Delete,
        Keycode::Return | Keycode::KpEnter => Key::Return,
        Keycode::Tab => Key::Tab,
        Keycode::Left => Key::LeftArrow,
        Keycode::Right => Key::RightArrow,
        Keycode::Up => Key::UpArrow,
        Keycode::Down => Key::DownArrow,
        Keycode::Home => Key::Home,
        Keycode::End => Key::End,
        Keycode::LShift | Keycode::RShift => Key::Shift,
        Keycode::LCtrl | Keycode::RCtrl => Key::Control,
        Keycode::LAlt | Keycode::RAlt => Key::Alt,
        Keycode::Escape => Key::Escape,
        _ => {
            return if ctrl {
                let name = key.name().to_lowercase();
                (name.len() == 1).then(|| name.into())
            } else {
                None
            };
        }
    };
    Some(special.into())
}
fn load_settings(ui: &AppWindow, s: &Settings) {
    ui.set_receiver_name(s.name.as_str().into());
    ui.set_resolution_width(s.mirror_width.to_string().into());
    ui.set_resolution_height(s.mirror_height.to_string().into());
    ui.set_fps(s.max_fps.to_string().into());
    ui.set_refresh(s.refresh_rate.to_string().into());
    ui.set_hevc(s.hevc_enabled);
    ui.set_hardware(s.hardware_decode);
    ui.set_vsync(s.vsync);
    ui.set_close_tray(s.close_to_tray);
    ui.set_minimize_tray(s.minimize_to_tray);
    ui.set_start_hidden(s.start_hidden);
    ui.set_autostart(s.autostart);
    ui.set_hls(s.hls_enabled);
    ui.set_recording_directory(s.recording_directory.to_string_lossy().as_ref().into());
    ui.set_recording_codec(i32::from(s.recording_codec == "hevc"));
    ui.set_recording_encoder(match s.recording_encoder.as_str() {
        "gpu" => 1,
        "cpu" => 2,
        _ => 0,
    });
    ui.set_bitrate(s.recording_mbps.to_string().into());
}
fn read_settings(ui: &AppWindow, mut s: Settings) -> Result<Settings> {
    s.name = ui.get_receiver_name().trim().into();
    s.mirror_width = ui.get_resolution_width().parse()?;
    s.mirror_height = ui.get_resolution_height().parse()?;
    s.max_fps = ui.get_fps().parse()?;
    s.refresh_rate = ui.get_refresh().parse()?;
    s.hevc_enabled = ui.get_hevc();
    s.hardware_decode = ui.get_hardware();
    s.vsync = ui.get_vsync();
    s.close_to_tray = ui.get_close_tray();
    s.minimize_to_tray = ui.get_minimize_tray();
    s.start_hidden = ui.get_start_hidden();
    s.autostart = ui.get_autostart();
    s.hls_enabled = ui.get_hls();
    s.recording_directory = PathBuf::from(ui.get_recording_directory().as_str());
    s.recording_codec = if ui.get_recording_codec() == 1 {
        "hevc"
    } else {
        "h264"
    }
    .into();
    s.recording_encoder = match ui.get_recording_encoder() {
        1 => "gpu",
        2 => "cpu",
        _ => "auto",
    }
    .into();
    s.recording_mbps = ui.get_bitrate().parse()?;
    s.validate()?;
    Ok(s)
}
fn cover_image(bytes: &[u8]) -> Result<slint::Image> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let decoder = reader.into_decoder()?;
    use image::ImageDecoder;
    let (w, h) = decoder.dimensions();
    ensure!(w <= 4096 && h <= 4096, "Cover image too large");
    let rgba = image::DynamicImage::from_decoder(decoder)?.to_rgba8();
    let buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba.as_raw(), w, h);
    Ok(slint::Image::from_rgba8(buffer))
}
