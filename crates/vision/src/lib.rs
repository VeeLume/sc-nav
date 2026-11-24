//! Image tools for reading HUD panels off a screenshot: the two feature
//! images readers match on, rotation, bands resampled at a spot and scale,
//! and normalised cross-correlation (NCC) over a whole image via FFT.
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
//! Starlume (`svc-capture::vision`, `mod-mining`'s `Features` and `patch`),
//! behaviour unchanged.

mod fft;

use image::RgbaImage;
pub use sc_nav_glyph::Gray;

pub use fft::{Correlator, peaks};

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
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let src = frame.as_raw();
    let (s, c) = degrees.to_radians().sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            // inverse map: where this output pixel comes from
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
    RgbaImage::from_raw(frame.width(), frame.height(), rgba).expect("buffer matches its size")
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
