// SPDX-License-Identifier: MPL-2.0
use std::time::{Duration, Instant};

/// One reveal per video presentation; FLUSH/seek keep its presentation token.
#[derive(Default)]
pub(super) struct ConnectionReveal {
    seen: Option<u64>,
    automatic: Option<u64>,
    ended_at: Option<Instant>,
}
impl ConnectionReveal {
    pub fn video(&mut self, session: Option<u64>, has_frame: bool, hidden: bool) -> bool {
        let Some(session) = session.filter(|_| has_frame) else {
            return false;
        };
        if self.seen == Some(session) {
            return false;
        }
        self.seen = Some(session);
        hidden
    }
    pub fn automatically_opened(&mut self, session: Option<u64>) {
        self.automatic = session;
        self.ended_at = None;
    }
    pub fn is_automatic(&self) -> bool {
        self.automatic.is_some()
    }
    pub fn keep_open(&mut self) {
        self.automatic = None;
        self.ended_at = None;
    }
    pub fn dismiss(&mut self, session: Option<u64>) {
        if session.is_some() {
            self.seen = session;
        }
        self.keep_open();
    }
    pub fn return_to_tray(&mut self, session: Option<u64>, now: Instant) -> bool {
        if self.automatic.is_none() {
            return false;
        }
        if let Some(session) = session {
            // A replacement owner cancels the grace period and inherits the
            // presentation. An old owner's cleanup cannot close its window.
            self.automatic = Some(session);
            self.ended_at = None;
            return false;
        }
        let ended = *self.ended_at.get_or_insert(now);
        if now.saturating_duration_since(ended) < Duration::from_millis(750) {
            return false;
        }
        self.keep_open();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_automatic_session_returns_after_grace() {
        let mut reveal = ConnectionReveal::default();
        let now = Instant::now();
        reveal.automatically_opened(Some(1));
        assert!(!reveal.return_to_tray(Some(1), now));
        assert!(!reveal.return_to_tray(None, now));
        assert!(!reveal.return_to_tray(None, now + Duration::from_millis(749)));
        assert!(reveal.return_to_tray(None, now + Duration::from_millis(750)));
        assert!(!reveal.return_to_tray(None, now + Duration::from_secs(2)));
    }

    #[test]
    fn replacement_session_and_manual_use_cancel_return() {
        let mut reveal = ConnectionReveal::default();
        let now = Instant::now();
        reveal.automatically_opened(Some(1));
        assert!(!reveal.return_to_tray(None, now));
        assert!(!reveal.return_to_tray(Some(2), now + Duration::from_secs(1)));
        assert!(!reveal.return_to_tray(Some(2), now + Duration::from_secs(3)));
        assert!(!reveal.return_to_tray(None, now + Duration::from_secs(4)));
        assert!(reveal.return_to_tray(None, now + Duration::from_secs(5)));
        reveal.automatically_opened(Some(3));
        reveal.keep_open();
        assert!(!reveal.return_to_tray(None, now));
        assert!(!reveal.return_to_tray(None, now + Duration::from_secs(4)));
    }
}
