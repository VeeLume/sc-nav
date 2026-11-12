//! The convenient layer: one navigation state fed by everything the game
//! gives away.
//!
//! - [`Pilot`]: the fusion core. [`Pilot::feed`] takes an [`Input`] (a
//!   telemetry frame, an overlay fix, a `/showlocation`, a [`Cmd`], a flag)
//!   with its time; [`Pilot::snapshot`] returns plain data to show: position
//!   on a body and in space, the target with surface or space steering, the
//!   braking cue, the ground warning. No threads.
//! - [`Director`]: the navigation state underneath (marks, targets, phase
//!   calibration, the space track).
//! - `runtime` (feature): the live feeds on their own threads, recording and
//!   replay, all through the same [`Input`]s.

pub mod director;
pub mod pilot;
#[cfg(feature = "runtime")]
pub mod runtime;

/// The telemetry types the pilot takes (sc-dossier's `telemetry` feature).
pub use sc_dossier::telemetry;

pub use director::{Director, Here, load_phases};
pub use pilot::{Cmd, Input, Link, Message, OverlayFix, Pilot, Snapshot, pretty};
