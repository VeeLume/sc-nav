//! Reads `CamPos Planet Zone` and the `Radius / ElevationUnderCam / Altitude`
//! line straight from pixels, with svc-glyph and a checksum.
//!
//! The format is fixed (six decimals), so svc-glyph places each glyph by a
//! grammar and keeps ranked candidates per digit. Among those candidates a
//! beam search looks for the reading that satisfies
//! `|cam| - radius = elevation + altitude` (exact to better than 0.00001 m on
//! verified readings). A frame is accepted only if exactly one position fits,
//! at a summed score loss of at most `Tuning::max_cost`. `Zone: Root` is then
//! read the same way (km, four decimals) and kept only if its z and horizontal
//! radius from the body's centre match `cam`'s.
//!
//! Templates start from the overlay's own font, `Engine\Fonts\VeraMono.ttf`,
//! rendered at 11 px: the overlay's digits are 8 px tall with 1 px strokes,
//! on an 8 px advance. Verified readings add labelled exemplars.

use std::cell::RefCell;
use std::collections::HashMap;

use sc_nav_glyph::{Grammar, Params, Position, Slot};
pub use sc_nav_glyph::{Gray, TemplateSet};

/// Rows read per line, from `MARGIN` above the line's top.
const BAND: usize = 13;
const MARGIN: usize = 2;
/// Rows a digit inks (cap height).
const INK_ROWS: usize = 8;
const DIGITS: &str = "0123456789";

/// The overlay's white text as 0..1: the minimum over RGB, so coloured lines
/// and the drop shadow go dark.
pub fn luminance(img: &image::RgbaImage) -> Gray {
    let (w, h) = img.dimensions();
    let px: Vec<u8> = img
        .pixels()
        .map(|p| p.0[0].min(p.0[1]).min(p.0[2]))
        .collect();
    Gray::from_u8(w as usize, h as usize, &px)
}

/// The top row of each text line's digits, from ink-over-shadow contrast
/// per row. Lines aren't on one grid: the `Zone` block sits apart.
pub fn line_tops(lum: &Gray) -> Vec<usize> {
    // ink over its own drop shadow (one px down and right)
    let energy: Vec<f32> = (0..lum.h)
        .map(|y| {
            if y + 1 >= lum.h {
                return 0.0;
            }
            (0..lum.w - 1)
                .map(|x| (lum.get(x, y) - lum.get(x + 1, y + 1)).max(0.0))
                .sum()
        })
        .collect();
    // Digits ink rows t..t+8 with sharp top and bottom bars, the rows just
    // above and below nearly empty.
    let edge: Vec<f32> = (0..lum.h)
        .map(|t| {
            if t < 1 || t + INK_ROWS >= energy.len() {
                return 0.0;
            }
            energy[t] + energy[t + INK_ROWS - 1] - 2.0 * (energy[t - 1] + energy[t + INK_ROWS])
        })
        .collect();
    let max = edge.iter().copied().fold(0.0, f32::max);
    let reach = INK_ROWS / 2 + 1;
    (MARGIN..lum.h)
        .filter(|&t| {
            let (lo, hi) = (t.saturating_sub(reach), (t + reach + 1).min(lum.h));
            edge[t] > 0.1 * max && edge[lo..hi].iter().all(|&e| e <= edge[t])
        })
        .collect()
}

/// One text line as coverage: (v - background) / (1 - background), the
/// background the 35th percentile of the ink rows within `BG_REACH` px
/// left and right (the sky behind the overlay changes along a line).
fn band(lum: &Gray, top: usize) -> Option<Gray> {
    const BG_REACH: usize = 24;
    let y0 = top.checked_sub(MARGIN)?;
    if y0 + BAND > lum.h {
        return None;
    }
    // per-column background, from coarse blocks
    let block = 8;
    let blocks = lum.w.div_ceil(block);
    let bgs: Vec<f32> = (0..blocks)
        .map(|b| {
            let c = b * block + block / 2;
            let (a, z) = (c.saturating_sub(BG_REACH), (c + BG_REACH).min(lum.w));
            let mut px: Vec<f32> = (top..top + INK_ROWS)
                .flat_map(|y| lum.v[y * lum.w + a..y * lum.w + z].iter().copied())
                .collect();
            let k = px.len() * 35 / 100;
            *px.select_nth_unstable_by(k, f32::total_cmp).1
        })
        .collect();
    let mut v = lum.v[y0 * lum.w..(y0 + BAND) * lum.w].to_vec();
    for (i, x) in v.iter_mut().enumerate() {
        let bg = bgs[(i % lum.w) / block];
        let span = (1.0 - bg).max(30.0 / 255.0);
        *x = ((*x - bg) / span).clamp(0.0, 1.0);
    }
    Some(Gray {
        w: lum.w,
        h: BAND,
        v,
    })
}

