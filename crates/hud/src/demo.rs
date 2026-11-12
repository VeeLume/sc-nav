//! A simulated flight on Lyria, so the app (recording and replay too) can be
//! tried without the game: telemetry at 15 Hz and an overlay fix every
//! second through [`Runtime::live`], the same path the game's feeds take, and
//! a pilot who steers to the cue and brakes on it.

use std::time::{Duration, Instant};

use sc_nav_geo::space::{Basis, basis_to_camdir, cross, norm};
use sc_nav_geo::{Body, Cue, G, Geo, Location, V3, angle_diff};
use sc_nav_pilot::runtime::Runtime;
use sc_nav_pilot::telemetry::{Flight, Frame, Gauge, Instruments, Vehicle};
use sc_nav_pilot::{Cmd, Input, OverlayFix};

const BODY: &str = "stanton3a";
const ALT: f64 = 1800.0;

pub fn run(rt: Runtime) {
    std::thread::spawn(move || {
        let Some(body) = rt.pilot().dir.data.body(BODY).cloned() else {
            rt.pilot().say(format!("demo: {BODY} is not in the game data"));
            return;
        };
        let r = body.radius_m + ALT;
        let mut pos = Geo { lat: -0.30, lon: 1.10, r };
        let (mut heading, mut speed) = (40.0f64, 0.0f64);
        let dt = 1.0 / 15.0;
        let start = Instant::now();
        let (mut next_fix, mut arrived_for, mut turn_rate, mut targets) = (0.0, 0.0, 0.0f64, 0u32);
        loop {
            std::thread::sleep(Duration::from_secs_f64(dt));
            if rt.replay_state(|r| r.is_some()) {
                continue;
            }
            let t = start.elapsed().as_secs_f64();
            let ms = rt.now_ms();
            let s = rt.pilot().snapshot();
            if s.target.is_none() && s.here.is_some() {
                // a point 25 km off, ahead-left: on the surface, and every
                // other time a point in space, shared as a scout's code
                let target = pos.step((heading + 300.0) % 360.0, 25_000.0 / r);
                targets += 1;
                let space = (targets % 2 == 1).then(|| body.game_to_global(target.to_fixed(), ms)).flatten();
                rt.command(match space {
                    Some(xyz) => {
                        let w = Location::Space { system: "stanton".into(), xyz };
                        Cmd::AddCode { code: w.code("scout rock"), go: true }
                    }
                    None => Cmd::Point {
                        name: "demo target".into(),
                        body: BODY.into(),
                        lat: target.lat.to_degrees(),
                        lon: target.lon.to_degrees(),
                        save: false,
                        input: "demo".into(),
                    },
                });
            }
            let cue = match (&s.space, &s.guidance) {
                (Some(g), _) if s.target_space => Some((g.cue, heading + g.target_view.map_or(0.0, |v| v.0))),
                (_, Some(g)) => Some((g.cue, g.bearing)),
                _ => None,
            };
            match cue {
                Some((Cue::Brake | Cue::Arrived, _)) => speed = (speed - s.brake.0 * G * dt).max(0.0),
                Some((_, bearing)) => {
                    let step = angle_diff(bearing, heading).clamp(-12.0 * dt, 12.0 * dt);
                    turn_rate = step / dt;
                    heading = (heading + step).rem_euclid(360.0);
                    speed = (speed + 30.0 * dt).min(420.0);
                }
                None => {}
            }
            if matches!(cue, Some((Cue::Arrived, _))) {
                arrived_for += dt;
                if arrived_for > 6.0 {
                    if let Some(name) = s.target.clone() {
                        rt.command(Cmd::Delete { name });
                    }
                    rt.command(Cmd::Clear);
                    arrived_for = 0.0;
                }
            }
            pos = Geo { r, ..pos.step(heading, speed * dt / r) };
            rt.live(ms, Input::Telemetry { frame: Box::new(frame(t, speed, heading, turn_rate)) });

            if t >= next_fix {
                next_fix = t + 1.0;
                let cam = pos.to_fixed();
                let fix = OverlayFix {
                    body: BODY.into(),
                    cam,
                    root: body.game_to_global(cam, ms),
                    camdir: view(&body, cam, heading, ms),
                    system: Some("stanton".into()),
                    agl: Some(ALT),
                };
                rt.live(ms, Input::Overlay { fix });
            }
        }
    });
}

fn frame(t: f64, speed: f64, heading: f64, turn_rate: f64) -> Frame {
    let mut i = Instruments::default();
    i.speed = speed as f32;
    i.altitude = Some(ALT as f32);
    i.radar_altitude = Some(ALT as f32);
    i.heading = Some(heading as f32);
    i.hydrogen_fuel = (0.9 - 0.001 * t).max(0.0) as f32;
    i.boost.reserves = 1.0;
    let mut g = Gauge::default();
    // the load a coordinated turn at this rate needs
    g.current = (1.0 + (turn_rate.to_radians() * speed / G).powi(2)).sqrt() as f32;
    g.max = 6.0;
    i.g_meter = Some(g);
    let mut flight = Flight::default();
    flight.instruments = i;
    let mut v = Vehicle::default();
    v.name = "Constellation Taurus (demo)".into();
    v.class_name = "RSI_Constellation_Taurus".into();
    v.flight = Some(flight);
    let mut f = Frame::default();
    f.vehicle = Some(v);
    f
}

/// What `r_DisplayInfo 2` would print as `CamDir` for a level camera at `cam`
/// (body frame) facing compass `heading`.
fn view(body: &Body, cam: V3, heading: f64, ms: i64) -> Option<[i32; 3]> {
    let unit = |v: V3| {
        let n = norm(v);
        [v[0] / n, v[1] / n, v[2] / n]
    };
    let up = unit(cam);
    let east = unit(cross([0.0, 0.0, 1.0], up));
    let north = cross(up, east);
    let (sh, ch) = heading.to_radians().sin_cos();
    let forward = [ch * north[0] + sh * east[0], ch * north[1] + sh * east[1], ch * north[2] + sh * east[2]];
    let right = cross(forward, up);
    let turn = |v: V3| -> Option<V3> {
        let g = body.game_to_global(v, ms)?;
        let o = body.game_to_global([0.0; 3], ms)?;
        Some([g[0] - o[0], g[1] - o[1], g[2] - o[2]])
    };
    let b = Basis { forward: turn(forward)?, right: turn(right)?, up: turn(up)? };
    let a = basis_to_camdir(&b);
    Some([a[0].round() as i32, a[1].round() as i32, a[2].round() as i32])
}
