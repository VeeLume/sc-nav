//! Planetary navigation maths, no I/O.
//!
//! Frames: the [`Navigator`] works in the game's own body frame, the one
//! `CamPos Planet Zone`, SCAR and community lat/long use. A fix that already is
//! in that frame (the overlay's CamPos, [`Navigator::on_body_fix`]) needs no
//! clock and no phase. A system-global fix (`/showlocation`, [`Navigator::on_fix`])
//! is rotated by the body's spin at the fix's time ([`Body::to_fixed`], zero at
//! the epoch) and then by the body's measured phase; without a phase the
//! navigator runs in that epoch frame and says so ([`Navigator::game_frame`]).
//! Telemetry speed and heading are body-relative, so tracking between fixes
//! runs entirely in the body frame.

use serde::{Deserialize, Serialize};

pub mod ground;
pub mod location;
pub mod navcode;
pub mod scar;
pub mod space;
pub mod world;

pub use location::Location;
pub use world::{Ship, System, World};

/// `t_realTimeSyncPoint`'s default, 2020-01-01T00:00:00Z.
pub const EPOCH_UNIX_MS: i64 = 1_577_836_800_000;
pub const G: f64 = 9.80665;

pub type V3 = [f64; 3];

/// Parse a `/showlocation` clipboard line. `None` for anything else: this sees
/// arbitrary clipboard text, so it never reports or keeps what it rejected.
pub fn parse_showlocation(text: &str) -> Option<V3> {
    if text.len() > 128 {
        return None;
    }
    let mut parts = text.trim_end().strip_prefix("Coordinates: ")?.split(' ');
    let x = coord(parts.next()?, "x:")?;
    let y = coord(parts.next()?, "y:")?;
    let z = coord(parts.next()?, "z:")?;
    parts.next().is_none().then_some([x, y, z])
}

fn coord(token: &str, prefix: &str) -> Option<f64> {
    let v: f64 = token.strip_prefix(prefix)?.parse().ok()?;
    v.is_finite().then_some(v)
}

/// A planet or moon, from the game files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Body {
    pub id: String,
    pub name: String,
    pub center: V3,
    pub radius_m: f64,
    pub period_h: f64,
    pub atmo_km: Option<f64>,
    /// Offset from our zero-at-epoch frame to the game's own body frame, degrees.
    /// Measured, see [`phase_from_reading`]. Only needed to match game coordinates.
    #[serde(default)]
    pub phase_deg: Option<f64>,
}

/// Measure a body's phase from one `r_DisplayInfo` reading: the horizontal parts
/// of its `Zone: OOC_*` and `Zone: Root` lines (metres), and `ServerTime`.
pub fn phase_from_reading(body: &Body, zone: [f64; 2], root: [f64; 2], unix_ms: i64) -> f64 {
    let frame = (root[1] - body.center[1]).atan2(root[0] - body.center[0]) - zone[1].atan2(zone[0]);
    (frame - body.spin(unix_ms)).to_degrees().rem_euclid(360.0)
}

/// Mean of angles in degrees.
pub fn circular_mean(degrees: &[f64]) -> f64 {
    let (s, c) = degrees.iter().fold((0.0, 0.0), |(s, c), d| {
        let r = d.to_radians();
        (s + r.sin(), c + r.cos())
    });
    f64::atan2(s, c).to_degrees().rem_euclid(360.0)
}

/// Unix milliseconds of a UTC date and time.
pub fn unix_ms_utc(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000
}

impl Body {
    /// The epoch frame (`to_fixed`) to the game's body frame. `None` until
    /// the phase is measured.
    pub fn epoch_to_game(&self, v: V3) -> Option<V3> {
        let (s, c) = self.phase_deg?.to_radians().sin_cos();
        Some([v[0] * c + v[1] * s, -v[0] * s + v[1] * c, v[2]])
    }

