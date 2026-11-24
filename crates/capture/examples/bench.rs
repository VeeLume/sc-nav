//! Times the overlay reader over recorded sessions and writes what it read,
//! so a change to the hot loops can be measured and diffed.
//!
//! ```text
//! cargo run --release -p sc-nav-capture --example bench -- [out.jsonl] [session dir ...]
//! ```
//!
//! Without session dirs it reads every session under the lab's
//! `hud-sessions`. Images are decoded before timing starts; only
//! `Reader::read` is timed. Each grab's log line goes to `out.jsonl`
//! (default `target/bench-reads.jsonl`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sc_nav_geo::Body;
use sc_nav_overlay::Reader;

const LAB: &str = "E:/vscode/sc-grpc/.scratch";

fn font() -> Vec<u8> {
    let cached = std::fs::read_dir("target/sc-nav-cache").ok().and_then(|d| {
        d.flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "ttf"))
    });
    let p =
        cached.unwrap_or_else(|| PathBuf::from(LAB).join("p4k-fonts/Engine/Fonts/VeraMono.ttf"));
    std::fs::read(&p).unwrap_or_else(|e| panic!("font {}: {e}", p.display()))
}

fn bodies() -> Vec<Body> {
    let p = PathBuf::from(LAB).join("nav-data/nav-data.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
    serde_json::from_value(v["bodies"].clone()).unwrap()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .unwrap_or_else(|| "target/bench-reads.jsonl".into());
    let mut dirs: Vec<PathBuf> = args.map(PathBuf::from).collect();
    if dirs.is_empty() {
        let root = Path::new(LAB).join("hud-sessions");
        dirs = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("screens").is_dir())
            .collect();
        dirs.sort();
    }
    let font = font();
    let bodies = bodies();
    let mut log = std::fs::File::create(&out).unwrap();
    let mut all: Vec<Duration> = vec![];
    for dir in &dirs {
        let screens = dir.join("screens");
        let grabs = sc_nav_capture::grab_log(&screens).unwrap();
        let imgs: Vec<_> = grabs
            .iter()
            .filter_map(|g| sc_nav_capture::grab_image(&screens, g.n).map(|i| (g, i)))
            .collect();
        let mut reader = Reader::new(&font).unwrap();
        let mut times = vec![];
        let mut accepted = 0;
        for (g, img) in &imgs {
            let t = Instant::now();
            let (line, _) = reader.read(img, &bodies, g.n, Instant::now(), g.unix_ms);
            let took = t.elapsed();
            times.push(took);
            if took > Duration::from_millis(500) {
                eprintln!(
                    "slow: {} grab {}: {:.0} ms, {}",
                    dir.file_name().unwrap().to_string_lossy(),
                    g.n,
                    took.as_secs_f64() * 1e3,
                    line.outcome
                );
            }
            accepted += usize::from(line.outcome == "accepted");
            writeln!(log, "{}", serde_json::to_string(&line).unwrap()).unwrap();
        }
        report(
            &format!(
                "{} ({} grabs, {accepted} accepted, {}x{})",
                dir.file_name().unwrap().to_string_lossy(),
                imgs.len(),
                imgs.first().map_or(0, |i| i.1.width()),
                imgs.first().map_or(0, |i| i.1.height()),
            ),
            &mut times,
        );
        all.extend(times);
    }
    report("all", &mut all);
}

fn report(name: &str, t: &mut [Duration]) {
    if t.is_empty() {
        return;
    }
    t.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let total: Duration = t.iter().sum();
    println!(
        "{name}: mean {:.2} ms, p50 {:.2}, p90 {:.2}, p99 {:.2}, max {:.2}",
        ms(total) / t.len() as f64,
        ms(t[t.len() / 2]),
        ms(t[t.len() * 9 / 10]),
        ms(t[t.len() * 99 / 100]),
        ms(t[t.len() - 1]),
    );
}
