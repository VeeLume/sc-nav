//! `sc-nav`: a click-through navigation HUD over Star Citizen and a control
//! window beside it.
//!
//! - **HUD**: a transparent, always-on-top strip below the crosshair:
//!   instruments, PULL UP, steering (tape on a body, view box in space), the
//!   braking cue. Borderless windowed only; exclusive fullscreen hides it.
//! - **Control window**: position and target, marks, lat/long and SCAR
//!   points, mark codes, recording and replay, settings. Closing it quits.
//!
//! The first start after a patch reads the game files (about two minutes);
//! later starts use the cache in `%LOCALAPPDATA%\sc-nav`.
//!
//!   `sc-nav`          # needs the game's telemetry: GRPC_CLIENT_SERVER_ENABLED=true
//!   `sc-nav --demo`   # a simulated flight on Lyria, no game

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod control;
mod demo;
mod hud;
mod store;
mod theme;

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use eframe::egui::{self, RichText, ViewportBuilder, ViewportCommand, ViewportId};
use sc_nav_geo::World;
use sc_nav_pilot::runtime::{Config, Runtime};
use sc_nav_pilot::{Director, Pilot, load_phases, telemetry};

use store::{Prefs, Store};
use theme::{ACCENT, ALERT, DIM, INK};

/// HUD position: fraction of the screen height from the top.
const HUD_Y: f32 = 0.66;

enum Phase {
    Loading { rx: Receiver<Result<(World, Vec<u8>)>>, since: Instant },
    Running { rt: Runtime },
    Failed(String),
}

struct App {
    phase: Phase,
    store: Store,
    prefs: Prefs,
    control: control::Control,
    control_pos: [f32; 2],
    demo: bool,
    layered: bool,
}

fn main() -> Result<()> {
    let demo = std::env::args().any(|a| a == "--demo");
    let store = Store::locate();

    let (hud_pos, control_pos) = placement().unwrap_or(([200.0, 600.0], [40.0, 160.0]));
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("sc-nav HUD")
            .with_inner_size([hud::W, hud::H])
            .with_position(hud_pos)
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_mouse_passthrough(true)
            .with_taskbar(false)
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "sc-nav",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let (tx, rx) = channel();
            let (ctx, cache) = (cc.egui_ctx.clone(), store.cache());
            std::thread::spawn(move || {
                let _ = tx.send(sc_nav_data::load(&cache));
                ctx.request_repaint();
            });
            Ok(Box::new(App {
                phase: Phase::Loading { rx, since: Instant::now() },
                prefs: Prefs::load(&store.prefs()),
                store,
                control: control::Control::new(demo),
                control_pos,
                demo,
                layered: false,
            }))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}

/// Where the HUD (centred, below the crosshair) and the control window go on
/// the primary monitor, in points.
fn placement() -> Option<([f32; 2], [f32; 2])> {
    let monitors = xcap::Monitor::all().ok()?;
    let m = monitors.iter().find(|m| m.is_primary().unwrap_or(false)).or(monitors.first())?;
    // xcap reports physical pixels; egui positions in points
    let scale = m.scale_factor().unwrap_or(1.0);
    let (x, y) = (m.x().ok()? as f32 / scale, m.y().ok()? as f32 / scale);
    let (w, h) = (m.width().ok()? as f32 / scale, m.height().ok()? as f32 / scale);
    Some(([x + (w - hud::W) / 2.0, y + h * HUD_Y], [x + 40.0, y + h * 0.15]))
}

