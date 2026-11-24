//! Normalised cross-correlation (NCC) of templates over a whole image via
//! FFT, and the best peaks of a score map. Moved from Starlume's
//! `svc-capture::vision`.
//!
//! The image and templates are real, so the transforms are real-input
//! ones (half the spectrum, half the work and memory of complex ones):
//! maps equal the complex transforms' to float rounding (within 2e-5).
//! Transforms are planned once per correlator, buffers are kept per
//! thread, and templates can be [`Prepared`] once per padded size.

use std::cell::RefCell;
use std::sync::Arc;

use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
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

/// The transforms a [`Correlator`] uses, planned once: real rows (forward
/// and back), complex columns over the rows' half spectra.
struct Plans {
    row: Arc<dyn RealToComplex<f32>>,
    row_inv: Arc<dyn ComplexToReal<f32>>,
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
    /// The half spectrum: `fw / 2 + 1` columns of `fh` (the image is real,
    /// so the other half mirrors it).
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

/// `src` (`w` x `h`, row-major) transposed into `dst` (`h` x `w`), in
/// tiles so both sides stay in cache.
fn transpose<T: Copy>(src: &[T], dst: &mut [T], w: usize, h: usize) {
    const TILE: usize = 32;
    for y0 in (0..h).step_by(TILE) {
        for x0 in (0..w).step_by(TILE) {
            for y in y0..(y0 + TILE).min(h) {
                for x in x0..(x0 + TILE).min(w) {
                    dst[x * h + y] = src[y * w + x];
                }
            }
        }
    }
}

const ZERO: Complex<f32> = Complex::new(0.0, 0.0);

/// Buffers kept per thread: a fresh 7 MB buffer per call costs its page
/// faults again every time.
#[derive(Default)]
struct Bufs {
    row: Vec<f32>,
    cols: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

thread_local! {
    static BUFS: RefCell<Bufs> = RefCell::new(Bufs::default());
}

impl Plans {
    fn new(fw: usize, fh: usize) -> Self {
        let mut real = RealFftPlanner::new();
        let mut complex = FftPlanner::new();
        Self {
            row: real.plan_fft_forward(fw),
            row_inv: real.plan_fft_inverse(fw),
            col: complex.plan_fft_forward(fh),
            col_inv: complex.plan_fft_inverse(fh),
        }
    }

    fn scratch_len(&self) -> usize {
        self.row
            .get_scratch_len()
            .max(self.row_inv.get_scratch_len())
            .max(self.col.get_inplace_scratch_len())
            .max(self.col_inv.get_inplace_scratch_len())
    }

    /// The 2D half spectrum of `src` (`fw` x `fh`, real) into `out`.
    fn forward(&self, src: &[f32], out: &mut Vec<Complex<f32>>, fw: usize, fh: usize) {
        let hw = fw / 2 + 1;
        out.clear();
        out.resize(hw * fh, ZERO);
        BUFS.with_borrow_mut(|b| {
            b.scratch.resize(self.scratch_len(), ZERO);
            b.row.resize(fw, 0.0);
            let n = self.row.get_scratch_len();
            for (y, spec) in out.chunks_exact_mut(hw).enumerate() {
                b.row.copy_from_slice(&src[y * fw..(y + 1) * fw]);
                // lengths are the plan's, so this cannot fail
                let _ = self
                    .row
                    .process_with_scratch(&mut b.row, spec, &mut b.scratch[..n]);
            }
            b.cols.resize(hw * fh, ZERO);
            transpose(out, &mut b.cols, hw, fh);
            let n = self.col.get_inplace_scratch_len();
            self.col
                .process_with_scratch(&mut b.cols, &mut b.scratch[..n]);
            transpose(&b.cols, out, fh, hw);
        });
    }

    /// The real 2D inverse (unnormalised) of half spectrum `spec` into
    /// `out` (`fw` x `fh`). `spec` is used as working space.
    fn inverse(&self, spec: &mut [Complex<f32>], out: &mut Vec<f32>, fw: usize, fh: usize) {
        let hw = fw / 2 + 1;
        out.clear();
        out.resize(fw * fh, 0.0);
        BUFS.with_borrow_mut(|b| {
            b.scratch.resize(self.scratch_len(), ZERO);
            b.cols.resize(hw * fh, ZERO);
            transpose(spec, &mut b.cols, hw, fh);
            let n = self.col_inv.get_inplace_scratch_len();
            self.col_inv
                .process_with_scratch(&mut b.cols, &mut b.scratch[..n]);
            transpose(&b.cols, spec, fh, hw);
            let n = self.row_inv.get_scratch_len();
            for (row, o) in spec.chunks_exact_mut(hw).zip(out.chunks_exact_mut(fw)) {
                // A real row's spectrum has real DC and Nyquist bins; what
                // the column pass leaves there is rounding.
                row[0].im = 0.0;
                if fw.is_multiple_of(2) {
                    row[hw - 1].im = 0.0;
                }
                let _ = self
                    .row_inv
                    .process_with_scratch(row, o, &mut b.scratch[..n]);
            }
        });
    }
}

impl Correlator {
    pub fn new(img: &Gray) -> Self {
        let (fw, fh) = (fast_len(img.w), fast_len(img.h));
        let plans = Plans::new(fw, fh);
        let mut padded = vec![0.0f32; fw * fh];
        for y in 0..img.h {
            padded[y * fw..y * fw + img.w].copy_from_slice(&img.v[y * img.w..(y + 1) * img.w]);
        }
        let mut spectrum = Vec::new();
        plans.forward(&padded, &mut spectrum, fw, fh);
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
        let mut spectrum = Vec::new();
        let tn = self.transform(t, &mut spectrum)?;
        Some(Prepared {
            tw: t.w,
            th: t.h,
            fw: self.fw,
            fh: self.fh,
            tn,
            spectrum,
        })
    }

