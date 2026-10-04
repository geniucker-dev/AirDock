use crate::{
    config::Settings,
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
    Settings(Settings),
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
                        Ok(Command::Settings(settings)) => {
                            let result = (|| -> Result<()> {
                                settings.save(&directory.join("settings.json"))?;
                                platform::autostart(settings.autostart)?;
                                *state.settings.write().unwrap() = settings;
                                drop(discovery.take());
                                discovery = Some(Discovery::start(server.device())?);
                                Ok(())
                            })();
                            if let Err(e) = result {
                                state.report(format!("Settings: {e:#}"));
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if last_devices.elapsed() >= Duration::from_secs(2) {
                        let addresses = if_addrs::get_if_addrs()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|a| !a.is_loopback())
                            .map(|a| format!("{}: {}", a.name, a.ip()))
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let mut status = state.ui.lock().unwrap();
                        status.usb = platform::usb_present();
                        status.addresses = addresses;
                        last_devices = std::time::Instant::now();
                    }
                }
                state.running.store(false, Ordering::Release);
                state.request_disconnect();
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
