//! Glyph matching: read a line of game text from an ink image of a known
//! region, against glyph templates passed in as data.
//!
//! Two steps, kept apart because they fail differently:
//! 1. **Place** ([`place`]). Every mean template slides over the line;
//!    a dynamic program picks the best string the [`Grammar`] allows, each
//!    glyph starting just after the previous one's ink (a pixel of overlap
//!    and a few of gap tolerated, so fractional glyph positions still
//!    chain). This decides where the glyphs are.
//! 2. **Name** ([`read`]). Each placed glyph is ranked again: against the
//!    labelled exemplars when the set has some for its slot (nearest
//!    example, a few pixels of slack; tells apart glyphs that differ by a
//!    one-pixel hole, which mean templates blur away), else by the template
//!    hits at the same spot.
//!
//! The result keeps **ranked candidates per position**, not one answer:
//! callers apply what they know (checksums, sums, a model) to pick among
//! them. Building templates (rendered from the game's font, or learned
//! from verified reads), capturing and locating the region stay with the
//! callers.
//!
//! Pure: no capture, no IO, no platform code.

mod gray;
pub mod ncc;

use std::collections::{BTreeMap, HashMap};

pub use gray::Gray;

/// The templates a line is read against.
#[derive(Clone, Debug, Default)]
pub struct TemplateSet {
    /// Mean templates, one or more per char: they place the glyphs.
    pub glyphs: Vec<(char, Gray)>,
    /// Labelled examples (every one kept, not averaged): they name the
    /// placed glyphs whose slot allows a char that has examples.
    pub exemplars: Vec<(char, Gray)>,
}

/// One run of glyphs in a line: which chars, how many.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub chars: String,
    pub min: usize,
    pub max: usize,
}

impl Slot {
    pub fn new(chars: &str, min: usize, max: usize) -> Self {
        Self {
            chars: chars.into(),
            min,
            max,
        }
    }
}

/// The shape of a line: its slots left to right, e.g. digits 1..3, `.`,
/// digits 2..2, `%`.
#[derive(Clone, Debug, PartialEq)]
pub struct Grammar(pub Vec<Slot>);

/// The matcher's tolerances. [`Default`] is what the mining scan panel
/// was tuned with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// The lowest template score that counts as a hit.
    pub min_score: f32,
    /// A hit must be the best within this many px left and right.
    pub peak_radius: usize,
    /// Subtracted per placed glyph: keeps the program from padding a
    /// string with weak hits.
    pub bias: f32,
    /// How far a glyph's ink may start before the previous one's ends.
    pub overlap: usize,
    /// How far after the previous glyph's ink a glyph may start.
    pub gap: usize,
    /// Naming searches the exemplars this many px left and right.
    pub reach: i64,
    /// Template hits within this many px of a placed glyph rank it when
    /// there are no exemplars.
    pub same_spot: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            min_score: 0.25,
            peak_radius: 2,
            bias: 0.45,
            overlap: 1,
            gap: 5,
            reach: 2,
            same_spot: 1,
        }
    }
}

/// A template hit: ink left edge and width in the line, the char, the
/// score (NCC, up to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub x: usize,
    pub w: usize,
    pub ch: char,
    pub score: f32,
}

/// One read glyph: where its ink is, the slot it fills, and the chars it
/// could be, best first, with their scores.
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    pub x: usize,
    pub w: usize,
    pub slot: usize,
    pub ranked: Vec<(char, f32)>,
}

impl Position {
    /// The best char.
    pub fn ch(&self) -> char {
        self.ranked[0].0
    }

    /// The runner-up and how much worse it scored (0: a toss-up); `None`
    /// when nothing else fits here.
    pub fn runner_up(&self) -> Option<(char, f32)> {
        self.ranked.get(1).map(|&(c, s)| (c, self.ranked[0].1 - s))
    }
}

