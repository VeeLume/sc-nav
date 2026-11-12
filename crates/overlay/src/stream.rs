//! From single readings to fixes: the continuity [`Gate`], the step-by-step
//! [`Reader`] (reader + learned exemplars + gate), and what one grab gave
//! ([`GrabLog`], the line a recording keeps per grab). No capture here: the
//! caller hands in each screenshot of the overlay [`region`].

use std::time::Instant;

use image::RgbaImage;
use sc_nav_geo::{Body, V3};
use serde::{Deserialize, Serialize};

use crate::read::{Font, Matcher, Outcome, Reading, Source, luminance};

/// Faster than this between two fixes, the second waits for confirmation.
pub const MAX_SPEED: f64 = 1500.0;

/// One accepted fix.
#[derive(Debug, Clone)]
pub struct Fix {
    /// When the screen was grabbed.
    pub at: Instant,
    pub unix_ms: i64,
    /// The body, by the printed radius (or Root when radii tie).
    pub body: String,
    /// The position in that body's game frame: what navigation uses.
    pub cam: V3,
    /// System-global, metres, when it was read and cross-checked: only
    /// calibration needs it.
    pub root: Option<V3>,
    pub reading: Reading,
}

/// One line of `grabs.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrabLog {
    pub n: u64,
    pub unix_ms: i64,
    /// `accepted`, `unreadable`, `no solution`, `ambiguous`.
    pub outcome: String,
    pub cam: Option<V3>,
    pub root: Option<V3>,
    /// The body, when the reading named one.
    #[serde(default)]
    pub body: Option<String>,
    /// `campos` or `zone`.
    #[serde(default)]
    pub source: Option<String>,
    /// `r_DisplayInfo 2`'s `CamDir` numbers as printed.
    #[serde(default)]
    pub camdir: Option<[i32; 3]>,
    /// `Current player location`, read by name: a body id or `system:<id>`.
    #[serde(default)]
    pub place: Option<String>,
    /// Let through by the [`Gate`]: sent on as a fix.
    pub admitted: bool,
}

/// The continuity gate between accepted readings, in the body's frame.
#[derive(Default)]
pub struct Gate {
    last: Option<(i64, String, V3)>,
    pending: Option<(i64, String, V3)>,
}

impl Gate {
    /// Whether a reading at `unix_ms` may pass as a fix. A new body passes
    /// at once (a frame change is not a jump).
    pub fn admit(&mut self, unix_ms: i64, body: &str, p: V3) -> bool {
        let fits = |prev: &Option<(i64, String, V3)>| {
            prev.as_ref().is_some_and(|(t0, b0, p0)| {
                b0 != body || dist(*p0, p) / ((unix_ms - t0) as f64 / 1000.0).max(0.5) < MAX_SPEED
            })
        };
        if self.last.is_none() || fits(&self.last) || fits(&self.pending) {
            self.last = Some((unix_ms, body.to_string(), p));
            self.pending = None;
            true
        } else {
            self.pending = Some((unix_ms, body.to_string(), p));
            false
        }
    }
}

/// The overlay's region: the right 30% of the width, the top 30% of the
/// height. Fits `r_DisplayInfo 1` and the taller `r_DisplayInfo 2` (its
/// block ends near 26%, its longest line starts near 76% across).
pub fn region(w: u32, h: u32) -> (u32, u32, u32, u32) {
    let x = w * 7 / 10;
    (x, 0, w - x, h * 3 / 10)
}

/// The reader with its font, learning as it goes.
pub fn matcher(ttf: &[u8]) -> Result<Matcher, String> {
    let bytes = ttf;
    let font = Font::render(bytes, 11.0, "0123456789.-", 0, 4)?;
    let text = Font::render(bytes, 11.0, crate::read::TEXT_CHARS, 0, 4)?;
    Ok(Matcher::new(font).with_text(text))
}

/// The reader, its learned exemplars and the gate: one grab at a time.
pub struct Reader {
    m: Matcher,
    gate: Gate,
}

impl Reader {
    pub fn new(ttf: &[u8]) -> Result<Self, String> {
        Ok(Self {
            m: matcher(ttf)?,
            gate: Gate::default(),
        })
    }

    /// Read one grab: its log line, and the fix if the gate lets it through.
    pub fn read(
        &mut self,
        crop: &RgbaImage,
        bodies: &[Body],
        n: u64,
        at: Instant,
        unix_ms: i64,
    ) -> (GrabLog, Option<Fix>) {
        let outcome = self.m.read(&luminance(crop), 0.0, bodies);
        let mut log = GrabLog {
            n,
            unix_ms,
            outcome: outcome.label().into(),
            cam: None,
            root: None,
            body: None,
            source: None,
            camdir: None,
            place: None,
            admitted: false,
        };
        let Outcome::Accepted(r) = outcome else {
            return (log, None);
        };
        log.camdir = r.camdir;
        log.place = r.place.as_ref().map(|p| match p {
            crate::read::Place::Body(b) => b.clone(),
            crate::read::Place::System(s) => format!("system:{s}"),
        });
        self.m.learn_from(&r);
        log.cam = Some(r.cam);
        log.root = r.root;
        log.body = r.body.clone();
        log.source = Some(match r.source {
            Source::CamPos => "campos".into(),
            Source::Zone => "zone".into(),
        });
        let Some(body) = r.body.clone() else {
            return (log, None);
        };
        log.admitted = self.gate.admit(unix_ms, &body, r.cam);
        let fix = log.admitted.then_some(Fix {
            at,
            unix_ms,
            body,
            cam: r.cam,
            root: r.root,
            reading: r,
        });
        (log, fix)
    }
}

fn dist(a: V3, b: V3) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
