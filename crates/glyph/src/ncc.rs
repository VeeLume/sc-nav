//! NCC without the per-pixel bookkeeping: templates centred and scaled to
//! unit norm once, window sums of the image from summed-area tables, so a
//! score is one dot product. Same scores as [`crate::ncc_at`] up to float
//! rounding.
//!
//! Public for other readers' template searches (sc-nav-vision's direct
//! NCC): the score of unit template `u` at (x, y) is the numerator from
//! [`row_dots_at`] over `spread(x, y, w, h).sqrt()`.

use crate::Gray;

/// Windows flatter than this score 0, as in [`crate::ncc_at`].
const FLAT: f64 = 1e-6;

/// `t` minus its mean, scaled to unit norm; `None` for a flat template.
pub fn unit(t: &Gray) -> Option<Vec<f32>> {
    let n = t.v.len() as f64;
    let mean = t.v.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var: f64 = t.v.iter().map(|&v| (v as f64 - mean).powi(2)).sum();
    if var <= FLAT {
        return None;
    }
    let k = var.sqrt();
    Some(
        t.v.iter()
            .map(|&v| ((v as f64 - mean) / k) as f32)
            .collect(),
    )
}

/// Summed-area tables of an image and of its square, for window variances.
pub struct Stats {
    w1: usize,
    s: Vec<f64>,
    ss: Vec<f64>,
}

impl Stats {
    pub fn new(img: &Gray) -> Self {
        let w1 = img.w + 1;
        let mut s = vec![0.0; w1 * (img.h + 1)];
        let mut ss = vec![0.0; w1 * (img.h + 1)];
        for y in 0..img.h {
            let (mut r, mut rr) = (0.0, 0.0);
            for x in 0..img.w {
                let v = img.v[y * img.w + x] as f64;
                r += v;
                rr += v * v;
                s[(y + 1) * w1 + x + 1] = s[y * w1 + x + 1] + r;
                ss[(y + 1) * w1 + x + 1] = ss[y * w1 + x + 1] + rr;
            }
        }
        Self { w1, s, ss }
    }

    /// Sum and sum of squares over the window at (x, y), `w` x `h`.
    fn sums(&self, x: usize, y: usize, w: usize, h: usize) -> (f64, f64) {
        let (a, b) = (y * self.w1 + x, (y + h) * self.w1 + x);
        let box_ = |t: &[f64]| t[b + w] - t[b] - t[a + w] + t[a];
        (box_(&self.s), box_(&self.ss))
    }

    /// Σ(v − mean)² over the window, or `None` when it is flat.
    pub fn spread(&self, x: usize, y: usize, w: usize, h: usize) -> Option<f64> {
        let var = self.var(x, y, w, h);
        (var > FLAT).then_some(var)
    }

    /// Σ(v − mean)² over the window, flat or not. The window must lie in
    /// the image.
    pub fn var(&self, x: usize, y: usize, w: usize, h: usize) -> f64 {
        let (s, ss) = self.sums(x, y, w, h);
        ss - s * s / (w * h) as f64
    }

    /// The window's mean.
    pub fn mean(&self, x: usize, y: usize, w: usize, h: usize) -> f64 {
        self.sums(x, y, w, h).0 / (w * h) as f64
    }
}

/// Dot product, eight lanes at a time.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<8>(), b.as_chunks::<8>());
    for (x, y) in ca.iter().zip(cb) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    let mut s: f32 = ra.iter().zip(rb).map(|(x, y)| x * y).sum();
    for v in acc {
        s += v;
    }
    s
}

/// [`dot`] of four vectors with the same `b`: `b` is read once for all
/// four, and each result is [`dot`]'s to the bit (the same sums in the
/// same order).
pub fn dot4(a: [&[f32]; 4], b: &[f32]) -> [f32; 4] {
    let mut acc = [[0.0f32; 8]; 4];
    let (cb, rb) = b.as_chunks::<8>();
    let ca = a.map(|x| x.as_chunks::<8>().0);
    for (k, y) in cb.iter().enumerate() {
        for q in 0..4 {
            let Some(x) = ca[q].get(k) else { continue };
            for i in 0..8 {
                acc[q][i] += x[i] * y[i];
            }
        }
    }
    std::array::from_fn(|q| {
        let ra = &a[q][ca[q].len().min(cb.len()) * 8..];
        let mut s: f32 = ra.iter().zip(rb).map(|(x, y)| x * y).sum();
        for v in acc[q] {
            s += v;
        }
        s
    })
}

/// Scores of the unit template `u` (`w` x `h`) at every top-left
/// (x, y) of `img` with x in `0..nx`, for one row offset `y`: the
/// numerators, accumulated across x so the loop vectorises. `out` is
/// overwritten.
pub fn row_dots(img: &Gray, u: &[f32], w: usize, h: usize, y: usize, out: &mut [f32]) {
    row_dots_at(img, u, w, h, 0, y, out);
}

/// [`row_dots`] for x in `x0..x0 + out.len()`. The template must fit at
/// every one of them.
pub fn row_dots_at(
    img: &Gray,
    u: &[f32],
    w: usize,
    h: usize,
    x0: usize,
    y: usize,
    out: &mut [f32],
) {
    let nx = out.len();
    out.fill(0.0);
    for ty in 0..h {
        let row = &img.v[(y + ty) * img.w + x0..(y + ty + 1) * img.w];
        for tx in 0..w {
            let c = u[ty * w + tx];
            if c == 0.0 {
                continue;
            }
            for (o, a) in out.iter_mut().zip(&row[tx..tx + nx]) {
                *o += c * a;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ncc_at;

    fn noise(w: usize, h: usize, seed: u32) -> Gray {
        let mut g = Gray::new(w, h);
        let mut s = seed;
        for v in &mut g.v {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *v = (s >> 8) as f32 / (1u32 << 24) as f32;
        }
        g
    }

    #[test]
    fn dot4_is_dot_to_the_bit() {
        let v = |n: usize, seed: u32| noise(n, 1, seed).v;
        for n in [0, 3, 8, 13, 88, 97] {
            let b = v(n, 9);
            let a: Vec<Vec<f32>> = (0..4).map(|k| v(n, 20 + k)).collect();
            let got = dot4([&a[0], &a[1], &a[2], &a[3]], &b);
            for q in 0..4 {
                assert_eq!(got[q].to_bits(), dot(&a[q], &b).to_bits(), "{n} {q}");
            }
        }
    }

    #[test]
    fn matches_ncc_at() {
        let img = noise(40, 13, 1);
        let t = noise(7, 9, 2);
        let u = unit(&t).unwrap();
        let st = Stats::new(&img);
        let nx = img.w - t.w + 1;
        let mut out = vec![0.0; nx];
        for y in 0..=img.h - t.h {
            row_dots(&img, &u, t.w, t.h, y, &mut out);
            for (x, &d) in out.iter().enumerate() {
                let fast = d / st.spread(x, y, t.w, t.h).unwrap().sqrt() as f32;
                let slow = ncc_at(&img, x as i64, y as i64, &t);
                assert!((fast - slow).abs() < 1e-4, "{x},{y}: {fast} vs {slow}");
            }
        }
    }
}
