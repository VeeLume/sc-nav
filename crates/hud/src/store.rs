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
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The HUD also shows what the game already shows and what only helps
    /// finding a fault.
    pub debug: bool,
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
