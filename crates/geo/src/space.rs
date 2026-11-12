//! Navigation in the system frame (`Root`), for points in space.
//!
//! Orientation comes from `r_DisplayInfo 2`'s `CamDir: a b c`: CryEngine
//! `Ang3` in degrees, `R = Rz(c)·Ry(b)·Rx(a)`, the camera looking along
//! `R·(0,1,0)` with `R·(1,0,0)` to its right and `R·(0,0,1)` up, all in the
//! system frame (checked against telemetry heading to 1.5° median and against
//! the ground track to ~1° in straight flight). It is the camera, so head
//! tracking moves it; cues are relative to where you look.

use crate::{Cue, V3, stopping_distance};

/// The camera's axes in the system frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Basis {
    pub forward: V3,
    pub right: V3,
    pub up: V3,
}

/// `CamDir: a b c` (degrees) to the camera's axes.
pub fn camdir_basis(a: f64, b: f64, c: f64) -> Basis {
    let (sa, ca) = a.to_radians().sin_cos();
    let (sb, cb) = b.to_radians().sin_cos();
    let (sc, cc) = c.to_radians().sin_cos();
    // columns of Rz(c)·Ry(b)·Rx(a)
    let right = [cc * cb, sc * cb, -sb];
    let forward = [cc * sb * sa - sc * ca, sc * sb * sa + cc * ca, cb * sa];
    let up = [cc * sb * ca + sc * sa, sc * sb * ca - cc * sa, cb * ca];
    Basis { forward, right, up }
}

