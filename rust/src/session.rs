// SPDX-License-Identifier: MPL-2.0
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub struct SessionOwner {
    pub id: u64,
    pub peer: std::net::IpAddr,
    pub stop: AtomicBool,
    pub done: Mutex<bool>,
    pub ready: Condvar,
}

/// Network ownership and cancellation, independent of any desktop window.
pub struct Sessions {
    pub owner: Mutex<Option<Arc<SessionOwner>>>,
    pub running: AtomicBool,
    pub disconnect: AtomicU64,
    /// Protocol video lifetime, independent of FLUSH/seek frame generations.
    pub video_owner: AtomicU64,
    pub video_serial: AtomicU64,
    pub video_started: Mutex<std::time::Instant>,
    pub epoch: Arc<AtomicU64>,
}
impl Default for Sessions {
    fn default() -> Self {
        Self {
            owner: Mutex::new(None),
            running: AtomicBool::new(true),
            disconnect: AtomicU64::new(0),
            video_owner: AtomicU64::new(0),
            video_serial: AtomicU64::new(0),
            video_started: Mutex::new(std::time::Instant::now()),
            epoch: Arc::new(AtomicU64::new(1)),
        }
    }
}
impl Sessions {
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
        if self.video_owner.load(Ordering::Acquire) == session {
            return; // Repeated SETUP/format change in the same live presentation.
        }
        *self.video_started.lock().unwrap() = std::time::Instant::now();
        self.video_serial.fetch_add(1, Ordering::AcqRel);
        self.video_owner.store(session, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
