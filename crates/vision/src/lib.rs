//! Image tools for reading HUD panels off a screenshot: the two feature
//! images readers match on, rotation (of a whole frame or only the
//! rectangle read), bands resampled at a spot and scale, and normalised
//! cross-correlation (NCC) two ways:
//! - [`Correlator`]: a template over a whole image via FFT (finding a
//!   panel anywhere); templates [`Prepared`] once per padded size.
//! - [`Scorer`] and [`Template`]: directly, at a range of positions (the
//!   check around a known spot, a name slid over its band).
//!
//! Feature images, both from the brightest channel v of each pixel:
//! - brightness ink ([`Gray::ramp`] high up): HUD text is near-saturated
//!   (250+) with a dark outline; a ramp high up keeps the glow out of the
//!   glyphs' holes.
//! - shadow ink ([`Gray::blackhat`]): the grey closing minus v, thin dark
//!   lines (the glyphs' drop shadow and their holes). Under glare,
//!   everything else saturates and these stay dark.
//!
//! Pure: images in, images out. Frames are [`image::RgbaImage`]. Moved from
//! Starlume (`svc-capture::vision`, `mod-mining`'s `Features` and `patch`);
//! what was moved gives the same pixels and maps as before, faster.

mod direct;
mod fft;

use image::RgbaImage;
pub use sc_nav_glyph::Gray;

pub use direct::{Scorer, Template};
pub use fft::{Correlator, Prepared, peaks};

/// The brightest of R, G, B per pixel, 0..255.
pub fn brightest(frame: &RgbaImage) -> Gray {
    let v = frame
        .as_raw()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[0].max(p[1]).max(p[2]) as f32)
        .collect();
    Gray {
        w: frame.width() as usize,
        h: frame.height() as usize,
        v,
    }
}

/// The two feature images of a frame, at full resolution.
pub struct Features {
    /// Brightness ink: the brightest channel ramped over 238..252.
    pub ink: Gray,
    /// Shadow ink: the grey closing (5x5) minus the brightest channel, /60.
    pub shadow: Gray,
}

impl Features {
    pub fn of(frame: &RgbaImage) -> Self {
        let v = brightest(frame);
        Self {
            ink: v.ramp(238.0, 252.0),
            shadow: v.blackhat(5, 60.0),
        }
    }
}

/// The band (left, top, right, bottom) of `img`, in reference pixels from
/// the origin (`x`, `y`), sampled once per reference pixel at `scale`
/// (bilinear, 0 outside).
pub fn patch(img: &Gray, x: f32, y: f32, scale: f32, band: (f32, f32, f32, f32)) -> Gray {
    let (l, t, r, b) = band;
    let (w, h) = ((r - l) as usize, (b - t) as usize);
    let mut v = Vec::with_capacity(w * h);
    for py in 0..h {
        for px in 0..w {
            v.push(img.at(x + (l + px as f32) * scale, y + (t + py as f32) * scale));
        }
    }
    Gray { w, h, v }
}

/// `frame` rotated by `degrees` (counter-clockwise) about its centre,
/// bilinear, black outside. A panel tilted by head tracking reads upright
/// in the frame rotated the other way.
pub fn rotate(frame: &RgbaImage, degrees: f32) -> RgbaImage {
    rotate_rect(frame, degrees, (0, 0, frame.width(), frame.height()))
}

/// `v.round() as u8` for v ≥ 0 (half away from zero, saturating), without
/// the library call `round` is on baseline x86-64. Exact: the fraction
/// `v − trunc(v)` is computed without rounding.
#[inline]
fn round_u8(v: f32) -> u8 {
    // also NaN
    if v.is_nan() || v <= 0.0 {
        return 0;
    }
    let t = v as u32;
    (t + (v - t as f32 >= 0.5) as u32).min(255) as u8
}

