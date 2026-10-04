//! OS integration is isolated from protocol and media workers.
#[cfg(windows)]
pub use crate::integration::tray;
pub use crate::integration::{autostart, open, usb_present};
