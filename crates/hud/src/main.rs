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
//!   `sc-nav --demo --backdrop shot.png`   # over a screenshot, to judge the look

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod control;
mod demo;
mod hud;
mod panel;
mod store;
mod theme;

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use eframe::egui::{self, RichText, ViewportBuilder, ViewportCommand, ViewportId};
use sc_nav_geo::World;
use sc_nav_pilot::runtime::{Config, Runtime};
use sc_nav_pilot::{Director, Pilot, load_phases, telemetry};

use store::{Prefs, Store};
use theme::{ACCENT, ALERT, DIM, INK};

enum Phase {
    Loading { rx: Receiver<Result<(World, Vec<u8>)>>, since: Instant },
    Running { rt: Runtime },
    Failed(String),
}

struct App {
    phase: Phase,
    /// What the control window works on, shared with its own render pass.
    shared: Arc<Mutex<Shared>>,
    control_pos: [f32; 2],
    demo: bool,
    layered: bool,
    /// A screenshot painted under the HUD (previews only).
    backdrop: Option<egui::TextureHandle>,
}

/// The control window is a deferred viewport: it repaints on its own input
/// and timer, not on every HUD frame (the HUD repaints on every telemetry
/// frame; rebuilding the control window with it took ~2 ms a frame).
struct Shared {
    view: View,
    store: Store,
    prefs: Prefs,
    control: control::Control,
}

/// What the control window shows.
enum View {
    Loading(Instant),
    Running(Runtime),
    Failed(String),
}

/// The control window's live values refresh this often without input.
const CONTROL_REFRESH: Duration = Duration::from_millis(250);

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// `--backdrop <image>`: a screenshot as the HUD's background.
fn backdrop(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let args: Vec<String> = std::env::args().collect();
    let path = args.iter().position(|a| a == "--backdrop").and_then(|i| args.get(i + 1))?;
    let img = image::open(path).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("backdrop", color, egui::TextureOptions::LINEAR))
}

fn main() -> Result<()> {
    let demo = std::env::args().any(|a| a == "--demo");
    let store = Store::locate();

    let (screen, control_pos) = placement().unwrap_or(([0.0, 0.0, 1920.0, 1080.0], [40.0, 160.0]));
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("sc-nav HUD")
            // one pixel short of the monitor: a borderless window that covers it
            // exactly counts as fullscreen, and the driver then skips the
            // desktop compositor, which turns the transparency black
            .with_inner_size([screen[2], screen[3] - 1.0])
            .with_position([screen[0], screen[1]])
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
                ctx.request_repaint_of(ViewportId::ROOT);
            });
            let since = Instant::now();
            Ok(Box::new(App {
                phase: Phase::Loading { rx, since },
                shared: Arc::new(Mutex::new(Shared {
                    view: View::Loading(since),
                    prefs: Prefs::load(&store.prefs()),
                    store,
                    control: control::Control::new(demo),
                })),
                control_pos,
                demo,
                layered: false,
                backdrop: backdrop(&cc.egui_ctx),
            }))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}

/// The primary monitor (`[x, y, w, h]`, which the HUD covers) and where the
/// control window goes, in points.
fn placement() -> Option<([f32; 4], [f32; 2])> {
    let monitors = xcap::Monitor::all().ok()?;
    let m = monitors.iter().find(|m| m.is_primary().unwrap_or(false)).or(monitors.first())?;
    // xcap reports physical pixels; egui positions in points
    let scale = m.scale_factor().unwrap_or(1.0);
    let (x, y) = (m.x().ok()? as f32 / scale, m.y().ok()? as f32 / scale);
    let (w, h) = (m.width().ok()? as f32 / scale, m.height().ok()? as f32 / scale);
    Some(([x, y, w, h], [x + 40.0, y + h * 0.15]))
}

impl App {
    /// Loading finished: build the pilot and start the feeds.
    fn start(&self, ctx: &egui::Context, world: World, font: Vec<u8>) -> Result<Runtime> {
        let store = lock(&self.shared).store.clone();
        let phases = load_phases(&store.phases());
        let world = world.with_phases(&phases);
        // the demo keeps its marks and phases in memory, away from the real ones
        let mut dir = Director::new(world.clone(), (!self.demo).then(|| store.marks()));
        dir.phases_path = (!self.demo).then(|| store.phases());
        let ms = sc_nav_clock::now_ms();
        let mut pilot = Pilot::new(dir, ms);
        pilot.say(format!("{} · {} bodies · {} ships", world.build, world.bodies.len(), world.ships.len()));
        let cfg = Config {
            addr: telemetry::DEFAULT_ADDR.to_string(),
            font,
            world,
            sessions: store.sessions(),
            feeds: !self.demo,
        };
        // a change repaints the HUD; the control window refreshes on its own
        let repaint = ctx.clone();
        let rt = Runtime::start(pilot, cfg, move || repaint.request_repaint_of(ViewportId::ROOT))?;
        if self.demo {
            demo::run(rt.clone());
        }
        Ok(rt)
    }

    /// Register the control window (each HUD pass; it renders on its own).
    fn control_window(&self, ctx: &egui::Context) {
        let builder = ViewportBuilder::default()
            .with_title(if self.demo { "sc-nav (demo)" } else { "sc-nav" })
            .with_inner_size([440.0, 760.0])
            .with_min_inner_size([380.0, 480.0])
            .with_position(self.control_pos)
            .with_transparent(false)
            .with_mouse_passthrough(false);
        let shared = self.shared.clone();
        ctx.show_viewport_deferred(ViewportId::from_hash_of("control"), builder, move |ui, _| {
            if ui.input(|i| i.viewport().close_requested()) {
                ui.ctx().send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
            let mut guard = lock(&shared);
            let Shared { view, store, prefs, control } = &mut *guard;
            match view {
                View::Running(rt) => {
                    let before = prefs.clone();
                    let snap = rt.pilot().snapshot();
                    control.ui(ui, rt, &snap, store, prefs);
                    if *prefs != before {
                        // a setting the HUD shows: repaint it now
                        ui.ctx().request_repaint_of(ViewportId::ROOT);
                    }
                    ui.ctx().request_repaint_after(CONTROL_REFRESH);
                }
                View::Loading(since) => {
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
                View::Failed(e) => {
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
            let next = match rx.try_recv() {
                Ok(Ok((world, font))) => Some(match self.start(&ctx, world, font) {
                    Ok(rt) => Phase::Running { rt },
                    Err(e) => Phase::Failed(format!("{e:#}")),
                }),
                Ok(Err(e)) => Some(Phase::Failed(format!("{e:#}"))),
                Err(TryRecvError::Disconnected) => Some(Phase::Failed("the loader stopped".into())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(next) = next {
                lock(&self.shared).view = match &next {
                    Phase::Running { rt } => View::Running(rt.clone()),
                    Phase::Failed(e) => View::Failed(e.clone()),
                    Phase::Loading { since, .. } => View::Loading(*since),
                };
                self.phase = next;
                ctx.request_repaint_of(ViewportId::from_hash_of("control"));
            }
        }
        if let Some(tex) = &self.backdrop {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(tex.id(), ctx.content_rect(), uv, egui::Color32::WHITE);
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
            hud::draw(ui, &snap, &lock(&self.shared).prefs);
        }
        self.control_window(&ctx);
    }
}
