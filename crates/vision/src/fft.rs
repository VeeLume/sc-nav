//! Normalised cross-correlation (NCC) of templates over a whole image via
//! FFT, and the best peaks of a score map. Moved from Starlume's
//! `svc-capture::vision`; the maps are unchanged, the transforms are planned
//! once per correlator and templates can be [`Prepared`] once.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use sc_nav_glyph::Gray;

/// The smallest n ≥ `min` whose prime factors are 2, 3 and 5 (fast FFTs).
fn fast_len(min: usize) -> usize {
    let mut n = min.max(1);
    loop {
        let mut m = n;
        for p in [2, 3, 5] {
            while m.is_multiple_of(p) {
                m /= p;
            }
        }
        if m == 1 {
            return n;
        }
        n += 1;
    }
}

/// The four transforms a [`Correlator`] uses, planned once.
struct Plans {
    row: Arc<dyn Fft<f32>>,
    row_inv: Arc<dyn Fft<f32>>,
    col: Arc<dyn Fft<f32>>,
    col_inv: Arc<dyn Fft<f32>>,
}

/// An image prepared for correlating many templates against it: its
/// spectrum and integral images (sum, sum of squares).
///
/// Read-only once built (`Sync`): threads can share one correlator per
/// image instead of each transforming the image again.
pub struct Correlator {
    w: usize,
    h: usize,
    fw: usize,
    fh: usize,
    spectrum: Vec<Complex<f32>>,
    sum: Vec<f64>,
    sq: Vec<f64>,
    plans: Plans,
}

/// A template ready for every [`Correlator`] of the same padded size
/// ([`Correlator::padded`]): its zero-mean spectrum, computed once. A
/// template searched in frame after frame of the same size is
/// transformed once instead of on every search.
pub struct Prepared {
    tw: usize,
    th: usize,
    fw: usize,
    fh: usize,
    /// Σ(t − mean)².
    tn: f64,
    spectrum: Vec<Complex<f32>>,
}

impl Prepared {
    /// The template's size.
    pub fn size(&self) -> (usize, usize) {
        (self.tw, self.th)
    }

    /// The padded size it was transformed at.
    pub fn padded(&self) -> (usize, usize) {
        (self.fw, self.fh)
    }
}

fn fft2(data: &mut [Complex<f32>], fw: usize, fh: usize, row: &dyn Fft<f32>, col: &dyn Fft<f32>) {
    for r in data.chunks_exact_mut(fw) {
        row.process(r);
    }
    let mut buf = vec![Complex::new(0.0, 0.0); fh];
    for x in 0..fw {
        for y in 0..fh {
            buf[y] = data[y * fw + x];
        }
        col.process(&mut buf);
        for y in 0..fh {
            data[y * fw + x] = buf[y];
        }
    }
}

impl Correlator {
    pub fn new(img: &Gray) -> Self {
        let (fw, fh) = (fast_len(img.w), fast_len(img.h));
        let mut planner = FftPlanner::new();
        let plans = Plans {
            row: planner.plan_fft_forward(fw),
            row_inv: planner.plan_fft_inverse(fw),
            col: planner.plan_fft_forward(fh),
            col_inv: planner.plan_fft_inverse(fh),
        };
        let mut spectrum = vec![Complex::new(0.0, 0.0); fw * fh];
        for y in 0..img.h {
            for x in 0..img.w {
                spectrum[y * fw + x].re = img.v[y * img.w + x];
            }
        }
        fft2(&mut spectrum, fw, fh, &*plans.row, &*plans.col);
        let (iw, ih) = (img.w + 1, img.h + 1);
        let mut sum = vec![0.0f64; iw * ih];
        let mut sq = vec![0.0f64; iw * ih];
        for y in 0..img.h {
            let (mut rs, mut rq) = (0.0f64, 0.0f64);
            for x in 0..img.w {
                let v = img.v[y * img.w + x] as f64;
                rs += v;
                rq += v * v;
                sum[(y + 1) * iw + x + 1] = sum[y * iw + x + 1] + rs;
                sq[(y + 1) * iw + x + 1] = sq[y * iw + x + 1] + rq;
            }
        }
        Self {
            w: img.w,
            h: img.h,
            fw,
            fh,
            spectrum,
            sum,
            sq,
            plans,
        }
    }

    /// The image's size.
    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// The padded size the transforms run at; [`Prepared`] templates are
    /// shared by every correlator with the same one.
    pub fn padded(&self) -> (usize, usize) {
        (self.fw, self.fh)
    }