    /// `t` minus its mean, transformed into `spectrum`; its Σ(t − mean)².
    fn transform(&self, t: &Gray, spectrum: &mut Vec<Complex<f32>>) -> Option<f64> {
        if t.w > self.fw || t.h > self.fh || t.v.is_empty() || t.v.len() != t.w * t.h {
            return None;
        }
        let n = (t.w * t.h) as f64;
        let mean = t.v.iter().map(|&x| x as f64).sum::<f64>() / n;
        let mut padded = vec![0.0f32; self.fw * self.fh];
        let mut tn = 0.0f64;
        for y in 0..t.h {
            for x in 0..t.w {
                let z = (t.v[y * t.w + x] as f64 - mean) as f32;
                tn += (z as f64) * (z as f64);
                padded[y * self.fw + x] = z;
            }
        }
        self.plans.forward(&padded, spectrum, self.fw, self.fh);
        Some(tn)
    }

    /// NCC of `t` at every position where it fits whole: a (w − tw + 1) x
    /// (h − th + 1) map in −1..1. Nearly flat windows (empty sky,
    /// saturated glare) have no shape to match and score 0.
    pub fn ncc(&self, t: &Gray) -> Gray {
        if t.w > self.w || t.h > self.h {
            return Gray::new(0, 0);
        }
        // a prepared template in a buffer kept per thread
        thread_local! {
            static TEMPLATE: RefCell<Prepared> = const {
                RefCell::new(Prepared { tw: 0, th: 0, fw: 0, fh: 0, tn: 0.0, spectrum: Vec::new() })
            };
        }
        TEMPLATE.with_borrow_mut(|p| {
            let Some(tn) = self.transform(t, &mut p.spectrum) else {
                return Gray::new(0, 0);
            };
            (p.tw, p.th, p.fw, p.fh, p.tn) = (t.w, t.h, self.fw, self.fh, tn);
            self.ncc_prepared(p)
        })
    }

    /// [`Correlator::ncc`] of a prepared template: the same map, without
    /// transforming the template again. An empty map when `t` was prepared
    /// for another padded size or does not fit the image.
    pub fn ncc_prepared(&self, t: &Prepared) -> Gray {
        if (t.fw, t.fh) != (self.fw, self.fh) || t.tw > self.w || t.th > self.h {
            return Gray::new(0, 0);
        }
        thread_local! {
            static WORK: RefCell<(Vec<Complex<f32>>, Vec<f32>)> =
                const { RefCell::new((Vec::new(), Vec::new())) };
        }
        WORK.with_borrow_mut(|(prod, nums)| {
            prod.clear();
            prod.extend(
                self.spectrum
                    .iter()
                    .zip(&t.spectrum)
                    .map(|(b, a)| b * a.conj()),
            );
            self.plans.inverse(prod, nums, self.fw, self.fh);
            self.scores(t, nums)
        })
    }

