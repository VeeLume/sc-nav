//! The pilot against real recordings (skipped where they aren't on disk) and
//! a record/replay round trip.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sc_dossier::telemetry::{Flight, Frame, Instruments, Vehicle};
use sc_nav_geo::ground::GroundLevel;
use sc_nav_geo::{Body, Geo, World};
use sc_nav_pilot::runtime::{Config, Runtime, read_session};
use sc_nav_pilot::{Cmd, Director, Input, OverlayFix, Pilot};

const LAB: &str = "E:/vscode/sc-grpc/.scratch";

fn font() -> Option<Vec<u8>> {
    let cached = std::fs::read_dir("../../target/sc-nav-cache")
        .ok()
        .and_then(|d| {
            d.flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "ttf"))
        });
    cached
        .or_else(|| Some(PathBuf::from(LAB).join("p4k-fonts/Engine/Fonts/VeraMono.ttf")))
        .and_then(|p| std::fs::read(p).ok())
}

fn world() -> Option<World> {
    let p = PathBuf::from(LAB).join("nav-data/nav-data.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
    Some(World {
        build: "lab".into(),
        bodies: serde_json::from_value(v["bodies"].clone()).ok()?,
        ships: serde_json::from_value(v["ships"].clone()).ok()?,
        systems: vec![],
    })
}

/// The crash on Arial (impact ~335 s, 940 m/s straight down), from the lab's
/// recording: telemetry replayed, the overlay re-read. PULL UP must come while
/// stopping was still possible.
#[test]
fn warns_before_the_arial_crash() {
    let dir = Path::new(LAB).join("hud-sessions/session-1794442146");
    let (Some(font), Some(world), true) = (font(), world(), dir.is_dir()) else {
        eprintln!("skipped: recording or font missing");
        return;
    };
    let (meta, inputs) = read_session(&dir).unwrap();
    let (from, impact) = (meta.started_ms + 270_000, meta.started_ms + 335_000);
    let bodies = world.bodies.clone();
    let mut p = Pilot::new(Director::new(world, None), meta.started_ms);
    let screens = dir.join("screens");
    let grabs: Vec<_> = sc_nav_capture::grab_log(&screens)
        .unwrap()
        .into_iter()
        .filter(|g| g.unix_ms >= from && g.unix_ms <= impact)
        .collect();
    let tel: Vec<_> = inputs
        .into_iter()
        .filter(|(ms, i)| *ms >= from && *ms <= impact && matches!(i, Input::Telemetry { .. }))
        .collect();
    let mut reader = sc_nav_overlay::Reader::new(&font).unwrap();
    let (mut k, mut caution, mut pull) = (0, None, None);
    for g in &grabs {
        while k < tel.len() && tel[k].0 <= g.unix_ms {
            p.feed(tel[k].0, &tel[k].1);
            k += 1;
        }
        let img = sc_nav_capture::grab_image(&screens, g.n).unwrap();
        if let (_, Some(f)) = reader.read(&img, &bodies, g.n, Instant::now(), g.unix_ms) {
            p.feed(
                g.unix_ms,
                &Input::Overlay {
                    fix: sc_nav_pilot::runtime::overlay_fix(&f),
                },
            );
        }
        let before = (impact - g.unix_ms) as f64 / 1000.0;
        if let Some(w) = p.snapshot().ground {
            match w.level {
                GroundLevel::PullUp => pull = pull.or(Some((before, w.agl, w.stop_m))),
                GroundLevel::Caution => caution = caution.or(Some(before)),
            }
        }
    }
    eprintln!("caution {caution:?} s before, pull up {pull:?} (s before, agl, stop)");
    let (pull_before, agl, stop) = pull.expect("a PULL UP before the impact");
    assert!(caution.is_some_and(|c| c > pull_before));
    assert!(
        pull_before > 15.0 && stop <= agl * 1.2,
        "{pull_before} s, agl {agl}, stop {stop}"
    );
}

fn lyria() -> Body {
    Body {
        id: "stanton3a".into(),
        name: "OOC_Stanton_3a_Lyria".into(),
        center: [18_778_498.0, -21_918_664.0, 0.0],
        radius_m: 223_000.0,
        period_h: 6.43,
        atmo_km: Some(15.0),
        phase_deg: Some(175.37),
    }
}

fn frame(speed: f32, heading: f32) -> Frame {
    let mut i = Instruments::default();
    i.speed = speed;
    i.heading = Some(heading);
    i.altitude = Some(1500.0);
    i.radar_altitude = Some(1500.0);
    let mut fl = Flight::default();
    fl.instruments = i;
    let mut v = Vehicle::default();
    v.flight = Some(fl);
    let mut f = Frame::default();
    f.vehicle = Some(v);
    f
}

/// Live inputs recorded, replayed at 4x into a fresh pilot: it ends where
/// the live one ended, and the target and the flag come back.
#[test]
fn record_and_replay_round_trip() {
    let root = std::env::temp_dir().join(format!("sc-nav-rt-{}", std::process::id()));
    let world = World {
        build: "test".into(),
        bodies: vec![lyria()],
        ships: Default::default(),
        systems: vec![],
    };
    let start = sc_nav_clock::now_ms();
    let pilot = Pilot::new(Director::new(world.clone(), None), start);
    let cfg = Config {
        addr: String::new(),
        font: vec![],
        world: world.clone(),
        sessions: root.clone(),
        feeds: false,
    };
    let rt = Runtime::start(pilot, cfg, || {}).unwrap();
    rt.start_recording();
    let session = rt.recording().unwrap().0;
    let body = lyria();
    let at = Geo {
        lat: -0.27,
        lon: -1.87,
        r: 224_500.0,
    };
    let ms = sc_nav_clock::now_ms();
    let fix = OverlayFix {
        body: "stanton3a".into(),
        cam: at.to_fixed(),
        root: body.game_to_global(at.to_fixed(), ms),
        camdir: None,
        system: None,
        agl: None,
    };
    rt.live(ms, Input::Overlay { fix });
    rt.command(Cmd::Point {
        name: "t".into(),
        body: "stanton3a".into(),
        lat: -15.0,
        lon: -106.0,
        save: false,
        input: "-15, -106".into(),
    });
    for k in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        rt.live(
            sc_nav_clock::now_ms(),
            Input::Telemetry {
                frame: Box::new(frame(200.0, 30.0 + k as f32)),
            },
        );
        if k == 20 {
            rt.flag("halfway");
        }
    }
    let live_end = rt.pilot().dir.nav.position().unwrap();
    rt.stop_recording();
    assert!(
        at.angle_to(&live_end) * 223_000.0 > 100.0,
        "the live run moved"
    );

    rt.replay(&session, 4.0, false).unwrap();
    let mut saw_target = false;
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(25));
        if !rt.replay_state(|r| r.is_some()) {
            break;
        }
        saw_target |= rt.pilot().snapshot().target.as_deref() == Some("t");
    }
    let replay_end = rt.replay_end();
    assert!(!rt.replay_state(|r| r.is_some()), "replay finished");
    assert!(saw_target, "the recorded command came back");
    assert!(
        rt.pilot()
            .messages()
            .any(|m| m.text.contains("FLAG: halfway"))
    );
    let off = replay_end.unwrap().angle_to(&live_end) * 223_000.0;
    assert!(off < 2.0, "replay ended {off:.2} m from the live run");
    let _ = std::fs::remove_dir_all(root);
}
