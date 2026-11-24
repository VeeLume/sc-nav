//! A grey image (row-major `f32`) and the pixel tools the readers build
//! their ink images with: ramps, grey morphology, blur, resizing.
//!
//! Pure: no capture, no file formats. svc-capture re-exports it as
//! `svc_capture::vision::Gray` and adds what needs a frame.

/// A grey image, row-major, f32.
#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

impl std::fmt::Debug for Gray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gray({}x{})", self.w, self.h)
    }
}

impl Gray {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            v: vec![0.0; w * h],
        }
    }

    /// A u8 image (0..255) as 0..1.
    pub fn from_u8(w: usize, h: usize, px: &[u8]) -> Self {
        Self {
            w,
            h,
            v: px.iter().map(|&p| p as f32 / 255.0).collect(),
        }
    }

    pub fn map(&self, f: impl Fn(f32) -> f32) -> Self {
        Self {
            w: self.w,
            h: self.h,
            v: self.v.iter().map(|&x| f(x)).collect(),
        }
    }

    /// (v − lo) / (hi − lo), clipped to 0..1.
    pub fn ramp(&self, lo: f32, hi: f32) -> Self {
        self.map(|x| ((x - lo) / (hi - lo)).clamp(0.0, 1.0))
    }

    /// `take` (max or min) over a k x k square, edges clamped: a pass along
    /// the rows, then one down the columns. Both run on whole slices so
    /// they vectorise; max and min are exact in any order, so this is the
    /// pixel-by-pixel filter's result.
    fn filter(&self, k: usize, take: impl Fn(f32, f32) -> f32 + Copy) -> Self {
        let r = k / 2;
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            return self.clone();
        }
        let mut tmp = self.v.clone();
        for (src, dst) in self.v.chunks_exact(w).zip(tmp.chunks_exact_mut(w)) {
            // edges, where the window is clamped
            let edge = |x: usize| {
                let mut m = src[x];
                for &v in &src[x.saturating_sub(r)..=(x + r).min(w - 1)] {
                    m = take(m, v);
                }
                m
            };
            if w <= 2 * r {
                for (x, d) in dst.iter_mut().enumerate() {
                    *d = edge(x);
                }
                continue;
            }
            for x in (0..r).chain(w - r..w) {
                dst[x] = edge(x);
            }
            let inner = &mut dst[r..w - r];
            for d in 0..=2 * r {
                for (m, &v) in inner.iter_mut().zip(&src[d..w - 2 * r + d]) {
                    *m = take(*m, v);
                }
            }
        }
        let mut out = tmp.clone();
        for y in 0..h {
            let row = &mut out[y * w..(y + 1) * w];
            for yy in y.saturating_sub(r)..=(y + r).min(h - 1) {
                for (m, &v) in row.iter_mut().zip(&tmp[yy * w..(yy + 1) * w]) {
                    *m = take(*m, v);
                }
            }
        }
        Self { w, h, v: out }
    }

    /// Grey dilation (max) over a k x k square.
    pub fn max_filter(&self, k: usize) -> Self {
        self.filter(k, f32::max)
    }

    /// Grey erosion (min) over a k x k square.
    pub fn min_filter(&self, k: usize) -> Self {
        self.filter(k, f32::min)
    }

    /// Shadow ink: (closing − v) / gain, clipped to 0..1 (closing = a max
    /// then a min filter over k x k).
    pub fn blackhat(&self, k: usize, gain: f32) -> Self {
        let closed = self.max_filter(k).min_filter(k);
        Self {
            w: self.w,
            h: self.h,
            v: closed
                .v
                .iter()
                .zip(&self.v)
                .map(|(c, x)| ((c - x) / gain).clamp(0.0, 1.0))
                .collect(),
        }
    }

    /// Gaussian blur (separable, kernel ±3σ, edges clamped).
    pub fn blur(&self, sigma: f32) -> Self {
        if sigma <= 0.0 {
            return self.clone();
        }
        let r = (3.0 * sigma).ceil() as isize;
        let k: Vec<f32> = (-r..=r)
            .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
            .collect();
        let sum: f32 = k.iter().sum();
        let k: Vec<f32> = k.into_iter().map(|v| v / sum).collect();
        let (w, h) = (self.w as isize, self.h as isize);
        let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
            let mut out = vec![0.0; src.len()];
            for y in 0..h {
                for x in 0..w {
                    let mut acc = 0.0;
                    for (j, kv) in k.iter().enumerate() {
                        let d = j as isize - r;
                        let (xx, yy) = if horizontal {
                            ((x + d).clamp(0, w - 1), y)
                        } else {
                            (x, (y + d).clamp(0, h - 1))
                        };
                        acc += kv * src[(yy * w + xx) as usize];
                    }
                    out[(y * w + x) as usize] = acc;
                }
            }
            out
        };
        let v = pass(&pass(&self.v, true), false);
        Self {
            w: self.w,
            h: self.h,
            v,
        }
    }

    /// Padded by `n` zero pixels on every side.
    pub fn padded(&self, n: usize) -> Self {
        let (w, h) = (self.w + 2 * n, self.h + 2 * n);
        let mut v = vec![0.0; w * h];
        for y in 0..self.h {
            for x in 0..self.w {
                v[(y + n) * w + x + n] = self.v[y * self.w + x];
            }
        }
        Self { w, h, v }
    }

    /// Half size, each pixel the mean of a 2x2 block.
    pub fn half(&self) -> Self {
        let (w, h) = (self.w / 2, self.h / 2);
        let mut v = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let i = 2 * y * self.w + 2 * x;
                v.push(
                    (self.v[i] + self.v[i + 1] + self.v[i + self.w] + self.v[i + self.w + 1]) / 4.0,
                );
            }
        }
        Self { w, h, v }
    }

    /// Resized by `s` with a triangle filter (PIL's BILINEAR: the filter
    /// widens when shrinking, so a downscale averages instead of aliasing).
    pub fn resized(&self, s: f32) -> Self {
        let w = ((self.w as f32 * s).round() as usize).max(2);
        let h = ((self.h as f32 * s).round() as usize).max(2);
        let rows = resample_axis(&self.v, self.w, self.h, w, true);
        let v = resample_axis(&rows, w, self.h, h, false);
        Self { w, h, v }
    }

    /// Bilinear sample; 0 outside.
    pub fn at(&self, x: f32, y: f32) -> f32 {
        if x < 0.0 || y < 0.0 || x >= (self.w - 1) as f32 || y >= (self.h - 1) as f32 {
            return 0.0;
        }
        let (x0, y0) = (x as usize, y as usize);
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let i = y0 * self.w + x0;
        self.v[i] * (1.0 - tx) * (1.0 - ty)
            + self.v[i + 1] * tx * (1.0 - ty)
            + self.v[i + self.w] * (1.0 - tx) * ty
            + self.v[i + self.w + 1] * tx * ty
    }

    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.v[y * self.w + x]
    }
}

