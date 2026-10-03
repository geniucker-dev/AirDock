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
        let title = "AirPlay-Windows"
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
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
                "AirPlay-Windows",
                &format!("\"{}\" --start-hidden", std::env::current_exe()?.display()),
            )?;
        } else {
            match key.delete_value("AirPlay-Windows") {
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
pub fn choose_folder() -> Result<Option<std::path::PathBuf>> {
    #[cfg(windows)]
    {
        use windows::Win32::{
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize,
            },
            UI::Shell::{
                FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
                SIGDN_FILESYSPATH,
            },
        };
        struct Com(bool);
        impl Drop for Com {
            fn drop(&mut self) {
                if self.0 {
                    unsafe { CoUninitialize() }
                }
            }
        }
        let _com = Com(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok());
        let dialog: IFileOpenDialog =
            unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }?;
        unsafe {
            dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
        }
        if unsafe { dialog.Show(None) }.is_err() {
            return Ok(None);
        }
        let display = unsafe { dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH) }?;
        let path = unsafe { display.to_string() }?;
        unsafe { CoTaskMemFree(Some(display.as_ptr().cast())) };
        Ok(Some(path.into()))
    }
    #[cfg(not(windows))]
    {
        let output = std::process::Command::new("zenity")
            .args([
                "--file-selection",
                "--directory",
                "--title=Recording directory",
            ])
            .output()?;
        if output.status.success() {
            Ok(Some(String::from_utf8(output.stdout)?.trim().into()))
        } else {
            Ok(None)
        }
    }
}

#[cfg(windows)]
pub mod tray {
    use anyhow::Result;
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };
    #[derive(Clone, Copy)]
    pub enum Command {
        Show,
        Hide,
        Record,
        Disconnect,
        Quit,
    }
    pub struct Tray {
        _icon: TrayIcon,
        items: Vec<(MenuItem, Command)>,
    }
    impl Tray {
        pub fn new() -> Result<Self> {
            let menu = Menu::new();
            let mut items = Vec::new();
            for (title, command) in [
                ("Show AirPlay-Windows", Command::Show),
                ("Hide to tray", Command::Hide),
                ("Start / stop recording", Command::Record),
                ("Disconnect device", Command::Disconnect),
            ] {
                let item = MenuItem::new(title, true, None);
                menu.append(&item)?;
                items.push((item, command));
            }
            menu.append(&PredefinedMenuItem::separator())?;
            let quit = MenuItem::new("Quit", true, None);
            menu.append(&quit)?;
            items.push((quit, Command::Quit));
            let mut pixels = vec![0u8; 32 * 32 * 4];
            for y in 0..32 {
                for x in 0..32 {
                    let i = (y * 32 + x) * 4;
                    let screen = (5..27).contains(&x)
                        && (5..23).contains(&y)
                        && (!(8..24).contains(&x) || !(8..20).contains(&y));
                    let triangle =
                        (19..=28).contains(&y) && x >= 16 - (y - 19) && x <= 16 + (y - 19);
                    if screen || triangle {
                        pixels[i..i + 4].copy_from_slice(&[117, 145, 255, 255]);
                    }
                }
            }
            let icon = TrayIconBuilder::new()
                .with_tooltip("AirPlay-Windows — receiver running")
                .with_menu(Box::new(menu))
                .with_icon(Icon::from_rgba(pixels, 32, 32)?)
                .build()?;
            Ok(Self { _icon: icon, items })
        }
        pub fn commands(&self) -> Vec<Command> {
            let mut result = Vec::new();
            for event in MenuEvent::receiver().try_iter() {
                if let Some((_, c)) = self.items.iter().find(|(i, _)| i.id() == &event.id) {
                    result.push(*c)
                }
            }
            for event in TrayIconEvent::receiver().try_iter() {
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
                    result.push(Command::Show)
                }
            }
            result
        }
    }
}