/// Ink column runs, neighbours a pixel apart merged into one glyph.
fn segments(cov: &Gray) -> Vec<(usize, usize)> {
    let ink = |x: usize| (MARGIN..MARGIN + INK_ROWS).any(|y| cov.get(x, y) > 0.45);
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for x in 0..=cov.w {
        let on = x < cov.w && ink(x);
        match (on, start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                match out.last_mut() {
                    Some(last) if s - last.1 <= 1 && x - last.0 <= 7 => last.1 = x,
                    _ => out.push((s, x)),
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// Words: runs of segments split at gaps of 7 px or more.
fn tokens(cov: &Gray) -> Vec<Vec<(usize, usize)>> {
    let mut out: Vec<Vec<(usize, usize)>> = vec![];
    for seg in segments(cov) {
        match out.last_mut() {
            Some(cur) if seg.0 - cur.last().unwrap().1 < 7 => cur.push(seg),
            _ => out.push(vec![seg]),
        }
    }
    out
}

fn crop(g: &Gray, x0: usize, x1: usize, y0: usize, y1: usize) -> Gray {
    let (w, h) = (x1 - x0, y1 - y0);
    let mut v = Vec::with_capacity(w * h);
    for y in y0..y1 {
        v.extend_from_slice(&g.v[y * g.w + x0..y * g.w + x1]);
    }
    Gray { w, h, v }
}

/// The overlay's glyphs rendered from its TTF.
pub struct Font {
    pub glyphs: Vec<(char, Gray)>,
}

impl Font {
    /// Render `chars` at `px` with `grow` extra hi-res pixels of weight (the
    /// overlay draws bolder than the plain font) at `phases` sub-pixel
    /// offsets each. All share one row range so `.` and `-` keep their height.
    pub fn render(
        ttf: &[u8],
        px: f32,
        chars: &str,
        grow: usize,
        phases: usize,
    ) -> Result<Self, String> {
        const S: usize = 8;
        let font = fontdue::Font::from_bytes(ttf, fontdue::FontSettings::default())
            .map_err(str::to_string)?;
        let hi = px * S as f32;
        let asc = font
            .horizontal_line_metrics(hi)
            .map(|m| m.ascent)
            .unwrap_or(hi);
        let (cw, ch) = (12 * S, 16 * S);
        let base = (asc.ceil() as usize + 2 * S).min(ch - 1);
        let mut canvases = vec![];
        for c in chars.chars() {
            let (m, bmp) = font.rasterize(c, hi);
            let mut g = Gray::new(cw, ch);
            for y in 0..m.height {
                for x in 0..m.width {
                    let gx = 2 * S as i64 + m.xmin as i64 + x as i64;
                    let gy = base as i64 - m.ymin as i64 - m.height as i64 + y as i64;
                    if gx >= 0 && gy >= 0 && (gx as usize) < cw && (gy as usize) < ch {
                        g.v[gy as usize * cw + gx as usize] = bmp[y * m.width + x] as f32 / 255.0;
                    }
                }
            }
            if grow > 0 {
                g = g.max_filter(2 * grow + 1);
            }
            canvases.push((c, g));
        }
        let mut glyphs = vec![];
        for p in 0..phases {
            let shift = p * S / phases;
            let lows: Vec<(char, Gray)> = canvases
                .iter()
                .map(|(c, g)| (*c, downsample(g, shift, S)))
                .collect();
            // the rows any glyph inks, plus one above and below
            let rows: Vec<usize> = (0..lows[0].1.h)
                .filter(|&y| {
                    lows.iter()
                        .any(|(_, g)| (0..g.w).any(|x| g.get(x, y) > 0.2))
                })
                .collect();
            let (Some(&r0), Some(&r1)) = (rows.first(), rows.last()) else {
                continue;
            };
            for (c, g) in &lows {
                let cols: Vec<usize> = (0..g.w)
                    .filter(|&x| (r0..=r1).any(|y| g.get(x, y) > 0.2))
                    .collect();
                let (Some(&c0), Some(&c1)) = (cols.first(), cols.last()) else {
                    continue;
                };
                glyphs.push((*c, crop(g, c0 - 1, c1 + 2, r0, r1 + 1)));
            }
        }
        Ok(Self { glyphs })
    }
}

/// Box-average `s`x`s` blocks, the source shifted right by `shift` hi-res px.
fn downsample(g: &Gray, shift: usize, s: usize) -> Gray {
    let (w, h) = (g.w / s, g.h / s);
    let mut out = Gray::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for dy in 0..s {
                for dx in 0..s {
                    let sx = (x * s + dx) as i64 - shift as i64;
                    if sx >= 0 {
                        acc += g.get(sx as usize, y * s + dy);
                    }
                }
            }
            out.v[y * w + x] = acc / (s * s) as f32;
        }
    }
    out
}

/// The matcher's tolerances.
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    pub glyph: Params,
    /// A candidate is an alternative if it scores at most this much below
    /// the best (NCC).
    pub alt: f32,
    /// Candidates kept per digit, the best included.
    pub top_k: usize,
    /// |residual| that counts as fitting, metres.
    pub tolerance: f64,
    /// States kept by the beam. The cheapest are kept, so a verified fit
    /// (cost 0 or close) survives; the cap bounds the time a frame of
    /// unclear digits can take (300,000 states took 9 s, 50,000 take under
    /// 0.5 s, with the same reads on every recorded grab).
    pub beam: usize,
    /// Another fit within this much cost, more than a metre away, makes the
    /// frame ambiguous.
    pub ambiguous: f32,
    /// The most summed score loss a fit may carry. Verified fits cost 0 or
    /// close; fits to a deliberately wrong checksum cost 0.6 and up.
    pub max_cost: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            glyph: Params {
                min_score: 0.2,
                ..Params::default()
            },
            alt: 0.25,
            top_k: 4,
            tolerance: 0.0015,
            beam: 50_000,
            ambiguous: 0.15,
            max_cost: 0.35,
        }
    }
}

