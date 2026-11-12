//! What the game files say about the world navigation runs in: bodies,
//! systems and ships' braking. Filled by `sc-nav-data` from the install; plain data here,
//! so the pure layers don't need the game files.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Body;

/// A ship's flight figures with its stock loadout.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Ship {
    pub mass_kg: f64,
    /// Acceleration in g, main thrusters.
    pub forward_g: f64,
    /// Acceleration in g, retro thrusters: the braking figure.
    pub backward_g: f64,
    pub backward_boost_g: f64,
    /// m/s.
    pub scm_speed: f32,
    pub max_speed: f32,
}

/// A star system: its id (as in the object containers, `stanton`) and
/// where the star map puts it in the galaxy (star-map units; only good for
/// telling systems apart, not for distances).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct System {
    pub id: String,
    pub galactic: Option<[f64; 3]>,
}

impl System {
    /// The three-letter tag nav codes carry: `STA`, `PYR`, `NYX`.
    pub fn tag(&self) -> String {
        self.id.chars().take(3).collect::<String>().to_ascii_uppercase()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct World {
    /// The game build this was read from, e.g. `LIVE 4.10.2`.
    pub build: String,
    pub bodies: Vec<Body>,
    /// By entity class name (telemetry's `class_name`).
    pub ships: BTreeMap<String, Ship>,
    #[serde(default)]
    pub systems: Vec<System>,
}

impl World {
    pub fn body(&self, id: &str) -> Option<&Body> {
        self.bodies.iter().find(|b| b.id == id)
    }

    /// Set each body's phase from measured values (degrees), keeping the
    /// others.
    pub fn with_phases(mut self, phases: &BTreeMap<String, f64>) -> Self {
        for b in &mut self.bodies {
            if let Some(p) = phases.get(&b.id) {
                b.phase_deg = Some(*p);
            }
        }
        self
    }
}
