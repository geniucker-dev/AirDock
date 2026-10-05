// SPDX-License-Identifier: MPL-2.0
//! Protocol adapters own discovery, authentication, transport and their workers.
//! The runtime owns adapters; adapters share session arbitration and media output.
pub mod airplay;

use crate::config::Settings;
use anyhow::{Result, bail};

/// Stable internal protocol identifier, distinct from a sender/platform name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolId(pub &'static str);
impl ProtocolId {
    pub const AIRPLAY: Self = Self("airplay");
}

/// Low-frequency control plane. Media never passes through these virtual calls.
pub trait ReceiverBackend: Send {
    fn protocol(&self) -> ProtocolId;
    /// Also called with identical settings during initialization/recovery.
    /// A failed refresh must leave the adapter retryable and existing media intact.
    fn refresh(&mut self, before: &Settings, after: &Settings) -> Result<()>;
    /// Called after common session cancellation and generation invalidation.
    fn disconnect(&mut self);
    /// Whether a timestamp-scheduled presentation is actively playing (not paused/EOF).
    fn timed_video_playing(&self) -> bool;
    /// Withdraw discovery, cancel/join workers, and release protocol resources.
    /// Must be idempotent and must not stop other adapters or common playback.
    fn shutdown(&mut self);
}

#[derive(Default)]
pub struct Backends {
    entries: Vec<Box<dyn ReceiverBackend>>,
}
impl Backends {
    pub fn add(&mut self, mut backend: Box<dyn ReceiverBackend>) -> Result<()> {
        if self
            .entries
            .iter()
            .any(|b| b.protocol() == backend.protocol())
        {
            backend.shutdown();
            bail!("Duplicate receiver protocol: {}", backend.protocol().0);
        }
        self.entries.push(backend);
        Ok(())
    }
    pub fn refresh(&mut self, before: &Settings, after: &Settings) -> Vec<String> {
        self.entries
            .iter_mut()
            .filter_map(|b| {
                b.refresh(before, after)
                    .err()
                    .map(|e| format!("{} receiver: {e:#}", b.protocol().0))
            })
            .collect()
    }
    pub fn disconnect(&mut self) {
        for backend in &mut self.entries {
            backend.disconnect();
        }
    }
    pub fn timed_video_playing(&self, protocol: Option<ProtocolId>) -> bool {
        self.entries
            .iter()
            .find(|b| Some(b.protocol()) == protocol)
            .is_some_and(|b| b.timed_video_playing())
    }
    pub fn shutdown(&mut self) {
        for mut backend in self.entries.drain(..).rev() {
            backend.shutdown();
        }
    }
}
impl Drop for Backends {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Fake {
        protocol: ProtocolId,
        calls: Arc<Mutex<Vec<(ProtocolId, &'static str)>>>,
        fail_refresh: bool,
        playing: bool,
    }
    impl ReceiverBackend for Fake {
        fn protocol(&self) -> ProtocolId {
            self.protocol
        }
        fn refresh(&mut self, _: &Settings, _: &Settings) -> Result<()> {
            self.calls.lock().unwrap().push((self.protocol, "refresh"));
            if std::mem::take(&mut self.fail_refresh) {
                bail!("Discovery failed")
            }
            Ok(())
        }
        fn disconnect(&mut self) {
            self.calls
                .lock()
                .unwrap()
                .push((self.protocol, "disconnect"));
        }
        fn timed_video_playing(&self) -> bool {
            self.playing
        }
        fn shutdown(&mut self) {
            self.calls.lock().unwrap().push((self.protocol, "shutdown"));
        }
    }
    #[test]
    fn adapter_failure_is_retryable_and_lifecycle_fans_out_without_media_dispatch() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let other = ProtocolId("test-cast");
        let mut backends = Backends::default();
        for (protocol, fail_refresh, playing) in
            [(ProtocolId::AIRPLAY, true, false), (other, false, true)]
        {
            backends
                .add(Box::new(Fake {
                    protocol,
                    calls: calls.clone(),
                    fail_refresh,
                    playing,
                }))
                .unwrap();
        }
        let settings = Settings::default();
        let errors = backends.refresh(&settings, &settings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("airplay"));
        assert!(backends.refresh(&settings, &settings).is_empty());
        assert!(!backends.timed_video_playing(Some(ProtocolId::AIRPLAY)));
        assert!(backends.timed_video_playing(Some(other)));
        assert!(!backends.timed_video_playing(None));
        assert!(!backends.timed_video_playing(Some(ProtocolId("unregistered"))));
        backends.disconnect();
        backends.shutdown();
        backends.shutdown();
        drop(backends);
        let calls = calls.lock().unwrap();
        for protocol in [ProtocolId::AIRPLAY, other] {
            assert_eq!(
                calls
                    .iter()
                    .filter(|c| **c == (protocol, "refresh"))
                    .count(),
                2
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|c| **c == (protocol, "disconnect"))
                    .count(),
                1
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|c| **c == (protocol, "shutdown"))
                    .count(),
                1
            );
        }
        assert_eq!(
            &calls[calls.len() - 2..],
            &[(other, "shutdown"), (ProtocolId::AIRPLAY, "shutdown")]
        );
    }
    #[test]
    fn duplicate_adapter_is_rejected_and_cleaned_up() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut backends = Backends::default();
        for duplicate in [false, true] {
            let result = backends.add(Box::new(Fake {
                protocol: ProtocolId::AIRPLAY,
                calls: calls.clone(),
                fail_refresh: false,
                playing: false,
            }));
            assert_eq!(result.is_err(), duplicate);
        }
        assert_eq!(calls.lock().unwrap().len(), 1);
        drop(backends);
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
}