/// How a number is printed: digits after the point, unit glyphs after
/// them, and the factor to metres.
#[derive(Clone, Copy)]
struct Format {
    decimals: usize,
    suffix: usize,
    scale: f64,
}

/// `-75099.154349`: CamPos, radius, elevation, altitude.
const METRES: Format = Format {
    decimals: 6,
    suffix: 0,
    scale: 1.0,
};
/// `18703399.8265km`: the `Zone: ... Pos:` lines.
const KM: Format = Format {
    decimals: 4,
    suffix: 2,
    scale: 1000.0,
};
/// `974.50m`: the same lines print values under 1 km in metres, 2 decimals.
const M2: Format = Format {
    decimals: 2,
    suffix: 1,
    scale: 1.0,
};
/// `-21`: whole numbers (`CamDir`).
const INT: Format = Format {
    decimals: 0,
    suffix: 0,
    scale: 1.0,
};

/// One digit: its place value in metres, the chars it could be (best
/// first, NCC), and its window as an exemplar.
#[derive(Clone)]
struct Digit {
    place: f64,
    ranked: Vec<(char, f32)>,
    window: Gray,
}

#[derive(Clone)]
struct Number {
    sign: f64,
    digits: Vec<Digit>,
}

impl Number {
    fn top(&self) -> f64 {
        self.sign
            * self
                .digits
                .iter()
                .map(|d| d.place * d.ranked[0].0.to_digit(10).unwrap_or(0) as f64)
                .sum::<f64>()
    }
}

/// Verified glyphs with their windows: new exemplars.
type Glyphs = Vec<(char, Gray)>;

/// One digit the search may change: (number, digit, [(extra cost, value delta, char)]).
type Choice = (usize, usize, Vec<(f32, f64, char)>);

/// What the candidate search found.
enum Search {
    None,
    Ambiguous,
    Found {
        vals: Vec<f64>,
        cost: f32,
        glyphs: Vec<(char, Gray)>,
    },
}

/// What became of a frame.
#[derive(Debug, Clone)]
pub enum Outcome {
    Accepted(Reading),
    /// The two lines aren't there or don't parse as numbers.
    Unreadable,
    /// No candidate combination satisfies the checksum.
    NoSolution,
    /// More than one position fits about as well.
    Ambiguous,
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Accepted(_) => "accepted",
            Outcome::Unreadable => "unreadable",
            Outcome::NoSolution => "no solution",
            Outcome::Ambiguous => "ambiguous",
        }
    }
}

/// Which line gave the body-frame position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `CamPos Planet Zone`, 1 µm, checksum to 0.0015 m.
    CamPos,
    /// `Zone: OOC_*` (km, 0.1 m), when CamPos isn't printed (landed, on
    /// foot); checksum to 0.12 m.
    Zone,
}

/// An accepted reading. Metres.
#[derive(Debug, Clone)]
pub struct Reading {
    /// Body-fixed position in the game's own body frame, from `source`.
    pub cam: [f64; 3],
    pub source: Source,
    /// The body by its radius (with nav-data), when that names one body.
    pub body: Option<String>,
    /// `r_DisplayInfo 2`'s `CamDir: a b c` as printed (whole numbers):
    /// the camera's orientation in the system frame (`sc_nav_geo::space`).
    pub camdir: Option<[i32; 3]>,
    /// `Current player location`, read against the known names.
    pub place: Option<Place>,
    /// `Zone: Root Pos`, 0.1 m: system-global, what `/showlocation` gives.
    /// Only when it agrees with `cam` about the body found by `radius`:
    /// same z and horizontal radius from the body's centre.
    pub root: Option<[f64; 3]>,
    pub radius: f64,
    pub elevation: f64,
    pub altitude: f64,
    /// |cam| - radius - elevation - altitude.
    pub residual: f64,
    /// Summed score loss of the digits that aren't their glyph's best.
    pub cost: f32,
    /// Every verified digit with its window: new exemplars.
    pub glyphs: Vec<(char, Gray)>,
}

/// The templates: rendered glyphs place, labelled exemplars name.
pub struct Matcher {
    pub set: TemplateSet,
    pub tuning: Tuning,
    pub max_per_char: usize,
    /// Letters, digits and `_:` for reading names by dictionary; `None`:
    /// names aren't read.
    pub text: Option<TemplateSet>,
    /// Numbers read in the current frame, by line, token and format: the
    /// `Zone`, `Root` and `CamDir` searches read the same lines.
    memo: RefCell<HashMap<NumberKey, Option<Number>>>,
}

/// A token of a line of the current frame (the line by its pixels'
/// address, stable while the frame is read) in a format.
type NumberKey = (usize, usize, usize, usize, usize, usize);

/// What names are read with: every letter and digit, `_` and `:`.
pub const TEXT_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_:";

