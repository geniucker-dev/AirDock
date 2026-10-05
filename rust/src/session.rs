// SPDX-License-Identifier: MPL-2.0
use crate::{playback::PlaybackMode, receiver::ProtocolId};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconnectPolicy {
    Reject,
    ReplaceSamePeer,
}

pub struct SessionOwner {
    pub id: u64,
    pub protocol: ProtocolId,
    pub peer: std::net::IpAddr,
    pub stop: AtomicBool,
    pub done: Mutex<bool>,
    pub ready: Condvar,
}

/// Network ownership and cancellation, independent of any desktop window.
pub struct Sessions {
    owner: Mutex<Option<Arc<SessionOwner>>>,
    /// Observer snapshot; controls still authorize against the ownership gate.
    /// Publish under `owner`, but never retain this lock during control/cleanup.
    observed_protocol: Mutex<Option<ProtocolId>>,
    /// Media workers only observe ownership. Their per-packet check must not
    /// wait for auxiliary controls; zero denotes no owner, never a wire ID.
    observed_owner_id: AtomicU64,
    next_id: AtomicU64,
    pub running: AtomicBool,
    pub disconnect: AtomicU64,
    /// Protocol video lifetime, independent of FLUSH/seek frame generations.
    pub video_owner: AtomicU64,
    video_mode: AtomicU8,
    pub idle_inhibited: AtomicBool,
    pub video_serial: AtomicU64,
    pub video_started: Mutex<std::time::Instant>,
    pub epoch: Arc<AtomicU64>,
}
impl Default for Sessions {
    fn default() -> Self {
        Self {
            owner: Mutex::new(None),
            observed_protocol: Mutex::new(None),
            observed_owner_id: AtomicU64::new(0),
            next_id: AtomicU64::new(1),
            running: AtomicBool::new(true),
            disconnect: AtomicU64::new(0),
            video_owner: AtomicU64::new(0),
            video_mode: AtomicU8::new(PlaybackMode::Live as u8),
            idle_inhibited: AtomicBool::new(false),
            video_serial: AtomicU64::new(0),
            video_started: Mutex::new(std::time::Instant::now()),
            epoch: Arc::new(AtomicU64::new(1)),
        }
    }
}
impl Sessions {
    /// Receiver-local IDs are unique across adapters and listener restarts.
    /// A wire session ID must never be used as an ownership key.
    pub fn allocate_id(&self) -> u64 {
        self.next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("Receiver session ID space exhausted")
    }
    pub fn owner_id(&self) -> Option<u64> {
        let id = self.observed_owner_id.load(Ordering::Acquire);
        (id != 0).then_some(id)
    }
    pub fn active_protocol(&self) -> Option<ProtocolId> {
        *self.observed_protocol.lock().unwrap()
    }
    /// Serialize auxiliary playback controls with ownership handover. The closure
    /// must not claim/release/cancel a session or otherwise re-enter this gate.
    pub fn with_protocol_control<R>(
        &self,
        protocol: ProtocolId,
        apply: impl FnOnce() -> R,
    ) -> Option<R> {
        let owner = self.owner.lock().unwrap();
        if owner
            .as_ref()
            .is_some_and(|owner| owner.protocol != protocol)
        {
            return None;
        }
        Some(apply())
    }
    /// At most one sender owns the shared playback devices. A protocol decides
    /// whether a same-peer reconnect may retire its own previous connection.
    pub fn claim(
        &self,
        id: u64,
        protocol: ProtocolId,
        peer: std::net::IpAddr,
        policy: ReconnectPolicy,
        timeout: std::time::Duration,
    ) -> Option<Arc<SessionOwner>> {
        assert_ne!(id, 0, "Zero is reserved for no session");
        let disconnect = self.disconnect.load(Ordering::Acquire);
        let previous = self.owner.lock().unwrap().clone();
        if let Some(previous) = previous {
            if previous.id == id && previous.protocol == protocol {
                return (!previous.stop.load(Ordering::Acquire)).then_some(previous);
            }
            if policy != ReconnectPolicy::ReplaceSamePeer
                || previous.protocol != protocol
                || previous.peer != peer
            {
                return None;
            }
            previous.stop.store(true, Ordering::Release);
            let done = previous.done.lock().unwrap();
            let (done, _) = previous
                .ready
                .wait_timeout_while(done, timeout, |done| !*done)
                .unwrap();
            if !*done {
                return None;
            }
        }
        let mut owner = self.owner.lock().unwrap();
        if owner.is_some()
            || !self.running.load(Ordering::Acquire)
            || self.disconnect.load(Ordering::Acquire) != disconnect
        {
            return None;
        }
        let lease = Arc::new(SessionOwner {
            id,
            protocol,
            peer,
            stop: Default::default(),
            done: Mutex::new(false),
            ready: Default::default(),
        });
        *owner = Some(lease.clone());
        *self.observed_protocol.lock().unwrap() = Some(protocol);
        self.observed_owner_id.store(id, Ordering::Release);
        Some(lease)
    }
    /// Workers must be joined first. Keep the ownership gate closed until old
    /// playback/status cleanup completes; stale teardown cannot reset a new owner.
    pub fn release(&self, lease: &Arc<SessionOwner>, cleanup: impl FnOnce()) {
        let mut owner = self.owner.lock().unwrap();
        if owner
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, lease))
        {
            self.end_video(lease.id);
            cleanup();
            *owner = None;
            *self.observed_protocol.lock().unwrap() = None;
            self.observed_owner_id.store(0, Ordering::Release);
        }
        drop(owner);
        *lease.done.lock().unwrap() = true;
        lease.ready.notify_all();
    }
    pub fn cancel(&self) {
        self.disconnect.fetch_add(1, Ordering::AcqRel);
        if let Some(owner) = self.owner.lock().unwrap().as_ref() {
            owner.stop.store(true, Ordering::Release);
        }
    }
    pub fn generation(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    pub fn advance(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::AcqRel) + 1
    }
    pub fn end_video(&self, session: u64) {
        let _ = self
            .video_owner
            .compare_exchange(session, 0, Ordering::AcqRel, Ordering::Acquire);
    }
    pub fn begin_video(&self, session: u64) {
        self.begin_presentation(session, PlaybackMode::Live);
    }
    pub fn begin_timed_video(&self, session: u64) {
        self.begin_presentation(session, PlaybackMode::Timed);
    }
    fn begin_presentation(&self, session: u64, mode: PlaybackMode) {
        if self.video_owner.load(Ordering::Acquire) == session
            && self.video_mode.load(Ordering::Acquire) == mode as u8
        {
            return; // Repeated SETUP/format change in the same live presentation.
        }
        *self.video_started.lock().unwrap() = std::time::Instant::now();
        self.video_serial.fetch_add(1, Ordering::AcqRel);
        self.video_mode.store(mode as u8, Ordering::Release);
        self.video_owner.store(session, Ordering::Release);
    }
    /// Window visibility and frame FLUSH do not end an active mirror session.
    pub fn needs_video_inhibition(&self, timed_playing: impl FnOnce() -> bool) -> bool {
        self.video_owner.load(Ordering::Acquire) != 0
            && (self.video_mode.load(Ordering::Acquire) == PlaybackMode::Live as u8
                || timed_playing())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::IpAddr, time::Duration};
    fn claim(
        sessions: &Sessions,
        protocol: ProtocolId,
        peer: IpAddr,
        policy: ReconnectPolicy,
    ) -> Option<Arc<SessionOwner>> {
        sessions.claim(
            sessions.allocate_id(),
            protocol,
            peer,
            policy,
            Duration::from_millis(50),
        )
    }
    #[test]
    fn session_observers_do_not_wait_for_an_in_flight_control() {
        use std::sync::mpsc;
        let sessions = Arc::new(Sessions::default());
        let lease = claim(
            &sessions,
            ProtocolId::AIRPLAY,
            "127.0.0.1".parse().unwrap(),
            ReconnectPolicy::Reject,
        )
        .unwrap();
        let (entered, ready) = mpsc::channel();
        let (finish, wait) = mpsc::channel();
        let controls = sessions.clone();
        let control = std::thread::spawn(move || {
            controls.with_protocol_control(ProtocolId::AIRPLAY, || {
                entered.send(()).unwrap();
                wait.recv().unwrap();
            });
        });
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let (reply, result) = mpsc::channel();
        let status = sessions.clone();
        let observer = std::thread::spawn(move || {
            reply
                .send((status.owner_id(), status.active_protocol()))
                .unwrap()
        });
        let observed = result.recv_timeout(Duration::from_secs(1));
        finish.send(()).unwrap();
        control.join().unwrap();
        observer.join().unwrap();
        assert_eq!(
            observed.ok(),
            Some((Some(lease.id), Some(ProtocolId::AIRPLAY)))
        );
        sessions.release(&lease, || {});
        assert_eq!(sessions.owner_id(), None);
        assert_eq!(sessions.active_protocol(), None);
    }
    #[test]
    fn session_observers_stay_available_until_cleanup_finishes() {
        use std::sync::mpsc;
        let sessions = Arc::new(Sessions::default());
        let lease = claim(
            &sessions,
            ProtocolId::AIRPLAY,
            "127.0.0.1".parse().unwrap(),
            ReconnectPolicy::Reject,
        )
        .unwrap();
        let (entered, ready) = mpsc::channel();
        let (finish, wait) = mpsc::channel();
        let cleanup_sessions = sessions.clone();
        let old = lease.clone();
        let cleanup = std::thread::spawn(move || {
            cleanup_sessions.release(&old, || {
                entered.send(()).unwrap();
                wait.recv().unwrap();
            });
        });
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let (reply, result) = mpsc::channel();
        let status = sessions.clone();
        let observer = std::thread::spawn(move || {
            reply
                .send((status.owner_id(), status.active_protocol()))
                .unwrap()
        });
        let observed = result.recv_timeout(Duration::from_secs(1));
        assert!(!*lease.done.lock().unwrap());
        finish.send(()).unwrap();
        cleanup.join().unwrap();
        observer.join().unwrap();
        assert_eq!(
            observed.ok(),
            Some((Some(lease.id), Some(ProtocolId::AIRPLAY)))
        );
        assert!(*lease.done.lock().unwrap());
        assert_eq!(sessions.owner_id(), None);
        assert_eq!(sessions.active_protocol(), None);
    }
    #[test]
    fn auxiliary_controls_and_new_protocol_claims_cannot_cross_in_flight() {
        use std::sync::mpsc;
        let sessions = Arc::new(Sessions::default());
        let (entered, ready) = mpsc::channel();
        let (release, continue_control) = mpsc::channel();
        let control_sessions = sessions.clone();
        let control = std::thread::spawn(move || {
            control_sessions
                .with_protocol_control(ProtocolId::AIRPLAY, || {
                    entered.send(()).unwrap();
                    continue_control
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                })
                .unwrap();
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let claim_sessions = sessions.clone();
        let (started, attempting) = mpsc::channel();
        let (granted, result) = mpsc::channel();
        let claimant = std::thread::spawn(move || {
            started.send(()).unwrap();
            granted
                .send(claim(
                    &claim_sessions,
                    ProtocolId("test-cast"),
                    "127.0.0.1".parse().unwrap(),
                    ReconnectPolicy::Reject,
                ))
                .unwrap();
        });
        attempting.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release.send(()).unwrap();
        control.join().unwrap();
        let lease = result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        claimant.join().unwrap();
        assert!(
            sessions
                .with_protocol_control(ProtocolId::AIRPLAY, || panic!(
                    "Foreign playback must not be changed"
                ))
                .is_none()
        );
        sessions.release(&lease, || {});
    }
    #[test]
    fn protocols_share_ownership_but_cannot_steal_each_others_same_peer() {
        let sessions = Sessions::default();
        let peer = "127.0.0.1".parse().unwrap();
        let other = ProtocolId("test-cast");
        let first = claim(
            &sessions,
            ProtocolId::AIRPLAY,
            peer,
            ReconnectPolicy::Reject,
        )
        .unwrap();
        assert!(claim(&sessions, other, peer, ReconnectPolicy::ReplaceSamePeer).is_none());
        assert!(
            claim(
                &sessions,
                ProtocolId::AIRPLAY,
                "127.0.0.2".parse().unwrap(),
                ReconnectPolicy::ReplaceSamePeer
            )
            .is_none()
        );
        assert!(
            claim(
                &sessions,
                ProtocolId::AIRPLAY,
                peer,
                ReconnectPolicy::Reject
            )
            .is_none()
        );
        assert!(!first.stop.load(Ordering::Acquire));
        sessions.release(&first, || {});
        let next = claim(&sessions, other, peer, ReconnectPolicy::Reject).unwrap();
        assert_ne!(first.id, next.id);
        sessions.begin_video(next.id);
        sessions.release(&first, || {
            panic!("Stale teardown must not reset a new owner")
        });
        assert_eq!(sessions.owner_id(), Some(next.id));
        assert_eq!(sessions.active_protocol(), Some(other));
        assert_eq!(sessions.video_owner.load(Ordering::Acquire), next.id);
        sessions.cancel();
        assert!(next.stop.load(Ordering::Acquire));
        sessions.release(&next, || {});
    }
    #[test]
    fn reconnect_waits_for_workers_and_cleanup_and_keeps_ids_unique() {
        let sessions = Arc::new(Sessions::default());
        let peer = "127.0.0.1".parse().unwrap();
        let first = claim(
            &sessions,
            ProtocolId::AIRPLAY,
            peer,
            ReconnectPolicy::Reject,
        )
        .unwrap();
        let worker = sessions.clone();
        let old = first.clone();
        let teardown = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !old.stop.load(Ordering::Acquire) {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
            worker.release(&old, || {
                worker.advance();
            });
        });
        let next = sessions
            .claim(
                sessions.allocate_id(),
                ProtocolId::AIRPLAY,
                peer,
                ReconnectPolicy::ReplaceSamePeer,
                Duration::from_secs(2),
            )
            .unwrap();
        teardown.join().unwrap();
        assert_eq!(sessions.generation(), 2);
        assert_ne!(next.id, first.id);
        assert!(*first.done.lock().unwrap());
        sessions.release(&next, || {});
    }
    #[test]
    fn blocked_reconnect_is_busy_and_shutdown_cannot_grant_ownership() {
        let sessions = Sessions::default();
        let peer = "127.0.0.1".parse().unwrap();
        let first = claim(
            &sessions,
            ProtocolId::AIRPLAY,
            peer,
            ReconnectPolicy::Reject,
        )
        .unwrap();
        assert!(
            sessions
                .claim(
                    sessions.allocate_id(),
                    ProtocolId::AIRPLAY,
                    peer,
                    ReconnectPolicy::ReplaceSamePeer,
                    Duration::ZERO
                )
                .is_none()
        );
        assert!(first.stop.load(Ordering::Acquire));
        sessions.release(&first, || {});
        sessions.running.store(false, Ordering::Release);
        assert!(
            claim(
                &sessions,
                ProtocolId::AIRPLAY,
                peer,
                ReconnectPolicy::Reject
            )
            .is_none()
        );
    }
    #[test]
    fn flush_and_old_teardown_cannot_end_replacement_video() {
        let sessions = Sessions::default();
        sessions.begin_video(2);
        let serial = sessions.video_serial.load(Ordering::Acquire);
        sessions.advance();
        sessions.begin_video(2);
        sessions.end_video(1);
        assert_eq!(sessions.video_owner.load(Ordering::Acquire), 2);
        assert_eq!(sessions.video_serial.load(Ordering::Acquire), serial);
        sessions.end_video(2);
        assert_eq!(sessions.video_owner.load(Ordering::Acquire), 0);
        sessions.begin_video(2);
        assert!(sessions.video_serial.load(Ordering::Acquire) > serial);
    }
    #[test]
    fn idle_inhibition_follows_video_not_control_or_frame_generations() {
        let sessions = Sessions::default();
        assert!(!sessions.needs_video_inhibition(|| true));
        sessions.begin_video(1);
        assert!(sessions.needs_video_inhibition(|| panic!("Mirror must not consult HLS")));
        sessions.advance(); // Audio FLUSH/seek must not release mirror protection.
        assert!(sessions.needs_video_inhibition(|| false));
        sessions.begin_video(2);
        sessions.end_video(1);
        assert!(sessions.needs_video_inhibition(|| false));
        sessions.end_video(2);
        assert!(!sessions.needs_video_inhibition(|| true));
        sessions.begin_timed_video(2);
        assert!(sessions.needs_video_inhibition(|| true));
        assert!(!sessions.needs_video_inhibition(|| false)); // Pause, EOF or failure.
        sessions.begin_video(2); // Same control connection switches back to mirror.
        assert!(sessions.needs_video_inhibition(|| false));
    }
}
