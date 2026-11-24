//! Capture on Windows:
//! - [`window`]: one frame of the game window (Windows Graphics Capture),
//!   on demand. Always there.
//! - With the `overlay` feature (default): the overlay region of the
//!   primary screen about once a second ([`watch`], with optional recording
//!   of every grab), reading recorded grabs back ([`grab_log`],
//!   [`grab_image`]), and `/showlocation` from the clipboard ([`clipboard`]).

pub mod window;

pub use window::{CaptureError, capture_game};

#[cfg(all(windows, feature = "overlay"))]
pub mod clipboard;

#[cfg(feature = "overlay")]
mod feeds;

#[cfg(feature = "overlay")]
pub use feeds::{Settings, grab_image, grab_log, watch};
