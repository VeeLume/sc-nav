//! Live feeds, recording and replay around one [`Pilot`].
//!
//! [`Runtime::start`] spawns the feeds: the game's telemetry stream
//! (sc-dossier), `/showlocation` from the clipboard, and the overlay read off
//! the screen while switched on. Each becomes an [`Input`] stamped with the
//! corrected clock; while recording it is written to the session, then fed to
//! the pilot, then `on_change` runs (repaint, push to a Stream Deck, …).
//!
//! A session folder: `meta.json` (start, build, phases, marks),
//! `inputs.jsonl` (`{"ms", "input"}` per line) and `screens/` (every overlay
//! grab with what the reader made of it). Replay reads it back, or reads the
//! sc-grpc lab's older layout (`telemetry.jsonl` + `events.jsonl`), and can
//! re-read the saved grabs with the current reader.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sc_dossier::telemetry::{self, Frame, Status};
use sc_nav_capture::{Settings, grab_image, grab_log};
use sc_nav_geo::{Location, V3, World};
use sc_nav_overlay::{GrabLog, Reader};
use serde::{Deserialize, Serialize};

use crate::director::Director;
use crate::pilot::{Cmd, Input, Link, OverlayFix, Pilot};

/// What the runtime needs besides the pilot.
pub struct Config {
    /// Telemetry address, e.g. [`telemetry::DEFAULT_ADDR`].
    pub addr: String,
    /// The overlay font (`sc-nav-data`).
    pub font: Vec<u8>,
    /// The world as loaded, for replays (each starts from it).
    pub world: World,
    /// Where sessions are recorded.
    pub sessions: PathBuf,
    /// Skip the live feeds (tests, a UI demo).
    pub feeds: bool,
}

/// `meta.json`.
#[derive(Serialize, Deserialize)]
pub struct Meta {
    pub version: u32,
    pub started_ms: i64,
    #[serde(default, alias = "nav_build")]
    pub build: String,
    /// Phase per body id at the start, degrees.
    pub phases: BTreeMap<String, f64>,
    pub waypoints: BTreeMap<String, Location>,
    pub brake_g: Option<f64>,
    pub overlay_on: bool,
    #[serde(default)]
    pub clock_offset_ms: Option<f64>,
}

/// A recording in progress.
pub struct Recording {
    pub dir: PathBuf,
    pub started: Instant,
    pub inputs: u64,
    pub flags: u64,
    out: BufWriter<File>,
    last_flush: Instant,
}

#[derive(Serialize, Deserialize)]
struct Line {
    ms: i64,
    input: Input,
}

