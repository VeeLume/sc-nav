//! The overlay feeds: the game window's overlay corner read about once a
//! second, with recording, and recorded grabs read back.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use image::RgbaImage;
use sc_nav_geo::Body;
use sc_nav_overlay::stream::{Fix, GrabLog, Reader, region};

use crate::window::{CaptureError, Session};

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

/// How long to wait before looking for the game window again.
const RETRY: Duration = Duration::from_secs(5);

/// Read the game's overlay until `on_fix` breaks. `ttf` is
/// `Engine\Fonts\VeraMono.ttf`.
///
/// The game window is captured with a kept [`Session`] and only the overlay's
/// corner ([`region`]) is copied off the GPU: under a millisecond a grab,
/// where grabbing the whole screen took ~18 ms of CPU. The frame read is the
/// next one the game shows after the grab starts (the session waits for it),
/// not one left over from the grab before. Windows in front of
/// the game (sc-nav's own HUD included) don't get in the way. Without the
/// game (not running, minimised) the loop looks again every few seconds.
pub fn watch(
    ttf: &[u8],
    bodies: &[Body],
    settings: impl Fn() -> Settings,
    mut on_fix: impl FnMut(Fix) -> ControlFlow<()>,
) -> Result<()> {
    let mut reader = Reader::new(ttf).map_err(|e| anyhow!(e))?;
    let mut session: Option<Session> = None;
    let mut looked: Option<Instant> = None;
    let mut rec: Option<Recorder> = None;
    let mut n = 0;
    loop {
        let at = Instant::now();
        let set = settings();
        if !set.enabled {
            // off: nothing captured, the session closed
            session = None;
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        if session.is_none() && looked.is_none_or(|t| t.elapsed() >= RETRY) {
            looked = Some(Instant::now());
            session = Session::open().ok();
        }
        let Some(s) = session.as_mut() else {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        };
        if rec.as_ref().map(|r| &r.dir) != set.record.as_ref() {
            rec = set.record.as_deref().map(Recorder::new).transpose()?;
            n = 0;
        }
        let unix_ms = sc_nav_clock::now_ms();
        let crop = match grab(s) {
            Ok(c) => c,
            Err(_) => {
                // closed or minimised: look again later
                session = None;
                continue;
            }
        };
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

/// The overlay's corner of the newest frame.
fn grab(s: &mut Session) -> Result<RgbaImage, CaptureError> {
    let (w, h) = s.newest_size()?;
    let (x, y, rw, rh) = region(w, h);
    Ok(s.region(x, y, rw, rh)?.image)
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