    /// The game's body frame back to system-global at `unix_ms`. `None`
    /// until the phase is measured.
    pub fn game_to_global(&self, v: V3, unix_ms: i64) -> Option<V3> {
        let (s, c) = self.phase_deg?.to_radians().sin_cos();
        Some(self.to_global([v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]], unix_ms))
    }

    /// Spin angle in radians at `unix_ms`. 0 for a body that does not turn
    /// (`period_h` 0, Fuego).
    pub fn spin(&self, unix_ms: i64) -> f64 {
        if self.period_h <= 0.0 {
            return 0.0;
        }
        let cycles = (unix_ms - EPOCH_UNIX_MS) as f64 / 3_600_000.0 / self.period_h;
        cycles.rem_euclid(1.0) * std::f64::consts::TAU
    }

    /// System-global point to the body-fixed frame.
    pub fn to_fixed(&self, p: V3, unix_ms: i64) -> V3 {
        let [x, y, z] = [
            p[0] - self.center[0],
            p[1] - self.center[1],
            p[2] - self.center[2],
        ];
        let (s, c) = self.spin(unix_ms).sin_cos();
        [x * c + y * s, -x * s + y * c, z]
    }

    /// Body-fixed point back to system-global.
    pub fn to_global(&self, v: V3, unix_ms: i64) -> V3 {
        let (s, c) = self.spin(unix_ms).sin_cos();
        [
            self.center[0] + v[0] * c - v[1] * s,
            self.center[1] + v[0] * s + v[1] * c,
            self.center[2] + v[2],
        ]
    }

    /// Eastward surface speed of the ground at the equator, m/s.
    pub fn equator_speed(&self) -> f64 {
        if self.period_h <= 0.0 {
            return 0.0;
        }
        std::f64::consts::TAU * self.radius_m / (self.period_h * 3600.0)
    }
}

/// The body a global point belongs to: closest surface, within four radii.
pub fn nearest_body(bodies: &[Body], p: V3) -> Option<&Body> {
    bodies
        .iter()
        .map(|b| (b, dist(p, b.center)))
        .filter(|(b, d)| *d < b.radius_m * 4.0)
        .min_by(|(a, da), (b, db)| (da - a.radius_m).total_cmp(&(db - b.radius_m)))
        .map(|(b, _)| b)
}

fn dist(a: V3, b: V3) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Body-fixed spherical position. Radians, metres from the body's centre.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Geo {
    pub lat: f64,
    pub lon: f64,
    pub r: f64,
}

impl Geo {
    /// Back to cartesian in the same frame.
    pub fn to_fixed(&self) -> V3 {
        [
            self.r * self.lat.cos() * self.lon.cos(),
            self.r * self.lat.cos() * self.lon.sin(),
            self.r * self.lat.sin(),
        ]
    }

    pub fn from_fixed(v: V3) -> Self {
        let r = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        Geo {
            lat: (v[2] / r).clamp(-1.0, 1.0).asin(),
            lon: v[1].atan2(v[0]),
            r,
        }
    }

    /// Central angle to `o`, radians.
    pub fn angle_to(&self, o: &Geo) -> f64 {
        let (dlat, dlon) = (o.lat - self.lat, o.lon - self.lon);
        let a =
            (dlat / 2.0).sin().powi(2) + self.lat.cos() * o.lat.cos() * (dlon / 2.0).sin().powi(2);
        2.0 * a.sqrt().min(1.0).asin()
    }

    /// Initial bearing to `o`: degrees, 0 north (+Z), 90 east (direction of spin).
    pub fn bearing_to(&self, o: &Geo) -> f64 {
        let dlon = o.lon - self.lon;
        let y = dlon.sin() * o.lat.cos();
        let x = self.lat.cos() * o.lat.sin() - self.lat.sin() * o.lat.cos() * dlon.cos();
        y.atan2(x).to_degrees().rem_euclid(360.0)
    }

    /// Move along a great circle by `angle` radians on `bearing` degrees.
    pub fn step(&self, bearing: f64, angle: f64) -> Geo {
        let b = bearing.to_radians();
        let lat = (self.lat.sin() * angle.cos() + self.lat.cos() * angle.sin() * b.cos())
            .clamp(-1.0, 1.0)
            .asin();
        let lon = self.lon
            + (b.sin() * angle.sin() * self.lat.cos())
                .atan2(angle.cos() - self.lat.sin() * lat.sin());
        Geo {
            lat,
            lon,
            r: self.r,
        }
    }
}

/// Signed smallest difference `a - b`, degrees in -180..180.
pub fn angle_diff(a: f64, b: f64) -> f64 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// One telemetry frame's flight instruments. `t` is seconds on the session clock.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub t: f64,
    pub speed: f64,
    pub heading: f64,
    pub vsi: f64,
    /// `None` outside the atmosphere, where heading is not updated either.
    pub altitude: Option<f64>,
    pub radar: Option<f64>,
    pub landed: bool,
    /// G-meter reading, in g.
    pub g: f64,
}