impl App {
    /// Loading finished: build the pilot and start the feeds.
    fn start(&self, ctx: &egui::Context, world: World, font: Vec<u8>) -> Result<Runtime> {
        let phases = load_phases(&self.store.phases());
        let world = world.with_phases(&phases);
        // the demo keeps its marks and phases in memory, away from the real ones
        let mut dir = Director::new(world.clone(), (!self.demo).then(|| self.store.marks()));
        dir.phases_path = (!self.demo).then(|| self.store.phases());
        let ms = sc_nav_clock::now_ms();
        let mut pilot = Pilot::new(dir, ms);
        pilot.say(format!("{} · {} bodies · {} ships", world.build, world.bodies.len(), world.ships.len()));
        let cfg = Config {
            addr: telemetry::DEFAULT_ADDR.to_string(),
            font,
            world,
            sessions: self.store.sessions(),
            feeds: !self.demo,
        };
        let repaint = ctx.clone();
        let rt = Runtime::start(pilot, cfg, move || repaint.request_repaint())?;
        if self.demo {
            demo::run(rt.clone());
        }
        Ok(rt)
    }

    fn control_window(&mut self, ctx: &egui::Context) {
        let builder = ViewportBuilder::default()
            .with_title(if self.demo { "sc-nav (demo)" } else { "sc-nav" })
            .with_inner_size([440.0, 760.0])
            .with_min_inner_size([380.0, 480.0])
            .with_position(self.control_pos)
            .with_transparent(false)
            .with_mouse_passthrough(false);
        ctx.show_viewport_immediate(ViewportId::from_hash_of("control"), builder, |ui, _| {
            if ui.input(|i| i.viewport().close_requested()) {
                ui.ctx().send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
            match &self.phase {
                Phase::Running { rt } => {
                    let snap = rt.pilot().snapshot();
                    self.control.ui(ui, rt, &snap, &self.store, &mut self.prefs);
                }
                Phase::Loading { since, .. } => {
                    let secs = since.elapsed().as_secs();
                    egui::CentralPanel::default().show(ui, |ui| {
                        ui.add_space(ui.available_height() * 0.3);
                        ui.vertical_centered(|ui| {
                            ui.add(egui::Spinner::new().size(28.0).color(ACCENT));
                            ui.add_space(10.0);
                            ui.label(RichText::new("Reading the game data").color(INK).size(17.0).strong());
                            ui.label(RichText::new(format!("{secs} s")).color(DIM).monospace());
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("The first start after a patch reads planets, ships and the overlay font from the install, about two minutes. Later starts use the cache.")
                                    .color(DIM),
                            );
                        });
                    });
                    ui.ctx().request_repaint_after(Duration::from_millis(500));
                }
                Phase::Failed(e) => {
                    let e = e.clone();
                    egui::CentralPanel::default().show(ui, |ui| {
                        ui.add_space(40.0);
                        ui.label(RichText::new("Can't start").color(ALERT).size(17.0).strong());
                        ui.label(RichText::new(e).monospace());
                        ui.add_space(6.0);
                        ui.label(RichText::new("sc-nav reads the planets and the overlay font from the installed game; it needs the RSI Launcher's install of Star Citizen.").color(DIM));
                    });
                }
            }
        });
    }
}

impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if !self.layered {
            self.layered = true;
            hud::show_layered(frame);
        }
        let ctx = ui.ctx().clone();
        if let Phase::Loading { rx, .. } = &self.phase {
            match rx.try_recv() {
                Ok(Ok((world, font))) => {
                    self.phase = match self.start(&ctx, world, font) {
                        Ok(rt) => Phase::Running { rt },
                        Err(e) => Phase::Failed(format!("{e:#}")),
                    };
                }
                Ok(Err(e)) => self.phase = Phase::Failed(format!("{e:#}")),
                Err(TryRecvError::Disconnected) => self.phase = Phase::Failed("the loader stopped".into()),
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Phase::Running { rt } = &self.phase {
            // keep ages and the blinking cue ticking without telemetry
            ctx.request_repaint_after(Duration::from_millis(250));
            let snap = {
                let now = rt.now_ms();
                let mut p = rt.pilot();
                p.tick(now);
                p.snapshot()
            };
            hud::draw(ui, &snap, self.prefs.debug);
        }
        self.control_window(&ctx);
    }
}