/// The inverse of [`camdir_basis`]: `CamDir` angles (degrees) for a camera
/// with these axes.
pub fn basis_to_camdir(b: &Basis) -> [f64; 3] {
    let pitch = b.forward[2].atan2(b.up[2]);
    let roll = (-b.right[2]).clamp(-1.0, 1.0).asin();
    let yaw = b.right[1].atan2(b.right[0]);
    [pitch.to_degrees(), roll.to_degrees(), yaw.to_degrees()]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Least-squares velocity through `(t, p)` points spanning at least 0.8 s.
pub fn fit_velocity(pts: &[(f64, V3)]) -> Option<V3> {
    let (t_first, t_last) = (pts.first()?.0, pts.last()?.0);
    if pts.len() < 2 || t_last - t_first < 0.8 {
        return None;
    }
    let n = pts.len() as f64;
    let tm = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let stt: f64 = pts.iter().map(|p| (p.0 - tm).powi(2)).sum();
    let mut v = [0.0; 3];
    for (k, vk) in v.iter_mut().enumerate() {
        let pm = pts.iter().map(|p| p.1[k]).sum::<f64>() / n;
        *vk = pts.iter().map(|p| (p.0 - tm) * (p.1[k] - pm)).sum::<f64>() / stt;
    }
    Some(v)
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// Where `dir` lies relative to the view: degrees right of forward and up
/// from the forward-right plane.
pub fn view_angles(basis: &Basis, dir: V3) -> (f64, f64) {
    let (f, r, u) = (
        dot(dir, basis.forward),
        dot(dir, basis.right),
        dot(dir, basis.up),
    );
    (r.atan2(f).to_degrees(), u.atan2(f.hypot(r)).to_degrees())
}

/// Guidance to a point in space.
#[derive(Debug, Clone, Copy)]
pub struct SpaceGuidance {
    pub distance_m: f64,
    /// Speed in the system frame, m/s (from consecutive fixes).
    pub speed: f64,
    /// Speed towards the target, m/s.
    pub closing: f64,
    pub eta_s: Option<f64>,
    pub stop_distance_m: f64,
    pub time_to_brake_s: Option<f64>,
    pub cue: Cue,
    /// The target relative to the view: (right, up) in degrees. `None`
    /// without a CamDir reading.
    pub target_view: Option<(f64, f64)>,
    /// The direction of travel relative to the view.
    pub travel_view: Option<(f64, f64)>,
    /// Angle between travel and the target direction, degrees.
    pub track_error: Option<f64>,
    /// Seconds since the position was measured (it is extrapolated).
    pub age_s: f64,
}

/// A position and velocity in the system frame, from `Root` fixes.
#[derive(Debug, Clone, Default)]
pub struct SpaceTrack {
    /// The latest fix: session time, position.
    pub fix: Option<(f64, V3)>,
    /// Recent fixes, for the velocity fit.
    recent: Vec<(f64, V3)>,
    pub velocity: Option<V3>,
    /// The latest camera orientation and its session time.
    pub view: Option<(f64, Basis)>,
    braking: bool,
    arrived: bool,
}

impl SpaceTrack {
    /// A `Root` position at session time `t`. Velocity is a straight-line
    /// fit through the fixes of the last [`Self::WINDOW`] seconds (a single
    /// pair is too jumpy for the braking cue); a step faster than
    /// `max_speed` (quantum travel, a misread) starts the fit over.
    pub fn on_fix(&mut self, t: f64, p: V3, max_speed: f64) {
        if let Some(&(t0, p0)) = self.recent.last() {
            let dt = t - t0;
            if dt <= 0.0 {
                return;
            }
            if norm(sub(p, p0)) / dt > max_speed || dt > Self::WINDOW {
                self.recent.clear();
            }
        }
        self.recent.push((t, p));
        self.recent.retain(|(ti, _)| t - ti <= Self::WINDOW);
        self.velocity = fit_velocity(&self.recent);
        self.fix = Some((t, p));
    }

    /// Seconds of fixes the velocity is fitted over.
    pub const WINDOW: f64 = 4.0;

    pub fn on_view(&mut self, t: f64, basis: Basis) {
        self.view = Some((t, basis));
    }

    /// Where the ship is at `t`, extrapolated from the last fix.
    pub fn position(&self, t: f64) -> Option<V3> {
        let (t0, p) = self.fix?;
        let v = self.velocity.unwrap_or([0.0; 3]);
        let dt = (t - t0).clamp(0.0, 10.0);
        Some([p[0] + v[0] * dt, p[1] + v[1] * dt, p[2] + v[2] * dt])
    }

    /// Reset the latched cue for a new target.
    pub fn new_target(&mut self) {
        self.braking = false;
        self.arrived = false;
    }

    /// Guidance to `target` at session time `t`, with `brake` m/s² and
    /// `reaction_s` before braking starts.
    pub fn guidance(
        &mut self,
        t: f64,
        target: V3,
        brake: f64,
        reaction_s: f64,
    ) -> Option<SpaceGuidance> {
        let (t0, _) = self.fix?;
        let pos = self.position(t)?;
        let to = sub(target, pos);
        let distance_m = norm(to);
        let dir = if distance_m > 0.0 {
            [to[0] / distance_m, to[1] / distance_m, to[2] / distance_m]
        } else {
            [0.0; 3]
        };
        let v = self.velocity.unwrap_or([0.0; 3]);
        let speed = norm(v);
        let closing = dot(v, dir);
        let stop_distance_m = stopping_distance(closing.max(0.0), brake, reaction_s);
        let eta_s = (closing > 5.0).then(|| distance_m / closing);
        let time_to_brake_s = (closing > 5.0).then(|| (distance_m - stop_distance_m) / closing);
        // wider release than entry, so hovering at the threshold doesn't flicker
        self.arrived = if self.arrived {
            distance_m < 1500.0 && speed < 30.0
        } else {
            distance_m < 1000.0 && speed < 15.0
        };
        let cue = if self.arrived {
            Cue::Arrived
        } else if closing <= 2.0 {
            Cue::NotClosing
        } else if self.braking || stop_distance_m >= distance_m {
            Cue::Brake
        } else if time_to_brake_s.is_some_and(|s| s < 10.0) {
            Cue::Prepare
        } else {
            Cue::Cruise
        };
        self.braking = cue == Cue::Brake;
        let view = self.view.filter(|(tv, _)| t - tv < 3.0).map(|(_, b)| b);
        let travel = (speed > 1.0).then(|| [v[0] / speed, v[1] / speed, v[2] / speed]);
        Some(SpaceGuidance {
            distance_m,
            speed,
            closing,
            eta_s,
            stop_distance_m,
            time_to_brake_s,
            cue,
            target_view: view.map(|b| view_angles(&b, dir)),
            travel_view: view.zip(travel).map(|(b, tr)| view_angles(&b, tr)),
            track_error: travel.map(|tr| dot(tr, dir).clamp(-1.0, 1.0).acos().to_degrees()),
            age_s: t - t0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-9)
    }

    #[test]
    fn zero_angles_look_along_y() {
        let b = camdir_basis(0.0, 0.0, 0.0);
        assert!(
            close(b.forward, [0.0, 1.0, 0.0])
                && close(b.right, [1.0, 0.0, 0.0])
                && close(b.up, [0.0, 0.0, 1.0])
        );
    }

    #[test]
    fn pitch_up_and_yaw_left() {
        // pitch +90 about x: forward goes up
        assert!(close(camdir_basis(90.0, 0.0, 0.0).forward, [0.0, 0.0, 1.0]));
        // yaw +90 about z: forward +y turns to -x
        assert!(close(
            camdir_basis(0.0, 0.0, 90.0).forward,
            [-1.0, 0.0, 0.0]
        ));
    }

    #[test]
    fn basis_is_orthonormal() {
        let b = camdir_basis(-75.0, 2.0, -162.0);
        for (x, y) in [(b.forward, b.right), (b.forward, b.up), (b.right, b.up)] {
            assert!(dot(x, y).abs() < 1e-12);
        }
        for x in [b.forward, b.right, b.up] {
            assert!((norm(x) - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn camdir_round_trips() {
        for cd in [[-75.0, 2.0, -162.0], [10.0, -30.0, 45.0], [0.0, 0.0, 0.0]] {
            let back = basis_to_camdir(&camdir_basis(cd[0], cd[1], cd[2]));
            assert!(
                (0..3).all(|k| (back[k] - cd[k]).abs() < 1e-9),
                "{cd:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn view_angles_right_and_up() {
        let b = camdir_basis(0.0, 0.0, 0.0);
        let (r, u) = view_angles(&b, [1.0, 1.0, 0.0]);
        assert!((r - 45.0).abs() < 1e-9 && u.abs() < 1e-9);
        let (r, u) = view_angles(&b, [0.0, 1.0, 1.0]);
        assert!(r.abs() < 1e-9 && (u - 45.0).abs() < 1e-9);
    }

    /// Flying straight at a point 50 km ahead at 1 km/s: on the nose, closing,
    /// and the cue goes from cruise to brake as it nears.
    #[test]
    fn straight_approach() {
        let mut s = SpaceTrack::default();
        s.on_view(0.0, camdir_basis(0.0, 0.0, 0.0));
        let target = [0.0, 50_000.0, 0.0];
        s.on_fix(0.0, [0.0, 0.0, 0.0], 5000.0);
        s.on_fix(1.0, [0.0, 1000.0, 0.0], 5000.0);
        s.on_view(1.0, camdir_basis(0.0, 0.0, 0.0));
        let g = s.guidance(1.0, target, 20.0, 2.0).unwrap();
        assert!((g.closing - 1000.0).abs() < 1e-6 && (g.distance_m - 49_000.0).abs() < 1e-6);
        let (r, u) = g.target_view.unwrap();
        assert!(r.abs() < 1e-9 && u.abs() < 1e-9);
        assert_eq!(g.cue, Cue::Cruise);
        // 30 km later: stop distance 25 km + 2 km reaction > 19 km left
        s.on_fix(30.0, [0.0, 30_000.0, 0.0], 5000.0);
        s.on_fix(31.0, [0.0, 31_000.0, 0.0], 5000.0);
        let g = s.guidance(31.0, target, 20.0, 2.0).unwrap();
        assert_eq!(g.cue, Cue::Brake);
    }

    /// Noisy fixes at 1 Hz along a straight 1 km/s run: the fit is far
    /// steadier than the last pair.
    #[test]
    fn fitted_velocity_smooths_noise() {
        let mut s = SpaceTrack::default();
        let noise = [3.0, -4.0, 5.0, -2.0, 4.0, -5.0];
        for (k, e) in noise.iter().enumerate() {
            s.on_fix(k as f64, [0.0, 1000.0 * k as f64 + e, 0.0], 5000.0);
        }
        let v = s.velocity.unwrap();
        assert!((v[1] - 1000.0).abs() < 4.0, "{v:?}");
    }

    #[test]
    fn a_jump_is_not_speed() {
        let mut s = SpaceTrack::default();
        s.on_fix(0.0, [0.0; 3], 5000.0);
        s.on_fix(1.0, [1e8, 0.0, 0.0], 5000.0);
        assert!(s.velocity.is_none());
    }
}