/// Names the overlay prints on `Current player location : <name>`: each
/// body as `Stanton1a` (its id, first letter up), each system as
/// `StantonSolarSystem`. With the matching body id or system.
pub fn location_names(bodies: &[sc_nav_geo::Body]) -> Vec<(String, Place)> {
    let cap = |s: &str| {
        let mut c = s.chars();
        c.next()
            .map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
    };
    let mut out: Vec<(String, Place)> = bodies
        .iter()
        .map(|b| (cap(&b.id), Place::Body(b.id.clone())))
        .collect();
    let mut systems: Vec<String> = bodies
        .iter()
        .map(|b| {
            b.id.chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect::<String>()
        })
        .collect();
    systems.sort();
    systems.dedup();
    for s in systems {
        out.push((format!("{}SolarSystem", cap(&s)), Place::System(s)));
    }
    out
}

/// Where the location line puts the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A body id (`stanton1a`).
    Body(String),
    /// A system (`stanton`), out in space.
    System(String),
}

impl Place {
    /// The system either way: `stanton1a` -> `stanton`.
    pub fn system(&self) -> String {
        match self {
            Place::Body(b) => b.chars().take_while(|c| c.is_ascii_alphabetic()).collect(),
            Place::System(s) => s.clone(),
        }
    }
}

impl Matcher {
    pub fn new(font: Font) -> Self {
        let glyphs: Vec<(char, Gray)> = font.glyphs;
        let exemplars = glyphs
            .iter()
            .filter(|(c, _)| c.is_ascii_digit())
            .cloned()
            .collect();
        Self {
            set: TemplateSet { glyphs, exemplars },
            tuning: Tuning::default(),
            max_per_char: 120,
            text: None,
            memo: RefCell::default(),
        }
    }

    /// Also read names, with `font` rendered from [`TEXT_CHARS`].
    pub fn with_text(mut self, font: Font) -> Self {
        self.text = Some(TemplateSet {
            glyphs: font.glyphs,
            exemplars: vec![],
        });
        self
    }

    /// Which of `candidates` the token spells, by fitting each candidate's
    /// letters in order: (index, mean score, lead over the next best).
    /// Name lines use the font's own advance, 6.62 px (0.602 em at 11 px;
    /// number lines are spaced wider): a candidate whose length can't give
    /// the token's ink width is skipped, which keeps `Stanton1` from fitting
    /// inside `Stanton1a`.
    fn word(
        &self,
        cov: &Gray,
        token: &[(usize, usize)],
        candidates: &[&str],
    ) -> Option<(usize, f32, f32)> {
        let set = self.text.as_ref()?;
        let (x0, x1) = (token.first()?.0, token.last()?.1);
        let ink = x1 - x0;
        let line = crop(cov, x0.saturating_sub(2), (x1 + 2).min(cov.w), 0, cov.h);
        let params = Params {
            min_score: 0.15,
            gap: 7,
            ..self.tuning.glyph
        };
        // n glyphs: the ink spans (n-1) advances plus most of the last glyph
        const ADVANCE: f32 = 6.62;
        let fits: Vec<(usize, usize)> = candidates
            .iter()
            .enumerate()
            .map(|(k, c)| (k, c.chars().count()))
            .filter(|&(_, n)| {
                let (lo, hi) = (ADVANCE * (n as f32 - 1.0) + 1.0, ADVANCE * n as f32 + 1.0);
                n > 0 && ink as f32 >= lo - 1.5 && ink as f32 <= hi
            })
            .collect();
        // the hits of every letter a fitting candidate has, found once
        let wanted: String = fits
            .iter()
            .flat_map(|&(k, _)| candidates[k].chars())
            .collect();
        let sub = TemplateSet {
            glyphs: set
                .glyphs
                .iter()
                .filter(|(c, _)| wanted.contains(*c))
                .cloned()
                .collect(),
            exemplars: vec![],
        };
        let glyphs = sc_nav_glyph::Line::new(&line, &sub, &params);
        let mut scores: Vec<(usize, f32)> = vec![];
        for &(k, n) in &fits {
            let grammar = Grammar(
                candidates[k]
                    .chars()
                    .map(|c| Slot::new(&c.to_string(), 1, 1))
                    .collect(),
            );
            let Some(read) = glyphs.read(&grammar) else {
                continue;
            };
            let mean = read
                .iter()
                .map(|p| p.ranked.first().map_or(0.0, |r| r.1))
                .sum::<f32>()
                / n as f32;
            scores.push((k, mean));
        }
        scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        let (best, score) = *scores.first()?;
        // the lead as a sum over the letters: names that differ in one letter
        // (Stanton1a/1b) differ by that letter's score, not a ninth of it
        let n = candidates[best].chars().count() as f32;
        let lead = scores.get(1).map_or(score * n, |s| (score - s.1) * n);
        Some((best, score, lead))
    }

