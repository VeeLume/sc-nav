//! Reads Star Citizen's `r_DisplayInfo` overlay (modes 1 and 2) from a
//! screenshot of its region, without OCR.
//!
//! - [`read`]: one image to one [`read::Outcome`]. Lines are found by the ink
//!   over its drop shadow; numbers are placed glyph by glyph against templates
//!   rendered from the overlay's own font (`VeraMono.ttf` at 11 px) and fixed
//!   by a checksum search: `|CamPos| − radius = elevation + altitude`. The
//!   system position (`Root`) must agree with it; the view (`CamDir`) and the
//!   location name come along.
//! - [`stream`]: readings to fixes over time, with a continuity gate and
//!   learning from readings that fit at no cost.
//!
//! The font bytes come from the caller (`sc-nav-data` reads them from the game
//! install).

pub mod read;
pub mod stream;

pub use read::{Matcher, Outcome, Place, Reading, Source};
pub use stream::{Fix, Gate, GrabLog, Reader};
