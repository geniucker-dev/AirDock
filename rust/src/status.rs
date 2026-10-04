// SPDX-License-Identifier: MPL-2.0
use std::time::Instant;
use std::{
    ops::{Deref, DerefMut},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};
#[derive(Clone, Default)]
pub struct UiState {
    pub peer: String,
    pub device: String,
    pub model: String,
    pub kind: String,
    pub codec: String,
    pub dimensions: String,
    pub decoder: String,
    pub audio_status: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Arc<Vec<u8>>,
    pub paused: bool,
    pub volume_db: f32,
    pub progress_seconds: f64,
    pub duration_seconds: f64,
    pub progress_at: Option<Instant>,
    pub error: String,
    pub usb: bool,
    pub addresses: String,
}
/// Status-only events; no commands or media bytes travel on this channel.
#[derive(Default)]
pub struct Status {
    inner: Mutex<UiState>,
    pub revision: AtomicU64,
    pub visible: std::sync::atomic::AtomicBool,
    pub wake: Mutex<Option<futures::channel::mpsc::Sender<()>>>,
}
pub struct Guard<'a> {
    guard: MutexGuard<'a, UiState>,
    owner: &'a Status,
    changed: bool,
}
impl Status {
    pub fn lock(&self) -> std::sync::LockResult<Guard<'_>> {
        match self.inner.lock() {
            Ok(guard) => Ok(Guard {
                guard,
                owner: self,
                changed: false,
            }),
            Err(e) => Err(std::sync::PoisonError::new(Guard {
                guard: e.into_inner(),
                owner: self,
                changed: false,
            })),
        }
    }
}
impl Deref for Guard<'_> {
    type Target = UiState;
    fn deref(&self) -> &UiState {
        &self.guard
    }
}
impl DerefMut for Guard<'_> {
    fn deref_mut(&mut self) -> &mut UiState {
        self.changed = true;
        &mut self.guard
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if self.changed {
            self.owner.revision.fetch_add(1, Ordering::Release);
            if self.owner.visible.load(Ordering::Relaxed)
                && let Some(wake) = self.owner.wake.lock().unwrap().as_mut()
            {
                let _ = wake.try_send(());
            }
        }
    }
}
