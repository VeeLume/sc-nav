//! The control window: position and target on top, then tabs (Navigate,
//! Marks, Record, Settings), the message log, and a status bar for
//! telemetry, overlay reader and clock. Every change to the navigation goes
//! through [`Runtime::command`], so recordings have it.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Layout, RichText};
use sc_nav_geo::Location;
use sc_nav_geo::space::{norm, sub};
use sc_nav_pilot::runtime::{Runtime, sessions};
use sc_nav_pilot::{Cmd, Link, Snapshot, pretty};

use crate::store::{LAB_MARKS, Prefs, Store, read_marks};
use crate::theme::{ACCENT, ALERT, DIM, INK, LINE, OK, WARN, card, dot, fmt_km, fmt_time};

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Navigate,
    Marks,
    Record,
    Settings,
}

#[derive(PartialEq, Clone, Copy)]
enum PointInput {
    LatLon,
    Scar,
}

pub struct Control {
    tab: Tab,
    mark_name: String,
    code: String,
    input: PointInput,
    body: Option<String>,
    lat: String,
    lon: String,
    scar: String,
    point_name: String,
    save_point: bool,
    flag_note: String,
    session: Option<PathBuf>,
    speed: f64,
    reread: bool,
    import_path: String,
    pub demo: bool,
}

