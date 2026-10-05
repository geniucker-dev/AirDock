// SPDX-License-Identifier: MPL-2.0
use crate::server::Device;
use anyhow::Result;

type Properties = Vec<(String, String)>;
fn properties(device: &Device) -> (Properties, Properties) {
    let features = device.features();
    let ft = format!("0x{:X},0x{:X}", features as u32, features >> 32);
    let pk = hex::encode(device.identity.verifying_key().to_bytes());
    let airplay = [
        ("deviceid", device.mac.as_str()),
        ("features", ft.as_str()),
        ("flags", "0x4"),
        ("model", "AppleTV3,2"),
        ("pi", "b08f5a79-db29-4384-b456-a4784d9e6055"),
        ("pk", pk.as_str()),
        ("pw", "false"),
        ("srcvers", "220.68"),
        ("vv", "2"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let raop = [
        ("txtvers", "1"),
        ("ch", "2"),
        ("cn", "0,1,2,3"),
        ("da", "true"),
        ("et", "0,3,5"),
        ("ft", ft.as_str()),
        ("md", "0,1,2"),
        ("am", "AppleTV3,2"),
        ("rhd", "5.6.0.0"),
        ("pk", pk.as_str()),
        ("pw", "false"),
        ("sf", "0x4"),
        ("sr", "44100"),
        ("ss", "16"),
        ("sv", "false"),
        ("tp", "UDP"),
        ("vn", "65537"),
        ("vs", "220.68"),
        ("vv", "2"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    (airplay, raop)
}

#[cfg(not(windows))]
pub struct Discovery {
    daemon: mdns_sd::ServiceDaemon,
    names: Vec<String>,
}
#[cfg(not(windows))]
impl Discovery {
    pub fn start(device: &Device) -> Result<Self> {
        use mdns_sd::{ServiceDaemon, ServiceInfo};
        let daemon = ServiceDaemon::new()?;
        let name = device.shared.settings.read().unwrap().name.clone();
        let (airplay, raop) = properties(device);
        let host = format!("airdock-{}.local.", device.mac.replace(':', ""));
        let mut names = Vec::new();
        for (kind, name, properties) in [
            ("_airplay._tcp.local.", name.clone(), airplay),
            (
                "_raop._tcp.local.",
                format!("{}@{name}", device.mac.replace(':', "")),
                raop,
            ),
        ] {
            let info =
                ServiceInfo::new(kind, &name, &host, "", device.port, properties.as_slice())?
                    .enable_addr_auto();
            names.push(info.get_fullname().to_owned());
            daemon.register(info)?;
        }
        Ok(Self { daemon, names })
    }
}
#[cfg(not(windows))]
impl Drop for Discovery {
    fn drop(&mut self) {
        for name in &self.names {
            let _ = self.daemon.unregister(name);
        }
        let _ = self.daemon.shutdown();
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    use windows::{Win32::NetworkManagement::Dns::*, core::PWSTR};

    // Every outstanding native call retains all pointer backing storage until
    // its completion callback. Cancel is asynchronous too: dropping a window
    // must never free a pending request or its UTF-16/TXT arrays.
    struct Data {
        _strings: Vec<Vec<u16>>,
        _keys: Vec<PWSTR>,
        _values: Vec<PWSTR>,
        _instance: Box<DNS_SERVICE_INSTANCE>,
        request: Box<DNS_SERVICE_REGISTER_REQUEST>,
    }
    // Pointers target allocations owned by Data; accesses to request are
    // serialized, and a new operation starts only after the previous callback.
    unsafe impl Send for Data {}
    struct Completion {
        _data: Arc<Mutex<Data>>,
        done: mpsc::Sender<u32>,
    }
    struct Registration {
        data: Arc<Mutex<Data>>,
        cancel: DNS_SERVICE_CANCEL,
        done: mpsc::Receiver<u32>,
    }
    impl Registration {
        fn new(name: &str, host: &str, port: u16, properties: Properties) -> Result<Self> {
            let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            let mut strings = vec![wide(name), wide(host)];
            for (k, v) in &properties {
                strings.push(wide(k));
                strings.push(wide(v));
            }
            let mut keys = Vec::new();
            let mut values = Vec::new();
            for i in 0..properties.len() {
                keys.push(PWSTR(strings[2 + i * 2].as_mut_ptr()));
                values.push(PWSTR(strings[3 + i * 2].as_mut_ptr()));
            }
            let mut instance = Box::new(DNS_SERVICE_INSTANCE::default());
            instance.pszInstanceName = PWSTR(strings[0].as_mut_ptr());
            instance.pszHostName = PWSTR(strings[1].as_mut_ptr());
            instance.wPort = port;
            instance.dwPropertyCount = properties.len() as u32;
            instance.keys = keys.as_mut_ptr();
            instance.values = values.as_mut_ptr();
            let mut request = Box::new(DNS_SERVICE_REGISTER_REQUEST::default());
            request.Version = 1;
            request.pServiceInstance = &mut *instance;
            request.pRegisterCompletionCallback = Some(completed);
            let data = Arc::new(Mutex::new(Data {
                _strings: strings,
                _keys: keys,
                _values: values,
                _instance: instance,
                request,
            }));
            let (sender, done) = mpsc::channel();
            let context = Box::into_raw(Box::new(Completion {
                _data: data.clone(),
                done: sender,
            }));
            let mut cancel = DNS_SERVICE_CANCEL::default();
            let status = {
                let mut d = data.lock().unwrap();
                d.request.pQueryContext = context.cast();
                unsafe { DnsServiceRegister(&*d.request, Some(&mut cancel)) }
            };
            if status != 9506 {
                // Only DNS_REQUEST_PENDING schedules a callback.
                let completion = unsafe { Box::from_raw(context) };
                let _ = completion.done.send(status);
            }
            anyhow::ensure!(
                status == 9506 || status == 0,
                "Windows mDNS registration failed: {status}"
            );
            Ok(Self { data, cancel, done })
        }
    }
    unsafe extern "system" fn completed(
        status: u32,
        context: *const std::ffi::c_void,
        instance: *const DNS_SERVICE_INSTANCE,
    ) {
        if !instance.is_null() {
            unsafe { DnsServiceFreeInstance(instance) };
        }
        if !context.is_null() {
            let completion = unsafe { Box::from_raw(context as *mut Completion) };
            if status != 0 {
                tracing::warn!("Windows mDNS completion: {status}");
            }
            let _ = completion.done.send(status);
        }
    }
    impl Drop for Registration {
        fn drop(&mut self) {
            let status = match self.done.recv_timeout(Duration::from_secs(2)) {
                Ok(status) => status,
                Err(_) => {
                    unsafe {
                        DnsServiceRegisterCancel(&self.cancel);
                    }
                    // The callback owns the backing data until cancellation
                    // completes, even when this Registration has gone away.
                    return;
                }
            };
            if status != 0 {
                return;
            }
            let (sender, _done) = mpsc::channel();
            let context = Box::into_raw(Box::new(Completion {
                _data: self.data.clone(),
                done: sender,
            }));
            let status = {
                let mut d = self.data.lock().unwrap();
                d.request.pQueryContext = context.cast();
                unsafe { DnsServiceDeRegister(&*d.request, None) }
            };
            if status != 9506 {
                unsafe {
                    drop(Box::from_raw(context));
                }
            }
        }
    }
    pub struct Discovery {
        _services: Vec<Registration>,
    }
    impl Discovery {
        pub fn start(device: &Device) -> Result<Self> {
            let name = device.shared.settings.read().unwrap().name.clone();
            let host = format!(
                "{}.local",
                std::env::var("COMPUTERNAME").unwrap_or_else(|_| crate::brand::NAME.into())
            );
            let (airplay, raop) = properties(device);
            let services = vec![
                Registration::new(
                    &format!("{name}._airplay._tcp.local"),
                    &host,
                    device.port,
                    airplay,
                )?,
                Registration::new(
                    &format!("{}@{name}._raop._tcp.local", device.mac.replace(':', "")),
                    &host,
                    device.port,
                    raop,
                )?,
            ];
            Ok(Self {
                _services: services,
            })
        }
    }
}
#[cfg(windows)]
pub use native::Discovery;