    /// The `Current player location : <name>` line: the last token of the
    /// lines given that spells a known name, clearly: mean score 0.7 (true
    /// names score ~0.9) and a lead of 0.1 summed over the letters.
    pub fn location(&self, lines: &[Gray], bodies: &[sc_nav_geo::Body]) -> Option<Place> {
        self.text.as_ref()?;
        let names = location_names(bodies);
        let cands: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
        let mut best: Option<(usize, f32, f32)> = None;
        for line in lines {
            let t = tokens(line);
            let Some(last) = t.last() else { continue };
            if let Some(w) = self.word(line, last, &cands)
                && best.is_none_or(|b| w.1 > b.1)
            {
                best = Some(w);
            }
        }
        let (k, score, lead) = best?;
        if std::env::var_os("SC_GLYPH_TEXT").is_some() {
            eprintln!("  location: {} score {score:.3} lead {lead:.3}", names[k].0);
        }
        (score >= 0.7 && lead >= 0.1).then(|| names[k].1.clone())
    }

    /// Learn from an accepted reading: only one that fit at no cost (every
    /// digit its glyph's best), so a fit the search had to bend never teaches.
    pub fn learn_from(&mut self, r: &Reading) -> usize {
        if r.cost > 0.0 {
            return 0;
        }
        self.learn(&r.glyphs)
    }

    /// Add verified glyphs as exemplars, skipping near-duplicates. Returns
    /// how many were new.
    pub fn learn(&mut self, glyphs: &[(char, Gray)]) -> usize {
        let mut added = 0;
        for (c, g) in glyphs {
            let same: Vec<&Gray> = self
                .set
                .exemplars
                .iter()
                .filter(|(e, _)| e == c)
                .map(|(_, g)| g)
                .collect();
            if same.len() >= self.max_per_char {
                continue;
            }
            let near = same
                .iter()
                .any(|e| e.w == g.w && sc_nav_glyph::ncc_at(e, 0, 0, g) > 0.97);
            if !near {
                self.set.exemplars.push((*c, g.clone()));
                added += 1;
            }
        }
        added
    }

    /// Exemplar counts per char.
    pub fn exemplar_counts(&self) -> HashMap<char, usize> {
        let mut m = HashMap::new();
        for (c, _) in &self.set.exemplars {
            *m.entry(*c).or_insert(0) += 1;
        }
        m
    }

    fn number(&self, cov: &Gray, token: &[(usize, usize)], f: Format) -> Option<Number> {
        let key = (
            cov.v.as_ptr() as usize,
            token.first()?.0,
            token.last()?.1,
            token.len(),
            f.decimals,
            f.suffix,
        );
        if let Some(n) = self.memo.borrow().get(&key) {
            return n.clone();
        }
        let n = self.read_number(cov, token, f);
        self.memo.borrow_mut().insert(key, n.clone());
        n
    }

    fn read_number(&self, cov: &Gray, token: &[(usize, usize)], f: Format) -> Option<Number> {
        let rows_at = |x: usize| -> Vec<usize> {
            (MARGIN..MARGIN + INK_ROWS)
                .filter(|&y| cov.get(x, y) > 0.45)
                .collect()
        };
        let token = &token[..token.len().checked_sub(f.suffix)?];
        // A minus: a leading run of 2+ columns inked on at most two middle
        // rows. Checked by column, since it can merge into the next glyph.
        let mut start = token.first()?.0;
        // A horizontal stroke: the same rows in every column. The tip of a
        // 4 slants ([6,7] then [5,7]) and must not pass.
        let mut run = 0;
        let mut stroke: Option<Vec<usize>> = None;
        for x in token[0].0..token.last()?.1 {
            let rows = rows_at(x);
            if rows.is_empty() {
                if run > 0 {
                    break;
                }
                continue;
            }
            let flat = stroke.as_ref().is_none_or(|s| *s == rows);
            if flat
                && rows.len() <= 2
                && rows.iter().all(|&r| (MARGIN + 2..MARGIN + 6).contains(&r))
            {
                run += 1;
                start = x + 1;
                stroke = Some(rows);
            } else {
                break;
            }
        }
        let sign = if run >= 2 { -1.0 } else { 1.0 };
        if run < 2 {
            start = token[0].0;
        }
        let segs: Vec<(usize, usize)> = token
            .iter()
            .filter(|s| s.1 > start)
            .map(|&(a, b)| (a.max(start), b))
            .filter(|s| (s.0..s.1).any(|x| !rows_at(x).is_empty()))
            .collect();
        if segs.len() < f.decimals + if f.decimals == 0 { 1 } else { 2 } {
            return None;
        }
        let x0 = segs[0].0.saturating_sub(2);
        let x1 = (segs.last()?.1 + 2).min(cov.w);
        let line = crop(cov, x0, x1, 0, cov.h);
        // The point and the decimals are fixed: the other segments are
        // integer digits, unless glyphs merged; then the grammar decides.
        let n = segs.len() - f.decimals - usize::from(f.decimals > 0);
        let grammar = |lo, hi| {
            if f.decimals == 0 {
                Grammar(vec![Slot::new(DIGITS, lo, hi)])
            } else {
                Grammar(vec![
                    Slot::new(DIGITS, lo, hi),
                    Slot::new(".", 1, 1),
                    Slot::new(DIGITS, f.decimals, f.decimals),
                ])
            }
        };
        let glyphs = sc_nav_glyph::Line::new(&line, &self.set, &self.tuning.glyph);
        let read: Vec<Position> = glyphs
            .read(&grammar(n, n))
            .or_else(|| glyphs.read(&grammar(1, 9)))?;
        let ints = read.iter().filter(|p| p.slot == 0).count() as i32;
        let (mut i, mut d) = (0, 0);
        let mut digits = vec![];
        for p in &read {
            let place = match p.slot {
                0 => {
                    i += 1;
                    10f64.powi(ints - i)
                }
                2 => {
                    d += 1;
                    10f64.powi(-d)
                }
                _ => continue,
            };
            let mut ranked: Vec<(char, f32)> = p
                .ranked
                .iter()
                .copied()
                .filter(|(c, _)| c.is_ascii_digit())
                .collect();
            ranked.truncate(self.tuning.top_k);
            if ranked.is_empty() {
                return None;
            }
            let window = crop(
                &line,
                p.x.saturating_sub(1),
                (p.x + p.w + 1).min(line.w),
                0,
                line.h,
            );
            digits.push(Digit {
                place: place * f.scale,
                ranked,
                window,
            });
        }
        Some(Number { sign, digits })
    }

