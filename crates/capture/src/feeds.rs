//! The overlay feeds: the screen region read about once a second, with
//! recording, and recorded grabs read back.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use image::{RgbaImage, imageops};
use sc_nav_geo::Body;
use sc_nav_overlay::stream::{Fix, GrabLog, Reader, region};
use xcap::Monitor;

struct Recorder {
    dir: PathBuf,
    log: File,
}

impl Recorder {
    fn new(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let log = File::options()
            .create(true)
            .append(true)
            .open(dir.join("grabs.jsonl"))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            log,
        })
    }

    fn save(&mut self, crop: &RgbaImage, line: &GrabLog) {
        let _ = crop.save(self.dir.join(format!("grab-{:05}.png", line.n)));
        if let Ok(s) = serde_json::to_string(line) {
            let _ = writeln!(self.log, "{s}");
        }
    }
}

/// What [`watch`] does on each grab, asked anew every time.
pub struct Settings {
    /// Read the overlay at all (off: the loop idles).
    pub enabled: bool,
    /// Save every grab and its outcome here.
    pub record: Option<PathBuf>,
}

/// Read the screen until `on_fix` breaks. `ttf` is `Engine\Fonts\VeraMono.ttf`.
pub fn watch(
    ttf: &[u8],
    bodies: &[Body],
    settings: impl Fn() -> Settings,
    mut on_fix: impl FnMut(Fix) -> ControlFlow<()>,
) -> Result<()> {
    let mut reader = Reader::new(ttf).map_err(|e| anyhow!(e))?;
    let monitors = Monitor::all()?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or(monitors.first())
        .ok_or_else(|| anyhow!("no monitor"))?;
    let mut rec: Option<Recorder> = None;
    let mut n = 0;
    loop {
        let at = Instant::now();
        let set = settings();
        if !set.enabled {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        if rec.as_ref().map(|r| &r.dir) != set.record.as_ref() {
            rec = set.record.as_deref().map(Recorder::new).transpose()?;
            n = 0;
        }
        let unix_ms = sc_nav_clock::now_ms();
        let screen = monitor.capture_image()?;
        let (x, y, w, h) = region(screen.width(), screen.height());
        let crop = imageops::crop_imm(&screen, x, y, w, h).to_image();
        let (log, fix) = reader.read(&crop, bodies, n, at, unix_ms);
        n += 1;
        if let Some(r) = rec.as_mut() {
            r.save(&crop, &log);
        }
        if let Some(fix) = fix
            && on_fix(fix).is_break()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1).saturating_sub(at.elapsed()));
    }
}

/// A recorded session's `grabs.jsonl`, in order.
pub fn grab_log(dir: &Path) -> Result<Vec<GrabLog>> {
    let lines = BufReader::new(File::open(dir.join("grabs.jsonl")).context("no grabs.jsonl")?);
    let mut out = vec![];
    for line in lines.lines() {
        if let Ok(g) = serde_json::from_str(&line?) {
            out.push(g);
        }
    }
    Ok(out)
}

/// A recorded grab's image.
pub fn grab_image(dir: &Path, n: u64) -> Option<RgbaImage> {
    image::open(dir.join(format!("grab-{n:05}.png")))
        .ok()
        .map(|i| i.to_rgba8())
}
