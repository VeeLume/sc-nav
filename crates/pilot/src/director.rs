//! The flight director's state, shared by `probe-flight-director` (console) and
//! `probe-hud-overlay` (egui): nav-data with measured phases, the brake model,
//! the [`Navigator`], the current target and the saved waypoints.
//!
//! Everything is in the game's own body frame (CamPos, SCAR, community
//! lat/long), waypoints included. Overlay fixes ([`Director::on_overlay`]) are
//! already in it: no clock, no phase. `/showlocation` fixes are global and need
//! the PC clock plus the body's phase; [`Director::calibrate`] measures that
//! phase from every overlay fix against the same PC clock, so a clock offset
//! cancels. Until a body has a phase, `/showlocation` there is provisional:
//! steering and marking wait for a calibrated or overlay fix.
//!
//! Marks in space are system-frame (`Root`) points, steered to with
//! [`sc_nav_geo::space`]: position from `Root` fixes (or CamPos turned global),
//! view from `r_DisplayInfo 2`'s `CamDir`. Marks are [`Location`]s; their codes
//! turn a mark into one line of text to share.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sc_nav_geo::space::{SpaceGuidance, SpaceTrack, camdir_basis};
use sc_nav_geo::{
    Body, G, Geo, Guidance, Leg, Location, Navigator, Sample, V3, World, angle_diff, circular_mean,
};

/// Measured phases from `phases.json` (`{body id: [samples]}`), as the mean
/// per body. Missing or unreadable: none.
pub fn load_phases(path: &Path) -> BTreeMap<String, f64> {
    let all: BTreeMap<String, Vec<f64>> = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    all.into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k, circular_mean(&v)))
        .collect()
}

/// Faster than this between two `Root` fixes is a jump, not speed (quantum
/// travel, a misread that slipped through).
pub const MAX_SPACE_SPEED: f64 = 3000.0;

/// Where the ship is, for display.
#[derive(Clone, Debug)]
pub struct Here {
    pub body_id: String,
    pub body_name: String,
    /// Game-frame (community) lat/long in degrees; `None` until the body's
    /// phase is calibrated.
    pub lat_lon: Option<(f64, f64)>,
    pub scar: Option<String>,
    /// Above the body's radius, m.
    pub height: f64,
    pub stale: bool,
}

pub struct Director {
    pub data: World,
    pub nav: Navigator,
    pub brake_g: Option<f64>,
    pub reaction: f64,
    /// Ship class, from telemetry: picks the brake figure.
    pub ship: String,
    target_name: Option<String>,
    target_body: Option<String>,
    /// The target when it is a point in space (system frame).
    space_target: Option<V3>,
    /// Position, velocity and view in the system frame.
    pub space: SpaceTrack,
    /// The system as the overlay names it (`Current player location`);
    /// `None`: guessed from the nearest body.
    system: Option<String>,
    /// Session time of the last telemetry sample with a heading reference.
    last_tel: Option<f64>,
    /// The previous overlay fix (session time, body-frame position), for
    /// speed on foot.
    prev_fix: Option<(f64, Geo)>,
    waypoints: BTreeMap<String, Location>,
    waypoints_path: Option<PathBuf>,
    /// Where measured phases are saved (`nav-calibrate`'s file); `None`
    /// keeps them for the session only.
    pub phases_path: Option<PathBuf>,
    /// Phase samples measured this session, per body id.
    calib: BTreeMap<String, Vec<f64>>,
}

/// One phase measurement and where the body's phase stands after it.
#[derive(Clone, Debug)]
pub struct Calibration {
    pub body_id: String,
    pub body_name: String,
    pub sample: f64,
    /// Session mean, applied once there are [`Director::MIN_SAMPLES`].
    pub mean: f64,
    pub samples: usize,
    /// The phase in use before this sample, if any.
    pub before: Option<f64>,
    /// Just reached `MIN_SAMPLES`: the body is calibrated from here on.
    pub applied_now: bool,
    /// Off the session mean by more than `MAX_SPREAD`: not used.
    pub rejected: bool,
}