    /// The last three tokens of a line as numbers.
    fn three(&self, line: &Gray, f: Format) -> Option<Vec<Number>> {
        let t = tokens(line);
        if t.len() < 4 {
            return None;
        }
        t[t.len() - 3..]
            .iter()
            .map(|t| self.number(line, t, f))
            .collect()
    }

    /// The last three tokens of a `Zone` line, every way they can be read:
    /// each value is km (`12895961.5177km`) or metres (`-1031.75m`, used
    /// under ~10 km), and a metre value also parses as a (wrong) km one. The
    /// caller's check picks the combination that fits.
    fn three_any(&self, line: &Gray) -> Vec<Vec<Number>> {
        let t = tokens(line);
        if t.len() < 4 {
            return vec![];
        }
        let per: Vec<Vec<Number>> = t[t.len() - 3..]
            .iter()
            .map(|tok| {
                [KM, M2]
                    .iter()
                    .filter_map(|f| self.number(line, tok, *f))
                    .collect()
            })
            .collect();
        let mut out = vec![vec![]];
        for options in per {
            let mut next = vec![];
            for prefix in &out {
                for o in &options {
                    let mut v: Vec<Number> = prefix.clone();
                    v.push(o.clone());
                    next.push(v);
                }
            }
            out = next;
        }
        out
    }

    /// Read one frame. `offset` corrupts the checksum (the honesty test).
    /// `bodies` (from nav-data) snap the read radius to a known body and
    /// unlock `Reading::root`; empty, the radius is taken as read.
    pub fn read(&self, lum: &Gray, offset: f64, bodies: &[sc_nav_geo::Body]) -> Outcome {
        self.memo.borrow_mut().clear();
        let out = self.read_frame(lum, offset, bodies);
        self.memo.borrow_mut().clear();
        out
    }