/// What one fix showed about the leg since the previous fix.
#[derive(Debug, Clone)]
pub struct Leg {
    pub body: String,
    pub dt: f64,
    pub path_m: f64,
    /// Distance between dead-reckoned and fixed position. `None` if tracking lapsed.
    pub error_m: Option<f64>,
    pub truth_m: f64,
    pub truth_bearing: f64,
    /// Distance-weighted mean telemetry heading. `None` if the ship didn't move.
    pub mean_heading: Option<f64>,
    pub landed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    Cruise,
    Prepare,
    Brake,
    Arrived,
    NotClosing,
}

#[derive(Debug, Clone, Copy)]
pub struct Guidance {
    pub bearing: f64,
    /// Turn needed: positive is right.
    pub relative: f64,
    /// Estimated direction of travel.
    pub track: f64,
    pub distance_m: f64,
    pub ground_speed: f64,
    pub closing: f64,
    pub eta_s: Option<f64>,
    /// Off the line from where the leg began: positive is right of track.
    pub cross_track_m: Option<f64>,
    pub stop_distance_m: f64,
    pub time_to_brake_s: Option<f64>,
    pub cue: Cue,
    /// Position is an estimate that lost tracking; take a new fix.
    pub stale: bool,
}

fn horizontal(s: &Sample) -> f64 {
    (s.speed * s.speed - s.vsi * s.vsi).max(0.0).sqrt()
}

/// Direction of travel between two points, averaged over the path: the initial
/// bearing alone is skewed by meridian convergence, noticeably on small moons.
pub fn mean_track(a: &Geo, b: &Geo) -> f64 {
    let start = a.bearing_to(b);
    let end = (b.bearing_to(a) + 180.0).rem_euclid(360.0);
    (start + angle_diff(end, start) / 2.0).rem_euclid(360.0)
}

/// Distance to stop from `speed` at `accel`, after `reaction_s` of no braking.
pub fn stopping_distance(speed: f64, accel: f64, reaction_s: f64) -> f64 {
    speed * speed / (2.0 * accel) + speed * reaction_s
}

/// Tracks position in the body-fixed frame from fixes and telemetry.
#[derive(Debug, Default)]
pub struct Navigator {
    body: Option<Body>,
    pos: Option<Geo>,
    stale: bool,
    last: Option<Sample>,
    last_fix: Option<(f64, Geo)>,
    path_m: f64,
    heading_sum: (f64, f64),
    moved_landed: bool,
    target: Option<Geo>,
    leg_start: Option<Geo>,
    braking: bool,
    arrived: bool,
    /// Estimated direction of travel, degrees. Differs from the nose in turns.
    track: Option<f64>,
    /// Arrival radius, m; `None`: 500 m (ships). On foot it is a few metres.
    pub arrive_radius: Option<f64>,
    /// The position is in the game's body frame (false: the epoch frame of
    /// a global fix on a body without a phase).
    game_frame: bool,
}

impl Navigator {
    pub fn track(&self) -> Option<f64> {
        self.track
    }

    pub fn body(&self) -> Option<&Body> {
        self.body.as_ref()
    }
    pub fn position(&self) -> Option<Geo> {
        self.pos
    }
    pub fn target(&self) -> Option<Geo> {
        self.target
    }
    pub fn stale(&self) -> bool {
        self.stale
    }
    /// Positions are in the game's body frame: lat/long, SCAR and saved
    /// marks mean something. False after a global fix on an uncalibrated body.
    pub fn game_frame(&self) -> bool {
        self.game_frame
    }

    pub fn set_target(&mut self, target: Option<Geo>) {
        self.target = target;
        self.leg_start = self.pos;
        self.braking = false;
        self.arrived = false;
    }

    /// Take a system-global fix (`/showlocation`) at `unix_ms`. Returns the
    /// position, and the leg report if a previous fix on the same body exists.
    /// `None` when no body is near.
    pub fn on_fix(
        &mut self,
        bodies: &[Body],
        t: f64,
        unix_ms: i64,
        p: V3,
    ) -> Option<(Geo, Option<Leg>)> {
        let body = nearest_body(bodies, p)?.clone();
        let epoch = body.to_fixed(p, unix_ms);
        let (v, game) = match body.epoch_to_game(epoch) {
            Some(v) => (v, true),
            None => (epoch, false),
        };
        Some(self.fix(body, t, Geo::from_fixed(v), game))
    }