/// NCC of `t` with its top-left at (x, y) in `img`; 0 outside or for a
/// flat window.
pub fn ncc_at(img: &Gray, x: i64, y: i64, t: &Gray) -> f32 {
    if x < 0 || y < 0 || x as usize + t.w > img.w || y as usize + t.h > img.h {
        return 0.0;
    }
    let n = (t.w * t.h) as f32;
    let (mut si, mut sii, mut st, mut stt, mut sit) = (0.0f32, 0.0, 0.0, 0.0, 0.0);
    for ty in 0..t.h {
        let row = (y as usize + ty) * img.w + x as usize;
        for tx in 0..t.w {
            let a = img.v[row + tx];
            let b = t.v[ty * t.w + tx];
            si += a;
            sii += a * a;
            st += b;
            stt += b * b;
            sit += a * b;
        }
    }
    let vi = sii - si * si / n;
    let vt = stt - st * st / n;
    if vi <= 1e-6 || vt <= 1e-6 {
        return 0.0;
    }
    (sit - si * st / n) / (vi * vt).sqrt()
}

/// Every template's local score peaks along the line, by ink left edge.
/// A template may sit a pixel above or below the line's own top/bottom.
pub fn hits(line: &Gray, set: &TemplateSet, p: &Params) -> Vec<Hit> {
    hits_with(line, &ncc::Stats::new(line), set, p)
}

fn hits_with(line: &Gray, stats: &ncc::Stats, set: &TemplateSet, p: &Params) -> Vec<Hit> {
    let mut out = Vec::new();
    let (mut scores, mut dots) = (Vec::new(), Vec::new());
    // Per template size: each window's √Σ(v − mean)², row by row (NaN:
    // flat), shared by every template of that size.
    let mut sds: HashMap<(usize, usize), Vec<f32>> = HashMap::new();
    for (ch, t) in &set.glyphs {
        if t.w > line.w || t.h > line.h + 2 {
            continue;
        }
        let cols: Vec<usize> = (0..t.w)
            .filter(|&x| (0..t.h).any(|y| t.get(x, y) > 0.5))
            .collect();
        let (Some(&il), Some(&ir)) = (cols.first(), cols.last()) else {
            continue;
        };
        // the best over the rows the template fits in; a row a pixel off
        // the line scores 0, so nothing scores below that
        let nx = line.w - t.w + 1;
        scores.clear();
        scores.resize(nx, 0.0f32);
        if t.h <= line.h
            && let Some(u) = ncc::unit(t)
        {
            dots.resize(nx, 0.0f32);
            let sd = sds.entry((t.w, t.h)).or_insert_with(|| {
                let mut sd = Vec::with_capacity(nx * (line.h - t.h + 1));
                for y in 0..=line.h - t.h {
                    sd.extend((0..nx).map(|x| {
                        stats
                            .spread(x, y, t.w, t.h)
                            .map_or(f32::NAN, |var| var.sqrt() as f32)
                    }));
                }
                sd
            });
            for (y, sd) in (0..=line.h - t.h).zip(sd.chunks_exact(nx)) {
                ncc::row_dots(line, &u, t.w, t.h, y, &mut dots);
                for ((s, &d), &k) in scores.iter_mut().zip(&dots).zip(sd) {
                    if !k.is_nan() {
                        *s = s.max(d / k);
                    }
                }
            }
        }
        for x in 0..scores.len() {
            let v = scores[x];
            let lo = x.saturating_sub(p.peak_radius);
            let hi = (x + p.peak_radius + 1).min(scores.len());
            if v >= p.min_score && scores[lo..hi].iter().all(|&o| o <= v) {
                out.push(Hit {
                    x: x + il,
                    w: ir - il + 1,
                    ch: *ch,
                    score: v,
                });
            }
        }
    }
    out.sort_by_key(|h| h.x);
    out
}

/// The best string of `grammar` along the line: (slot, hit) per glyph,
/// left to right; empty when no string of the grammar fits.
pub fn place(line: &Gray, set: &TemplateSet, grammar: &Grammar, p: &Params) -> Vec<(usize, Hit)> {
    Line::new(line, set, p).place(grammar)
}

