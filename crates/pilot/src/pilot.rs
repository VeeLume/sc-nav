//! The fusion core: every input that drives navigation goes through
//! [`Pilot::feed`] with its time (corrected unix ms), and [`Pilot::snapshot`]
//! says what to show. No threads, no I/O beyond saving marks and phases, so
//! live use, replays and tests run the same code.

use std::collections::VecDeque;

use sc_dossier::telemetry::{Frame, Instruments};
use sc_nav_geo::ground::{GroundWarning, ground_warning};
use sc_nav_geo::space::SpaceGuidance;
use sc_nav_geo::{G, Guidance, Location, Sample, V3};
use serde::{Deserialize, Serialize};

use crate::director::{Director, Here};

/// One overlay fix as the reader delivered it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayFix {
    /// The body by the printed radius (or the location line).
    pub body: String,
    /// Position in the body's game frame (CamPos, or the `Zone` line).
    pub cam: V3,
    /// System frame, when read and cross-checked.
    pub root: Option<V3>,
    /// `r_DisplayInfo 2` only.
    pub camdir: Option<[i32; 3]>,
    /// The system the location line names.
    pub system: Option<String>,
    /// The overlay's `Altitude`: height above ground, m.
    pub agl: Option<f64>,
}

/// What the control surface (window, Stream Deck, …) can ask for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    /// Mark the position on the body below.
    Mark {
        name: String,
    },
    /// Mark the position in space (system frame).
    MarkSpace {
        name: String,
    },
    Goto {
        name: String,
    },
    Delete {
        name: String,
    },
    Clear,
    /// A lat/long point (SCAR already decoded; `input` is what was typed).
    Point {
        name: String,
        body: String,
        lat: f64,
        lon: f64,
        save: bool,
        input: String,
    },
    /// A pasted mark code; `go` also steers to it.
    AddCode {
        code: String,
        go: bool,
    },
}

/// Everything that drives the pilot. A recording is a list of these with
/// their times.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Input {
    Telemetry {
        frame: Box<Frame>,
    },
    Overlay {
        fix: OverlayFix,
    },
    Showlocation {
        xyz: V3,
    },
    Command {
        cmd: Cmd,
    },
    /// "Something's off here", for finding it in a recording.
    Flag {
        note: String,
    },
    /// The telemetry stream's state.
    Link {
        link: Link,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum Link {
    #[default]
    Connecting,
    Live,
    Idle,
    Down(String),
}

/// A line for the log, with its time.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub ms: i64,
    pub text: String,
}

/// Inputs to the ground warning.
#[derive(Debug, Clone, Default)]
struct Ground {
    /// The overlay's last height above ground: (t, m).
    overlay: Option<(f64, f64)>,
    /// Sink rate from the last two overlay readings, m/s.
    overlay_sink: Option<f64>,
    /// Telemetry: (t, vertical speed when it has a reference, radar).
    tel: Option<(f64, Option<f64>, Option<f64>)>,
    /// Local gravity from telemetry, in g.
    gravity_g: Option<f64>,
}

