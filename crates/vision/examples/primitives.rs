//! Time the vision primitives on a real screenshot, at the sizes Starlume's
//! scan panel reader uses (2560x1440 frames, a ~570x700 window around the
//! panel, anchor templates of ~120x20 reference pixels).
//!
//! ```bash
//! cargo run --release -p sc-nav-vision --example primitives -- "D:\Recordings\Gaming\Screenshot ....png"
//! ```

use std::time::Instant;

use image::RgbaImage;
use sc_nav_vision::{
    Correlator, Features, Gray, Scorer, Template, brightest, patch, peaks, rotate, rotate_rect,
};

fn time<T>(name: &str, n: usize, mut f: impl FnMut() -> T) -> T {
    let mut ms = Vec::new();
    let mut out = None;
    for _ in 0..n {
        let t = Instant::now();
        out = Some(std::hint::black_box(f()));
        ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    ms.sort_by(f64::total_cmp);
    println!("{name:<40} {:>9.3} ms", ms[ms.len() / 2]);
    out.expect("ran")
}

fn crop(f: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
    image::imageops::crop_imm(f, x, y, w, h).to_image()
}

fn main() {
    let path = std::env::args().nth(1).expect("screenshot");
    let frame = image::open(&path).expect("png").to_rgba8();
    let (fw, fh) = frame.dimensions();
    println!("{fw}x{fh}");
    let n = 7;

    time("brightest (frame)", n, || brightest(&frame));
    let f = time("Features::of (frame)", n, || Features::of(&frame));
    time("rotate (frame)", n, || rotate(&frame, 8.0));
    time("rotate 0 deg (frame)", n, || rotate(&frame, 0.0));
    time("alloc + zero frame", n, || {
        vec![0u8; (fw * fh * 4) as usize]
    });
    time("rotate_rect 572x700", n, || {
        rotate_rect(&frame, 8.0, (fw - 700, 300, 572, 700))
    });
    let win = crop(&frame, fw - 700, 300, 572, 700);
    time("crop 572x700", n, || crop(&frame, fw - 700, 300, 572, 700));
    let fc = time("Features::of (572x700)", n, || Features::of(&win));
    let half = time("half (frame)", n, || f.ink.half());
    let halfc = fc.ink.half();
    let cf = time("Correlator::new (half frame)", n, || Correlator::new(&half));
    let cc = time("Correlator::new (half window)", n, || {
        Correlator::new(&halfc)
    });
    let t = patch(&fc.ink, 100.0, 200.0, 1.0, (0.0, 0.0, 120.0, 20.0));
    let th = time("resized template 120x20 at 0.5", n, || t.resized(0.5));
    let map = time("ncc 60x10 (half frame)", n, || cf.ncc(&th));
    time("ncc 60x10 (half window)", n, || cc.ncc(&th));
    time("peaks 8 (half frame map)", n, || peaks(&map, 8, 10, 0.2));
    time("patch 120x20", n, || {
        patch(&fc.ink, 100.5, 200.5, 1.03, (0.0, 0.0, 120.0, 20.0))
    });
    time("direct ncc 120x20, 9x9 spots", n, || {
        let mut best = f32::MIN;
        for dy in -4..=4i64 {
            for dx in -4..=4i64 {
                best = best.max(sc_nav_glyph::ncc_at(&fc.ink, 100 + dx, 200 + dy, &t));
            }
        }
        best
    });
    let p = cf.prepare(&th).expect("fits");
    time("ncc_prepared 60x10 (half frame)", n, || cf.ncc_prepared(&p));

    // A name band and 40 name-sized templates, against Starlume's slide:
    // the window's most inked 30-row band, templates from other inked rows.
    let ink_rows: Vec<f32> = (0..fc.ink.h)
        .map(|y| (0..fc.ink.w).map(|x| fc.ink.get(x, y)).sum())
        .collect();
    let mut by_ink: Vec<usize> = (0..fc.ink.h - 30).collect();
    by_ink.sort_by(|&a, &b| {
        let s = |y: usize| ink_rows[y..y + 30].iter().sum::<f32>();
        s(b).total_cmp(&s(a))
    });
    let by = by_ink[0] as f32;
    println!("  name band at row {by}");
    let band = patch(&fc.ink, 0.0, by, 1.0, (0.0, 0.0, 300.0, 30.0));
    let names: Vec<Gray> = (0..40)
        .map(|i| {
            let w = 60.0 + (i * 4) as f32;
            let y = by_ink[(i * 37) % 200] as f32 + 4.0;
            patch(&fc.ink, (i * 5) as f32, y, 1.0, (0.0, 0.0, w, 22.0))
        })
        .collect();
    println!(
        "  {} of 40 templates have shape",
        names.iter().filter_map(Template::new).count()
    );
    let slow = time("slide, 40 names (Starlume)", n, || {
        names.iter().map(|t| slide(&band, t)).collect::<Vec<_>>()
    });
    let prepared: Vec<Template> = names.iter().filter_map(Template::new).collect();
    let fast = time("Scorer::best, 40 names", n, || {
        let s = Scorer::new(&band);
        prepared
            .iter()
            .map(|t| {
                s.best(t, 0..i64::MAX, 0..i64::MAX, 0.0)
                    .map_or(0.0, |b| b.0.max(0.0))
            })
            .collect::<Vec<_>>()
    });
    let worst = slow
        .iter()
        .zip(&fast)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    println!(
        "  largest score difference {worst:e}; scores {:?}",
        &slow[..6.min(slow.len())]
    );

    // refine's COMPOSITION check: ±6 px, 9 scales, a 200x20 template
    let t = patch(&fc.ink, 100.0, 400.0, 1.0, (0.0, 0.0, 200.0, 20.0));
    let ts: Vec<Gray> = (-4..=4).map(|k| t.resized(1.0 + k as f32 * 0.01)).collect();
    time("ncc_at 13x13 spots x 9 scales (f64)", n, || {
        let mut best = f32::MIN;
        for t in &ts {
            for dy in -6..=6i64 {
                for dx in -6..=6i64 {
                    best = best.max(ncc_at64(&fc.ink, 100 + dx, 400 + dy, t));
                }
            }
        }
        best
    });
    let s = time("Scorer::new (572x700)", n, || Scorer::new(&fc.ink));
    time("Scorer::best 13x13 spots x 9 scales", n, || {
        let mut best = f32::MIN;
        for t in &ts {
            let u = Template::new(t).expect("not flat");
            let floor = 0.02 * u.spread();
            if let Some(b) = s.best(&u, 94..107, 394..407, floor) {
                best = best.max(b.0);
            }
        }
        best
    });
    let _: &Gray = &fc.shadow;
}

/// Starlume's names.rs slide, for comparison.
fn slide(band: &Gray, t: &Gray) -> f32 {
    let (tw, th) = (t.w.min(band.w), t.h.min(band.h));
    let n = (tw * th) as f32;
    let (mut st, mut stt) = (0.0f32, 0.0f32);
    for y in 0..th {
        for x in 0..tw {
            let v = t.get(x, y);
            st += v;
            stt += v * v;
        }
    }
    let vt = stt - st * st / n;
    if vt <= 1e-6 {
        return 0.0;
    }
    let mut best = f32::MIN;
    for oy in 0..=band.h - th {
        for ox in 0..=band.w - tw {
            let (mut si, mut sii, mut sit) = (0.0f32, 0.0f32, 0.0f32);
            for y in 0..th {
                let row = (oy + y) * band.w + ox;
                for x in 0..tw {
                    let a = band.v[row + x];
                    si += a;
                    sii += a * a;
                    sit += a * t.v[y * t.w + x];
                }
            }
            let vi = sii - si * si / n;
            if vi > 1e-6 {
                best = best.max((sit - si * st / n) / (vi * vt).sqrt());
            }
        }
    }
    best.max(0.0)
}

/// Starlume's locate.rs ncc_at (f64), for comparison.
fn ncc_at64(img: &Gray, x: i64, y: i64, t: &Gray) -> f32 {
    if x < 0 || y < 0 || x as usize + t.w > img.w || y as usize + t.h > img.h {
        return 0.0;
    }
    let n = (t.w * t.h) as f64;
    let (mut si, mut sii, mut st, mut stt, mut sit) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
    for ty in 0..t.h {
        for tx in 0..t.w {
            let a = img.get(x as usize + tx, y as usize + ty) as f64;
            let b = t.get(tx, ty) as f64;
            si += a;
            sii += a * a;
            st += b;
            stt += b * b;
            sit += a * b;
        }
    }
    let vt = stt - st * st / n;
    let vi = sii - si * si / n;
    if vt <= 0.0 || vi <= 0.02 * vt {
        return 0.0;
    }
    ((sit - si * st / n) / (vi * vt).sqrt()) as f32
}