    /// `t` transformed for this correlator's padded size, or `None` when it
    /// is larger than the padded size.
    pub fn prepare(&self, t: &Gray) -> Option<Prepared> {
        if t.w > self.fw || t.h > self.fh || t.v.is_empty() {
            return None;
        }
        let n = (t.w * t.h) as f64;
        let mean = t.v.iter().map(|&x| x as f64).sum::<f64>() / n;
        let tz: Vec<f32> = t.v.iter().map(|&x| (x as f64 - mean) as f32).collect();
        let tn = tz.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>();
        let mut spectrum = vec![Complex::new(0.0, 0.0); self.fw * self.fh];
        for y in 0..t.h {
            for x in 0..t.w {
                spectrum[y * self.fw + x].re = tz[y * t.w + x];
            }
        }
        fft2(
            &mut spectrum,
            self.fw,
            self.fh,
            &*self.plans.row,
            &*self.plans.col,
        );
        Some(Prepared {
            tw: t.w,
            th: t.h,
            fw: self.fw,
            fh: self.fh,
            tn,
            spectrum,
        })
    }

    /// NCC of `t` at every position where it fits whole: a (w − tw + 1) x
    /// (h − th + 1) map in −1..1. Nearly flat windows (empty sky,
    /// saturated glare) have no shape to match and score 0.
    pub fn ncc(&self, t: &Gray) -> Gray {
        if t.w > self.w || t.h > self.h {
            return Gray::new(0, 0);
        }
        match self.prepare(t) {
            Some(p) => self.ncc_prepared(&p),
            None => Gray::new(0, 0),
        }
    }

    /// [`Correlator::ncc`] of a prepared template: the same map, without
    /// transforming the template again. An empty map when `t` was prepared
    /// for another padded size or does not fit the image.
    pub fn ncc_prepared(&self, t: &Prepared) -> Gray {
        if (t.fw, t.fh) != (self.fw, self.fh) || t.tw > self.w || t.th > self.h {
            return Gray::new(0, 0);
        }
        let (tw, th) = (t.tw, t.th);
        let n = (tw * th) as f64;
        let tn = t.tn;
        let mut ts: Vec<Complex<f32>> = self
            .spectrum
            .iter()
            .zip(&t.spectrum)
            .map(|(b, a)| b * a.conj())
            .collect();
        fft2(
            &mut ts,
            self.fw,
            self.fh,
            &*self.plans.row_inv,
            &*self.plans.col_inv,
        );
        let scale = 1.0 / (self.fw * self.fh) as f64;
        let (ow, oh) = (self.w - tw + 1, self.h - th + 1);
        let iw = self.w + 1;
        let floor = 0.02 * tn;
        let mut out = Vec::with_capacity(ow * oh);
        for y in 0..oh {
            for x in 0..ow {
                let bx = |a: &[f64]| {
                    a[(y + th) * iw + x + tw] - a[y * iw + x + tw] - a[(y + th) * iw + x]
                        + a[y * iw + x]
                };
                let s1 = bx(&self.sum);
                let s2 = bx(&self.sq);
                let var = s2 - s1 * s1 / n;
                let num = ts[y * self.fw + x].re as f64 * scale;
                out.push(if var > floor && tn > 0.0 {
                    (num / (var.sqrt() * tn.sqrt())).clamp(-1.0, 1.0) as f32
                } else {
                    0.0
                });
            }
        }
        Gray {
            w: ow,
            h: oh,
            v: out,
        }
    }
}