/// The rectangle (x, y, w, h) of [`rotate`]`(frame, degrees)`, computing
/// only those pixels: the window a reader looks at, without turning the
/// whole frame. Pixels are exactly [`rotate`]'s; the rectangle may reach
/// outside the frame (black there).
pub fn rotate_rect(frame: &RgbaImage, degrees: f32, rect: (u32, u32, u32, u32)) -> RgbaImage {
    let centre = (frame.width() as f32 / 2.0, frame.height() as f32 / 2.0);
    rotate_rect_at(frame, degrees, centre, rect)
}

/// [`rotate_rect`] about `centre` (in `img`'s pixels) instead of the
/// image's own centre: for an image that is a region of a larger frame,
/// pass the frame's centre minus the region's origin to get the frame's
/// rotation (to float rounding) without capturing the whole frame. The
/// region must hold the pixels the rectangle turns in from; outside it
/// is black.
pub fn rotate_rect_at(
    img: &RgbaImage,
    degrees: f32,
    centre: (f32, f32),
    rect: (u32, u32, u32, u32),
) -> RgbaImage {
    let frame = img;
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let (rx, ry, rw, rh) = (
        rect.0 as usize,
        rect.1 as usize,
        rect.2 as usize,
        rect.3 as usize,
    );
    let mut rgba = vec![0u8; rw * rh * 4];
    if w < 2 || h < 2 {
        return RgbaImage::from_raw(rect.2, rect.3, rgba).expect("buffer matches its size");
    }
    let src = frame.as_raw();
    let (s, c) = degrees.to_radians().sin_cos();
    let (cx, cy) = centre;
    let (xmax, ymax) = ((w - 1) as f32, (h - 1) as f32);
    for (oy, out) in rgba.chunks_exact_mut(rw * 4).enumerate() {
        let dy = (ry + oy) as f32 - cy;
        for (ox, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let dx = (rx + ox) as f32 - cx;
            // inverse map: where this output pixel comes from
            let sx = c * dx - s * dy + cx;
            let sy = s * dx + c * dy + cy;
            if !(sx >= 0.0 && sy >= 0.0 && sx < xmax && sy < ymax) {
                continue;
            }
            let (x0, y0) = (sx as usize, sy as usize);
            let (tx, ty) = (sx - x0 as f32, sy - y0 as f32);
            let i = (y0 * w + x0) * 4;
            let f = |j: usize| -> [f32; 4] {
                let p: &[u8; 4] = src[j..j + 4].try_into().expect("4 bytes");
                p.map(f32::from)
            };
            let (p00, p10, p01, p11) = (f(i), f(i + 4), f(i + w * 4), f(i + w * 4 + 4));
            let (ax, ay) = (1.0 - tx, 1.0 - ty);
            // all four lanes at once (alpha is dropped), each in the same
            // order of operations as one channel at a time
            let v: [f32; 4] = std::array::from_fn(|c| {
                p00[c] * ax * ay + p10[c] * tx * ay + p01[c] * ax * ty + p11[c] * tx * ty
            });
            *px = [round_u8(v[0]), round_u8(v[1]), round_u8(v[2]), 255];
        }
    }
    RgbaImage::from_raw(rect.2, rect.3, rgba).expect("buffer matches its size")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightest_takes_the_max_channel() {
        let img = RgbaImage::from_raw(2, 1, vec![10, 200, 30, 255, 90, 80, 70, 0]).unwrap();
        assert_eq!(brightest(&img).v, vec![200.0, 90.0]);
    }

    #[test]
    fn a_zero_turn_keeps_the_frame() {
        let mut img = RgbaImage::new(6, 5);
        for (i, p) in img.pixels_mut().enumerate() {
            p.0 = [i as u8 * 7, 3, 200, 255];
        }
        let r = rotate(&img, 0.0);
        // the last row and column have no right/lower neighbour: black
        for y in 0..4 {
            for x in 0..5 {
                assert_eq!(r.get_pixel(x, y), img.get_pixel(x, y));
            }
        }
    }

    /// rotate as it was moved from Starlume.
    fn rotate_ref(frame: &RgbaImage, degrees: f32) -> RgbaImage {
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let src = frame.as_raw();
        let (s, c) = degrees.to_radians().sin_cos();
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let sx = c * dx - s * dy + cx;
                let sy = s * dx + c * dy + cy;
                if sx < 0.0 || sy < 0.0 || sx >= (w - 1) as f32 || sy >= (h - 1) as f32 {
                    continue;
                }
                let (x0, y0) = (sx as usize, sy as usize);
                let (tx, ty) = (sx - x0 as f32, sy - y0 as f32);
                for ch in 0..3 {
                    let p = |xx: usize, yy: usize| src[(yy * w + xx) * 4 + ch] as f32;
                    let v = p(x0, y0) * (1.0 - tx) * (1.0 - ty)
                        + p(x0 + 1, y0) * tx * (1.0 - ty)
                        + p(x0, y0 + 1) * (1.0 - tx) * ty
                        + p(x0 + 1, y0 + 1) * tx * ty;
                    rgba[(y * w + x) * 4 + ch] = v.round() as u8;
                }
                rgba[(y * w + x) * 4 + 3] = 255;
            }
        }
        RgbaImage::from_raw(frame.width(), frame.height(), rgba).unwrap()
    }

    #[test]
    fn rotation_matches_the_original_and_its_rects() {
        let mut img = RgbaImage::new(61, 37);
        let mut seed = 99u32;
        for p in img.pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let b = seed.to_le_bytes();
            p.0 = [b[0], b[1], b[2], 255];
        }
        for deg in [0.0, 8.0, -8.0, 16.0, -16.0, 33.3, 90.0] {
            let full = rotate(&img, deg);
            assert_eq!(full, rotate_ref(&img, deg), "{deg}");
            let r = rotate_rect(&img, deg, (13, 5, 30, 20));
            for y in 0..20 {
                for x in 0..30 {
                    assert_eq!(r.get_pixel(x, y), full.get_pixel(13 + x, 5 + y));
                }
            }
            // reaching outside the frame: black there
            let out = rotate_rect(&img, deg, (50, 30, 20, 20));
            assert_eq!(out.get_pixel(15, 15).0, [0, 0, 0, 0]);
            // from a region of the frame, about the frame's centre
            let region = image::imageops::crop_imm(&img, 5, 0, 50, 37).to_image();
            let r2 = rotate_rect_at(&region, deg, (61.0 / 2.0 - 5.0, 37.0 / 2.0), (8, 5, 30, 20));
            let close = (0..20).all(|y| {
                (0..30).all(|x| {
                    let (a, b) = (r2.get_pixel(x, y).0, r.get_pixel(x, y).0);
                    (0..4).all(|c| a[c].abs_diff(b[c]) <= 1)
                })
            });
            assert!(close, "{deg}");
        }
    }

    #[test]
    fn round_u8_is_round() {
        let mut vs = vec![-0.3f32, -0.0, 0.0, 255.00002, 255.4, 255.6, 300.0, 1e9];
        for k in 0..256 {
            // around every half: a few ulps either side
            let mut v = (k as f32 + 0.5).next_down().next_down().next_down();
            for _ in 0..6 {
                vs.push(v);
                v = v.next_up();
            }
            vs.extend([k as f32, k as f32 + 0.25, k as f32 + 0.75]);
        }
        for v in vs {
            assert_eq!(round_u8(v), v.round() as u8, "{v}");
        }
    }

    #[test]
    fn patch_at_scale_one_copies() {
        let mut g = Gray::new(10, 8);
        for (i, v) in g.v.iter_mut().enumerate() {
            *v = i as f32;
        }
        let p = patch(&g, 2.0, 1.0, 1.0, (1.0, 2.0, 4.0, 4.0));
        assert_eq!((p.w, p.h), (3, 2));
        assert_eq!(p.get(0, 0), g.get(3, 3));
        assert_eq!(p.get(2, 1), g.get(5, 4));
    }
}
