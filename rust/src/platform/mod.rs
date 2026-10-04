// SPDX-License-Identifier: MPL-2.0
use anyhow::Result;
use std::path::Path;

pub fn attach_parent_console() {
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Console::AttachConsole(
            windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}
pub fn startup_error(message: &str) {
    #[cfg(windows)]
    if !std::env::args_os().any(|s| s == "--headless")
        && !std::env::args_os().any(|s| s == "--exit-after")
    {
        use windows::{
            Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
            core::PCWSTR,
        };
        let text = message.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let title = "AirDock".encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = message;
}

pub fn open(target: &str) -> Result<()> {
    #[cfg(windows)]
    {
        use windows::{
            Win32::{UI::Shell::ShellExecuteW, UI::WindowsAndMessaging::SW_SHOWNORMAL},
            core::PCWSTR,
        };
        let wide: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(wide.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        anyhow::ensure!(result.0 as usize > 32, "Windows could not open {target}");
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(target).spawn()?;
    }
    Ok(())
}
pub fn autostart(enabled: bool) -> Result<()> {
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
        if enabled {
            key.set_value(
                "AirDock",
                &format!("\"{}\" --start-hidden", std::env::current_exe()?.display()),
            )?;
        } else {
            match key.delete_value("AirDock") {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    #[cfg(not(windows))]
    {
        anyhow::ensure!(!enabled, "Autostart is supported on Windows");
    }
    Ok(())
}
pub fn usb_present() -> bool {
    #[cfg(windows)]
    {
        use windows::{Win32::Devices::DeviceAndDriverInstallation::*, core::PCWSTR};
        let info = unsafe {
            SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
        };
        if let Ok(info) = info
            && info.0 != -1
        {
            let mut index = 0;
            let mut found = false;
            loop {
                let mut device = SP_DEVINFO_DATA {
                    cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                    ..Default::default()
                };
                if unsafe { SetupDiEnumDeviceInfo(info, index, &mut device) }.is_err() {
                    break;
                }
                let mut id = [0u16; 1024];
                if unsafe { SetupDiGetDeviceInstanceIdW(info, &device, Some(&mut id), None) }
                    .is_ok()
                {
                    let s = String::from_utf16_lossy(
                        &id[..id.iter().position(|c| *c == 0).unwrap_or(id.len())],
                    );
                    if s.to_ascii_uppercase().contains("USB\\VID_05AC") {
                        found = true;
                        break;
                    }
                }
                index += 1;
            }
            unsafe {
                let _ = SetupDiDestroyDeviceInfoList(info);
            };
            return found;
        }
    }
    false
}
pub fn reveal(path: &Path) -> Result<()> {
    open(&path.to_string_lossy())
}
#[cfg(windows)]
pub mod tray {
    use anyhow::Result;
    use tray_icon::{
        MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };
    #[derive(Clone, Copy, Debug)]
    pub enum Command {
        Show,
        Hide,
        Disconnect,
        Quit,
    }
    pub struct Tray {
        _icon: TrayIcon,
        items: Vec<(MenuItem, Command)>,
        events: Option<futures::channel::mpsc::UnboundedReceiver<Command>>,
    }
    impl Tray {
        pub fn new(language: crate::i18n::Language) -> Result<Self> {
            let menu = Menu::new();
            let mut items = Vec::new();
            for (title, command) in [
                ("Show AirDock", Command::Show),
                ("Hide to tray", Command::Hide),
                ("Disconnect device", Command::Disconnect),
            ] {
                let item = MenuItem::new(language.translate(title).as_ref(), true, None);
                menu.append(&item)?;
                items.push((item, command));
            }
            menu.append(&PredefinedMenuItem::separator())?;
            let quit = MenuItem::new(language.translate("Quit").as_ref(), true, None);
            menu.append(&quit)?;
            items.push((quit, Command::Quit));
            let icon = TrayIconBuilder::new()
                .with_tooltip(language.translate("AirDock — receiver running").as_ref())
                .with_menu(Box::new(menu))
                .with_icon(crate::brand::tray_icon()?)
                .build()?;
            let (send, events) = futures::channel::mpsc::unbounded();
            let ids = items
                .iter()
                .map(|(item, command)| (item.id().clone(), *command))
                .collect::<Vec<_>>();
            let menu_send = send.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if let Some((_, command)) = ids.iter().find(|(id, _)| id == &event.id) {
                    let _ = menu_send.unbounded_send(*command);
                }
            }));
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                ) {
                    let _ = send.unbounded_send(Command::Show);
                }
            }));
            Ok(Self {
                _icon: icon,
                items,
                events: Some(events),
            })
        }
        pub fn set_language(&mut self, language: crate::i18n::Language) {
            for (item, command) in &self.items {
                let label = match command {
                    Command::Show => "Show AirDock",
                    Command::Hide => "Hide to tray",
                    Command::Disconnect => "Disconnect device",
                    Command::Quit => "Quit",
                };
                item.set_text(language.translate(label).as_ref());
            }
            let _ = self._icon.set_tooltip(Some(
                language.translate("AirDock — receiver running").as_ref(),
            ));
        }
        pub fn take_events(&mut self) -> futures::channel::mpsc::UnboundedReceiver<Command> {
            self.events.take().expect("tray subscription starts once")
        }
    }
}

