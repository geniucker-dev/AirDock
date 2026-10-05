// SPDX-License-Identifier: MPL-2.0
use crate::{
    config::{Settings, WindowPreferences},
    crypto,
    discovery::Discovery,
    platform,
    server::{Device, Server},
    state::Shared,
};
use anyhow::Result;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};
pub enum Command {
    Disconnect,
    Settings(
        Settings,
        futures::channel::oneshot::Sender<Result<Settings, String>>,
    ),
    Language(
        crate::i18n::Language,
        futures::channel::oneshot::Sender<Result<Settings, String>>,
    ),
    AudioOutput(
        String,
        futures::channel::oneshot::Sender<Result<Settings, String>>,
    ),
    WindowPreferences(
        WindowPreferences,
        futures::channel::oneshot::Sender<Result<Settings, String>>,
    ),
    Quit,
}
#[derive(Clone)]
pub struct Client {
    pub shared: Arc<Shared>,
    pub commands: SyncSender<Command>,
}
pub struct Runtime {
    pub client: Client,
    worker: Option<thread::JoinHandle<()>>,
}
impl Runtime {
    pub fn start(directory: PathBuf, port: u16, settings: Settings) -> Result<Self> {
        let identity = crypto::load_identity(&directory.join("identity.key"))?;
        let shared = Shared::new(settings);
        let server = Server::start(Device::new(identity, port, shared.clone())?)?;
        let state = shared.clone();
        let (commands, receiver) = mpsc::sync_channel(16);
        let worker = thread::Builder::new()
            .name("receiver-runtime".into())
            .spawn(move || {
                // Windows execution state belongs to this thread, never to the window.
                let mut inhibitor: platform::power::Inhibitor = Default::default();
                let mut last_power_error = None;
                let mut discovery = match Discovery::start(server.device()) {
                    Ok(d) => Some(d),
                    Err(e) => {
                        state.report(format!("Discovery unavailable: {e:#}"));
                        None
                    }
                };
                let mut last_devices = std::time::Instant::now() - Duration::from_secs(5);
                while state.running.load(Ordering::Acquire) && !platform::exit_requested() {
                    match receiver.recv_timeout(Duration::from_secs(1)) {
                        Ok(Command::Disconnect) => state.request_disconnect(),
                        Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(Command::Settings(mut settings, reply)) => {
                            let result = (|| -> Result<()> {
                                let current = state.settings.read().unwrap().clone();
                                // Draft saves cannot overwrite independently committed audio/geometry.
                                WindowPreferences::from_settings(&current).apply(&mut settings);
                                settings.audio_device.clone_from(&current.audio_device);
                                settings.language = current.language;
                                settings.validate()?;
                                let previous_autostart = current.autostart;
                                if settings.autostart != previous_autostart {
                                    platform::autostart(settings.autostart)?;
                                }
                                if let Err(error) = settings.save(&directory.join("settings.json"))
                                {
                                    if settings.autostart != previous_autostart {
                                        let _ = platform::autostart(previous_autostart);
                                    }
                                    return Err(error);
                                }
                                *state.settings.write().unwrap() = settings;
                                drop(discovery.take());
                                match Discovery::start(server.device()) {
                                    Ok(d) => discovery = Some(d),
                                    Err(e) => state.report(format!(
                                        "Settings saved; discovery unavailable: {e:#}"
                                    )),
                                }
                                Ok(())
                            })();
                            let _ = reply.send(
                                result
                                    .map(|_| state.settings.read().unwrap().clone())
                                    .map_err(|e| format!("{e:#}")),
                            );
                        }
                        Ok(Command::Language(language, reply)) => {
                            let mut settings = state.settings.read().unwrap().clone();
                            settings.language = language;
                            let result = settings.save(&directory.join("settings.json"));
                            if result.is_ok() {
                                *state.settings.write().unwrap() = settings;
                            }
                            let _ = reply.send(
                                result
                                    .map(|_| state.settings.read().unwrap().clone())
                                    .map_err(|e| format!("{e:#}")),
                            );
                        }
                        Ok(Command::AudioOutput(device, reply)) => {
                            let mut settings = state.settings.read().unwrap().clone();
                            settings.audio_device = device;
                            let result = settings.save(&directory.join("settings.json"));
                            if result.is_ok() {
                                *state.settings.write().unwrap() = settings;
                            }
                            let _ = reply.send(
                                result
                                    .map(|_| state.settings.read().unwrap().clone())
                                    .map_err(|e| format!("{e:#}")),
                            );
                        }
                        Ok(Command::WindowPreferences(preferences, reply)) => {
                            let mut settings = state.settings.read().unwrap().clone();
                            preferences.apply(&mut settings);
                            let result = if settings == *state.settings.read().unwrap() {
                                Ok(())
                            } else {
                                settings.save(&directory.join("settings.json"))
                            };
                            if result.is_ok() {
                                *state.settings.write().unwrap() = settings;
                            }
                            let _ = reply.send(
                                result
                                    .map(|_| state.settings.read().unwrap().clone())
                                    .map_err(|e| format!("{e:#}")),
                            );
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let inhibit = state
                        .sessions
                        .needs_video_inhibition(|| server.device().hls.is_playing());
                    match inhibitor.update(inhibit) {
                        Ok(()) => last_power_error = None,
                        Err(error) => {
                            if last_power_error.is_none_or(|at: std::time::Instant| {
                                at.elapsed() >= Duration::from_secs(30)
                            }) {
                                state
                                    .report(format!("Windows playback idle protection: {error:#}"));
                                last_power_error = Some(std::time::Instant::now());
                            }
                        }
                    }
                    state
                        .sessions
                        .idle_inhibited
                        .store(inhibitor.active(), Ordering::Release);
                    if last_devices.elapsed() >= Duration::from_secs(2) {
                        let addresses = if_addrs::get_if_addrs()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|a| !a.is_loopback())
                            .map(|a| format!("{}: {}", a.name, a.ip()))
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let mut status = state.ui.lock().unwrap();
                        let usb = platform::usb_present();
                        if status.usb != usb {
                            status.usb = usb;
                        }
                        if status.addresses != addresses {
                            status.addresses = addresses;
                        }
                        last_devices = std::time::Instant::now();
                    }
                }
                state.running.store(false, Ordering::Release);
                state.request_disconnect();
                drop(inhibitor);
                state
                    .sessions
                    .idle_inhibited
                    .store(false, Ordering::Release);
                drop(discovery);
                drop(server);
                state.media.stop();
            })?;
        Ok(Self {
            client: Client { shared, commands },
            worker: Some(worker),
        })
    }
    pub fn stop(&mut self) {
        let _ = self.client.commands.send(Command::Quit);
        if let Some(t) = self.worker.take() {
            let _ = t.join();
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::oneshot;
    #[cfg(windows)]
    #[test]
    fn hidden_receiver_inhibits_video_and_releases_after_disconnect_and_quit() {
        let directory = std::env::temp_dir().join(format!(
            "airdock-power-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let mut runtime = Runtime::start(directory.clone(), 0, Settings::default()).unwrap();
        let shared = &runtime.client.shared;
        let wait = |active| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while shared.sessions.idle_inhibited.load(Ordering::Acquire) != active {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Runtime power transition timed out"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        assert!(!shared.sessions.idle_inhibited.load(Ordering::Acquire));
        shared.ui.visible.store(false, Ordering::Release);
        shared.sessions.begin_video(1);
        wait(true);
        shared.sessions.advance();
        shared.sessions.begin_video(2);
        shared.sessions.end_video(1);
        std::thread::sleep(Duration::from_millis(1100));
        assert!(shared.sessions.idle_inhibited.load(Ordering::Acquire));
        shared.sessions.end_video(2);
        wait(false);
        shared.sessions.begin_video(3);
        wait(true);
        let shared = shared.clone();
        runtime.stop();
        assert!(!shared.sessions.idle_inhibited.load(Ordering::Acquire));
        std::fs::remove_dir_all(directory).unwrap();
    }
    fn request(
        runtime: &Runtime,
        command: impl FnOnce(oneshot::Sender<Result<Settings, String>>) -> Command,
    ) -> Result<Settings, String> {
        let (sender, receiver) = oneshot::channel();
        runtime.client.commands.send(command(sender)).unwrap();
        futures::executor::block_on(receiver).unwrap()
    }
    #[test]
    fn independent_preferences_survive_stale_form_and_failed_write_is_not_applied() {
        let directory = std::env::temp_dir().join(format!(
            "airdock-ui-preferences-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let initial = Settings::default();
        let mut runtime = Runtime::start(directory.clone(), 0, initial.clone()).unwrap();
        let prefs = WindowPreferences {
            width: 960,
            height: 640,
            fullscreen: false,
            hide_ui: false,
        };
        request(&runtime, |reply| Command::WindowPreferences(prefs, reply)).unwrap();
        request(&runtime, |reply| {
            Command::AudioOutput("Selected device".into(), reply)
        })
        .unwrap();
        request(&runtime, |reply| {
            Command::Language(crate::i18n::Language::Chinese, reply)
        })
        .unwrap();
        let mut stale_form = initial;
        stale_form.name = "Saved form".into();
        let saved = request(&runtime, |reply| Command::Settings(stale_form, reply)).unwrap();
        assert_eq!(saved.window_width, 960);
        assert_eq!(saved.audio_device, "Selected device");
        assert_eq!(saved.language, crate::i18n::Language::Chinese);
        assert_eq!(saved.name, "Saved form");
        assert_eq!(
            Settings::load(&directory.join("settings.json")).unwrap(),
            saved
        );
        std::fs::remove_file(directory.join("settings.json")).unwrap();
        std::fs::create_dir(directory.join("settings.json")).unwrap();
        assert!(
            request(&runtime, |reply| Command::AudioOutput(
                "Must not apply".into(),
                reply
            ))
            .is_err()
        );
        assert!(
            request(&runtime, |reply| Command::Language(
                crate::i18n::Language::English,
                reply
            ))
            .is_err()
        );
        assert_eq!(*runtime.client.shared.settings.read().unwrap(), saved);
        runtime.stop();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