impl Control {
    pub fn new(demo: bool) -> Self {
        Self {
            tab: Tab::Navigate,
            mark_name: String::new(),
            code: String::new(),
            input: PointInput::LatLon,
            body: None,
            lat: String::new(),
            lon: String::new(),
            scar: String::new(),
            point_name: String::new(),
            save_point: true,
            flag_note: String::new(),
            session: None,
            speed: 1.0,
            reread: false,
            import_path: LAB_MARKS.to_string(),
            demo,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, rt: &Runtime, s: &Snapshot, store: &Store, prefs: &mut Prefs) {
        let replaying = rt.replay_state(|r| r.is_some());
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(crate::theme::BG).inner_margin(egui::Margin::symmetric(12, 6)))
            .show(ui, |ui| self.status(ui, rt, s));
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(line) = rt.replay_state(|r| {
                r.map(|r| format!("REPLAY  {}  {} / {}", r.name, clock(r.pos_ms), clock(r.total_ms)))
            }) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(line).color(WARN).strong());
                    if ui.small_button("Stop").clicked() {
                        rt.stop_replay();
                    }
                });
            }
            position(ui, rt, s);
            ui.add_space(6.0);
            target(ui, rt, s, replaying);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (t, label) in [
                    (Tab::Navigate, "Navigate"),
                    (Tab::Marks, "Marks"),
                    (Tab::Record, "Record"),
                    (Tab::Settings, "Settings"),
                ] {
                    let on = self.tab == t;
                    let text = RichText::new(label).color(if on { INK } else { DIM }).strong();
                    let b = egui::Button::new(text)
                        .fill(if on { ACCENT.gamma_multiply(0.22) } else { egui::Color32::TRANSPARENT })
                        .stroke(egui::Stroke::new(1.0, if on { ACCENT } else { LINE }));
                    if ui.add(b).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                match self.tab {
                    Tab::Navigate => self.navigate(ui, rt, s, replaying),
                    Tab::Marks => self.marks(ui, rt, s, replaying),
                    Tab::Record => self.record(ui, rt, store, replaying),
                    Tab::Settings => self.settings(ui, rt, store, prefs),
                }
                ui.add_space(8.0);
                card(ui, "Log", |ui| {
                    let p = rt.pilot();
                    let mut any = false;
                    for m in p.messages().take(8) {
                        any = true;
                        let age = fmt_time((s.ms - m.ms) as f64 / 1000.0);
                        ui.label(RichText::new(format!("{age:>5}  {}", m.text)).color(DIM).monospace().size(12.0));
                    }
                    if !any {
                        ui.label(RichText::new("nothing yet").color(DIM));
                    }
                });
            });
        });
    }

    // --- tabs ---

    fn navigate(&mut self, ui: &mut egui::Ui, rt: &Runtime, s: &Snapshot, replaying: bool) {
        card(ui, "Mark this position", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.mark_name).hint_text("name (optional)").desired_width(150.0));
                let on_body = s.here.is_some() && !replaying;
                if ui.add_enabled(on_body, egui::Button::new("On the surface")).on_hover_text("in the frame of the body below: stays put while it spins").clicked() {
                    rt.command(Cmd::Mark { name: self.mark_name.trim().to_string() });
                    self.mark_name.clear();
                }
                let in_space = s.here_space.is_some() && !replaying;
                if ui.add_enabled(in_space, egui::Button::new("In space")).on_hover_text("a fixed point in the system").clicked() {
                    rt.command(Cmd::MarkSpace { name: self.mark_name.trim().to_string() });
                    self.mark_name.clear();
                }
            });
        });
        ui.add_space(6.0);
        card(ui, "Go to a point", |ui| self.point(ui, rt, replaying));
        ui.add_space(6.0);
        card(ui, "Mark code", |ui| self.code_input(ui, rt, replaying));
    }

    fn code_input(&mut self, ui: &mut egui::Ui, rt: &Runtime, replaying: bool) {
        ui.add(egui::TextEdit::singleline(&mut self.code).hint_text("paste a code (SCNAV1;…)").desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            let ok = !self.code.trim().is_empty() && !replaying;
            if ui.add_enabled(ok, egui::Button::new("Add")).clicked() {
                rt.command(Cmd::AddCode { code: self.code.trim().to_string(), go: false });
                self.code.clear();
            }
            if ui.add_enabled(ok, egui::Button::new("Add and go")).clicked() {
                rt.command(Cmd::AddCode { code: self.code.trim().to_string(), go: true });
                self.code.clear();
            }
        });
    }

    fn point(&mut self, ui: &mut egui::Ui, rt: &Runtime, replaying: bool) {
        let bodies: Vec<(String, String)> = {
            let p = rt.pilot();
            if self.body.is_none() {
                self.body = p.dir.nav.body().map(|b| b.id.clone());
            }
            p.dir.data.bodies.iter().map(|b| (b.id.clone(), pretty(&b.name))).collect()
        };
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.input, PointInput::LatLon, "Lat / long");
            ui.selectable_value(&mut self.input, PointInput::Scar, "SCAR");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = |id: &str| bodies.iter().find(|(b, _)| b == id).map_or(id.to_string(), |(_, n)| n.clone());
                let selected = self.body.as_deref().map_or("pick a body".into(), label);
                egui::ComboBox::from_id_salt("body").selected_text(selected).width(150.0).show_ui(ui, |ui| {
                    for (id, name) in &bodies {
                        ui.selectable_value(&mut self.body, Some(id.clone()), name);
                    }
                });
            });
        });
        egui::Grid::new("point").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            match self.input {
                PointInput::LatLon => {
                    ui.label(RichText::new("latitude").color(DIM));
                    ui.add(egui::TextEdit::singleline(&mut self.lat).hint_text("-15.6612  (north +)"));
                    ui.end_row();
                    ui.label(RichText::new("longitude").color(DIM));
                    ui.add(egui::TextEdit::singleline(&mut self.lon).hint_text("68.2905  (east +)"));
                    ui.end_row();
                }
                PointInput::Scar => {
                    ui.label(RichText::new("SCAR").color(DIM));
                    ui.add(egui::TextEdit::singleline(&mut self.scar).hint_text("13L-BA15 / V"));
                    ui.end_row();
                }
            }
            ui.label(RichText::new("name").color(DIM));
            ui.add(egui::TextEdit::singleline(&mut self.point_name).hint_text("optional"));
            ui.end_row();
        });
        let resolved = self.resolve(rt);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.save_point, "save as mark");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let go = egui::Button::new(RichText::new("Go").strong()).fill(ACCENT.gamma_multiply(0.35));
                if ui.add_enabled(resolved.is_ok() && !replaying, go).clicked() {
                    self.go(rt);
                }
            });
        });
        let (text, col) = match &resolved {
            Ok(w) => match (w.lat_lon(), w.scar()) {
                (Some((lat, lon)), Some(scar)) => (format!("→ {lat:.4}, {lon:.4} · {scar}"), DIM),
                _ => (String::new(), DIM),
            },
            Err(e) if self.has_input() => (e.clone(), WARN),
            Err(_) => (String::new(), DIM),
        };
        if !text.is_empty() {
            ui.label(RichText::new(text).color(col).size(12.0));
        }
    }

    fn has_input(&self) -> bool {
        match self.input {
            PointInput::LatLon => !self.lat.trim().is_empty() || !self.lon.trim().is_empty(),
            PointInput::Scar => !self.scar.trim().is_empty(),
        }
    }

    fn resolve(&self, rt: &Runtime) -> Result<Location, String> {
        let body = self.body.as_deref().ok_or("pick a body")?;
        let p = rt.pilot();
        match self.input {
            PointInput::LatLon => {
                let lat: f64 = self.lat.trim().parse().map_err(|_| "latitude is not a number")?;
                let lon: f64 = self.lon.trim().parse().map_err(|_| "longitude is not a number")?;
                p.dir.point(body, lat, lon)
            }
            PointInput::Scar => p.dir.scar_point(body, &self.scar),
        }
    }

    fn go(&mut self, rt: &Runtime) {
        let w = match self.resolve(rt) {
            Ok(w) => w,
            Err(e) => return rt.pilot().say(e),
        };
        let (Location::Surface { body, .. }, Some((lat, lon))) = (&w, w.lat_lon()) else {
            return;
        };
        let input = match self.input {
            PointInput::LatLon => format!("{}, {}", self.lat.trim(), self.lon.trim()),
            PointInput::Scar => self.scar.trim().to_uppercase(),
        };
        let name = if self.point_name.trim().is_empty() { input.clone() } else { self.point_name.trim().to_string() };
        rt.command(Cmd::Point { name, body: body.clone(), lat, lon, save: self.save_point, input });
    }

    fn marks(&mut self, ui: &mut egui::Ui, rt: &Runtime, s: &Snapshot, replaying: bool) {
        let (marks, here, bodies) = {
            let p = rt.pilot();
            let here = p.dir.nav.position().zip(p.dir.nav.body().map(|b| (b.id.clone(), b.radius_m)));
            let marks: Vec<(String, Location)> = p.dir.waypoints().iter().map(|(k, w)| (k.clone(), w.clone())).collect();
            let bodies: Vec<(String, String)> = p.dir.data.bodies.iter().map(|b| (b.id.clone(), pretty(&b.name))).collect();
            (marks, here, bodies)
        };
        card(ui, &format!("Marks ({})", marks.len()), |ui| {
            if marks.is_empty() {
                ui.label(RichText::new("No marks yet. Mark a position, paste a code, or import in Settings.").color(DIM));
                return;
            }
            let mut cmd = None;
            egui::Grid::new("marks").striped(true).num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
                for (name, w) in &marks {
                    let current = s.target.as_deref() == Some(name.as_str());
                    ui.label(RichText::new(name).color(if current { ACCENT } else { INK }).strong());
                    let place = match w {
                        Location::Surface { body, geo } => match &here {
                            Some((pos, (b, radius))) if b == body => fmt_km(pos.angle_to(geo) * radius),
                            _ => bodies.iter().find(|(b, _)| b == body).map_or(body.clone(), |(_, n)| n.clone()),
                        },
                        Location::Space { system, xyz } => match &s.here_space {
                            Some((sys, p)) if sys == system => format!("space · {}", fmt_km(norm(sub(*xyz, *p)))),
                            _ => format!("space · {}", pretty(system)),
                        },
                    };
                    ui.label(RichText::new(place).color(DIM));
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!replaying && !current, egui::Button::new("Go").small()).clicked() {
                            cmd = Some(Cmd::Goto { name: name.clone() });
                        }
                        if ui.small_button("Copy code").on_hover_text("one line of text to share this mark").clicked() {
                            ui.ctx().copy_text(w.code(name));
                            rt.pilot().say(format!("copied {name} as a code"));
                        }
                        if ui.add_enabled(!replaying, egui::Button::new(RichText::new("Delete").color(ALERT)).small()).clicked() {
                            cmd = Some(Cmd::Delete { name: name.clone() });
                        }
                    });
                    ui.end_row();
                }
            });
            if let Some(cmd) = cmd {
                rt.command(cmd);
            }
        });
        ui.add_space(6.0);
        card(ui, "Add from a code", |ui| self.code_input(ui, rt, replaying));
    }

    fn record(&mut self, ui: &mut egui::Ui, rt: &Runtime, store: &Store, replaying: bool) {
        card(ui, "Record", |ui| match rt.recording() {
            None => {
                ui.label(RichText::new("Saves every input (telemetry, fixes, commands, screen grabs) to replay a problem later.").color(DIM));
                ui.horizontal(|ui| {
                    if ui.add_enabled(!replaying, egui::Button::new("Start recording")).clicked() {
                        rt.start_recording();
                    }
                    if ui.small_button("Open folder").clicked() {
                        open(&store.sessions());
                    }
                });
                if !rt.overlay_on() && !self.demo {
                    ui.label(RichText::new("Screen grabs are saved only while the overlay reader is on.").color(DIM).size(12.0));
                }
            }
            Some((dir, took, inputs, flags, size)) => {
                ui.horizontal(|ui| {
                    dot(ui, ALERT, format!("REC {}", fmt_time(took.as_secs_f64())));
                    ui.label(RichText::new(format!("{inputs} inputs · {flags} flags · {:.1} MB", size as f64 / 1e6)).color(DIM));
                });
                ui.label(RichText::new(dir.display().to_string()).color(DIM).size(11.0));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.flag_note).hint_text("what's off?").desired_width(200.0));
                    if ui.button("Flag").on_hover_text("mark this moment in the recording").clicked() {
                        rt.flag(&self.flag_note);
                        self.flag_note.clear();
                    }
                });
                if ui.button("Stop recording").clicked() {
                    rt.stop_recording();
                }
            }
        });
        ui.add_space(6.0);
        card(ui, "Replay", |ui| self.replay(ui, rt, store));
    }

    fn replay(&mut self, ui: &mut egui::Ui, rt: &Runtime, store: &Store) {
        let running = rt.replay_state(|r| {
            r.map(|r| (r.pos_ms, r.total_ms, r.speed, r.reread, r.reread_counts, r.flags.clone()))
        });
        if let Some((pos, total, speed, reread, (then, now, changed), flags)) = running {
            let frac = if total > 0 { pos as f32 / total as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(frac).text(format!("{} / {}  at {speed}x", clock(pos), clock(total))));
            if reread {
                ui.label(RichText::new(format!("re-read: {now} accepted (recorded {then}), {changed} changed")).color(DIM));
            }
            for (ms, note) in flags {
                ui.label(RichText::new(format!("flag {}  {note}", clock(ms))).color(if ms <= pos { WARN } else { DIM }));
            }
            if ui.button("Stop replay").clicked() {
                rt.stop_replay();
            }
            return;
        }
        let list = sessions(&store.sessions());
        if list.is_empty() {
            ui.label(RichText::new("No recorded sessions yet.").color(DIM));
            return;
        }
        if self.session.as_ref().is_none_or(|p| !list.contains(p)) {
            self.session = list.first().cloned();
        }
        let name = |p: &PathBuf| p.file_name().map_or(String::new(), |n| n.to_string_lossy().into());
        egui::ComboBox::from_id_salt("session")
            .selected_text(self.session.as_ref().map_or(String::new(), name))
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                for p in &list {
                    ui.selectable_value(&mut self.session, Some(p.clone()), name(p));
                }
            });
        ui.horizontal(|ui| {
            ui.label(RichText::new("speed").color(DIM));
            for sp in [1.0, 2.0, 5.0, 10.0] {
                ui.selectable_value(&mut self.speed, sp, format!("{sp}x"));
            }
        });
        ui.checkbox(&mut self.reread, "re-read screen grabs with the current reader");
        let recording = rt.recording().is_some();
        if recording {
            ui.label(RichText::new("Stop recording to replay.").color(DIM));
        }
        if ui.add_enabled(!recording && self.session.is_some(), egui::Button::new("Replay")).clicked()
            && let Some(dir) = self.session.clone()
            && let Err(e) = rt.replay(&dir, self.speed, self.reread)
        {
            rt.pilot().say(format!("can't replay: {e}"));
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui, rt: &Runtime, store: &Store, prefs: &mut Prefs) {
        card(ui, "HUD", |ui| {
            if ui.checkbox(&mut prefs.debug, "Debug details").changed() {
                prefs.save(&store.prefs());
            }
            ui.label(
                RichText::new("Adds the game's own instruments, the telemetry link, where the position came from and the raw bearing. Off, the HUD shows only while there is a target or a ground warning.")
                    .color(DIM)
                    .size(12.0),
            );
        });
        ui.add_space(6.0);
        card(ui, "Inputs", |ui| {
            if self.demo {
                ui.label(RichText::new("Demo: a simulated flight on Lyria, no game.").color(WARN));
            } else {
                let mut on = rt.overlay_on();
                if ui.checkbox(&mut on, "Read r_DisplayInfo from the screen").changed() {
                    rt.set_overlay(on);
                }
                ui.label(RichText::new("About once a second. r_DisplayInfo 2 adds the view direction, which space steering needs.").color(DIM).size(12.0));
            }
            let (brake, reaction) = {
                let p = rt.pilot();
                (p.dir.brake(), p.dir.reaction)
            };
            ui.label(RichText::new(format!("Brake model {:.2} g ({}), {reaction:.0} s reaction", brake.0, brake.1)).color(DIM));
        });
        ui.add_space(6.0);
        card(ui, "Clock", |ui| {
            let st = sc_nav_clock::status();
            match (st.offset_ms, st.rtt_ms, &st.server, st.synced_at) {
                (Some(off), Some(rtt), Some(server), Some(at)) => {
                    let col = if off.abs() > 500.0 { WARN } else { DIM };
                    ui.label(
                        RichText::new(format!(
                            "PC clock {} by {:.0} ms (±{:.0}) · {server} · {} ago",
                            if off >= 0.0 { "behind" } else { "ahead" },
                            off.abs(),
                            rtt / 2.0,
                            fmt_time(at.elapsed().as_secs_f64())
                        ))
                        .color(col),
                    );
                }
                _ => {
                    let why = st.error.as_deref().map_or("syncing…".to_string(), |e| format!("not synced: {e}"));
                    ui.label(RichText::new(format!("Using the PC clock as is, {why}")).color(WARN));
                }
            }
            if ui.small_button("Sync now").clicked() {
                sc_nav_clock::sync_now();
            }
        });
        ui.add_space(6.0);
        card(ui, "Import marks", |ui| {
            ui.label(RichText::new("A marks file of the same shape, e.g. the lab's waypoints.json. Existing names are kept.").color(DIM).size(12.0));
            ui.add(egui::TextEdit::singleline(&mut self.import_path).desired_width(f32::INFINITY));
            if ui.button("Import").clicked() {
                self.import(rt);
            }
        });
        ui.add_space(6.0);
        card(ui, "Storage", |ui| {
            ui.label(RichText::new(store.root.display().to_string()).monospace().size(12.0));
            ui.label(RichText::new("cache (game data per build) · sessions · marks.json · phases.json").color(DIM).size(12.0));
            if ui.small_button("Open folder").clicked() {
                open(&store.root);
            }
        });
    }

    fn import(&self, rt: &Runtime) {
        let marks = match read_marks(Path::new(self.import_path.trim())) {
            Ok(m) => m,
            Err(e) => return rt.pilot().say(format!("can't import: {e}")),
        };
        let have: Vec<String> = rt.pilot().dir.waypoints().keys().cloned().collect();
        let (mut added, mut kept) = (0, 0);
        for (name, w) in marks {
            if have.contains(&name) {
                kept += 1;
                continue;
            }
            rt.command(Cmd::AddCode { code: w.code(&name), go: false });
            added += 1;
        }
        rt.pilot().say(format!("imported {added} marks ({kept} already there)"));
    }

    // --- status bar ---

    fn status(&self, ui: &mut egui::Ui, rt: &Runtime, s: &Snapshot) {
        ui.horizontal(|ui| {
            let stale = s.frame_ms.is_none_or(|f| s.ms - f > 2000);
            let (col, text) = match &s.link {
                Link::Live if !stale => (OK, "telemetry"),
                Link::Live | Link::Idle => (WARN, "telemetry idle"),
                Link::Connecting => (DIM, "telemetry …"),
                Link::Down(_) => (ALERT, "no telemetry"),
            };
            let r = ui.horizontal(|ui| dot(ui, col, text)).response;
            if let Link::Down(e) = &s.link {
                r.on_hover_text(format!("{e}\nStart the game with GRPC_CLIENT_SERVER_ENABLED=true"));
            }
            ui.separator();
            let (col, text) = match (rt.overlay_on() || self.demo, s.last_fix) {
                (false, _) => (DIM, "overlay off".to_string()),
                (true, Some((ms, src))) if s.ms - ms < 5000 => (OK, format!("fix {src}")),
                (true, _) => (WARN, "overlay: no fix".to_string()),
            };
            dot(ui, col, text);
            ui.separator();
            let st = sc_nav_clock::status();
            let (col, text) = match st.offset_ms {
                Some(off) => (OK, format!("clock {off:+.0} ms")),
                None => (WARN, "clock unsynced".to_string()),
            };
            dot(ui, col, text);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if rt.recording().is_some() {
                    dot(ui, ALERT, "REC");
                }
                let build = rt.pilot().dir.data.build.clone();
                ui.label(RichText::new(build).color(DIM).size(11.0));
            });
        });
    }
}