impl Recording {
    fn start(root: &Path, dir: &Director, overlay_on: bool) -> std::io::Result<Self> {
        let started_ms = sc_nav_clock::now_ms();
        let path = root.join(format!("session-{}", started_ms / 1000));
        std::fs::create_dir_all(path.join("screens"))?;
        let meta = Meta {
            version: 2,
            started_ms,
            build: dir.data.build.clone(),
            phases: dir
                .data
                .bodies
                .iter()
                .filter_map(|b| Some((b.id.clone(), b.phase_deg?)))
                .collect(),
            waypoints: dir.waypoints().clone(),
            brake_g: dir.brake_g,
            overlay_on,
            clock_offset_ms: sc_nav_clock::status().offset_ms,
        };
        std::fs::write(path.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
        Ok(Self {
            out: BufWriter::new(File::create(path.join("inputs.jsonl"))?),
            dir: path,
            started: Instant::now(),
            inputs: 0,
            flags: 0,
            last_flush: Instant::now(),
        })
    }

    fn write(&mut self, ms: i64, input: &Input) {
        if let Ok(s) = serde_json::to_string(&Line {
            ms,
            input: input.clone(),
        }) {
            let _ = writeln!(self.out, "{s}");
            self.inputs += 1;
        }
        // telemetry is buffered; everything else is what you come back for
        if !matches!(input, Input::Telemetry { .. }) || self.last_flush.elapsed().as_secs() >= 1 {
            let _ = self.out.flush();
            self.last_flush = Instant::now();
        }
        if matches!(input, Input::Flag { .. }) {
            self.flags += 1;
        }
    }

    /// Bytes on disk so far.
    pub fn size(&self) -> u64 {
        dir_size(&self.dir)
    }
}

fn dir_size(p: &Path) -> u64 {
    std::fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Recorded sessions under `root`, newest first.
pub fn sessions(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("meta.json").is_file())
        .collect();
    out.sort();
    out.reverse();
    out
}

/// A session's meta and inputs in time order, from either layout.
pub fn read_session(dir: &Path) -> std::io::Result<(Meta, Vec<(i64, Input)>)> {
    let meta: Meta = serde_json::from_slice(&std::fs::read(dir.join("meta.json"))?)?;
    let mut inputs = vec![];
    let lines = |name: &str| -> Vec<String> {
        File::open(dir.join(name))
            .map(|f| BufReader::new(f).lines().map_while(Result::ok).collect())
            .unwrap_or_default()
    };
    for l in lines("inputs.jsonl") {
        if let Ok(line) = serde_json::from_str::<Line>(&l) {
            inputs.push((line.ms, line.input));
        }
    }
    // the lab's layout: telemetry.jsonl + events.jsonl
    #[derive(Deserialize)]
    struct OldFrame {
        ms: i64,
        frame: Frame,
    }
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum OldEv {
        Showlocation {
            ms: i64,
            xyz: V3,
        },
        Overlay {
            ms: i64,
            cam: V3,
            root: Option<V3>,
            #[serde(default)]
            body: Option<String>,
            #[serde(default)]
            camdir: Option<[i32; 3]>,
            #[serde(default)]
            system: Option<String>,
            #[serde(default)]
            agl: Option<f64>,
        },
        Command {
            ms: i64,
            cmd: Cmd,
        },
        Flag {
            ms: i64,
            note: String,
        },
    }
    for l in lines("telemetry.jsonl") {
        if let Ok(f) = serde_json::from_str::<OldFrame>(&l) {
            inputs.push((
                f.ms,
                Input::Telemetry {
                    frame: Box::new(f.frame),
                },
            ));
        }
    }
    for l in lines("events.jsonl") {
        let Ok(ev) = serde_json::from_str::<OldEv>(&l) else {
            continue;
        };
        inputs.push(match ev {
            OldEv::Showlocation { ms, xyz } => (ms, Input::Showlocation { xyz }),
            OldEv::Overlay {
                ms,
                cam,
                root,
                body,
                camdir,
                system,
                agl,
            } => {
                let body = body.unwrap_or_default();
                (
                    ms,
                    Input::Overlay {
                        fix: OverlayFix {
                            body,
                            cam,
                            root,
                            camdir,
                            system,
                            agl,
                        },
                    },
                )
            }
            OldEv::Command { ms, cmd } => (ms, Input::Command { cmd }),
            OldEv::Flag { ms, note } => (ms, Input::Flag { note }),
        });
    }
    inputs.sort_by_key(|(ms, _)| *ms);
    Ok((meta, inputs))
}

/// A replay in progress, as a UI shows it.
pub struct ReplayState {
    pub name: String,
    pub speed: f64,
    pub reread: bool,
    pub total_ms: i64,
    pub pos_ms: i64,
    /// Re-read grabs: (accepted then, accepted now, outcome changed).
    pub reread_counts: (u32, u32, u32),
    pub flags: Vec<(i64, String)>,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct Control {
    overlay_on: bool,
    recording: Option<Recording>,
    replay: Option<ReplayState>,
    /// The live pilot, parked while a replay runs.
    parked: Option<Pilot>,
    /// Replay clock: (recorded ms at replay start, wall instant, speed).
    replay_clock: Option<(i64, Instant, f64)>,
    /// Where the last replay's position ended (body frame).
    replay_end: Option<sc_nav_geo::Geo>,
}

/// The pilot with its feeds. Clone it freely: all handles share one state.
#[derive(Clone)]
pub struct Runtime {
    pilot: Arc<Mutex<Pilot>>,
    ctl: Arc<Mutex<Control>>,
    cfg: Arc<Config>,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl Runtime {
    /// Spawn the live feeds around `pilot` (unless `cfg.feeds` is off).
    pub fn start(
        pilot: Pilot,
        cfg: Config,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        sc_nav_clock::start();
        let rt = Self {
            pilot: Arc::new(Mutex::new(pilot)),
            ctl: Arc::new(Mutex::new(Control::default())),
            cfg: Arc::new(cfg),
            on_change: Arc::new(on_change),
        };
        if rt.cfg.feeds {
            rt.spawn_telemetry()?;
            #[cfg(windows)]
            rt.spawn_clipboard();
            rt.spawn_screen();
        }
        Ok(rt)
    }

    /// The pilot, locked. Keep the guard short.
    pub fn pilot(&self) -> MutexGuard<'_, Pilot> {
        self.pilot.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ctl(&self) -> MutexGuard<'_, Control> {
        self.ctl.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Now on the pilot's clock: the corrected clock, or the replay's.
    pub fn now_ms(&self) -> i64 {
        match self.ctl().replay_clock {
            Some((rec_ms, at, speed)) => {
                rec_ms + (at.elapsed().as_secs_f64() * speed * 1000.0) as i64
            }
            None => sc_nav_clock::now_ms(),
        }
    }

    /// A live input: muted during a replay, recorded while recording.
    pub fn live(&self, ms: i64, input: Input) {
        {
            let mut ctl = self.ctl();
            if ctl.replay.is_some() {
                return;
            }
            if let Some(r) = ctl.recording.as_mut() {
                r.write(ms, &input);
            }
        }
        self.pilot().feed(ms, &input);
        (self.on_change)();
    }

    /// A command from the control surface, now.
    pub fn command(&self, cmd: Cmd) {
        if self.ctl().replay.is_some() {
            return self.pilot().say("stop the replay first");
        }
        self.live(sc_nav_clock::now_ms(), Input::Command { cmd });
    }

    pub fn flag(&self, note: &str) {
        let note = if note.trim().is_empty() {
            "flag".to_string()
        } else {
            note.trim().to_string()
        };
        self.live(sc_nav_clock::now_ms(), Input::Flag { note });
    }

    pub fn overlay_on(&self) -> bool {
        self.ctl().overlay_on
    }

    pub fn set_overlay(&self, on: bool) {
        self.ctl().overlay_on = on;
    }

    // --- recording ---

    pub fn recording(&self) -> Option<(PathBuf, Duration, u64, u64, u64)> {
        self.ctl().recording.as_ref().map(|r| {
            (
                r.dir.clone(),
                r.started.elapsed(),
                r.inputs,
                r.flags,
                r.size(),
            )
        })
    }

    pub fn start_recording(&self) {
        let overlay_on = self.overlay_on();
        let result = Recording::start(&self.cfg.sessions, &self.pilot().dir, overlay_on);
        let msg = match result {
            Ok(r) => {
                let msg = format!("recording to {}", r.dir.display());
                self.ctl().recording = Some(r);
                msg
            }
            Err(e) => format!("can't record: {e}"),
        };
        self.pilot().say(msg);
    }

    pub fn stop_recording(&self) {
        let rec = self.ctl().recording.take();
        if let Some(mut r) = rec {
            let _ = r.out.flush();
            self.pilot()
                .say(format!("saved {} ({} inputs)", r.dir.display(), r.inputs));
        }
    }

    // --- replay ---

    pub fn replay_state<T>(&self, f: impl FnOnce(Option<&ReplayState>) -> T) -> T {
        f(self.ctl().replay.as_ref())
    }

    /// Where the last replay ended, to compare with the live run.
    pub fn replay_end(&self) -> Option<sc_nav_geo::Geo> {
        self.ctl().replay_end
    }

    pub fn stop_replay(&self) {
        if let Some(r) = self.ctl().replay.as_ref() {
            r.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Play `dir` back into a fresh pilot (the live one is parked) at
    /// `speed`; `reread` runs the saved grabs through a fresh reader instead
    /// of the recorded overlay fixes.
    pub fn replay(&self, dir: &Path, speed: f64, reread: bool) -> std::io::Result<()> {
        let (meta, mut inputs) = read_session(dir)?;
        let screens = dir.join("screens");
        let mut flags = vec![];
        for (ms, i) in &inputs {
            if let Input::Flag { note } = i {
                flags.push((ms - meta.started_ms, note.clone()));
            }
        }
        let mut grabs: Vec<(i64, GrabLog)> = vec![];
        if reread {
            inputs.retain(|(_, i)| !matches!(i, Input::Overlay { .. }));
            grabs = grab_log(&screens)
                .unwrap_or_default()
                .into_iter()
                .map(|g| (g.unix_ms, g))
                .collect();
        }
        let total_ms = inputs
            .last()
            .map_or(0, |(ms, _)| ms - meta.started_ms)
            .max(grabs.last().map_or(0, |(ms, _)| ms - meta.started_ms));

        let mut world = self.cfg.world.clone();
        for b in &mut world.bodies {
            b.phase_deg = meta.phases.get(&b.id).copied();
        }
        let mut dir_ = Director::new(world, None).with_waypoints(meta.waypoints.clone());
        dir_.brake_g = meta.brake_g;
        let replay_pilot = Pilot::new(dir_, meta.started_ms);
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut ctl = self.ctl();
            if let Some(mut r) = ctl.recording.take() {
                let _ = r.out.flush();
            }
            let live = std::mem::replace(&mut *self.pilot(), replay_pilot);
            ctl.parked = Some(live);
            ctl.replay_clock = Some((meta.started_ms, Instant::now(), speed));
            ctl.replay = Some(ReplayState {
                name: dir
                    .file_name()
                    .map_or(String::new(), |n| n.to_string_lossy().into()),
                speed,
                reread,
                total_ms,
                pos_ms: 0,
                reread_counts: (0, 0, 0),
                flags,
                stop: stop.clone(),
            });
        }

        let rt = self.clone();
        let font = self.cfg.font.clone();
        let bodies = self.cfg.world.bodies.clone();
        std::thread::spawn(move || {
            let mut reader = if reread {
                Reader::new(&font).ok()
            } else {
                None
            };
            enum Item {
                In(Input),
                Grab(GrabLog),
            }
            let mut items: Vec<(i64, Item)> = inputs
                .into_iter()
                .map(|(ms, i)| (ms, Item::In(i)))
                .collect();
            items.extend(grabs.into_iter().map(|(ms, g)| (ms, Item::Grab(g))));
            items.sort_by_key(|(ms, _)| *ms);
            let base = Instant::now();
            for (ms, item) in items {
                let due = base
                    + Duration::from_millis((ms - meta.started_ms).max(0) as u64)
                        .div_f64(speed.max(0.01));
                while Instant::now() < due && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep((due - Instant::now()).min(Duration::from_millis(50)));
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                // the reader runs outside the locks: ~200 ms a grab
                let input = match item {
                    Item::In(i) => Some(i),
                    Item::Grab(old) => {
                        let read = reader.as_mut().and_then(|r| {
                            let img = grab_image(&screens, old.n)?;
                            Some(r.read(&img, &bodies, old.n, Instant::now(), old.unix_ms))
                        });
                        let Some((new, fix)) = read else { continue };
                        if let Some(run) = rt.ctl().replay.as_mut() {
                            let c = &mut run.reread_counts;
                            c.0 += u32::from(old.outcome == "accepted");
                            c.1 += u32::from(new.outcome == "accepted");
                            c.2 += u32::from(new.outcome != old.outcome);
                        }
                        fix.map(|f| Input::Overlay {
                            fix: overlay_fix(&f),
                        })
                    }
                };
                if let Some(run) = rt.ctl().replay.as_mut() {
                    run.pos_ms = ms - meta.started_ms;
                }
                if let Some(i) = input {
                    rt.pilot().feed(ms, &i);
                }
                (rt.on_change)();
            }
            rt.end_replay(stop.load(Ordering::Relaxed));
        });
        Ok(())
    }

    fn end_replay(&self, stopped: bool) {
        let (run, parked) = {
            let mut ctl = self.ctl();
            ctl.replay_clock = None;
            (ctl.replay.take(), ctl.parked.take())
        };
        if let Some(live) = parked {
            let replayed = std::mem::replace(&mut *self.pilot(), live);
            self.ctl().replay_end = replayed.dir.nav.position();
        }
        let mut p = self.pilot();
        if let Some(run) = run.filter(|r| r.reread) {
            let (then, now, changed) = run.reread_counts;
            p.say(format!(
                "re-read: {now} accepted now, {then} when recorded, {changed} outcomes changed"
            ));
        }
        p.say(if stopped {
            "replay stopped"
        } else {
            "replay finished"
        });
        drop(p);
        (self.on_change)();
    }

    // --- feeds ---

    fn spawn_telemetry(&self) -> std::io::Result<()> {
        let rt = self.clone();
        telemetry::spawn(self.cfg.addr.clone(), move |status| {
            let ms = sc_nav_clock::now_ms();
            let input = match status {
                Status::Frame(f) => Input::Telemetry { frame: f },
                Status::Connected => Input::Link { link: Link::Live },
                Status::Idle => Input::Link { link: Link::Idle },
                Status::Unavailable(e) => Input::Link {
                    link: Link::Down(e.to_string()),
                },
                _ => return ControlFlow::Continue(()),
            };
            rt.live(ms, input);
            ControlFlow::Continue(())
        })?;
        Ok(())
    }

    #[cfg(windows)]
    fn spawn_clipboard(&self) {
        let rt = self.clone();
        std::thread::spawn(move || {
            sc_nav_capture::clipboard::watch(|_, ms, xyz| {
                rt.live(ms, Input::Showlocation { xyz });
                ControlFlow::Continue(())
            })
        });
    }

    /// The screen reader: idles until switched on, saves grabs while
    /// recording, pauses during a replay.
    fn spawn_screen(&self) {
        let rt = self.clone();
        std::thread::spawn(move || {
            let settings = || {
                let ctl = rt.ctl();
                Settings {
                    enabled: ctl.overlay_on && ctl.replay.is_none(),
                    record: ctl.recording.as_ref().map(|r| r.dir.join("screens")),
                }
            };
            let result =
                sc_nav_capture::watch(&rt.cfg.font, &rt.cfg.world.bodies, settings, |fix| {
                    rt.live(
                        fix.unix_ms,
                        Input::Overlay {
                            fix: overlay_fix(&fix),
                        },
                    );
                    ControlFlow::Continue(())
                });
            if let Err(e) = result {
                rt.pilot().say(format!("overlay reader stopped: {e:#}"));
            }
        });
    }
}

/// What the pilot takes from a reader fix.
pub fn overlay_fix(f: &sc_nav_overlay::Fix) -> OverlayFix {
    OverlayFix {
        body: f.body.clone(),
        cam: f.cam,
        root: f.root,
        camdir: f.reading.camdir,
        system: f.reading.place.as_ref().map(|p| p.system()),
        agl: Some(f.reading.altitude),
    }
}