fn place_hits(cands: &[Hit], grammar: &Grammar, p: &Params) -> Vec<(usize, Hit)> {
    let slots = &grammar.0;
    if slots.is_empty() || cands.is_empty() {
        return vec![];
    }
    // State (hit i, slot si, count cnt in the slot) at i * stride + at[si] +
    // cnt - 1: the best total ending there and its back pointer.
    let mut at = Vec::with_capacity(slots.len());
    let mut stride = 0;
    for s in slots {
        at.push(stride);
        stride += s.max;
    }
    let key = |i: usize, si: usize, cnt: usize| i * stride + at[si] + cnt - 1;
    const NONE: usize = usize::MAX;
    let mut val = vec![f32::NAN; cands.len() * stride];
    let mut back = vec![NONE; cands.len() * stride];
    let set = |val: &[f32], k: usize| !val[k].is_nan();
    let ns = slots.len();
    // Per (hit, slot), filled once the hit is done: the state that ends the
    // slot best there (its count within min..=max, ties to the larger), and
    // the highest count set (0: none). A later hit reads these instead of
    // scanning the counts again for every predecessor.
    let mut ends: Vec<usize> = vec![NONE; cands.len() * ns];
    let mut top: Vec<usize> = vec![0; cands.len() * ns];
    // which slots take which hit
    let takes: Vec<bool> = cands
        .iter()
        .flat_map(|c| {
            slots
                .iter()
                .map(move |s| s.max > 0 && s.chars.contains(c.ch))
        })
        .collect();
    // a predecessor's ink ends at most `gap` before this one's starts, so it
    // starts no earlier than that minus the widest hit
    let widest = cands.iter().map(|c| c.w).max().unwrap_or(0);
    let mut adjacent: Vec<usize> = vec![];
    let mut row: Vec<(f32, usize)> = vec![];
    for (i, c) in cands.iter().enumerate() {
        let first = cands[..i].partition_point(|q| q.x + widest + p.gap < c.x);
        adjacent.clear();
        adjacent.extend((first..i).filter(|&j| {
            let q = &cands[j];
            q.x + q.w <= c.x + p.overlap && c.x <= q.x + q.w + p.gap
        }));
        let gain = c.score - p.bias;
        for (si, slot) in slots.iter().enumerate() {
            if !takes[i * ns + si] {
                continue;
            }
            // per count: the best total so far (NaN: none) and where from;
            // the first predecessor wins ties
            row.clear();
            row.resize(slot.max, (f32::NAN, NONE));
            if si == 0 {
                row[0] = (gain, NONE);
            }
            for &j in &adjacent {
                if si > 0 {
                    // count 1: the best count the slot before allows
                    let prev = ends[j * ns + si - 1];
                    if prev != NONE {
                        let v = val[prev] + gain;
                        if row[0].0.is_nan() || v > row[0].0 {
                            row[0] = (v, prev);
                        }
                    }
                }
                // count n: the same slot's count n - 1 at j. Branch-free:
                // take v when it beats the count's best, or fills an empty
                // one (v is NaN exactly when there is no state at j)
                let set_to = top[j * ns + si].min(slot.max - 1);
                let k0 = key(j, si, 1);
                let prevs = &val[k0..k0 + set_to];
                for (n, (r, &pv)) in row[1..=set_to].iter_mut().zip(prevs).enumerate() {
                    let v = pv + gain;
                    let take = v > r.0 || (r.0.is_nan() && !v.is_nan());
                    *r = if take { (v, k0 + n) } else { *r };
                }
            }
            let k0 = key(i, si, 1);
            for (n, &(v, prev)) in row.iter().enumerate() {
                val[k0 + n] = v;
                back[k0 + n] = prev;
            }
            top[i * ns + si] = row.iter().rposition(|r| !r.0.is_nan()).map_or(0, |n| n + 1);
            let mut pick: Option<usize> = None;
            for k in slot.min.max(1)..=slot.max {
                let kk = key(i, si, k);
                if set(&val, kk) && pick.is_none_or(|b| val[kk].total_cmp(&val[b]).is_ge()) {
                    pick = Some(kk);
                }
            }
            ends[i * ns + si] = pick.unwrap_or(NONE);
        }
    }
    let last = slots.len() - 1;
    let (lo, hi) = (slots[last].min.max(1), slots[last].max);
    // the best end; ties go to the smallest (hit, count)
    let mut end: Option<usize> = None;
    for i in 0..cands.len() {
        for cnt in lo..=hi {
            let k = key(i, last, cnt);
            if set(&val, k) && end.is_none_or(|e| val[k].total_cmp(&val[e]).is_gt()) {
                end = Some(k);
            }
        }
    }
    let Some(mut k) = end else {
        return vec![];
    };
    let mut out = Vec::new();
    loop {
        let (i, r) = (k / stride, k % stride);
        let si = at.iter().rposition(|&a| a <= r).unwrap_or(0);
        out.push((si, cands[i]));
        match back[k] {
            NONE => break,
            prev => k = prev,
        }
    }
    out.reverse();
    out
}

