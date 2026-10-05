// SPDX-License-Identifier: MPL-2.0
use crate::server::Device;
use anyhow::Result;

pub(crate) fn advertisement_changed(
    before: &crate::config::Settings,
    after: &crate::config::Settings,
) -> bool {
    before.name != after.name
        || before.hls_enabled != after.hls_enabled
        || before.hevc_enabled != after.hevc_enabled
}

// The completion channel owns a successful registration until the owner takes
// it. An unread/late success must withdraw itself, including a cancellation race.
#[cfg(any(windows, test))]
struct ServiceLease<T> {
    resource: Option<T>,
    withdraw: fn(T) -> std::sync::mpsc::Receiver<u32>,
}
#[cfg(any(windows, test))]
impl<T> ServiceLease<T> {
    fn withdraw(mut self) -> std::sync::mpsc::Receiver<u32> {
        (self.withdraw)(self.resource.take().unwrap())
    }
}
#[cfg(any(windows, test))]
impl<T> Drop for ServiceLease<T> {
    fn drop(&mut self) {
        if let Some(resource) = self.resource.take() {
            // The native withdrawal callback retains storage and logs failures,
            // even if shutdown no longer has a receiver waiting for it.
            let _ = (self.withdraw)(resource);
        }
    }
}
#[cfg(any(windows, test))]
fn wait_or_cancel<T>(
    done: &std::sync::mpsc::Receiver<T>,
    timeout: std::time::Duration,
    cancel: impl FnOnce(),
) -> Result<T, std::sync::mpsc::RecvTimeoutError> {
    match done.recv_timeout(timeout) {
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            cancel();
            done.recv_timeout(timeout)
        }
        result => result,
    }
}

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
        ("pi", device.receiver_id.as_str()),
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
        let mut discovery = Self {
            daemon,
            names: Vec::new(),
        };
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
            let fullname = info.get_fullname().to_owned();
            discovery.daemon.register(info)?;
            discovery.names.push(fullname);
        }
        Ok(discovery)
    }
}
#[cfg(not(windows))]
impl Drop for Discovery {
    fn drop(&mut self) {
        let pending: Vec<_> = self
            .names
            .iter()
            .filter_map(|name| match self.daemon.unregister(name) {
                Ok(done) => Some((name, done)),
                Err(error) => {
                    tracing::warn!(%name, %error, "mDNS withdrawal failed");
                    None
                }
            })
            .collect();
        for (name, done) in pending {
            match done.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(mdns_sd::UnregisterStatus::OK) => {
                    tracing::debug!(%name, "mDNS service withdrawn")
                }
                result => tracing::warn!(%name, ?result, "mDNS withdrawal was not acknowledged"),
            }
        }
        if let Ok(done) = self.daemon.shutdown()
            && let Err(error) = done.recv_timeout(std::time::Duration::from_secs(2))
        {
            tracing::warn!(%error, "mDNS daemon shutdown was not acknowledged");
        }
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
        name: String,
        _strings: Vec<Vec<u16>>,
        _keys: Vec<PWSTR>,
        _values: Vec<PWSTR>,
        _instance: Box<DNS_SERVICE_INSTANCE>,
        request: Box<DNS_SERVICE_REGISTER_REQUEST>,
        cancel: Box<DNS_SERVICE_CANCEL>,
    }
    // Pointers target allocations owned by Data; accesses to request are
    // serialized, and a new operation starts only after the previous callback.
    unsafe impl Send for Data {}
    const DNS_PENDING: u32 = 9506;
    const COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
    type Lease = ServiceLease<Arc<Mutex<Data>>>;
    struct Completion {
        data: Arc<Mutex<Data>>,
        name: String,
        done: mpsc::Sender<Result<Lease, u32>>,
    }
    struct Withdrawal {
        _data: Arc<Mutex<Data>>,
        name: String,
        done: mpsc::Sender<u32>,
    }
    struct Registration {
        name: String,
        data: Arc<Mutex<Data>>,
        done: Option<mpsc::Receiver<Result<Lease, u32>>>,
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
                name: name.to_owned(),
                _strings: strings,
                _keys: keys,
                _values: values,
                _instance: instance,
                request,
                // Retained by completion ownership even after shutdown times out.
                cancel: Box::default(),
            }));
            let (sender, done) = mpsc::channel();
            let context = Box::into_raw(Box::new(Completion {
                data: data.clone(),
                name: name.to_owned(),
                done: sender,
            }));
            let status = {
                let mut d = data.lock().unwrap();
                d.request.pQueryContext = context.cast();
                let Data {
                    request, cancel, ..
                } = &mut *d;
                unsafe { DnsServiceRegister(&**request, Some(&mut **cancel)) }
            };
            if status != DNS_PENDING {
                // Only DNS_REQUEST_PENDING schedules a callback.
                unsafe { completed(status, context.cast(), std::ptr::null()) };
            }
            anyhow::ensure!(
                status == DNS_PENDING || status == 0,
                "Windows mDNS registration failed for {name}: {status}"
            );
            Ok(Self {
                name: name.to_owned(),
                data,
                done: Some(done),
            })
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
            let result = if status == 0 {
                tracing::debug!(name = %completion.name, "Windows mDNS service registered");
                Ok(ServiceLease {
                    resource: Some(completion.data.clone()),
                    withdraw: withdraw_service,
                })
            } else {
                tracing::warn!(name = %completion.name, status, "Windows mDNS registration failed");
                Err(status)
            };
            // SendError drops the lease when cancellation/shutdown abandoned
            // the channel; a successful late registration is still withdrawn.
            let _ = completion.done.send(result);
        }
    }
    fn withdraw_service(data: Arc<Mutex<Data>>) -> mpsc::Receiver<u32> {
        let (sender, done) = mpsc::channel();
        let name = data.lock().unwrap().name.clone();
        let context = Box::into_raw(Box::new(Withdrawal {
            _data: data.clone(),
            name,
            done: sender,
        }));
        let status = {
            let mut d = data.lock().unwrap();
            // DeRegister requires the original registration request and a null
            // cancellation pointer. It has its own asynchronous completion.
            d.request.pRegisterCompletionCallback = Some(withdrawn);
            d.request.pQueryContext = context.cast();
            unsafe { DnsServiceDeRegister(&*d.request, None) }
        };
        if status != DNS_PENDING {
            unsafe { withdrawn(status, context.cast(), std::ptr::null()) };
        }
        done
    }
    unsafe extern "system" fn withdrawn(
        status: u32,
        context: *const std::ffi::c_void,
        instance: *const DNS_SERVICE_INSTANCE,
    ) {
        if !instance.is_null() {
            unsafe { DnsServiceFreeInstance(instance) };
        }
        if !context.is_null() {
            let completion = unsafe { Box::from_raw(context as *mut Withdrawal) };
            if status == 0 {
                tracing::debug!(name = %completion.name, "Windows mDNS service withdrawn");
            } else {
                tracing::warn!(name = %completion.name, status, "Windows mDNS withdrawal failed");
            }
            let _ = completion.done.send(status);
        }
    }
    impl Registration {
        fn close(&mut self) -> Result<()> {
            let Some(done) = self.done.take() else {
                return Ok(());
            };
            let result = wait_or_cancel(&done, COMPLETION_TIMEOUT, || {
                let d = self.data.lock().unwrap();
                let status = unsafe { DnsServiceRegisterCancel(&*d.cancel) };
                tracing::debug!(name = %self.name, status, "Cancelling pending mDNS registration");
            });
            match result {
                Ok(Ok(lease)) => {
                    let status = lease.withdraw().recv_timeout(COMPLETION_TIMEOUT)?;
                    anyhow::ensure!(status == 0, "mDNS withdrawal failed: {status}");
                    Ok(())
                }
                // Failed/cancelled registration has no service to withdraw.
                Ok(Err(_)) => Ok(()),
                Err(error) => Err(anyhow::anyhow!(
                    "mDNS cancellation still pending ({error}); late success will withdraw itself"
                )),
            }
        }
    }
    impl Drop for Registration {
        fn drop(&mut self) {
            if let Err(error) = self.close() {
                tracing::warn!(name = %self.name, %error, "mDNS shutdown incomplete");
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn windows_dns_registration_and_withdrawal_are_acknowledged() {
            let host = format!("{}.local", std::env::var("COMPUTERNAME").unwrap());
            let name = format!(
                "AirDock-test-{}._airdock-test._tcp.local",
                std::process::id()
            );
            let mut service =
                Registration::new(&name, &host, 7000, vec![("txtvers".into(), "1".into())])
                    .unwrap();
            // Wait for actual Windows registration before exercising shutdown;
            // close() must receive the native DeRegister completion too.
            let lease = service
                .done
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .ok()
                .unwrap();
            let (sender, receiver) = mpsc::channel();
            sender.send(Ok(lease)).ok().unwrap();
            service.done = Some(receiver);
            service.close().unwrap();
            // A second close is harmless and cannot issue another native call.
            service.close().unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Settings, state::Shared};
    use ed25519_dalek::SigningKey;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::Duration,
    };

    fn fake_withdraw(count: Arc<AtomicUsize>) -> mpsc::Receiver<u32> {
        count.fetch_add(1, Ordering::SeqCst);
        let (sender, done) = mpsc::channel();
        sender.send(0).unwrap();
        done
    }
    fn lease(count: &Arc<AtomicUsize>) -> ServiceLease<Arc<AtomicUsize>> {
        ServiceLease {
            resource: Some(count.clone()),
            withdraw: fake_withdraw,
        }
    }
    #[test]
    fn withdrawal_happens_once_and_is_acknowledged() {
        let count = Arc::new(AtomicUsize::new(0));
        assert_eq!(lease(&count).withdraw().recv().unwrap(), 0);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn queued_success_is_withdrawn_if_shutdown_abandons_receiver() {
        let count = Arc::new(AtomicUsize::new(0));
        let (sender, done) = mpsc::channel();
        sender.send(lease(&count)).ok().unwrap();
        drop(done);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn late_success_after_cancellation_timeout_is_withdrawn() {
        let count = Arc::new(AtomicUsize::new(0));
        let (sender, done) = mpsc::channel();
        let cancelled = AtomicUsize::new(0);
        assert!(
            wait_or_cancel(&done, Duration::ZERO, || {
                cancelled.fetch_add(1, Ordering::SeqCst);
            })
            .is_err()
        );
        assert_eq!(cancelled.load(Ordering::SeqCst), 1);
        drop(done);
        // Same ownership path as the native completion callback's SendError.
        let _ = sender.send(lease(&count));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn success_racing_cancellation_is_still_withdrawn() {
        let count = Arc::new(AtomicUsize::new(0));
        let (sender, done) = mpsc::channel();
        let registered = wait_or_cancel(&done, Duration::ZERO, || {
            sender.send(lease(&count)).ok().unwrap();
        })
        .unwrap();
        assert_eq!(registered.withdraw().recv().unwrap(), 0);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn completed_registration_does_not_get_cancelled() {
        let (sender, done) = mpsc::channel();
        sender.send(0).unwrap();
        assert_eq!(
            wait_or_cancel(&done, Duration::ZERO, || panic!("unexpected cancellation")).unwrap(),
            0
        );
        drop(sender);
        assert!(
            wait_or_cancel(&done, Duration::ZERO, || panic!(
                "cancelled a disconnected channel"
            ))
            .is_err()
        );
    }
    #[test]
    fn only_advertised_settings_require_reregistration() {
        let original = Settings::default();
        let mut changed = original.clone();
        changed.window_width += 1;
        changed.max_fps += 1;
        changed.audio_device = "Other device".into();
        assert!(!advertisement_changed(&original, &changed));
        changed.name = "Another receiver".into();
        assert!(advertisement_changed(&original, &changed));
        changed = original.clone();
        changed.hls_enabled = !changed.hls_enabled;
        assert!(advertisement_changed(&original, &changed));
        changed = original.clone();
        changed.hevc_enabled = !changed.hevc_enabled;
        assert!(advertisement_changed(&original, &changed));
    }
    #[test]
    fn receiver_identity_is_unique_persistent_and_matches_info_and_txt() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let shared = Shared::new(Settings::default());
        let device = Device::new(key.clone(), 7000, shared.clone()).unwrap();
        let info = device.info();
        let info = info.as_dictionary().unwrap();
        let (txt, raop) = properties(&device);
        let txt: std::collections::BTreeMap<_, _> = txt.into_iter().collect();
        assert_eq!(txt["pi"], info["pi"].as_string().unwrap());
        assert_eq!(txt["deviceid"], info["deviceid"].as_string().unwrap());
        assert_eq!(txt["pk"], hex::encode(info["pk"].as_data().unwrap()));
        assert_eq!(
            raop.iter().find(|(key, _)| key == "pk").unwrap().1,
            txt["pk"]
        );
        let display = info["displays"].as_array().unwrap()[0]
            .as_dictionary()
            .unwrap();
        assert_eq!(display["uuid"].as_string().unwrap(), device.display_id);
        assert_ne!(device.receiver_id, device.display_id);
        assert_ne!(device.receiver_id, "b08f5a79-db29-4384-b456-a4784d9e6055");
        for uuid in [&device.receiver_id, &device.display_id] {
            assert_eq!(uuid.len(), 36);
            assert_eq!(&uuid[14..15], "8");
            assert!(matches!(&uuid[19..20], "8" | "9" | "a" | "b"));
            assert_eq!(hex::decode(uuid.replace('-', "")).unwrap().len(), 16);
        }
        shared.settings.write().unwrap().name = "Renamed receiver".into();
        let restarted = Device::new(key, 7010, shared.clone()).unwrap();
        assert_eq!(device.receiver_id, restarted.receiver_id);
        assert_eq!(device.display_id, restarted.display_id);
        assert_eq!(device.mac, restarted.mac);
        assert_eq!(device.identity.to_bytes(), restarted.identity.to_bytes());
        assert_eq!(
            restarted.info().as_dictionary().unwrap()["name"]
                .as_string()
                .unwrap(),
            "Renamed receiver"
        );
        let other = Device::new(SigningKey::from_bytes(&[2; 32]), 7000, shared.clone()).unwrap();
        assert_ne!(device.receiver_id, other.receiver_id);
        assert_ne!(device.display_id, other.display_id);
        shared.media.stop();
    }
}