fn position(ui: &mut egui::Ui, _rt: &Runtime, s: &Snapshot) {
    card(ui, "Position", |ui| {
        match &s.here {
            Some(h) => {
                let col = if h.stale { WARN } else { INK };
                ui.label(RichText::new(format!("{} · {:.0} m above datum", pretty(&h.body_name), h.height)).color(col).strong());
                match (h.lat_lon, &h.scar) {
                    (Some((lat, lon)), Some(scar)) => {
                        ui.label(RichText::new(format!("{lat:.4}, {lon:.4}  ·  {scar}")).monospace());
                    }
                    _ => {
                        ui.label(RichText::new("Provisional: this body has no phase yet; one overlay fix fixes it.").color(WARN));
                    }
                }
                let age = s.last_fix.map_or("--".into(), |(ms, src)| format!("{} ago ({src})", fmt_time((s.ms - ms) as f64 / 1000.0)));
                ui.label(RichText::new(format!("fix {age}{}", if h.stale { " · stale, take a new fix" } else { "" })).color(DIM).size(12.0));
            }
            None if s.here_space.is_none() => {
                ui.label(RichText::new("No position yet: /showlocation, or the overlay reader with r_DisplayInfo 1 or 2.").color(WARN));
            }
            None => {}
        }
        if let Some((system, p)) = &s.here_space {
            let view = if s.view { "view known" } else { "no view (r_DisplayInfo 2)" };
            let guessed = if s.system_read { "" } else { " · system guessed" };
            ui.label(
                RichText::new(format!("{} · {:.1} {:.1} {:.1} km · {view}{guessed}", pretty(system), p[0] / 1e3, p[1] / 1e3, p[2] / 1e3))
                    .color(DIM)
                    .size(12.0),
            );
        }
    });
}