    /// The map from the correlation's numerators (`fw` x `fh`,
    /// unnormalised) and the windows' sums.
    fn scores(&self, t: &Prepared, nums: &[f32]) -> Gray {
        let (tw, th, tn) = (t.tw, t.th, t.tn);
        let n = (tw * th) as f64;
        let scale = 1.0 / (self.fw * self.fh) as f64;
        let (ow, oh) = (self.w - tw + 1, self.h - th + 1);
        let iw = self.w + 1;
        let floor = 0.02 * tn;
        let tn_sqrt = tn.sqrt();
        let mut out = vec![0.0f32; ow * oh];
        if tn > 0.0 {
            for (y, row) in out.chunks_exact_mut(ow).enumerate() {
                // the window's box sums: bottom row − top row, per side
                let (top, bottom) = (y * iw, (y + th) * iw);
                let (s0, s1r) = (&self.sum[top..top + iw], &self.sum[bottom..bottom + iw]);
                let (q0, q1r) = (&self.sq[top..top + iw], &self.sq[bottom..bottom + iw]);
                let nums = &nums[y * self.fw..y * self.fw + ow];
                for (x, o) in row.iter_mut().enumerate() {
                    let s1 = s1r[x + tw] - s0[x + tw] - s1r[x] + s0[x];
                    let s2 = q1r[x + tw] - q0[x + tw] - q1r[x] + q0[x];
                    let var = s2 - s1 * s1 / n;
                    if var > floor {
                        let num = nums[x] as f64 * scale;
                        *o = (num / (var.sqrt() * tn_sqrt)).clamp(-1.0, 1.0) as f32;
                    }
                }
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
        let Some((i, v)) = top(&m) else {
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

/// The largest value by `total_cmp` and its index, the last on ties: what
/// `max_by(total_cmp)` gives, with the scan vectorised. Zeros (whose sign
/// `total_cmp` tells apart) and NaN take the plain scan.
fn top(m: &[f32]) -> Option<(usize, f32)> {
    let slow = |m: &[f32]| {
        m.iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, &v)| (i, v))
    };
    let (chunks, rest) = m.as_chunks::<8>();
    let mut lanes = [f32::NEG_INFINITY; 8];
    let mut nan = false;
    for c in chunks {
        for j in 0..8 {
            nan |= c[j].is_nan();
            lanes[j] = if c[j] > lanes[j] { c[j] } else { lanes[j] };
        }
    }
    let mut best = f32::NEG_INFINITY;
    for &v in lanes.iter().chain(rest) {
        nan |= v.is_nan();
        if v > best {
            best = v;
        }
    }
    if m.is_empty() || nan || best == 0.0 || best == f32::NEG_INFINITY {
        return slow(m);
    }
    // non-zero and not NaN: equal values have equal bits. The last one,
    // looked for a block at a time from the end.
    const BLOCK: usize = 64;
    let mut end = m.len();
    while end > 0 {
        let start = end.saturating_sub(BLOCK);
        let block = &m[start..end];
        if block.iter().fold(false, |hit, &v| hit | (v == best)) {
            return block
                .iter()
                .rposition(|&v| v == best)
                .map(|i| (start + i, best));
        }
        end = start;
    }
    None
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

    /// fft2 as it was: one call per row, one column at a time.
    fn fft2_ref(
        data: &mut [Complex<f32>],
        fw: usize,
        fh: usize,
        row: &dyn Fft<f32>,
        col: &dyn Fft<f32>,
    ) {
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
            fft2_ref(d, fw, fh, &*row, &*col);
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
    fn maps_are_the_complex_ones_to_rounding_prepared_or_not() {
        let close = |a: &[f32], b: &[f32]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-5)
        };
        // even and odd padded widths
        for (w, h, ow, oh) in [(70, 45, 68, 41), (45, 32, 44, 31)] {
            let img = noise(w, h, 3);
            let other = noise(ow, oh, 4); // the same padded size
            let c = Correlator::new(&img);
            let d = Correlator::new(&other);
            assert_eq!(c.padded(), d.padded());
            for (tw, th, seed) in [(9, 7, 5), (20, 4, 6), (1, 1, 7)] {
                let t = noise(tw, th, seed);
                let want = ncc_ref(&img, &t).v;
                let plain = c.ncc(&t).v;
                assert!(close(&plain, &want), "{w}x{h} {tw}x{th}");
                let p = d.prepare(&t).unwrap();
                assert_eq!(c.ncc_prepared(&p).v, plain, "prepared is the same map");
                assert!(close(&d.ncc_prepared(&p).v, &ncc_ref(&other, &t).v));
            }
        }
        // another padded size: refused, not wrong
        let c = Correlator::new(&noise(70, 45, 3));
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
    #[ignore = "timing"]
    fn top_timing() {
        let g = noise(1221, 711, 14);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(top(&g.v));
        }
        println!("top: {:.3} ms", t.elapsed().as_secs_f64() * 1e3 / 20.0);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(g.v.clone());
        }
        println!("clone: {:.3} ms", t.elapsed().as_secs_f64() * 1e3 / 20.0);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(peaks(&g, 8, 10, 0.2));
        }
        println!("peaks: {:.3} ms", t.elapsed().as_secs_f64() * 1e3 / 20.0);
    }

    #[test]
    fn top_is_max_by_total_cmp() {
        let slow = |m: &[f32]| {
            m.iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, &v)| (i, v.to_bits()))
        };
        let g = noise(37, 11, 13);
        let mut cases: Vec<Vec<f32>> = vec![
            vec![],
            vec![0.0, -0.0, 0.0],
            vec![-0.0, 0.0, -0.0],
            vec![-1.0, -2.0, -1.0],
            vec![f32::MIN; 20],
            vec![f32::NEG_INFINITY, f32::MIN],
            vec![0.5, f32::NAN, 0.7],
            g.v.clone(),
        ];
        let mut ties = g.v.clone();
        ties[3] = 2.0;
        ties[300] = 2.0;
        cases.push(ties);
        for m in cases {
            assert_eq!(top(&m).map(|(i, v)| (i, v.to_bits())), slow(&m), "{m:?}");
        }
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
