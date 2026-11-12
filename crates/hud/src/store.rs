//! Where the app keeps things: one folder, `%LOCALAPPDATA%\sc-nav` (or
//! `$XDG_DATA_HOME/sc-nav`, `~/.local/share/sc-nav`; `SC_NAV_HOME` wins).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sc_nav_geo::Location;

#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn locate() -> Self {
        let root = std::env::var_os("SC_NAV_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("sc-nav")))
            .or_else(|| std::env::var_os("XDG_DATA_HOME").map(|d| PathBuf::from(d).join("sc-nav")))
            .or_else(|| std::env::var_os("HOME").map(|d| PathBuf::from(d).join(".local/share/sc-nav")))
            .unwrap_or_else(|| PathBuf::from("sc-nav-data"));
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }

    /// Game data per build (`sc-nav-data`).
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn marks(&self) -> PathBuf {
        self.root.join("marks.json")
    }

    /// The app's own settings ([`Prefs`]).
    pub fn prefs(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// Measured phases per body, refined by every overlay fix.
    pub fn phases(&self) -> PathBuf {
        self.root.join("phases.json")
    }
}

/// The lab's marks file (`waypoints.json`), if it is on this machine.
pub const LAB_MARKS: &str = r"E:\vscode\sc-grpc\.scratch\nav-data\waypoints.json";

/// Marks from a file in the same shape (`{name: location}`).
pub fn read_marks(path: &Path) -> Result<BTreeMap<String, Location>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// What the user sets in the app. Unknown or missing fields fall back to
/// the defaults, so old files keep loading.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The debug panel beside the HUD: what the game already shows and
    /// what only helps finding a fault.
    pub debug: bool,
    /// The game's compass in a seat (the ship's HUD).
    pub ship: Compass,
    /// The game's compass on foot (the visor).
    pub foot: Compass,
    /// The game's vertical field of view (`FOV` on `r_DisplayInfo 2`'s
    /// `CamDir` line), for placing space targets on screen.
    pub fov_deg: f32,
    /// Draw a sample marker and rows while adjusting the layout.
    #[serde(skip)]
    pub preview: bool,
}

/// Where the game draws a compass: its tape's height and scale, as
/// fractions of the screen height, so they hold at any resolution.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Compass {
    /// The tape's line, from the top.
    pub y: f32,
    /// One degree of heading along the tape.
    pub per_deg: f32,
    /// Degrees the tape shows each side of the heading.
    pub span: f32,
    /// Name, distance and ETA above the tape (room there in a seat), the
    /// braking cue alone below it; otherwise everything below.
    #[serde(default)]
    pub above: bool,
}

impl Compass {
    /// Measured on a Constellation's HUD at 16:9.
    pub const SHIP: Compass = Compass { y: 0.364, per_deg: 0.00249, span: 38.0, above: true };
    /// Measured on the visor at 16:9.
    pub const FOOT: Compass = Compass { y: 0.031, per_deg: 0.00373, span: 40.0, above: false };
}

impl Default for Prefs {
    fn default() -> Self {
        Self { debug: false, ship: Compass::SHIP, foot: Compass::FOOT, fov_deg: 63.0, preview: false }
    }
}

impl Prefs {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, b);
        }
    }
}