fn target(ui: &mut egui::Ui, rt: &Runtime, s: &Snapshot, replaying: bool) {
    card(ui, "Target", |ui| {
        ui.horizontal(|ui| {
            let (text, col) = match (&s.target, &s.space, &s.guidance) {
                (None, _, _) => ("none".to_string(), DIM),
                (Some(name), Some(g), _) if s.target_space => (
                    format!("{name} (space) · {} · closing {:.0} m/s · ETA {}", fmt_km(g.distance_m), g.closing, g.eta_s.map_or("--".into(), fmt_time)),
                    INK,
                ),
                (Some(name), _, _) if s.target_space => (format!("{name} (space) · waiting for a Root position"), WARN),
                (Some(name), _, Some(g)) => (
                    format!("{name} · {} · bearing {:03.0} · ETA {}", fmt_km(g.distance_m), g.bearing, g.eta_s.map_or("--".into(), fmt_time)),
                    INK,
                ),
                (Some(name), _, None) => match &s.target_elsewhere {
                    Some(b) => (format!("{name} · on {}, not here", pretty(b)), WARN),
                    None => (format!("{name} · waiting for a fix and telemetry"), WARN),
                },
            };
            ui.label(RichText::new(text).color(col));
            if s.target.is_some() && !replaying {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Clear").clicked() {
                        rt.command(Cmd::Clear);
                    }
                });
            }
        });
    });
}

fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

fn open(path: &Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}
