// SPDX-License-Identifier: MPL-2.0
//! Thread-owned idle protection, without simulated input or power setting changes.
use anyhow::Result;
use std::{marker::PhantomData, rc::Rc};

pub(crate) trait Backend {
    fn supported(&self) -> bool;
    fn set(&mut self, active: bool) -> Result<()>;
}
#[derive(Default)]
pub(crate) struct Native {
    #[cfg(windows)]
    previous: Option<windows::Win32::System::Power::EXECUTION_STATE>,
}
impl Backend for Native {
    fn supported(&self) -> bool {
        cfg!(windows)
    }
    fn set(&mut self, active: bool) -> Result<()> {
        #[cfg(windows)]
        {
            use windows::Win32::System::Power::{
                ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
            };
            let flags = if active {
                ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED
            } else {
                self.previous.unwrap_or(ES_CONTINUOUS) | ES_CONTINUOUS
            };
            let previous = unsafe { SetThreadExecutionState(flags) };
            anyhow::ensure!(previous.0 != 0, "SetThreadExecutionState was rejected");
            if active {
                self.previous.get_or_insert(previous);
            } else {
                self.previous = None;
            }
        }
        #[cfg(not(windows))]
        let _ = active;
        Ok(())
    }
}
/// !Send/!Sync: Windows execution state must be restored on the owning thread.
#[derive(Default)]
pub(crate) struct Inhibitor<B: Backend = Native> {
    backend: B,
    active: bool,
    _thread_bound: PhantomData<Rc<()>>,
}
impl<B: Backend> Inhibitor<B> {
    pub fn update(&mut self, wanted: bool) -> Result<()> {
        let wanted = wanted && self.backend.supported();
        if self.active != wanted {
            self.backend.set(wanted)?;
            self.active = wanted;
        }
        Ok(())
    }
    pub fn active(&self) -> bool {
        self.active
    }
}
impl<B: Backend> Drop for Inhibitor<B> {
    fn drop(&mut self) {
        if self.active
            && let Err(error) = self.backend.set(false)
        {
            // Windows also clears the request when the owning thread exits.
            tracing::warn!("Could not restore playback idle protection: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    struct Fake {
        calls: Rc<RefCell<Vec<bool>>>,
        fail: Rc<Cell<bool>>,
    }
    impl Backend for Fake {
        fn supported(&self) -> bool {
            true
        }
        fn set(&mut self, active: bool) -> Result<()> {
            self.calls.borrow_mut().push(active);
            anyhow::ensure!(!self.fail.replace(false), "Rejected test request");
            Ok(())
        }
    }
    type FakeState = (Inhibitor<Fake>, Rc<RefCell<Vec<bool>>>, Rc<Cell<bool>>);
    fn fake() -> FakeState {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let fail = Rc::new(Cell::new(false));
        (
            Inhibitor {
                backend: Fake {
                    calls: calls.clone(),
                    fail: fail.clone(),
                },
                active: false,
                _thread_bound: PhantomData,
            },
            calls,
            fail,
        )
    }
    #[test]
    fn transitions_only_and_exit_restores_without_per_frame_requests() {
        let (mut guard, calls, _) = fake();
        guard.update(false).unwrap();
        guard.update(true).unwrap();
        for _ in 0..1000 {
            guard.update(true).unwrap();
        }
        guard.update(false).unwrap();
        guard.update(true).unwrap();
        drop(guard);
        assert_eq!(*calls.borrow(), [true, false, true, false]);
    }
    #[test]
    fn rejected_requests_do_not_claim_success_and_are_retried() {
        let (mut guard, calls, fail) = fake();
        fail.set(true);
        assert!(guard.update(true).is_err());
        assert!(!guard.active());
        guard.update(true).unwrap();
        fail.set(true);
        assert!(guard.update(false).is_err());
        assert!(guard.active());
        guard.update(false).unwrap();
        drop(guard);
        assert_eq!(*calls.borrow(), [true, true, false, false]);
    }
    #[cfg(windows)]
    #[test]
    fn actual_windows_execution_state_is_set_and_restored_on_the_owning_thread() {
        use windows::Win32::System::Power::{
            ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
        };
        std::thread::spawn(|| {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            let mut guard: Inhibitor = Inhibitor::default();
            guard.update(true).unwrap();
            let flags = ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED;
            assert_eq!(unsafe { SetThreadExecutionState(flags) }.0, flags.0);
            guard.update(false).unwrap();
            assert_eq!(
                unsafe { SetThreadExecutionState(ES_CONTINUOUS) }.0,
                ES_CONTINUOUS.0
            );
            unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            guard.update(true).unwrap();
            drop(guard);
            assert_eq!(
                unsafe { SetThreadExecutionState(ES_CONTINUOUS) }.0,
                (ES_CONTINUOUS | ES_SYSTEM_REQUIRED).0
            );
        })
        .join()
        .unwrap();
    }
}