/// A set's exemplars ready to name glyphs: centred and scaled to unit norm
/// (`None`: flat), and grouped by char.
struct Named {
    units: Vec<Option<Vec<f32>>>,
    /// The exemplars' chars, sorted, each once.
    chars: Vec<char>,
    /// Per exemplar, its char's index in `chars`.
    class: Vec<usize>,
}

impl Named {
    fn new(set: &TemplateSet) -> Self {
        let mut chars: Vec<char> = set.exemplars.iter().map(|(c, _)| *c).collect();
        chars.sort_unstable();
        chars.dedup();
        Self {
            units: set.exemplars.iter().map(|(_, g)| ncc::unit(g)).collect(),
            class: set
                .exemplars
                .iter()
                .map(|(c, _)| chars.binary_search(c).expect("listed"))
                .collect(),
            chars,
        }
    }

    fn has(&self, c: char) -> bool {
        self.chars.binary_search(&c).is_ok()
    }
}

/// The exemplars of `allowed` chars around the glyph whose ink starts at
/// `x` (width `w`), best first: one score per char, its nearest example.
/// A score is a dot product of the exemplar's unit vector with the window,
/// normalised once.
fn name(
    line: &Gray,
    stats: &ncc::Stats,
    set: &TemplateSet,
    named: &Named,
    (x, w): (usize, usize),
    allowed: &str,
    p: &Params,
) -> Vec<(char, f32)> {
    let ok: Vec<bool> = named.chars.iter().map(|&c| allowed.contains(c)).collect();
    // per char: the best score (an entry once an exemplar was tried)
    let mut per: Vec<Option<f32>> = vec![None; named.chars.len()];
    // the line's windows per exemplar size, normalised
    let mut windows: HashMap<(usize, usize), Vec<Vec<f32>>> = HashMap::new();
    for (((_, g), u), &c) in set.exemplars.iter().zip(&named.units).zip(&named.class) {
        if !ok[c] {
            continue;
        }
        // a window a pixel above the line scores 0, so nothing scores below
        // that (unless there are no windows at all)
        let mut b = if p.reach >= 0 { 0.0f32 } else { f32::MIN };
        if let Some(u) = u {
            let ws = windows
                .entry((g.w, g.h))
                .or_insert_with(|| exemplar_windows(line, stats, x, w, g.w, g.h, p.reach));
            let (quads, rest) = ws.as_chunks::<4>();
            for q in quads {
                for d in ncc::dot4([&q[0], &q[1], &q[2], &q[3]], u) {
                    b = b.max(d);
                }
            }
            for win in rest {
                b = b.max(ncc::dot(win, u));
            }
        }
        let e = per[c].get_or_insert(f32::MIN);
        *e = e.max(b);
    }
    // by char, then by score: what a map by char sorted by score gives
    let mut ranked: Vec<(char, f32)> = named
        .chars
        .iter()
        .zip(per)
        .filter_map(|(&ch, b)| b.map(|b| (ch, b)))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked
}

