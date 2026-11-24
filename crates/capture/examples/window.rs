//! Time the window capture: the one-shot [`capture_game`] against a kept
//! [`Session`], whole frames and a panel-sized rectangle.
//!
//! ```bash
//! cargo run --release -p sc-nav-capture --example window            # the game
//! cargo run --release -p sc-nav-capture --example window -- Code.exe # any app
//! ```
//! A window only sends frames while its content changes: an idle app's
//! session hands out the last one again.

use std::time::Instant;

use sc_nav_capture::{Session, capture_game};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let exe = std::env::args().nth(1);
    let open = || match &exe {
        Some(e) => Session::open_exe(e),
        None => Session::open(),
    };

    if exe.is_none() {
        let t = Instant::now();
        match capture_game() {
            Ok(f) => println!("one shot: {}x{} in {:.1} ms", f.width(), f.height(), ms(t)),
            Err(e) => println!("one shot: {e}"),
        }
    }

    let t = Instant::now();
    let mut s = match open() {
        Ok(s) => s,
        Err(e) => return println!("open: {e}"),
    };
    let opened = ms(t);
    let t = Instant::now();
    let f = s.frame().expect("first frame");
    println!(
        "session open {opened:.1} ms, first frame {}x{} {:.1} ms",
        f.width(),
        f.height(),
        ms(t)
    );

    let n = 30;
    let t = Instant::now();
    for _ in 0..n {
        s.frame().expect("frame");
    }
    println!("whole frame: {:.2} ms each", ms(t) / n as f64);

    let (w, h) = s.size();
    let (rw, rh) = (600.min(w), 700.min(h));
    let t = Instant::now();
    for _ in 0..n {
        let r = s.region(w - rw, h / 4, rw, rh).expect("region");
        assert_eq!((r.image.width(), r.image.height()), (rw, rh.min(h - h / 4)));
    }
    println!("{rw}x{rh} region: {:.2} ms each", ms(t) / n as f64);

    // The region is the same pixels as the whole frame there.
    let whole = s.frame().expect("frame");
    let r = s.region(w / 3, h / 3, 50, 40).expect("region");
    let same = (0..r.image.height()).all(|y| {
        (0..r.image.width()).all(|x| r.image.get_pixel(x, y) == whole.get_pixel(r.x + x, r.y + y))
    });
    println!("region matches the whole frame: {same} (exact while the window is still)");
    let off = s.region(w + 10, 5, 20, 20).expect("outside");
    println!(
        "outside the window: {}x{} at ({}, {})",
        off.image.width(),
        off.image.height(),
        off.x,
        off.y
    );
}