    /// Take a fix already in the game's body frame (the overlay's CamPos): no
    /// clock, no phase.
    pub fn on_body_fix(&mut self, body: &Body, t: f64, v: V3) -> (Geo, Option<Leg>) {
        self.fix(body.clone(), t, Geo::from_fixed(v), true)
    }

    fn fix(&mut self, body: Body, t: f64, geo: Geo, game_frame: bool) -> (Geo, Option<Leg>) {
        // A change of frame makes the previous fix and the leg incomparable.
        if game_frame != self.game_frame {
            self.last_fix = None;
            self.leg_start = None;
        }
        self.game_frame = game_frame;
        let same_body = self.body.as_ref().is_some_and(|b| b.id == body.id);
        let leg = match (self.last_fix, same_body) {
            (Some((t0, g0)), true) => {
                let (s, c) = self.heading_sum;
                Some(Leg {
                    body: body.id.clone(),
                    dt: t - t0,
                    path_m: self.path_m,
                    error_m: match (self.stale, self.pos) {
                        (false, Some(est)) => Some(est.angle_to(&geo) * geo.r),
                        _ => None,
                    },
                    truth_m: g0.angle_to(&geo) * geo.r,
                    truth_bearing: mean_track(&g0, &geo),
                    mean_heading: (self.path_m > 0.0)
                        .then(|| s.atan2(c).to_degrees().rem_euclid(360.0)),
                    landed: self.moved_landed && self.path_m < 1.0,
                })
            }
            _ => None,
        };
        // Two close fixes measure the real direction of travel, strafing included.
        if let (Some((t0, g0)), true) = (self.last_fix, same_body)
            && t - t0 < 5.0
            && g0.angle_to(&geo) * geo.r > 2.0
        {
            self.track = Some(mean_track(&g0, &geo));
        }
        self.body = Some(body);
        self.pos = Some(geo);
        self.stale = false;
        self.last_fix = Some((t, geo));
        self.path_m = 0.0;
        self.heading_sum = (0.0, 0.0);
        self.moved_landed = self.last.is_some_and(|s| s.landed);
        if self.leg_start.is_none() {
            self.leg_start = Some(geo);
        }
        (geo, leg)
    }

    pub fn on_sample(&mut self, s: Sample) {
        if let (Some(prev), Some(pos), Some(body)) = (self.last, self.pos, self.body.as_ref()) {
            let dt = s.t - prev.t;
            match s.altitude {
                Some(alt) if dt > 0.0 && dt < 2.0 => {
                    // Velocity chases the nose, limited by the lateral part of
                    // the G the ship is actually pulling.
                    let along = (s.speed - prev.speed) / dt;
                    let lateral = ((s.g * G).powi(2) - along * along).max(0.0).sqrt();
                    let track = match self.track {
                        Some(track) if s.speed >= 5.0 => {
                            let limit = (lateral / horizontal(&s).max(1.0)).to_degrees() * dt;
                            (track + angle_diff(s.heading, track).clamp(-limit, limit))
                                .rem_euclid(360.0)
                        }
                        _ => s.heading,
                    };
                    self.track = Some(track);
                    // Trapezoid: the frames bracket the interval.
                    let d = (horizontal(&prev) + horizontal(&s)) / 2.0 * dt;
                    if d > 0.0 {
                        let r = body.radius_m + alt;
                        self.pos = Some(Geo {
                            r,
                            ..pos.step(track, d / r)
                        });
                        self.path_m += d;
                        let h = s.heading.to_radians();
                        self.heading_sum.0 += h.sin() * d;
                        self.heading_sum.1 += h.cos() * d;
                    }
                }
                _ if s.speed > 1.0 => self.stale = true,
                _ => {}
            }
        }
        self.last = Some(s);
    }