/// The line's `gw` x `gh` windows an exemplar is tried at, around the
/// glyph whose ink starts at `x` (width `w`): centred, unit norm, the flat
/// and the out-of-bounds ones left out.
fn exemplar_windows(
    line: &Gray,
    stats: &ncc::Stats,
    x: usize,
    w: usize,
    gw: usize,
    gh: usize,
    reach: i64,
) -> Vec<Vec<f32>> {
    // the exemplar window centres the glyph's ink in its cell
    let x0 = x as i64 - 1 - (gw as i64 - 2 - w as i64) / 2;
    let top = line.h as i64 - gh as i64;
    let mut out = vec![];
    for dx in -reach..=reach {
        for y in -1..=top + 1 {
            let wx = x0 + dx;
            if wx < 0 || y < 0 || wx as usize + gw > line.w || y as usize + gh > line.h {
                continue;
            }
            let (wx, wy) = (wx as usize, y as usize);
            let Some(var) = stats.spread(wx, wy, gw, gh) else {
                continue;
            };
            let (mean, k) = (stats.mean(wx, wy, gw, gh), var.sqrt());
            let mut win = Vec::with_capacity(gw * gh);
            for ry in wy..wy + gh {
                let row = &line.v[ry * line.w + wx..ry * line.w + wx + gw];
                win.extend(row.iter().map(|&v| ((v as f64 - mean) / k) as f32));
            }
            out.push(win);
        }
    }
    out
}

/// Read the line as `grammar`: each placed glyph with its ranked
/// candidates. `None` when no string of the grammar fits.
pub fn read(
    line: &Gray,
    set: &TemplateSet,
    grammar: &Grammar,
    p: &Params,
) -> Option<Vec<Position>> {
    Line::new(line, set, p).read(grammar)
}

/// A line with its template hits found once, to place or read it as
/// several grammars (e.g. an exact count first, a looser one if that
/// fails, or one word per dictionary entry). Same results as [`place`]
/// and [`read`] with the same arguments.
pub struct Line<'a> {
    line: &'a Gray,
    set: &'a TemplateSet,
    p: Params,
    stats: ncc::Stats,
    hits: Vec<Hit>,
    /// The exemplars ready to name glyphs, once one is needed.
    named: std::cell::OnceCell<Named>,
}

impl<'a> Line<'a> {
    pub fn new(line: &'a Gray, set: &'a TemplateSet, p: &Params) -> Self {
        let stats = ncc::Stats::new(line);
        let hits = hits_with(line, &stats, set, p);
        Self {
            line,
            set,
            p: *p,
            stats,
            hits,
            named: Default::default(),
        }
    }

    /// Every template's hits, as [`hits`] gives them.
    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    /// The hits of chars the grammar has, in order: the others can't take
    /// part in a placement.
    fn usable(&self, grammar: &Grammar) -> Vec<Hit> {
        self.hits
            .iter()
            .filter(|h| grammar.0.iter().any(|s| s.chars.contains(h.ch)))
            .copied()
            .collect()
    }

    /// As [`place`].
    pub fn place(&self, grammar: &Grammar) -> Vec<(usize, Hit)> {
        place_hits(&self.usable(grammar), grammar, &self.p)
    }

    /// As [`read`].
    pub fn read(&self, grammar: &Grammar) -> Option<Vec<Position>> {
        let cands = self.usable(grammar);
        let placed = place_hits(&cands, grammar, &self.p);
        if placed.is_empty() {
            return None;
        }
        let set = self.set;
        let out = placed
            .into_iter()
            .map(|(slot, h)| {
                let allowed = &grammar.0[slot].chars;
                let named =
                    (!set.exemplars.is_empty()).then(|| self.named.get_or_init(|| Named::new(set)));
                let mut ranked = match named {
                    Some(named) if named.has(h.ch) => name(
                        self.line,
                        &self.stats,
                        set,
                        named,
                        (h.x, h.w),
                        allowed,
                        &self.p,
                    ),
                    _ => vec![],
                };
                if ranked.is_empty() {
                    ranked = spot_ranking(&cands, &h, allowed, &self.p);
                }
                Position {
                    x: h.x,
                    w: h.w,
                    slot,
                    ranked,
                }
            })
            .collect();
        Some(out)
    }
}