/// What to show, as plain data: copy it out and drop the lock.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub ms: i64,
    pub link: Link,
    /// Unix ms of the last telemetry frame.
    pub frame_ms: Option<i64>,
    pub ship: Option<String>,
    pub instruments: Option<Instruments>,
    pub health: Option<f32>,
    pub on_foot: bool,
    /// Where on a body (lat/long, SCAR, height).
    pub here: Option<Here>,
    /// Where in space: system and system-frame position.
    pub here_space: Option<(String, V3)>,
    /// The system is read from the overlay, not guessed.
    pub system_read: bool,
    /// The view direction is known (`r_DisplayInfo 2`).
    pub view: bool,
    pub target: Option<String>,
    /// The target is a point in space.
    pub target_space: bool,
    /// The target's body when it isn't the one below.
    pub target_elsewhere: Option<String>,
    /// The position is provisional (a `/showlocation` on an uncalibrated body).
    pub provisional: bool,
    pub guidance: Option<Guidance>,
    pub space: Option<SpaceGuidance>,
    pub ground: Option<GroundWarning>,
    /// Unix ms and source of the last fix.
    pub last_fix: Option<(i64, &'static str)>,
    pub fixes: u64,
    /// Braking acceleration in g and its source.
    pub brake: (f64, &'static str),
    pub reaction_s: f64,
}

pub struct Pilot {
    pub dir: Director,
    /// Session clock zero: times are seconds since this unix ms.
    start_ms: i64,
    now_ms: i64,
    link: Link,
    frame_ms: Option<i64>,
    ship: Option<String>,
    inst: Option<Instruments>,
    health: Option<f32>,
    on_foot: bool,
    guidance: Option<Guidance>,
    space: Option<SpaceGuidance>,
    ground: Ground,
    ground_w: Option<GroundWarning>,
    last_fix: Option<(i64, &'static str)>,
    fixes: u64,
    log: VecDeque<Message>,
}

impl Pilot {
    /// Messages kept.
    pub const LOG: usize = 12;

    pub fn new(dir: Director, start_ms: i64) -> Self {
        Self {
            dir,
            start_ms,
            now_ms: start_ms,
            link: Link::default(),
            frame_ms: None,
            ship: None,
            inst: None,
            health: None,
            on_foot: false,
            guidance: None,
            space: None,
            ground: Ground::default(),
            ground_w: None,
            last_fix: None,
            fixes: 0,
            log: VecDeque::new(),
        }
    }

    fn t(&self, ms: i64) -> f64 {
        (ms - self.start_ms) as f64 / 1000.0
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.log.push_front(Message {
            ms: self.now_ms,
            text: text.into(),
        });
        self.log.truncate(Self::LOG);
    }

    /// Newest first.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.log.iter()
    }

    /// One input at unix ms `ms` (the corrected clock).
    pub fn feed(&mut self, ms: i64, input: &Input) {
        self.now_ms = self.now_ms.max(ms);
        match input {
            Input::Telemetry { frame } => self.frame(ms, frame),
            Input::Overlay { fix } => self.overlay(ms, fix),
            Input::Showlocation { xyz } => self.showlocation(ms, *xyz),
            Input::Command { cmd } => self.command(cmd),
            Input::Flag { note } => self.say(format!("FLAG: {note}")),
            Input::Link { link } => self.link = link.clone(),
        }
    }

    /// Steering recomputed for `ms` without new input (extrapolates the space
    /// track; call it when drawing).
    pub fn tick(&mut self, ms: i64) {
        self.now_ms = self.now_ms.max(ms);
        self.space = self.dir.space_guidance(self.t(ms));
        self.update_ground(self.t(ms));
    }

    fn frame(&mut self, ms: i64, f: &Frame) {
        let t = self.t(ms);
        self.link = Link::Live;
        self.frame_ms = Some(ms);
        self.ship = f.vehicle.as_ref().map(|v| v.name.clone());
        self.health = f.actor.as_ref().map(|a| a.health.current);
        let flight = f.vehicle.as_ref().and_then(|v| v.flight.as_ref());
        self.inst = flight.map(|fl| fl.instruments);
        self.on_foot = f.vehicle.is_none();
        self.dir.set_on_foot(self.on_foot);
        if let (Some(v), Some(fl)) = (&f.vehicle, flight) {
            self.dir.ship.clone_from(&v.class_name);
            let i = fl.instruments;
            let heading = i
                .heading
                .map(f64::from)
                .or_else(|| self.dir.nav.track())
                .unwrap_or(0.0);
            self.dir.on_sample(Sample {
                t,
                speed: f64::from(i.speed),
                heading,
                vsi: f64::from(i.vertical_speed),
                altitude: i.altitude.map(f64::from),
                radar: i.radar_altitude.map(f64::from),
                landed: fl.landed,
                g: i.g_meter.map_or(0.0, |g| f64::from(g.current)),
            });
            self.guidance = self.dir.guidance();
        }
        self.space = self.dir.space_guidance(t);
        self.ground.tel = self.inst.map(|i| {
            (
                t,
                i.altitude.is_some().then_some(f64::from(i.vertical_speed)),
                i.radar_altitude.map(f64::from),
            )
        });
        self.ground.gravity_g = f
            .actor
            .as_ref()
            .map(|a| f64::from(a.gravity))
            .filter(|g| *g > 0.01)
            .or(self.ground.gravity_g);
        self.update_ground(t);
    }

    /// CamPos is the position (no clock, no phase); Root and the clock
    /// calibrate the body for `/showlocation`.
    fn overlay(&mut self, ms: i64, fix: &OverlayFix) {
        let t = self.t(ms);
        self.dir.set_system(fix.system.clone());
        let Some((cal, _, _)) = self
            .dir
            .on_overlay(t, ms, &fix.body, fix.cam, fix.root, fix.camdir)
        else {
            return self.say(format!("overlay fix on unknown body {}", fix.body));
        };
        if let Some(c) = cal {
            let name = pretty(&c.body_name);
            if c.rejected {
                self.say(format!(
                    "{name} phase sample {:.4} off the mean {:.4}: skipped",
                    c.sample, c.mean
                ));
            } else if c.applied_now {
                let was = c.before.map_or("none".into(), |b| format!("{b:.4}"));
                self.say(format!(
                    "{name}: /showlocation calibrated, phase {:.4} (was {was})",
                    c.mean
                ));
            }
        }
        if let Some(agl) = fix.agl {
            if let Some((t0, a0)) = self.ground.overlay
                && (0.3..=5.0).contains(&(t - t0))
            {
                self.ground.overlay_sink = Some((a0 - agl) / (t - t0));
            }
            self.ground.overlay = Some((t, agl));
        }
        self.fixed(ms, "overlay");
    }

    /// A `/showlocation` fix: global, at `ms`.
    fn showlocation(&mut self, ms: i64, xyz: V3) {
        if self.dir.on_fix(self.t(ms), ms, xyz).is_none() {
            return self.say("/showlocation fix not near any known body");
        }
        if self.dir.provisional() {
            let name = self
                .dir
                .nav
                .body()
                .map_or(String::new(), |b| pretty(&b.name));
            self.say(format!(
                "{name} has no phase yet: surface position provisional until one overlay fix"
            ));
        }
        self.fixed(ms, "/showlocation");
    }

    fn fixed(&mut self, ms: i64, source: &'static str) {
        self.fixes += 1;
        self.last_fix = Some((ms, source));
        self.guidance = self.dir.guidance();
        self.space = self.dir.space_guidance(self.t(ms));
        self.update_ground(self.t(ms));
    }

    /// Height above ground: telemetry radar below 500 m (true AGL there),
    /// else the overlay's carried forward by the sink. Sink from telemetry's
    /// vertical speed when it has a reference, else the overlay's.
    fn update_ground(&mut self, t: f64) {
        let g = &self.ground;
        let tel_fresh = g.tel.is_some_and(|(tt, ..)| t - tt < 1.0);
        let sink = match g.tel {
            Some((_, Some(vsi), _)) if tel_fresh => Some(-vsi),
            _ => g.overlay_sink,
        };
        let agl = match (g.tel, g.overlay) {
            (Some((_, _, Some(radar))), _) if tel_fresh && radar < 500.0 => Some(radar),
            (_, Some((t0, a0))) if t - t0 < 10.0 => Some(a0 - sink.unwrap_or(0.0) * (t - t0)),
            _ => None,
        };
        let gravity = g.gravity_g.unwrap_or(1.0) * G;
        let (brake, _) = self.dir.brake();
        self.ground_w = agl
            .zip(sink)
            .and_then(|(a, s)| ground_warning(a, s, brake * G, gravity, self.dir.reaction));
    }

    fn command(&mut self, cmd: &Cmd) {
        let result = match cmd {
            Cmd::Mark { name } => self.dir.mark(name).map(|()| format!("marked {name}")),
            Cmd::MarkSpace { name } => self
                .dir
                .mark_space(name)
                .map(|()| format!("marked {name} in space")),
            Cmd::Goto { name } => self.dir.goto(name).map(|k| format!("direct to {k}")),
            Cmd::Delete { name } => self.dir.delete(name).map(|()| format!("deleted {name}")),
            Cmd::Clear => {
                self.dir.clear();
                Ok("target cleared".into())
            }
            Cmd::Point {
                name,
                body,
                lat,
                lon,
                save,
                ..
            } => self
                .dir
                .point(body, *lat, *lon)
                .and_then(|w| self.dir.go_to_point(name, w, *save))
                .map(|()| format!("direct to {name}")),
            Cmd::AddCode { code, go } => Location::parse_code(code, &self.dir.data.bodies)
                .and_then(|(name, w)| {
                    let name = if name.trim().is_empty() {
                        "shared".to_string()
                    } else {
                        name
                    };
                    if *go {
                        self.dir
                            .go_to_point(&name, w, true)
                            .map(|()| format!("added {name}, direct to it"))
                    } else {
                        self.dir.add(&name, w).map(|()| format!("added {name}"))
                    }
                }),
        };
        self.guidance = self.dir.guidance();
        self.space = self.dir.space_guidance(self.t(self.now_ms));
        match result {
            Ok(msg) | Err(msg) => self.say(msg),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            ms: self.now_ms,
            link: self.link.clone(),
            frame_ms: self.frame_ms,
            ship: self.ship.clone(),
            instruments: self.inst,
            health: self.health,
            on_foot: self.on_foot,
            here: self.dir.here(),
            here_space: self.dir.here_space(),
            system_read: self.dir.system_read(),
            view: self.dir.space.view.is_some(),
            target: self.dir.target_name().map(str::to_string),
            target_space: self.dir.space_targeted(),
            target_elsewhere: self.dir.target_elsewhere().map(str::to_string),
            provisional: self.dir.provisional(),
            guidance: self.guidance,
            space: self.space,
            ground: self.ground_w,
            last_fix: self.last_fix,
            fixes: self.fixes,
            brake: self.dir.brake(),
            reaction_s: self.dir.reaction,
        }
    }
}

/// `OOC_Stanton_1a_Ariel` → `Stanton 1a Ariel`.
pub fn pretty(name: &str) -> String {
    name.trim_start_matches("OOC_").replace('_', " ")
}