    /// Guidance to the target with `brake` m/s² available. Once the brake cue has
    /// fired it holds until the ship arrives or stops closing: slowing down moves
    /// the stopping point back, which must not read as "you can speed up again".
    pub fn guidance(&mut self, brake: f64, reaction_s: f64) -> Option<Guidance> {
        let (pos, target, s) = (self.pos?, self.target?, self.last?);
        let radius = self.body.as_ref()?.radius_m;
        let bearing = pos.bearing_to(&target);
        let relative = angle_diff(bearing, s.heading);
        let distance_m = pos.angle_to(&target) * radius;
        let ground_speed = horizontal(&s);
        let track = self.track.unwrap_or(s.heading);
        let closing = ground_speed * angle_diff(bearing, track).to_radians().cos();
        let stop_distance_m = stopping_distance(closing.max(0.0), brake, reaction_s);
        let eta_s = (closing > 5.0).then(|| distance_m / closing);
        let time_to_brake_s = (closing > 5.0).then(|| (distance_m - stop_distance_m) / closing);
        // Wider release than entry, so hovering at the threshold doesn't flicker.
        let r = self.arrive_radius.unwrap_or(500.0);
        self.arrived = if self.arrived {
            distance_m < r * 1.4 && ground_speed < 15.0
        } else {
            distance_m < r && ground_speed < 5.0
        };
        let cue = if self.arrived {
            Cue::Arrived
        } else if closing <= 2.0 {
            Cue::NotClosing
        } else if self.braking || stop_distance_m >= distance_m {
            Cue::Brake
        } else if time_to_brake_s.is_some_and(|t| t < 10.0) {
            Cue::Prepare
        } else {
            Cue::Cruise
        };
        self.braking = cue == Cue::Brake;
        let cross_track_m = self
            .leg_start
            .filter(|l| l.angle_to(&target) > 1e-6)
            .map(|l| {
                let d13 = l.angle_to(&pos);
                let delta = (l.bearing_to(&pos) - l.bearing_to(&target)).to_radians();
                (d13.sin() * delta.sin()).clamp(-1.0, 1.0).asin() * radius
            });
        Some(Guidance {
            bearing,
            relative,
            track,
            distance_m,
            ground_speed,
            closing,
            eta_s,
            cross_track_m,
            stop_distance_m,
            time_to_brake_s,
            cue,
            stale: self.stale,
        })
    }
}

#[cfg(test)]
#[allow(clippy::excessive_precision, clippy::cloned_ref_to_slice_refs)]
mod tests {
    use super::*;

    fn lyria() -> Body {
        Body {
            id: "stanton3a".into(),
            name: "Lyria".into(),
            center: [18_703_607_171.556, -22_121_650_133.864, 0.0],
            radius_m: 223_000.0,
            period_h: 6.42999983,
            atmo_km: Some(10.0),
            phase_deg: None,
        }
    }

    #[test]
    fn unix_time_from_date() {
        assert_eq!(unix_ms_utc(2020, 1, 1, 0, 0, 0), EPOCH_UNIX_MS);
        assert_eq!(unix_ms_utc(2026, 11, 11, 3, 38, 21), 1_794_368_301_000);
    }

    /// Two real readings from LIVE 4.10.2, 39 s apart.
    #[test]
    fn phase_from_real_readings_agrees() {
        let b = lyria();
        let a = phase_from_reading(
            &b,
            [-64_568.6, -205_952.7],
            [18_703_426_338.2, -22_121_532_299.4],
            unix_ms_utc(2026, 11, 11, 3, 37, 42),
        );
        let c = phase_from_reading(
            &b,
            [-64_583.1, -205_899.9],
            [18_703_425_149.0, -22_121_534_238.7],
            unix_ms_utc(2026, 11, 11, 3, 38, 21),
        );
        assert!(angle_diff(a, c).abs() < 0.03, "{a} {c}");
        assert!(angle_diff(a, 175.39).abs() < 0.1, "{a}");
    }

    #[test]
    fn game_frame_applies_phase() {
        let mut b = lyria();
        let g = Geo {
            lat: 0.0,
            lon: 1.0,
            r: 223_000.0,
        };
        assert!(b.epoch_to_game(g.to_fixed()).is_none());
        b.phase_deg = Some(1f64.to_degrees());
        let v = b.epoch_to_game(g.to_fixed()).unwrap();
        assert!((v[0] - 223_000.0).abs() < 1e-6 && v[1].abs() < 1e-6);
    }