impl Director {
    /// Session samples before a measured phase replaces the stored one.
    pub const MIN_SAMPLES: usize = 3;
    /// A sample this far (degrees) from the session mean is a misread or a
    /// clock jump. Recorded grabs spread 0.0016°.
    pub const MAX_SPREAD: f64 = 0.02;
    const KEEP: usize = 60;

    /// `waypoints_path`: where waypoints load from and save to; `None` keeps
    /// them in memory (replays).
    pub fn new(data: World, waypoints_path: Option<PathBuf>) -> Self {
        let waypoints = waypoints_path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            data,
            nav: Navigator::default(),
            brake_g: None,
            reaction: 2.0,
            ship: String::new(),
            target_name: None,
            target_body: None,
            space_target: None,
            space: SpaceTrack::default(),
            system: None,
            last_tel: None,
            prev_fix: None,
            waypoints,
            waypoints_path,
            phases_path: None,
            calib: BTreeMap::new(),
        }
    }

    /// Measure the phase of the body at `root` from one overlay reading:
    /// `cam` (`CamPos Planet Zone`, body frame) and `root` (global), both at
    /// `unix_ms` on this PC's clock. Call before [`Self::on_fix`] with the
    /// same reading, so the fix already converts with the new phase.
    pub fn calibrate(&mut self, cam: V3, root: V3, unix_ms: i64) -> Option<Calibration> {
        let body = sc_nav_geo::nearest_body(&self.data.bodies, root)?.clone();
        let sample =
            sc_nav_geo::phase_from_reading(&body, [cam[0], cam[1]], [root[0], root[1]], unix_ms);
        let list = self.calib.entry(body.id.clone()).or_default();
        let rejected = list.len() >= Self::MIN_SAMPLES
            && angle_diff(sample, circular_mean(list)).abs() > Self::MAX_SPREAD;
        if !rejected {
            list.push(sample);
            if list.len() > Self::KEEP {
                list.remove(0);
            }
        }
        let (mean, samples) = (circular_mean(list), list.len());
        let before = body.phase_deg;
        let applied_now = !rejected && samples == Self::MIN_SAMPLES;
        if samples >= Self::MIN_SAMPLES && !rejected {
            if let Some(b) = self.data.bodies.iter_mut().find(|b| b.id == body.id) {
                b.phase_deg = Some(mean);
            }
            if applied_now || samples % 20 == 0 {
                self.save_phases(&body.id);
            }
        }
        Some(Calibration {
            body_id: body.id,
            body_name: body.name,
            sample,
            mean,
            samples,
            before,
            applied_now,
            rejected,
        })
    }

    /// This session's samples replace the body's list in `phases.json`.
    fn save_phases(&self, body_id: &str) {
        let (Some(path), Some(list)) = (&self.phases_path, self.calib.get(body_id)) else {
            return;
        };
        let mut all: BTreeMap<String, Vec<f64>> = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        all.insert(body_id.to_string(), list.clone());
        if let Ok(bytes) = serde_json::to_vec_pretty(&all) {
            let _ = std::fs::write(path, bytes);
        }
    }

    /// Replace the marks, in memory only (a replay starts from the ones that
    /// were saved when the session was recorded).
    pub fn with_waypoints(mut self, waypoints: BTreeMap<String, Location>) -> Self {
        self.waypoints = waypoints;
        self.waypoints_path = None;
        self
    }

    /// Braking acceleration in g and where it came from.
    pub fn brake(&self) -> (f64, &'static str) {
        if let Some(g) = self.brake_g {
            return (g, "override");
        }
        match self.data.ships.get(&self.ship) {
            Some(s) if s.backward_g > 0.0 => (s.backward_g, "retro"),
            _ => (1.0, "assumed"),
        }
    }

    pub fn on_sample(&mut self, s: Sample) {
        if s.altitude.is_some() {
            self.last_tel = Some(s.t);
        }
        self.nav.on_sample(s);
    }

    /// On foot (no vehicle in telemetry): arrival within 15 m instead of
    /// 500 m.
    pub fn set_on_foot(&mut self, on_foot: bool) {
        self.nav.arrive_radius = on_foot.then_some(15.0);
    }

    /// The camera's compass heading on `body` at `cam` (body frame), from
    /// `CamDir` (system frame) turned by the body's rotation at `unix_ms`.
    /// `None` without a phase for the body.
    pub fn view_heading(body: &Body, cam: V3, camdir: [i32; 3], unix_ms: i64) -> Option<f64> {
        use sc_nav_geo::space::{cross, dot, norm};
        let f = camdir_basis(camdir[0] as f64, camdir[1] as f64, camdir[2] as f64).forward;
        let c = body.center;
        let f_body =
            body.epoch_to_game(body.to_fixed([c[0] + f[0], c[1] + f[1], c[2] + f[2]], unix_ms))?;
        let n = norm(cam);
        if n == 0.0 {
            return None;
        }
        let up = [cam[0] / n, cam[1] / n, cam[2] / n];
        let east = cross([0.0, 0.0, 1.0], up);
        let north = cross(up, east);
        let (e, nn) = (dot(f_body, east), dot(f_body, north));
        (e.hypot(nn) > 1e-6).then(|| e.atan2(nn).to_degrees().rem_euclid(360.0))
    }

    /// Call once per telemetry frame: the brake cue latches in here. `None`
    /// without a fix, a target, or when the target is on another body.
    pub fn guidance(&mut self) -> Option<Guidance> {
        if self.provisional() || self.space_target.is_some() {
            return None;
        }
        if self.target_body.is_some()
            && self.target_body.as_deref() != self.nav.body().map(|b| b.id.as_str())
        {
            return None;
        }
        let (g, _) = self.brake();
        self.nav.guidance(g * G, self.reaction)
    }

    /// Guidance to a point in space at session time `t`. `None` without a
    /// space target or a `Root` fix.
    pub fn space_guidance(&mut self, t: f64) -> Option<SpaceGuidance> {
        let target = self.space_target?;
        let (g, _) = self.brake();
        self.space.guidance(t, target, g * G, self.reaction)
    }

    /// The target is a point in space.
    pub fn space_targeted(&self) -> bool {
        self.space_target.is_some()
    }

    /// A `/showlocation` fix: global, at `unix_ms` on the PC clock.
    pub fn on_fix(&mut self, t: f64, unix_ms: i64, xyz: V3) -> Option<(Geo, Option<Leg>)> {
        self.space.on_fix(t, xyz, MAX_SPACE_SPEED);
        self.nav.on_fix(&self.data.bodies, t, unix_ms, xyz)
    }

    /// A `Root` position (and `CamDir`) without a body-frame position.
    pub fn on_root(&mut self, t: f64, root: V3, camdir: Option<[i32; 3]>) {
        self.space.on_fix(t, root, MAX_SPACE_SPEED);
        if let Some([a, b, c]) = camdir {
            self.space
                .on_view(t, camdir_basis(a as f64, b as f64, c as f64));
        }
    }

    /// An overlay fix: `cam` is the position, in `body_id`'s game frame
    /// already. `root`, when it was read, calibrates that body's phase (with
    /// `unix_ms`) for later `/showlocation` fixes.
    pub fn on_overlay(
        &mut self,
        t: f64,
        unix_ms: i64,
        body_id: &str,
        cam: V3,
        root: Option<V3>,
        camdir: Option<[i32; 3]>,
    ) -> Option<(Option<Calibration>, Geo, Option<Leg>)> {
        let body = self.data.body(body_id)?.clone();
        let calibration = root.and_then(|r| self.calibrate(cam, r, unix_ms));
        // the system-frame position from Root only: CamPos turned global
        // carries the phase and clock errors times the distance (2 km at
        // 3,300 km off Arial for 0.04°)
        if let Some(global) = root {
            self.on_root(t, global, camdir);
        } else if let Some([a, b, c]) = camdir {
            self.space
                .on_view(t, camdir_basis(a as f64, b as f64, c as f64));
        }
        let (geo, leg) = self.nav.on_body_fix(&body, t, cam);
        self.sample_from_overlay(&body, t, unix_ms, cam, geo, camdir);
        Some((calibration, geo, leg))
    }

    /// With no telemetry heading (on foot, or outside an atmosphere), the
    /// overlay stands in: heading from `CamDir` (where you look; else the
    /// track from fixes), speed from the last two fixes. Fed as a sample, so
    /// guidance and its turn cue keep working.
    fn sample_from_overlay(
        &mut self,
        body: &Body,
        t: f64,
        unix_ms: i64,
        cam: V3,
        geo: Geo,
        camdir: Option<[i32; 3]>,
    ) {
        let prev = self.prev_fix.replace((t, geo));
        if self.last_tel.is_some_and(|lt| t - lt < 1.5) {
            return;
        }
        let heading = camdir
            .and_then(|cd| Self::view_heading(body, cam, cd, unix_ms))
            .or_else(|| self.nav.track());
        let Some(heading) = heading else { return };
        let speed = prev
            .filter(|(t0, _)| (0.3..=5.0).contains(&(t - t0)))
            .map_or(0.0, |(t0, g0)| g0.angle_to(&geo) * geo.r / (t - t0));
        self.nav.on_sample(Sample {
            t,
            speed,
            heading,
            vsi: 0.0,
            altitude: Some(geo.r - body.radius_m),
            radar: None,
            landed: false,
            g: 0.0,
        });
    }

    /// The last measured system-frame position and its system: as the
    /// overlay names it, else the nearest body's.
    pub fn here_space(&self) -> Option<(String, V3)> {
        let (_, p) = self.space.fix?;
        Some((
            self.system
                .clone()
                .or_else(|| sc_nav_geo::location::system_near(&self.data.bodies, p))?,
            p,
        ))
    }

    /// The system the overlay's location line names (`stanton`).
    pub fn set_system(&mut self, system: Option<String>) {
        if system.is_some() {
            self.system = system;
        }
    }

    /// Whether the system is read from the overlay rather than guessed.
    pub fn system_read(&self) -> bool {
        self.system.is_some()
    }

    /// A fix in the game's body frame of `body_id`, from anywhere.
    pub fn on_body_fix(&mut self, t: f64, body_id: &str, v: V3) -> Option<(Geo, Option<Leg>)> {
        let body = self.data.body(body_id)?.clone();
        Some(self.nav.on_body_fix(&body, t, v))
    }

    /// The position is in the epoch frame of an uncalibrated body: one
    /// overlay fix (or a phase) is needed before marks and targets apply.
    pub fn provisional(&self) -> bool {
        self.nav.position().is_some() && !self.nav.game_frame()
    }

    pub fn here(&self) -> Option<Here> {
        let (body, geo) = (self.nav.body()?, self.nav.position()?);
        let game = self.nav.game_frame().then(|| {
            let lon = (geo.lon.to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
            (geo.lat.to_degrees(), lon)
        });
        Some(Here {
            body_id: body.id.clone(),
            body_name: body.name.clone(),
            lat_lon: game,
            scar: game.map(|(lat, lon)| sc_nav_geo::scar::scar(lat, lon)),
            height: geo.r - body.radius_m,
            stale: self.nav.stale(),
        })
    }

    pub fn target_name(&self) -> Option<&str> {
        self.target_name.as_deref()
    }

    /// The target's body, when it isn't the one the ship is on.
    pub fn target_elsewhere(&self) -> Option<&str> {
        let t = self.target_body.as_deref()?;
        (Some(t) != self.nav.body().map(|b| b.id.as_str())).then_some(t)
    }

    pub fn waypoints(&self) -> &BTreeMap<String, Location> {
        &self.waypoints
    }

    pub fn clear(&mut self) {
        self.nav.set_target(None);
        self.target_name = None;
        self.target_body = None;
        self.space_target = None;
    }

    fn set(&mut self, name: String, w: Location) {
        self.clear();
        self.target_name = Some(name);
        match w {
            Location::Surface { body, geo } => {
                self.nav.set_target(Some(geo));
                self.target_body = Some(body);
            }
            Location::Space { xyz, .. } => {
                self.space_target = Some(xyz);
                self.space.new_target();
            }
        }
    }

    /// Save the current position in space (system frame) as `name`.
    pub fn mark_space(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("give the mark a name".into());
        }
        let (system, xyz) = self
            .here_space()
            .ok_or("no position in space yet: needs a Root reading or /showlocation")?;
        self.waypoints
            .insert(name.to_string(), Location::Space { system, xyz });
        self.save()
    }

    /// Add a mark (e.g. from a pasted code).
    pub fn add(&mut self, name: &str, w: Location) -> Result<(), String> {
        let name = if name.trim().is_empty() {
            "shared".to_string()
        } else {
            name.trim().to_string()
        };
        self.waypoints.insert(name, w);
        self.save()
    }

    /// Save the current position as `name`.
    pub fn mark(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("give the mark a name".into());
        }
        let (Some(geo), Some(body)) = (self.nav.position(), self.nav.body()) else {
            return Err("no position yet: take a fix (/showlocation or the overlay)".into());
        };
        if !self.nav.game_frame() {
            return Err(format!(
                "{} is not calibrated: take one overlay fix first",
                body.name
            ));
        }
        let w = Location::Surface {
            body: body.id.clone(),
            geo,
        };
        self.waypoints.insert(name.to_string(), w);
        self.save()
    }

    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        self.waypoints.remove(name);
        self.save()
    }

    /// Steer to a saved waypoint.
    pub fn goto(&mut self, name: &str) -> Result<String, String> {
        let (key, w) = self
            .waypoints
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(k, w)| (k.clone(), w.clone()))
            .ok_or_else(|| format!("no waypoint {name}"))?;
        self.set(key.clone(), w);
        Ok(key)
    }

    /// A point from game-frame lat/long (degrees) on `body_id`.
    pub fn point(&self, body_id: &str, lat: f64, lon: f64) -> Result<Location, String> {
        let body = self
            .data
            .body(body_id)
            .ok_or_else(|| format!("unknown body {body_id}"))?;
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=360.0).contains(&lon) {
            return Err("lat must be -90..90, lon -180..360".into());
        }
        let geo = Geo {
            lat: lat.to_radians(),
            lon: lon.to_radians(),
            r: body.radius_m,
        };
        Ok(Location::Surface {
            body: body.id.clone(),
            geo,
        })
    }

    /// A point from a SCAR code (its one-arcminute cell's centre).
    pub fn scar_point(&self, body_id: &str, code: &str) -> Result<Location, String> {
        let (lat, lon) =
            sc_nav_geo::scar::decode(code).ok_or_else(|| format!("not a SCAR code: {code}"))?;
        self.point(body_id, lat, lon)
    }

    /// Steer to a point; `save` also keeps it as a waypoint.
    pub fn go_to_point(&mut self, name: &str, w: Location, save: bool) -> Result<(), String> {
        let name = if name.trim().is_empty() {
            "point".to_string()
        } else {
            name.trim().to_string()
        };
        if save {
            self.waypoints.insert(name.clone(), w.clone());
            self.save()?;
        }
        self.set(name, w);
        Ok(())
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.waypoints_path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let bytes = serde_json::to_vec_pretty(&self.waypoints).map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| format!("saving waypoints: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lyria() -> Body {
        Body {
            id: "stanton3a".into(),
            name: "OOC_Stanton_3a_Lyria".into(),
            center: [18_778_498.0, -21_918_664.0, 0.0],
            radius_m: 223_000.0,
            period_h: 6.4,
            atmo_km: Some(15.0),
            phase_deg: Some(175.37),
        }
    }

    fn director() -> Director {
        let data = World {
            build: "test".into(),
            bodies: vec![lyria()],
            ships: BTreeMap::new(),
            systems: vec![],
        };
        Director::new(data, None)
    }

    /// A `/showlocation` fix at game-frame lat/long.
    fn fix_at(d: &mut Director, lat: f64, lon: f64) {
        let Location::Surface { geo, .. } = d.point("stanton3a", lat, lon).unwrap() else {
            unreachable!()
        };
        let g = Geo {
            r: 224_000.0,
            ..geo
        };
        let unix_ms = 1_794_000_000_000;
        let body = d.data.bodies[0].clone();
        d.on_fix(
            0.0,
            unix_ms,
            body.game_to_global(g.to_fixed(), unix_ms).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn lat_lon_round_trips_through_a_fix() {
        let mut d = director();
        fix_at(&mut d, -15.6612, 68.2905);
        let h = d.here().unwrap();
        let (lat, lon) = h.lat_lon.unwrap();
        assert!(
            (lat + 15.6612).abs() < 1e-6 && (lon - 68.2905).abs() < 1e-6,
            "{lat} {lon}"
        );
        assert_eq!(h.scar.unwrap(), sc_nav_geo::scar::scar(-15.6612, 68.2905));
        assert!((h.height - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn scar_point_lands_in_its_cell() {
        let mut d = director();
        let code = sc_nav_geo::scar::scar(-15.6612, 68.2905);
        let w = d.scar_point("stanton3a", &code).unwrap();
        fix_at(&mut d, -15.6612, 68.2905);
        let here = d.nav.position().unwrap();
        // one arcminute on Lyria is ~65 m; the centre is within a cell diagonal
        let Location::Surface { geo, .. } = w else {
            unreachable!()
        };
        assert!(here.angle_to(&geo) * 223_000.0 < 100.0);
    }

    #[test]
    fn mark_then_goto_targets_it() {
        let mut d = director();
        assert!(d.mark("A").is_err(), "no fix yet");
        fix_at(&mut d, 10.0, 20.0);
        d.mark("A").unwrap();
        fix_at(&mut d, 10.5, 20.0);
        assert_eq!(d.goto("a").unwrap(), "A");
        assert_eq!(d.target_name(), Some("A"));
        assert!(d.target_elsewhere().is_none());
        let t = d.nav.target().unwrap();
        assert!(d.nav.position().unwrap().angle_to(&t) * 223_000.0 > 1000.0);
        d.clear();
        assert!(d.target_name().is_none());
        d.delete("A").unwrap();
        assert!(d.goto("A").is_err());
    }

    /// Overlay readings at a known phase calibrate an uncalibrated body;
    /// a reading off by a clock jump is refused.
    #[test]
    fn calibrates_from_overlay_readings() {
        let mut d = director();
        let truth = d.data.bodies[0].phase_deg.take().unwrap();
        let body = d.data.bodies[0].clone();
        let reading = |unix_ms: i64, phase: f64| {
            // cam in the game frame; global = our frame rotated by spin
            let cam: V3 = [150_000.0, -120_000.0, 90_000.0];
            let ours_lon = cam[1].atan2(cam[0]) + phase.to_radians();
            let h = cam[0].hypot(cam[1]);
            let fixed = [h * ours_lon.cos(), h * ours_lon.sin(), cam[2]];
            (cam, body.to_global(fixed, unix_ms))
        };
        for k in 0..3 {
            let t = 1_794_000_000_000 + k * 1000;
            let (cam, root) = reading(t, truth);
            let c = d.calibrate(cam, root, t).unwrap();
            assert_eq!(c.applied_now, k == 2);
        }
        let got = d.data.bodies[0].phase_deg.unwrap();
        assert!(angle_diff(got, truth).abs() < 1e-6, "{got} vs {truth}");
        let (cam, root) = reading(1_794_000_010_000, truth + 0.3);
        assert!(d.calibrate(cam, root, 1_794_000_010_000).unwrap().rejected);
    }

    /// `/showlocation` on an uncalibrated body is provisional until one
    /// overlay fix, which needs no phase or clock.
    /// Codes round-trip, old `waypoints.json` entries still load, and a
    /// space mark steers with the space track.
    #[test]
    fn space_marks_and_codes() {
        let mut d = director();
        let old = r#"{"A":{"body":"stanton3a","geo":{"lat":-0.27,"lon":-1.87,"r":224081.5}}}"#;
        let m: BTreeMap<String, Location> = serde_json::from_str(old).unwrap();
        assert!(matches!(m["A"], Location::Surface { .. }));
        for w in [
            Location::Space {
                system: "stanton".into(),
                xyz: [12_892_406_889.4, -31_460_113.7, -222_064.1],
            },
            d.point("stanton3a", -15.63768, -107.39331).unwrap(),
        ] {
            let (name, back) =
                Location::parse_code(&w.code("Rock; field"), &d.data.bodies).unwrap();
            assert_eq!(name, "Rock; field");
            match (&w, &back) {
                (Location::Space { xyz: a, .. }, Location::Space { xyz: b, .. }) => {
                    assert!((0..3).all(|k| (a[k] - b[k]).abs() < 0.06))
                }
                (Location::Surface { geo: a, .. }, Location::Surface { geo: b, .. }) => {
                    assert!(a.angle_to(b) * 223_000.0 < 0.2)
                }
                _ => panic!("kind changed"),
            }
        }
        assert!(Location::parse_code("hello", &d.data.bodies).is_err());

        // a scout marks a spot in space; flying away and back steers to it
        d.on_root(0.0, [1_000_000.0, 0.0, 0.0], Some([0, 0, 0]));
        d.mark_space("rocks").unwrap();
        assert!(matches!(d.waypoints()["rocks"], Location::Space { .. }));
        d.on_root(10.0, [1_000_000.0, -20_000.0, 0.0], Some([0, 0, 0]));
        d.on_root(11.0, [1_000_000.0, -19_000.0, 0.0], Some([0, 0, 0]));
        d.goto("rocks").unwrap();
        assert!(
            d.guidance().is_none(),
            "no surface guidance to a space mark"
        );
        let g = d.space_guidance(11.0).unwrap();
        assert!((g.distance_m - 19_000.0).abs() < 1.0 && (g.closing - 1000.0).abs() < 1e-6);
        let (right, up) = g.target_view.unwrap();
        assert!(right.abs() < 1e-6 && up.abs() < 1e-6, "dead ahead along +y");
    }

    #[test]
    fn uncalibrated_body_waits_for_an_overlay_fix() {
        let mut d = director();
        let body = d.data.bodies[0].clone();
        d.data.bodies[0].phase_deg = None;
        let t = 1_794_000_000_000;
        let cam: V3 = [-75_099.154349, -202_924.256298, -63_649.054938];
        let root = body.game_to_global(cam, t).unwrap();
        d.on_fix(0.0, t, root).unwrap();
        assert!(d.provisional());
        assert!(d.here().unwrap().lat_lon.is_none());
        assert!(d.mark("x").is_err());
        // lat/long itself needs no phase
        let w = d.point("stanton3a", 1.0, 2.0).unwrap();
        d.go_to_point("p", w, false).unwrap();
        assert!(d.guidance().is_none());
        d.on_body_fix(1.0, "stanton3a", cam).unwrap();
        assert!(!d.provisional());
        let (lat, lon) = d.here().unwrap().lat_lon.unwrap();
        let g = Geo::from_fixed(cam);
        assert!((lat - g.lat.to_degrees()).abs() < 1e-9 && (lon - g.lon.to_degrees()).abs() < 1e-9);
        d.mark("x").unwrap();
    }
}
