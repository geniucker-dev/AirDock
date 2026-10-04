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
    pub epoch: Arc<AtomicU64>,
}
impl Default for Sessions {
    fn default() -> Self {
        Self {
            owner: Mutex::new(None),
            running: AtomicBool::new(true),
            disconnect: AtomicU64::new(0),
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
}