/// One axis of a PIL-style triangle resample: `src` is (sw x sh); the
/// axis is x when `horizontal`, else y; the other size is kept.
fn resample_axis(src: &[f32], sw: usize, sh: usize, n: usize, horizontal: bool) -> Vec<f32> {
    let len = if horizontal { sw } else { sh };
    let scale = len as f32 / n as f32;
    let support = scale.max(1.0);
    // per output index: (first source index, weights)
    let taps: Vec<(usize, Vec<f32>)> = (0..n)
        .map(|i| {
            let center = (i as f32 + 0.5) * scale;
            let lo = ((center - support).floor().max(0.0)) as usize;
            let hi = ((center + support).ceil() as usize).min(len);
            let mut ws: Vec<f32> = (lo..hi)
                .map(|j| {
                    let d = ((j as f32 + 0.5) - center) / support;
                    (1.0 - d.abs()).max(0.0)
                })
                .collect();
            let sum: f32 = ws.iter().sum();
            if sum > 0.0 {
                ws.iter_mut().for_each(|w| *w /= sum);
            }
            (lo, ws)
        })
        .collect();
    let (ow, oh) = if horizontal { (n, sh) } else { (sw, n) };
    let mut out = vec![0.0; ow * oh];
    for y in 0..oh {
        for x in 0..ow {
            let (i, other) = if horizontal { (x, y) } else { (y, x) };
            let (lo, ws) = &taps[i];
            let mut acc = 0.0;
            for (k, w) in ws.iter().enumerate() {
                let j = lo + k;
                let s = if horizontal {
                    src[other * sw + j]
                } else {
                    src[j * sw + other]
                };
                acc += w * s;
            }
            out[y * ow + x] = acc;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_keeps_a_flat_image_flat() {
        let img = Gray::new(40, 20).map(|_| 0.5);
        let r = img.resized(0.37);
        assert!(r.v.iter().all(|&v| (v - 0.5).abs() < 1e-5));
    }

    /// The filter as it was, pixel by pixel with clamped indices.
    fn filter_ref(g: &Gray, k: usize, take: fn(f32, f32) -> f32) -> Vec<f32> {
        let r = k as isize / 2;
        let (w, h) = (g.w as isize, g.h as isize);
        let mut tmp = vec![0.0; g.v.len()];
        for y in 0..h {
            for x in 0..w {
                let mut m = g.v[(y * w + x) as usize];
                for d in -r..=r {
                    m = take(m, g.v[(y * w + (x + d).clamp(0, w - 1)) as usize]);
                }
                tmp[(y * w + x) as usize] = m;
            }
        }
        let mut out = vec![0.0; g.v.len()];
        for y in 0..h {
            for x in 0..w {
                let mut m = tmp[(y * w + x) as usize];
                for d in -r..=r {
                    m = take(m, tmp[((y + d).clamp(0, h - 1) * w + x) as usize]);
                }
                out[(y * w + x) as usize] = m;
            }
        }
        out
    }

    #[test]
    fn filters_match_the_pixel_by_pixel_ones() {
        let mut seed = 7u32;
        for (w, h) in [(1, 1), (3, 2), (4, 9), (5, 5), (6, 3), (37, 23)] {
            let mut g = Gray::new(w, h);
            for v in &mut g.v {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *v = (seed >> 16) as f32;
            }
            for k in [1, 3, 5, 7] {
                assert_eq!(
                    g.max_filter(k).v,
                    filter_ref(&g, k, f32::max),
                    "{w}x{h} k{k}"
                );
                assert_eq!(
                    g.min_filter(k).v,
                    filter_ref(&g, k, f32::min),
                    "{w}x{h} k{k}"
                );
            }
        }
    }
}
