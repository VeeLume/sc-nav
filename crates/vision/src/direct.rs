//! NCC of a template at a range of positions, computed directly: the
//! check around a known spot (a panel's refinement, a tracked readout) and
//! the slide of a name over its band. Templates are prepared once
//! (centred, unit norm), the image once per search (summed-area tables),
//! so a score is one dot product, accumulated across a row of positions so
//! the loop vectorises. sc-nav-glyph's NCC.
//!
//! For a whole image and a large template, the FFT [`crate::Correlator`]
//! is cheaper; for a few hundred positions, this is.

use std::ops::Range;

use sc_nav_glyph::Gray;
use sc_nav_glyph::ncc::{Stats, row_dots_at, unit};

/// A template prepared for direct NCC.
#[derive(Debug, Clone)]
pub struct Template {
    w: usize,
    h: usize,
    unit: Vec<f32>,
    spread: f64,
}

impl Template {
    /// `t` centred and scaled to unit norm; `None` when it is empty or
    /// flat (no shape to match).
    pub fn new(t: &Gray) -> Option<Self> {
        if t.w == 0 || t.h == 0 || t.v.len() != t.w * t.h {
            return None;
        }
        let unit = unit(t)?;
        let n = t.v.len() as f64;
        let mean = t.v.iter().map(|&v| v as f64).sum::<f64>() / n;
        let spread = t.v.iter().map(|&v| (v as f64 - mean).powi(2)).sum();
        Some(Self {
            w: t.w,
            h: t.h,
            unit,
            spread,
        })
    }

    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// Σ(t − mean)²: for a window floor relative to the template's own
    /// contrast (`0.02 * t.spread()`, say).
    pub fn spread(&self) -> f64 {
        self.spread
    }
}

/// An image prepared for direct NCC searches: borrowed, plus its
/// summed-area tables. Build one per image and search every template in
/// it.
pub struct Scorer<'a> {
    img: &'a Gray,
    stats: Stats,
}

impl<'a> Scorer<'a> {
    pub fn new(img: &'a Gray) -> Self {
        Self {
            img,
            stats: Stats::new(img),
        }
    }

    pub fn image(&self) -> &Gray {
        self.img
    }

    /// The best NCC of `t` with its top-left at (x, y), x in `xs` and y in
    /// `ys`, over the positions where it fits whole: (score, x, y), the
    /// first in row order on ties. A window whose Σ(v − mean)² is at most
    /// `floor` (or 1e-6) scores 0. `None` when no position fits.
    pub fn best(
        &self,
        t: &Template,
        xs: Range<i64>,
        ys: Range<i64>,
        floor: f64,
    ) -> Option<(f32, usize, usize)> {
        let mut best: Option<(f32, usize, usize)> = None;
        self.scan(t, xs, ys, floor, |v, x, y| {
            if best.is_none_or(|b| v > b.0) {
                best = Some((v, x, y));
            }
        });
        best
    }

    /// The NCC of `t` at every position where it fits whole, as
    /// [`Scorer::best`] scores them: a (w − tw + 1) x (h − th + 1) map.
    pub fn map(&self, t: &Template, floor: f64) -> Gray {
        let (w, h) = (
            (self.img.w + 1).saturating_sub(t.w),
            (self.img.h + 1).saturating_sub(t.h),
        );
        let mut out = Gray::new(w, h);
        self.scan(t, 0..w as i64, 0..h as i64, floor, |v, x, y| {
            out.v[y * w + x] = v;
        });
        out
    }

    fn scan(
        &self,
        t: &Template,
        xs: Range<i64>,
        ys: Range<i64>,
        floor: f64,
        mut each: impl FnMut(f32, usize, usize),
    ) {
        let img = self.img;
        if t.w > img.w || t.h > img.h {
            return;
        }
        let clip = |r: Range<i64>, max: usize| {
            let lo = r.start.max(0) as usize;
            let hi = r.end.min(max as i64 + 1).max(0) as usize;
            lo..hi.max(lo)
        };
        let (xs, ys) = (clip(xs, img.w - t.w), clip(ys, img.h - t.h));
        if xs.is_empty() || ys.is_empty() {
            return;
        }
        let floor = floor.max(1e-6);
        let mut dots = vec![0.0f32; xs.len()];
        for y in ys {
            row_dots_at(img, &t.unit, t.w, t.h, xs.start, y, &mut dots);
            for (i, &d) in dots.iter().enumerate() {
                let x = xs.start + i;
                let var = self.stats.var(x, y, t.w, t.h);
                let v = if var > floor {
                    (d / var.sqrt() as f32).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                each(v, x, y);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_nav_glyph::ncc_at;

    fn noise(w: usize, h: usize, seed: u32) -> Gray {
        let mut g = Gray::new(w, h);
        let mut s = seed;
        for v in g.v.iter_mut() {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *v = (s >> 16) as f32 / 65_536.0;
        }
        g
    }

    #[test]
    fn scores_are_ncc_and_find_a_planted_patch() {
        let img = noise(80, 40, 1);
        let mut patch = Gray::new(13, 9);
        for y in 0..9 {
            for x in 0..13 {
                patch.v[y * 13 + x] = img.v[(17 + y) * 80 + 41 + x];
            }
        }
        let t = Template::new(&patch).unwrap();
        let s = Scorer::new(&img);
        let (v, x, y) = s.best(&t, 30..60, 10..30, 0.0).unwrap();
        assert_eq!((x, y), (41, 17));
        assert!((v - 1.0).abs() < 1e-4, "{v}");
        let map = s.map(&t, 0.0);
        assert_eq!((map.w, map.h), (68, 32));
        for (y, x) in [(0, 0), (5, 33), (31, 67)] {
            let slow = ncc_at(&img, x as i64, y as i64, &patch);
            assert!((map.get(x, y) - slow).abs() < 1e-4, "{x},{y}");
        }
    }

    #[test]
    fn ranges_are_clipped_and_flat_windows_score_zero() {
        let img = noise(30, 20, 2);
        let t = Template::new(&noise(8, 6, 3)).unwrap();
        let s = Scorer::new(&img);
        // reaching past every edge: only where it fits (x 0..=22, y 0..=14)
        let map = s.map(&t, 0.0);
        let (_, x, y) = s.best(&t, -50..50, -50..50, 0.0).unwrap();
        assert!(x <= 22 && y <= 14);
        let top = map.v.iter().cloned().fold(f32::MIN, f32::max);
        assert_eq!(map.get(x, y), top);
        assert!(s.best(&t, 40..50, 0..5, 0.0).is_none());
        assert!(Template::new(&Gray::new(4, 4)).is_none());
        let flat = Gray::new(30, 20);
        let fs = Scorer::new(&flat);
        assert_eq!(fs.best(&t, 0..30, 0..20, 0.0).unwrap().0, 0.0);
        // a floor above every window's spread: all 0
        assert!(s.map(&t, 1e9).v.iter().all(|&v| v == 0.0));
    }
}