/// The `k` best local maxima of a score map above `min`, at least `sep`
/// pixels apart: (score, x, y).
pub fn peaks(map: &Gray, k: usize, sep: usize, min: f32) -> Vec<(f32, usize, usize)> {
    let mut m = map.v.clone();
    let mut out = Vec::new();
    for _ in 0..k {
        let Some((i, &v)) = m.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
            break;
        };
        if v <= min {
            break;
        }
        let (x, y) = (i % map.w, i / map.w);
        out.push((v, x, y));
        for yy in y.saturating_sub(sep)..(y + sep + 1).min(map.h) {
            for xx in x.saturating_sub(sep)..(x + sep + 1).min(map.w) {
                m[yy * map.w + xx] = f32::MIN;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ncc_finds_a_planted_patch() {
        let mut img = Gray::new(64, 48);
        let mut seed = 12345u32;
        for v in img.v.iter_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *v = (seed >> 16) as f32 / 65_536.0;
        }
        let mut t = Gray::new(9, 7);
        for y in 0..7 {
            for x in 0..9 {
                t.v[y * 9 + x] = img.v[(20 + y) * 64 + 30 + x];
            }
        }
        let c = Correlator::new(&img);
        let map = c.ncc(&t);
        let (v, x, y) = peaks(&map, 1, 2, 0.0)[0];
        assert_eq!((x, y), (30, 20));
        assert!((v - 1.0).abs() < 1e-3, "{v}");
    }

    /// ncc as it was moved from Starlume: a planner per call.
    fn ncc_ref(img: &Gray, t: &Gray) -> Gray {
        let (fw, fh) = (fast_len(img.w), fast_len(img.h));
        let mut planner = FftPlanner::new();
        let run = |planner: &mut FftPlanner<f32>, d: &mut [Complex<f32>], inv: bool| {
            let (row, col) = if inv {
                (planner.plan_fft_inverse(fw), planner.plan_fft_inverse(fh))
            } else {
                (planner.plan_fft_forward(fw), planner.plan_fft_forward(fh))
            };
            fft2(d, fw, fh, &*row, &*col);
        };
        let mut spec = vec![Complex::new(0.0, 0.0); fw * fh];
        for y in 0..img.h {
            for x in 0..img.w {
                spec[y * fw + x].re = img.v[y * img.w + x];
            }
        }
        run(&mut planner, &mut spec, false);
        let c = Correlator::new(img);
        let n = (t.w * t.h) as f64;
        let mean = t.v.iter().map(|&x| x as f64).sum::<f64>() / n;
        let tz: Vec<f32> = t.v.iter().map(|&x| (x as f64 - mean) as f32).collect();
        let tn = tz.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>();
        let mut ts = vec![Complex::new(0.0, 0.0); fw * fh];
        for y in 0..t.h {
            for x in 0..t.w {
                ts[y * fw + x].re = tz[y * t.w + x];
            }
        }
        run(&mut planner, &mut ts, false);
        for (a, b) in ts.iter_mut().zip(&spec) {
            *a = b * a.conj();
        }
        run(&mut planner, &mut ts, true);
        let scale = 1.0 / (fw * fh) as f64;
        let (ow, oh) = (img.w - t.w + 1, img.h - t.h + 1);
        let iw = img.w + 1;
        let mut v = Vec::new();
        for y in 0..oh {
            for x in 0..ow {
                let bx = |a: &[f64]| {
                    a[(y + t.h) * iw + x + t.w] - a[y * iw + x + t.w] - a[(y + t.h) * iw + x]
                        + a[y * iw + x]
                };
                let (s1, s2) = (bx(&c.sum), bx(&c.sq));
                let var = s2 - s1 * s1 / n;
                let num = ts[y * fw + x].re as f64 * scale;
                v.push(if var > 0.02 * tn && tn > 0.0 {
                    (num / (var.sqrt() * tn.sqrt())).clamp(-1.0, 1.0) as f32
                } else {
                    0.0
                });
            }
        }
        Gray { w: ow, h: oh, v }
    }

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
    fn maps_are_the_original_ones_prepared_or_not() {
        let img = noise(70, 45, 3);
        let other = noise(68, 41, 4); // same padded size (72 x 45)
        let c = Correlator::new(&img);
        let d = Correlator::new(&other);
        assert_eq!(c.padded(), d.padded());
        for (tw, th, seed) in [(9, 7, 5), (20, 4, 6), (1, 1, 7)] {
            let t = noise(tw, th, seed);
            let want = ncc_ref(&img, &t).v;
            assert_eq!(c.ncc(&t).v, want);
            let p = d.prepare(&t).unwrap();
            assert_eq!(c.ncc_prepared(&p).v, want);
            assert_eq!(d.ncc_prepared(&p).v, ncc_ref(&other, &t).v);
        }
        // another padded size: refused, not wrong
        let small = Correlator::new(&noise(30, 30, 8));
        let p = c.prepare(&noise(5, 5, 9)).unwrap();
        assert_eq!(small.ncc_prepared(&p).v.len(), 0);
    }

    #[test]
    fn correlators_are_shared_between_threads() {
        fn sync<T: Sync + Send>() {}
        sync::<Correlator>();
        sync::<Prepared>();
    }

    #[test]
    fn flat_windows_score_zero() {
        let img = Gray::new(32, 32);
        let mut t = Gray::new(5, 5);
        t.v[12] = 1.0;
        let map = Correlator::new(&img).ncc(&t);
        assert!(map.v.iter().all(|&v| v == 0.0));
    }
}