    /// A global fix on a calibrated body and a CamPos fix of the same spot
    /// land on the same game-frame position.
    #[test]
    fn global_and_body_fixes_agree() {
        let mut b = lyria();
        b.phase_deg = Some(175.37);
        let cam = [-75_099.154349, -202_924.256298, -63_649.054938];
        let t0 = 1_794_356_801_000;
        let root = b.game_to_global(cam, t0).unwrap();
        let mut a = Navigator::default();
        let (ga, _) = a.on_fix(&[b.clone()], 0.0, t0, root).unwrap();
        assert!(a.game_frame());
        let mut c = Navigator::default();
        let (gc, _) = c.on_body_fix(&b, 0.0, cam);
        assert!(ga.angle_to(&gc) * 223_000.0 < 1e-3);
        // uncalibrated: the epoch frame, flagged
        b.phase_deg = None;
        let mut u = Navigator::default();
        u.on_fix(&[b.clone()], 0.0, t0, root).unwrap();
        assert!(!u.game_frame());
    }

    /// A hard turn at speed: the nose swings at once, the velocity follows.
    #[test]
    fn track_lags_nose_in_a_turn() {
        let b = lyria();
        let t0 = 1_794_356_801_000i64;
        let mut nav = Navigator::default();
        nav.on_sample(sample(0.0, 900.0, 0.0));
        nav.on_fix(
            &[b.clone()],
            0.0,
            t0,
            b.to_global([223_000.0, 0.0, 0.0], t0),
        )
        .unwrap();
        nav.on_sample(sample(0.1, 900.0, 0.0));
        // 2 g lateral at 900 m/s turns about 1.25 deg/s.
        let mut t = 0.1;
        for _ in 0..10 {
            t += 0.1;
            nav.on_sample(Sample {
                g: 2.0,
                ..sample(t, 900.0, 90.0)
            });
        }
        let track = nav.track().unwrap();
        assert!((track - 1.25).abs() < 0.05, "{track}");
    }

    /// Close fixes override the estimate: a strafe the nose can't show.
    #[test]
    fn close_fixes_measure_real_track() {
        let b = lyria();
        let t0 = 1_794_356_801_000i64;
        let r = 224_000.0;
        let start = Geo {
            lat: 0.0,
            lon: 0.5,
            r,
        };
        let fixed = |g: Geo| {
            [
                g.r * g.lat.cos() * g.lon.cos(),
                g.r * g.lat.cos() * g.lon.sin(),
                g.r * g.lat.sin(),
            ]
        };
        let mut nav = Navigator::default();
        nav.on_sample(sample(0.0, 100.0, 90.0));
        nav.on_fix(&[b.clone()], 0.0, t0, b.to_global(fixed(start), t0))
            .unwrap();
        let end = start.step(0.0, 100.0 / r);
        nav.on_fix(
            &[b.clone()],
            1.0,
            t0 + 1000,
            b.to_global(fixed(end), t0 + 1000),
        )
        .unwrap();
        assert!(angle_diff(nav.track().unwrap(), 0.0).abs() < 0.1);
    }

    fn sample(t: f64, speed: f64, heading: f64) -> Sample {
        Sample {
            t,
            speed,
            heading,
            vsi: 0.0,
            altitude: Some(1000.0),
            radar: Some(100.0),
            landed: false,
            g: 0.0,
        }
    }