    fn read_frame(&self, lum: &Gray, offset: f64, bodies: &[sc_nav_geo::Body]) -> Outcome {
        let lines: Vec<Gray> = line_tops(lum)
            .into_iter()
            .filter_map(|t| band(lum, t))
            .collect();
        // The radius line: radius, elevation and altitude at tokens -5, -3
        // and -1. CamPos is the line above it; when the game doesn't print
        // CamPos (landed, on foot), the `Zone: OOC_*` line further up stands in.
        for i in (1..lines.len()).rev() {
            let elev_t = tokens(&lines[i]);
            if elev_t.len() < 6 {
                continue;
            }
            let n = elev_t.len();
            let rest: Option<Vec<Number>> = [n - 5, n - 3, n - 1]
                .iter()
                .map(|&k| self.number(&lines[i], &elev_t[k], METRES))
                .collect();
            let Some(rest) = rest else { continue };
            let mut rest = rest.into_iter();
            let mut radius = rest.next().unwrap().top();
            let elev_alt: Vec<Number> = rest.collect();
            let mut near: Vec<&sc_nav_geo::Body> = vec![];
            if !bodies.is_empty() {
                let best = bodies
                    .iter()
                    .map(|b| (b.radius_m - radius).abs())
                    .fold(f64::MAX, f64::min);
                if best > 0.01 * radius.abs().max(1.0) {
                    return Outcome::NoSolution;
                }
                near = bodies
                    .iter()
                    .filter(|b| (b.radius_m - radius).abs() == best)
                    .collect();
                radius = near[0].radius_m;
            }
            let body = (near.len() == 1).then(|| near[0].id.clone());
            let checksum = |tol: f64| {
                move |v: &[f64]| {
                    ((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
                        - radius
                        - v[3]
                        - v[4]
                        - offset)
                        .abs()
                        < tol
                }
            };

            let (found, source, above) = match self.three(&lines[i - 1], METRES) {
                Some(mut nums) => {
                    nums.extend(elev_alt);
                    (
                        self.search(&nums, 0.001, checksum(self.tuning.tolerance)),
                        Source::CamPos,
                        i - 1,
                    )
                }
                None => match self.zone(&lines[..i - 1], radius, elev_alt, offset) {
                    Some((found, at)) => (found, Source::Zone, at),
                    None => return Outcome::NoSolution,
                },
            };
            let (vals, cost, mut glyphs) = match found {
                Search::None => return Outcome::NoSolution,
                Search::Ambiguous => return Outcome::Ambiguous,
                Search::Found { vals, cost, glyphs } => (vals, cost, glyphs),
            };
            let cam = [vals[0], vals[1], vals[2]];
            let root = self.root(&lines[..above], cam, &near).map(|(r, g)| {
                glyphs.extend(g);
                r
            });
            let body = body.or_else(|| {
                // several bodies share the radius: Root says which
                let r = root?;
                near.iter()
                    .min_by(|a, b| dist(a.center, r).total_cmp(&dist(b.center, r)))
                    .map(|b| b.id.clone())
            });
            let residual = (cam[0] * cam[0] + cam[1] * cam[1] + cam[2] * cam[2]).sqrt()
                - radius
                - vals[3]
                - vals[4];
            // every line above the radius line: on foot the planet's Zone
            // line is the topmost Zone line, and `above` already excludes it
            let camdir = self.camdir(&lines[..i]);
            let place = self.location(&lines[..i], bodies);
            // the name settles bodies whose radii tie (Hurston and microTech),
            // but only a name the printed radius allows
            let body = match &place {
                Some(Place::Body(id)) if near.iter().any(|b| &b.id == id) => Some(id.clone()),
                _ => body,
            };
            return Outcome::Accepted(Reading {
                cam,
                source,
                body,
                camdir,
                place,
                root,
                radius,
                elevation: vals[3],
                altitude: vals[4],
                residual,
                cost,
                glyphs,
            });
        }
        Outcome::Unreadable
    }

    /// `CamDir:  81  50  -21  FOV: 63  Focal: 3.00  FStop: 64.0`: the first
    /// line above the `Zone` lines whose tokens 1..=3 read as whole numbers
    /// in -360..=360 (the ship's own `Zone` line and the planet's come and go
    /// with seat, foot and space, so the search doesn't count on them).
    fn camdir(&self, lines: &[Gray]) -> Option<[i32; 3]> {
        for line in lines {
            if !self.three_any(line).is_empty() {
                return None;
            }
            let t = tokens(line);
            if t.len() < 8 {
                continue;
            }
            let read = || -> Option<[i32; 3]> {
                let mut out = [0i32; 3];
                for (k, tok) in t[1..4].iter().enumerate() {
                    let n = self.number(line, tok, INT)?;
                    if n.digits.len() > 3 {
                        return None;
                    }
                    let v = n.top().round() as i32;
                    if !(-360..=360).contains(&v) {
                        return None;
                    }
                    out[k] = v;
                }
                Some(out)
            };
            if let Some(v) = read() {
                return Some(v);
            }
        }
        None
    }

    /// The `Zone: OOC_*` line (km) that satisfies the radius checksum, at
    /// its 0.1 m resolution: the search plus the line it was on.
    fn zone(
        &self,
        lines: &[Gray],
        radius: f64,
        elev_alt: Vec<Number>,
        offset: f64,
    ) -> Option<(Search, usize)> {
        let ea: f64 = elev_alt.iter().map(Number::top).sum();
        let mut elev_alt = Some(elev_alt);
        for (k, line) in lines.iter().enumerate() {
            for mut nums in self.three_any(line) {
                let top: Vec<f64> = nums.iter().map(Number::top).collect();
                // only the body's own frame comes near; SolarSystem and Root are millions off
                if ((top[0] * top[0] + top[1] * top[1] + top[2] * top[2]).sqrt()
                    - radius
                    - ea
                    - offset)
                    .abs()
                    > 2000.0
                {
                    continue;
                }
                nums.extend(elev_alt.take()?);
                let fits = |v: &[f64]| {
                    ((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
                        - radius
                        - v[3]
                        - v[4]
                        - offset)
                        .abs()
                        < 0.12
                };
                return Some((self.search(&nums, 0.1, fits), k));
            }
        }
        None
    }

    /// The `Zone: ... Pos:` line, lowest first, that agrees with `cam` about
    /// one of `bodies`: z and horizontal radius from its centre, to the
    /// line's 0.1 m. In Stanton `SolarSystem` and `Root` print the same.
    fn root(
        &self,
        lines: &[Gray],
        cam: [f64; 3],
        bodies: &[&sc_nav_geo::Body],
    ) -> Option<([f64; 3], Glyphs)> {
        let hyp = cam[0].hypot(cam[1]);
        for line in lines.iter().rev() {
            for nums in self.three_any(line) {
                let top: Vec<f64> = nums.iter().map(Number::top).collect();
                for b in bodies {
                    // the search fixes a digit or two, not a different line
                    let c = b.center;
                    if (top[2] - c[2] - cam[2]).abs() > 1e5
                        || ((top[0] - c[0]).hypot(top[1] - c[1]) - hyp).abs() > 1e5
                    {
                        continue;
                    }
                    // Root and CamPos agree to cm, at the surface and 3,289 km out
                    // alike; a looser check lets wrong digits through (a y error
                    // barely moves the distance from the axis)
                    let tol = 0.1;
                    let fits = |v: &[f64]| {
                        (v[2] - c[2] - cam[2]).abs() < tol
                            && ((v[0] - c[0]).hypot(v[1] - c[1]) - hyp).abs() < tol
                    };
                    if let Search::Found { vals, glyphs, .. } = self.search(&nums, 0.1, fits) {
                        return Some(([vals[0], vals[1], vals[2]], glyphs));
                    }
                }
            }
        }
        None
    }

    /// Search each digit's candidates for the cheapest values that `fits`.
    /// Digits below `min_place` metres are taken as read.
    fn search(&self, nums: &[Number], min_place: f64, fits: impl Fn(&[f64]) -> bool) -> Search {
        let t = &self.tuning;
        let nv = nums.len();
        // (var, digit, [(extra cost, value delta, char)])
        let mut choices: Vec<Choice> = vec![];
        for (v, num) in nums.iter().enumerate() {
            for (k, d) in num.digits.iter().enumerate() {
                if d.place < min_place - 1e-9 || d.ranked.len() < 2 {
                    continue;
                }
                let (bc, bs) = d.ranked[0];
                let b = bc.to_digit(10).unwrap() as f64;
                let alts: Vec<(f32, f64, char)> = d.ranked[1..]
                    .iter()
                    .filter(|(_, s)| bs - s < t.alt.min(t.max_cost))
                    .map(|&(c, s)| {
                        (
                            bs - s,
                            num.sign * (c.to_digit(10).unwrap() as f64 - b) * d.place,
                            c,
                        )
                    })
                    .collect();
                if !alts.is_empty() {
                    choices.push((v, k, alts));
                }
            }
        }
        let cheapest = |c: &Choice| c.2.iter().map(|a| a.0).fold(f32::MAX, f32::min);
        choices.sort_by(|a, b| cheapest(a).total_cmp(&cheapest(b)));

        // States, flat: values (nv each) and cost. Which alternative each
        // state took is kept per choice as (parent state, pick; 0 = as read),
        // so a state costs its values, not a copy of every pick so far.
        let nc = choices.len();
        let mut vals: Vec<f64> = nums.iter().map(Number::top).collect();
        let mut cost: Vec<f32> = vec![0.0];
        let mut trail: Vec<(Vec<u32>, Vec<u8>)> = Vec::with_capacity(nc);
        for (var, _, alts) in &choices {
            let n0 = cost.len();
            let mut parent: Vec<u32> = (0..n0 as u32).collect();
            let mut pick: Vec<u8> = vec![0; n0];
            for (a, &(extra, delta, _)) in alts.iter().enumerate() {
                for s in 0..n0 {
                    if cost[s] + extra > t.max_cost {
                        continue;
                    }
                    let at = vals.len();
                    vals.extend_from_within(s * nv..(s + 1) * nv);
                    vals[at + var] += delta;
                    cost.push(cost[s] + extra);
                    parent.push(s as u32);
                    pick.push(a as u8 + 1);
                }
            }
            if cost.len() > t.beam {
                let mut idx: Vec<usize> = (0..cost.len()).collect();
                idx.select_nth_unstable_by(t.beam, |&a, &b| cost[a].total_cmp(&cost[b]));
                idx.truncate(t.beam);
                let mut kept = Vec::with_capacity(idx.len() * nv);
                for &i in &idx {
                    kept.extend_from_slice(&vals[i * nv..(i + 1) * nv]);
                }
                vals = kept;
                cost = idx.iter().map(|&i| cost[i]).collect();
                parent = idx.iter().map(|&i| parent[i]).collect();
                pick = idx.iter().map(|&i| pick[i]).collect();
            }
            trail.push((parent, pick));
        }

        let state = |i: usize| &vals[i * nv..(i + 1) * nv];
        let mut ok: Vec<usize> = (0..cost.len()).filter(|&i| fits(state(i))).collect();
        if ok.is_empty() {
            return Search::None;
        }
        ok.sort_by(|&a, &b| cost[a].total_cmp(&cost[b]));
        let best = ok[0];
        for &o in &ok[1..] {
            if cost[o] - cost[best] > t.ambiguous {
                break;
            }
            if (0..nv).any(|k| (state(o)[k] - state(best)[k]).abs() > 1.0) {
                return Search::Ambiguous;
            }
        }
        // the best state's picks, back along its parents
        let mut picks = vec![0u8; nc];
        let mut at = best;
        for (ci, (parent, pick)) in trail.iter().enumerate().rev() {
            picks[ci] = pick[at];
            at = parent[at] as usize;
        }
        let mut chosen: HashMap<(usize, usize), char> = HashMap::new();
        for (ci, (var, k, alts)) in choices.iter().enumerate() {
            let p = picks[ci];
            if p > 0 {
                chosen.insert((*var, *k), alts[p as usize - 1].2);
            }
        }
        let mut glyphs = vec![];
        for (v, num) in nums.iter().enumerate() {
            for (k, d) in num.digits.iter().enumerate() {
                // only digits the check covers count as verified
                if d.place >= min_place - 1e-9 {
                    glyphs.push((
                        *chosen.get(&(v, k)).unwrap_or(&d.ranked[0].0),
                        d.window.clone(),
                    ));
                }
            }
        }
        Search::Found {
            vals: state(best).to_vec(),
            cost: cost[best],
            glyphs,
        }
    }
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