/// The placed hit first, then the other allowed chars' best hits at the
/// same spot.
fn spot_ranking(cands: &[Hit], h: &Hit, allowed: &str, p: &Params) -> Vec<(char, f32)> {
    let mut per: BTreeMap<char, f32> = BTreeMap::new();
    for c in cands {
        if c.ch != h.ch && allowed.contains(c.ch) && c.x.abs_diff(h.x) <= p.same_spot {
            let e = per.entry(c.ch).or_insert(f32::MIN);
            *e = e.max(c.score);
        }
    }
    let mut rest: Vec<(char, f32)> = per.into_iter().collect();
    rest.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut ranked = vec![(h.ch, h.score)];
    ranked.extend(rest);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 5x7 glyph from rows of '#'.
    fn glyph(rows: &[&str]) -> Gray {
        let (w, h) = (rows[0].len(), rows.len());
        let mut g = Gray::new(w, h);
        for (y, r) in rows.iter().enumerate() {
            for (x, c) in r.chars().enumerate() {
                if c == '#' {
                    g.v[y * w + x] = 1.0;
                }
            }
        }
        g
    }

    fn font() -> Vec<(char, Gray)> {
        vec![
            (
                '1',
                glyph(&[
                    "..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###.",
                ]),
            ),
            (
                '7',
                glyph(&[
                    "#####", "....#", "...#.", "..#..", "..#..", "..#..", "..#..",
                ]),
            ),
            (
                '0',
                glyph(&[
                    ".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
                ]),
            ),
            (
                '8',
                glyph(&[
                    ".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###.",
                ]),
            ),
            (
                '.',
                glyph(&[
                    ".....", ".....", ".....", ".....", ".....", ".##..", ".##..",
                ]),
            ),
        ]
    }

    /// The glyphs of `text` side by side with a pixel between, on a line
    /// a pixel taller above and below.
    fn line(text: &str) -> Gray {
        let f = font();
        let mut out = Gray::new(text.len() * 6 + 2, 9);
        for (i, ch) in text.chars().enumerate() {
            let g = &f.iter().find(|(c, _)| *c == ch).unwrap().1;
            for y in 0..g.h {
                for x in 0..g.w {
                    out.v[(y + 1) * out.w + 1 + i * 6 + x] = g.get(x, y);
                }
            }
        }
        out
    }

    fn set() -> TemplateSet {
        TemplateSet {
            glyphs: font(),
            exemplars: vec![],
        }
    }

    #[test]
    fn reads_a_line_of_its_grammar() {
        let g = Grammar(vec![
            Slot::new("0178", 1, 3),
            Slot::new(".", 1, 1),
            Slot::new("0178", 1, 1),
        ]);
        let read = read(&line("10.7"), &set(), &g, &Params::default()).unwrap();
        let text: String = read.iter().map(Position::ch).collect();
        assert_eq!(text, "10.7");
        assert_eq!(
            read.iter().map(|p| p.slot).collect::<Vec<_>>(),
            vec![0, 0, 1, 2]
        );
    }

    #[test]
    fn a_line_outside_the_grammar_reads_nothing() {
        // three glyphs wanted, two on the line
        let g = Grammar(vec![Slot::new("0178", 3, 3)]);
        assert_eq!(read(&line("17"), &set(), &g, &Params::default()), None);
    }

    #[test]
    fn close_glyphs_keep_a_runner_up() {
        // 0 and 8 differ by the middle bar: each is the other's runner-up.
        let g = Grammar(vec![Slot::new("0178", 1, 2)]);
        let read = read(&line("80"), &set(), &g, &Params::default()).unwrap();
        assert_eq!(read[0].ch(), '8');
        assert_eq!(read[0].runner_up().map(|r| r.0), Some('0'));
        assert_eq!(read[1].ch(), '0');
        assert_eq!(read[1].runner_up().map(|r| r.0), Some('8'));
    }

    #[test]
    fn exemplars_rank_within_the_slot() {
        let mut s = set();
        s.exemplars = font()
            .into_iter()
            .filter(|(c, _)| c.is_ascii_digit())
            .collect();
        let g = Grammar(vec![
            Slot::new("018", 1, 1),
            Slot::new(".", 1, 1),
            Slot::new("18", 1, 1),
        ]);
        let read = read(&line("0.8"), &s, &g, &Params::default()).unwrap();
        assert_eq!(read[0].ch(), '0');
        assert!(read[0].ranked.iter().all(|(c, _)| "018".contains(*c)));
        // the '.' has no exemplars: ranked by its own hit alone
        assert_eq!(read[1].ranked.len(), 1);
        // the last slot leaves out 0 and 7
        assert!(read[2].ranked.iter().all(|(c, _)| "18".contains(*c)));
        assert_eq!(read[2].ch(), '8');
    }
}