    #[test]
    fn parses_real_line() {
        let s = "Coordinates: x:18703236341.617867 y:-22121750258.425072 z:-565.772854";
        assert_eq!(
            parse_showlocation(s),
            Some([18703236341.617867, -22121750258.425072, -565.772854])
        );
        assert!(parse_showlocation(&format!("{s}\r\n")).is_some());
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "",
            "hello",
            "Coordinates: x:1 y:2",
            "Coordinates: x:1 y:2 z:3 w:4",
            "Coordinates: y:1 x:2 z:3",
            "Coordinates: x:a y:2 z:3",
            "Coordinates: x:inf y:2 z:3",
            "Coordinates: x:NaN y:2 z:3",
            " Coordinates: x:1 y:2 z:3",
            "password Coordinates: x:1 y:2 z:3",
        ] {
            assert_eq!(parse_showlocation(bad), None, "{bad:?}");
        }
        assert_eq!(parse_showlocation(&"9".repeat(500)), None);
    }

    #[test]
    fn real_sample_resolves_to_lyria() {
        let bodies = [lyria()];
        let p = [18703236341.617867, -22121750258.425072, -565.772854];
        assert_eq!(nearest_body(&bodies, p).unwrap().id, "stanton3a");
    }

    #[test]
    fn surface_point_is_stationary_in_fixed_frame() {
        let b = lyria();
        let v = [150_000.0, 120_000.0, 90_000.0];
        for t in [EPOCH_UNIX_MS, 1_794_356_801_000, 1_794_400_000_123] {
            let back = b.to_fixed(b.to_global(v, t), t);
            for i in 0..3 {
                assert!((back[i] - v[i]).abs() < 1e-3, "{back:?}");
            }
        }
    }

    #[test]
    fn full_period_returns_to_start() {
        let b = lyria();
        let t0 = 1_794_356_801_000;
        let (a0, a1) = (b.spin(t0), b.spin(t0 + (b.period_h * 3_600_000.0) as i64));
        assert!((a0 - a1).abs() < 1e-5);
    }

    #[test]
    fn bearings_are_compass() {
        let o = Geo {
            lat: 0.0,
            lon: 0.0,
            r: 1.0,
        };
        let n = Geo {
            lat: 0.1,
            lon: 0.0,
            r: 1.0,
        };
        let e = Geo {
            lat: 0.0,
            lon: 0.1,
            r: 1.0,
        };
        assert!(o.bearing_to(&n).abs() < 1e-9);
        assert!((o.bearing_to(&e) - 90.0).abs() < 1e-9);
        assert!((n.bearing_to(&o) - 180.0).abs() < 1e-9);
        assert!((e.bearing_to(&o) - 270.0).abs() < 1e-9);
    }

    #[test]
    fn step_then_measure_agrees() {
        let o = Geo {
            lat: 0.4,
            lon: -1.1,
            r: 1.0,
        };
        let d = o.step(37.0, 0.05);
        assert!((o.angle_to(&d) - 0.05).abs() < 1e-9);
        assert!((o.bearing_to(&d) - 37.0).abs() < 1e-6);
    }

    #[test]
    fn angle_diff_wraps() {
        assert_eq!(angle_diff(10.0, 350.0), 20.0);
        assert_eq!(angle_diff(350.0, 10.0), -20.0);
    }

    #[test]
    fn stopping_distance_formula() {
        assert_eq!(stopping_distance(100.0, 10.0, 0.0), 500.0);
        assert_eq!(stopping_distance(100.0, 10.0, 2.0), 700.0);
    }

    /// A rotating body and a ship flying a straight heading: the fix at the end
    /// must land where dead reckoning said, despite the spin in between.
    #[test]
    fn dead_reckoning_matches_truth_through_rotation() {
        let b = lyria();
        let bodies = [b.clone()];
        let t0 = 1_794_356_801_000i64;
        let r = b.radius_m + 1000.0;
        let start = Geo {
            lat: 0.3,
            lon: 1.0,
            r,
        };
        let fixed = |g: Geo| {
            [
                g.r * g.lat.cos() * g.lon.cos(),
                g.r * g.lat.cos() * g.lon.sin(),
                g.r * g.lat.sin(),
            ]
        };

        let mut nav = Navigator::default();
        nav.on_sample(sample(0.0, 200.0, 70.0));
        nav.on_fix(&bodies, 0.0, t0, b.to_global(fixed(start), t0))
            .unwrap();
        let mut truth = start;
        for i in 1..=600 {
            let t = i as f64 * 0.1;
            // Constant-bearing updates, the same way a compass-holding pilot flies.
            let heading = 70.0;
            nav.on_sample(sample(t, 200.0, heading));
            truth = truth.step(heading, 200.0 * 0.1 / r);
        }
        let t1 = t0 + 60_000;
        let (_, leg) = nav
            .on_fix(&bodies, 60.0, t1, b.to_global(fixed(truth), t1))
            .unwrap();
        let leg = leg.unwrap();
        assert!(leg.error_m.unwrap() < 1.0, "{leg:?}");
        assert!((leg.path_m - 12_000.0).abs() < 1.0, "{leg:?}");
        assert!(angle_diff(leg.mean_heading.unwrap(), 70.0).abs() < 1e-6);
        assert!(angle_diff(leg.truth_bearing, 70.0).abs() < 0.05, "{leg:?}");
    }

    #[test]
    fn brake_cue_latches_while_slowing() {
        let b = lyria();
        let t0 = 1_794_356_801_000i64;
        let r = b.radius_m;
        let mut nav = Navigator::default();
        nav.on_sample(sample(0.0, 200.0, 80.0));
        nav.on_fix(&[b.clone()], 0.0, t0, b.to_global([r, 0.0, 0.0], t0))
            .unwrap();
        nav.set_target(Some(nav.position().unwrap().step(80.0, 1_000.0 / r)));
        assert_eq!(nav.guidance(20.0, 2.0).unwrap().cue, Cue::Brake);
        // Slower, so the stopping distance now fits easily; still braking.
        nav.on_sample(sample(0.1, 50.0, 80.0));
        assert_eq!(nav.guidance(20.0, 2.0).unwrap().cue, Cue::Brake);
        // Stopped well short of the target: the latch releases, but not "arrived".
        nav.on_sample(sample(0.2, 0.0, 80.0));
        assert_eq!(nav.guidance(20.0, 2.0).unwrap().cue, Cue::NotClosing);
    }

    #[test]
    fn stationary_ship_keeps_position_across_fixes() {
        let b = lyria();
        let bodies = [b.clone()];
        let v = [150_000.0, 120_000.0, 90_000.0];
        let t0 = 1_794_356_801_000i64;
        let mut nav = Navigator::default();
        nav.on_fix(&bodies, 0.0, t0, b.to_global(v, t0)).unwrap();
        let t1 = t0 + 600_000;
        let (_, leg) = nav.on_fix(&bodies, 600.0, t1, b.to_global(v, t1)).unwrap();
        assert!(leg.unwrap().truth_m < 0.01);
    }

    #[test]
    fn leaving_atmosphere_marks_position_stale() {
        let b = lyria();
        let mut nav = Navigator::default();
        nav.on_fix(
            &[b.clone()],
            0.0,
            1_794_356_801_000,
            b.to_global([223_000.0, 0.0, 0.0], 1_794_356_801_000),
        )
        .unwrap();
        nav.on_sample(sample(0.0, 100.0, 0.0));
        nav.on_sample(Sample {
            altitude: None,
            ..sample(0.1, 100.0, 0.0)
        });
        assert!(nav.stale());
    }

    #[test]
    fn guidance_steers_and_cues_brake() {
        let b = lyria();
        let t0 = 1_794_356_801_000i64;
        let r = b.radius_m;
        let mut nav = Navigator::default();
        nav.on_sample(sample(0.0, 200.0, 80.0));
        nav.on_fix(&[b.clone()], 0.0, t0, b.to_global([r, 0.0, 0.0], t0))
            .unwrap();
        let start = nav.position().unwrap();

        // 10 km due east, flying 080: needs a right turn of 10 degrees.
        nav.set_target(Some(start.step(90.0, 10_000.0 / r)));
        let g = nav.guidance(20.0, 2.0).unwrap();
        assert!((g.relative - 10.0).abs() < 0.01, "{g:?}");
        assert!((g.distance_m - 10_000.0).abs() < 1.0);
        assert_eq!(g.cue, Cue::Cruise);

        // 1 km out at 200 m/s with 20 m/s²: 1,000 + 400 m needed, so brake.
        nav.set_target(Some(start.step(80.0, 1_000.0 / r)));
        assert_eq!(nav.guidance(20.0, 2.0).unwrap().cue, Cue::Brake);

        // Target behind the ship.
        nav.set_target(Some(start.step(260.0, 5_000.0 / r)));
        assert_eq!(nav.guidance(20.0, 2.0).unwrap().cue, Cue::NotClosing);
    }

    #[test]
    fn cross_track_sign() {
        let o = Geo {
            lat: 0.0,
            lon: 0.0,
            r: 1000.0,
        };
        let target = o.step(90.0, 0.1);
        let right = o.step(100.0, 0.05);
        let left = o.step(80.0, 0.05);
        let xt = |p: Geo| {
            let delta = (o.bearing_to(&p) - o.bearing_to(&target)).to_radians();
            (o.angle_to(&p).sin() * delta.sin()).asin()
        };
        assert!(xt(right) > 0.0 && xt(left) < 0.0);
    }
}