/// FFmpeg D3D11VA accepts a DXGI adapter index. Match the render PCI identity
/// where possible; software transfer remains in use, so this is not GPU interop.
#[cfg(windows)]
pub fn decoder_adapter(vendor: u32, device: u32) -> Option<String> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    // A hidden-start receiver can decode before Iced has created a GPU device.
    // Choose the same policy up front rather than accepting DXGI adapter zero.
    let (vendor, device) = if vendor == 0 {
        startup_adapter()?
    } else {
        (vendor, device)
    };
    unsafe {
        let factory = CreateDXGIFactory1::<IDXGIFactory1>().ok()?;
        for index in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let Ok(description) = adapter.GetDesc1() else {
                continue;
            };
            if description.VendorId == vendor && description.DeviceId == device {
                return Some(index.to_string());
            }
        }
    }
    None
}

static EXIT_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn exit_requested() -> bool {
    EXIT_REQUESTED.load(std::sync::atomic::Ordering::Acquire)
}
pub fn install_exit_handlers() -> Result<()> {
    #[cfg(windows)]
    {
        unsafe extern "system" fn handler(kind: u32) -> windows::core::BOOL {
            use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
            if matches!(kind, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
                EXIT_REQUESTED.store(true, std::sync::atomic::Ordering::Release);
                true.into()
            } else {
                false.into()
            }
        }
        unsafe {
            if windows::Win32::System::Console::GetConsoleCP() != 0 {
                windows::Win32::System::Console::SetConsoleCtrlHandler(Some(handler), true)?;
            }
        }
    }
    #[cfg(unix)]
    {
        extern "C" fn handler(_: libc::c_int) {
            EXIT_REQUESTED.store(true, std::sync::atomic::Ordering::Release);
        }
        // Signal handlers perform only one lock-free atomic store. All cleanup
        // takes place on normal runtime/UI threads, never inside the handler.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            anyhow::ensure!(
                libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) == 0,
                "Could not install SIGINT handler"
            );
            anyhow::ensure!(
                libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()) == 0,
                "Could not install SIGTERM handler"
            );
        }
    }
    Ok(())
}

/// Physical adapter driving the monitor nearest this HWND. Surface support alone
/// also admits hybrid adapters that must copy their output to the display GPU.
#[cfg(windows)]
pub fn window_adapter(window: &impl iced_wgpu::graphics::compositor::Window) -> Option<(u32, u32)> {
    use iced_wgpu::wgpu::rwh::RawWindowHandle;
    use windows::Win32::Foundation::HWND;
    let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    monitor_adapter(HWND(handle.hwnd.get() as *mut _))
}
#[cfg(windows)]
fn monitor_adapter(hwnd: windows::Win32::Foundation::HWND) -> Option<(u32, u32)> {
    use windows::Win32::Graphics::{
        Dxgi::{CreateDXGIFactory1, IDXGIFactory1},
        Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow},
    };
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let factory = CreateDXGIFactory1::<IDXGIFactory1>().ok()?;
        for index in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let Ok(description) = adapter.GetDesc1() else {
                continue;
            };
            for output_index in 0..32 {
                let Ok(output) = adapter.EnumOutputs(output_index) else {
                    break;
                };
                if output
                    .GetDesc()
                    .is_ok_and(|d| d.AttachedToDesktop.as_bool() && d.Monitor == monitor)
                {
                    return Some((description.VendorId, description.DeviceId));
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn startup_adapter() -> Option<(u32, u32)> {
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Dxgi::{
            CreateDXGIFactory1, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
            DXGI_GPU_PREFERENCE_MINIMUM_POWER, IDXGIAdapter1, IDXGIFactory6,
        },
    };
    let preference =
        crate::render::compositor::PREFERENCE.load(std::sync::atomic::Ordering::Relaxed);
    if preference == 0 {
        return monitor_adapter(HWND(std::ptr::null_mut()));
    }
    unsafe {
        let factory = CreateDXGIFactory1::<IDXGIFactory6>().ok()?;
        let adapter = factory
            .EnumAdapterByGpuPreference::<IDXGIAdapter1>(
                0,
                if preference == 2 {
                    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE
                } else {
                    DXGI_GPU_PREFERENCE_MINIMUM_POWER
                },
            )
            .ok()?;
        let description = adapter.GetDesc1().ok()?;
        Some((description.VendorId, description.DeviceId))
    }
}

/// Primary monitor logical work area with a margin for window borders/titlebar.
pub fn desktop_work_area() -> Option<(u32, u32)> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{
            Foundation::RECT,
            UI::{
                HiDpi::GetDpiForSystem,
                WindowsAndMessaging::{
                    SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
                },
            },
        };
        let mut area = RECT::default();
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&mut area as *mut RECT).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
        let scale = GetDpiForSystem().max(96) as f32 / 96.;
        Some((
            ((area.right - area.left) as f32 / scale - 32.).max(1.) as u32,
            ((area.bottom - area.top) as f32 / scale - 64.).max(1.) as u32,
        ))
    }
    #[cfg(not(windows))]
    None
}
