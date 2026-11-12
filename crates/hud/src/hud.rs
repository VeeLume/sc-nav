//! The HUD strip: instruments, the ground warning, steering (a tape on a
//! body, a view box in space) with the braking cue, and the fix line. Drawn
//! from a [`Snapshot`] only.

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, RichText, Stroke};
use sc_nav_geo::ground::GroundLevel;
use sc_nav_geo::{Cue, Guidance, angle_diff};
use sc_nav_pilot::telemetry::Instruments;
use sc_nav_pilot::{Link, Snapshot, pretty};

use crate::theme::{ACCENT, ALERT, DIM, INK, OK, WARN, fmt_km, fmt_time, glass, short};

pub const W: f32 = 580.0;
pub const H: f32 = 236.0;

/// Click-through makes the window layered (`WS_EX_LAYERED`), and Windows
/// shows nothing of a layered window until its attributes are set: full
/// opacity here, the GL alpha still does the transparency.
pub fn show_layered(frame: &eframe::Frame) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::UI::WindowsAndMessaging::{LWA_ALPHA, SetLayeredWindowAttributes};
        if let Ok(h) = frame.window_handle()
            && let RawWindowHandle::Win32(w) = h.as_raw()
        {
            unsafe { SetLayeredWindowAttributes(w.hwnd.get() as _, 0, 255, LWA_ALPHA) };
        }
    }
    #[cfg(not(windows))]
    let _ = frame;
}

/// `debug` adds what the game already shows or what only helps finding a
/// fault: the instruments, the link, the fix source, the raw bearing.
pub fn draw(ui: &mut egui::Ui, s: &Snapshot, debug: bool) {
    let stale = s.frame_ms.is_none_or(|f| s.ms - f > 2000);
    // without debug the strip shows only while there is something to act on
    if !debug && s.target.is_none() && s.ground.is_none() {
        return;
    }
    egui::Frame::new()
        .fill(glass())
        .stroke(Stroke::new(1.0, ACCENT.gamma_multiply(0.25)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_min_width(W - 26.0);
            if debug {
                ui.set_min_height(H - 26.0);
                header(ui, s, stale);
                ui.add_space(2.0);
                match &s.instruments {
                    Some(i) => instruments(ui, i, stale),
                    None => {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), egui::Sense::hover());
                        let text = if s.on_foot { "on foot" } else { "no vehicle telemetry" };
                        ui.painter().text(r.left_center(), Align2::LEFT_CENTER, text, FontId::proportional(15.0), DIM);
                    }
                }
            }
            ground(ui, s, debug);
            steering(ui, s, debug);
            if debug {
                ui.add_space(2.0);
                fix_line(ui, s);
            }
        });
}

fn header(ui: &mut egui::Ui, s: &Snapshot, stale: bool) {
    ui.horizontal(|ui| {
        let (txt, col) = match &s.link {
            Link::Connecting => ("connecting".to_string(), DIM),
            Link::Live if stale => ("telemetry stale".to_string(), WARN),
            Link::Live => ("live".to_string(), OK),
            Link::Idle => ("telemetry idle".to_string(), WARN),
            Link::Down(e) => (format!("telemetry down · {}", short(e, 40)), WARN),
        };
        crate::theme::dot(ui, col, txt);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(hp) = s.health {
                ui.label(RichText::new(format!("HP {hp:.0}")).color(if hp < 50.0 { WARN } else { DIM }).size(12.0));
            }
            if let Some(name) = &s.ship {
                ui.label(RichText::new(short(name, 34)).color(DIM).size(12.0));
            }
        });
    });
}

fn instruments(ui: &mut egui::Ui, i: &Instruments, stale: bool) {
    let ink = if stale { DIM } else { INK };
    ui.columns(4, |c| {
        big(&mut c[0], "SPEED", format!("{:.0}", i.speed), "m/s", ink);
        big(&mut c[1], "ALTITUDE", i.altitude.map_or("—".into(), fmt_alt), "m", ink);
        big(&mut c[2], "HEADING", i.heading.map_or("—".into(), |h| format!("{h:03.0}")), "°", ink);
        big(&mut c[3], "VERT", format!("{:+.0}", i.vertical_speed), "m/s", ink);
    });
    ui.horizontal(|ui| {
        small(ui, "AGL", i.radar_altitude.filter(|r| *r < 500.0).map_or("—".into(), |r| format!("{r:.0} m")));
        small(ui, "H₂", format!("{:.0}%", i.hydrogen_fuel * 100.0));
        let boost = if i.boost.engaged { WARN } else { DIM };
        ui.label(RichText::new(format!("BOOST {:.0}%", i.boost.reserves * 100.0)).color(boost).size(12.0));
        if let Some(g) = i.g_meter {
            small(ui, "G", format!("{:.1} / {:.1}", g.current, g.max));
        }
    });
}

/// The steering line: a tape on a body, a view box in space.
fn steering(ui: &mut egui::Ui, s: &Snapshot, debug: bool) {
    if s.target_space {
        return space(ui, s, debug);
    }
    let Some(g) = &s.guidance else {
        let msg = match (&s.target, &s.target_elsewhere) {
            _ if s.provisional => "position provisional: one overlay fix calibrates this body".to_string(),
            (Some(name), Some(body)) => format!("{name} is on {body}, not here"),
            (Some(name), None) if s.here.is_none() => format!("{name} · waiting for a position fix"),
            (Some(name), None) => format!("{name} · waiting for telemetry"),
            (None, _) if debug => "no target · mark or pick one in the control window".into(),
            (None, _) => return,
        };
        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 58.0), egui::Sense::hover());
        ui.painter().text(r.center(), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), DIM);
        return;
    };
    tape(ui, g, s.track, debug);
    ui.horizontal(|ui| {
        let eta = g.eta_s.map_or("--".into(), fmt_time);
        let extra = if debug {
            let xtk = g.cross_track_m.map_or(String::new(), |x| format!(" · xtk {x:+.0} m"));
            format!(" · gs {:.0}{xtk}", g.ground_speed)
        } else {
            String::new()
        };
        ui.label(
            RichText::new(format!("{} · {} · ETA {eta}{extra}", s.target.as_deref().unwrap_or("?"), fmt_km(g.distance_m)))
                .color(INK)
                .size(13.0),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| cue_box(ui, g.cue, g.time_to_brake_s, g.stale));
    });
}

/// Nose at the centre, the target bearing as a triangle (pinned at the edge
/// beyond ±60°), the direction of travel as a dot.
fn tape(ui: &mut egui::Ui, g: &Guidance, track: Option<f64>, debug: bool) {
    const SPAN: f32 = 60.0;
    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::hover());
    let p = ui.painter();
    let (cx, y) = (r.center().x, r.top() + 12.0);
    let half = r.width() / 2.0 - 100.0;
    let x_of = |deg: f32| cx + deg.clamp(-SPAN, SPAN) / SPAN * half;
    let tick = Stroke::new(1.0, ACCENT.gamma_multiply(0.45));
    p.line_segment([Pos2::new(cx - half, y), Pos2::new(cx + half, y)], tick);
    for d in [-60.0, -45.0, -30.0, -15.0, 15.0, 30.0, 45.0, 60.0] {
        let len = if d as i32 % 30 == 0 { 6.0 } else { 3.0 };
        p.line_segment([Pos2::new(x_of(d), y - len), Pos2::new(x_of(d), y + len)], tick);
    }
    p.line_segment([Pos2::new(cx, y - 10.0), Pos2::new(cx, y + 10.0)], Stroke::new(2.0, INK));
    if let Some(t) = track {
        let rel_track = angle_diff(t, g.bearing - g.relative) as f32;
        p.circle_filled(Pos2::new(x_of(rel_track), y), 3.5, ACCENT);
    }
    let rel = g.relative as f32;
    let tx = x_of(rel);
    let col = if rel.abs() < 2.0 {
        OK
    } else if rel.abs() > SPAN {
        WARN
    } else {
        INK
    };
    p.add(egui::Shape::convex_polygon(
        vec![Pos2::new(tx, y + 2.0), Pos2::new(tx - 7.0, y + 14.0), Pos2::new(tx + 7.0, y + 14.0)],
        col,
        Stroke::NONE,
    ));
    let (turn, size) = if rel.abs() < 1.0 {
        ("ON COURSE".to_string(), 13.0)
    } else {
        (format!("{:.0}° {}", rel.abs(), if rel > 0.0 { "R" } else { "L" }), 18.0)
    };
    let (anchor, x) = if rel >= 0.0 { (Align2::LEFT_CENTER, cx + half + 10.0) } else { (Align2::RIGHT_CENTER, cx - half - 10.0) };
    p.text(Pos2::new(x, y), anchor, turn, FontId::monospace(size), col);
    if debug {
        p.text(Pos2::new(cx - half, r.bottom()), Align2::LEFT_BOTTOM, format!("BRG {:03.0}", g.bearing), FontId::monospace(11.0), DIM);
    }
}

fn cue_box(ui: &mut egui::Ui, cue: Cue, time_to_brake_s: Option<f64>, stale: bool) {
    let blink = (ui.input(|i| i.time) * 3.0) as i64 % 2 == 0;
    let (text, col) = match cue {
        Cue::Cruise => (format!("brake in {}", time_to_brake_s.map_or("--".into(), fmt_time)), DIM),
        Cue::Prepare => (format!("PREPARE {}", time_to_brake_s.map_or("--".into(), fmt_time)), WARN),
        Cue::Brake => ("BRAKE NOW".to_string(), if blink { ALERT } else { WARN }),
        Cue::Arrived => ("ARRIVED".to_string(), OK),
        Cue::NotClosing => ("not closing".to_string(), WARN),
    };
    let text = if stale { format!("{text} · stale fix") } else { text };
    egui::Frame::new()
        .fill(col.gamma_multiply(0.12))
        .stroke(Stroke::new(1.5, col))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text).color(col).strong().size(14.0)));
}

fn fix_line(ui: &mut egui::Ui, s: &Snapshot) {
    let text = match (&s.last_fix, &s.here) {
        (Some((ms, source)), Some(h)) => {
            let age = (s.ms - ms) as f64 / 1000.0;
            let place = h.scar.clone().unwrap_or_else(|| format!("{:.0} m up", h.height));
            let col = if h.stale {
                WARN
            } else if age < 3.0 {
                OK
            } else {
                DIM
            };
            RichText::new(format!("{} · {place} · fix {} ago ({source})", pretty(&h.body_name), fmt_time(age))).color(col)
        }
        (Some(_), None) => match &s.here_space {
            Some((system, p)) => RichText::new(format!("{system} · {:.0} {:.0} {:.0} km", p[0] / 1e3, p[1] / 1e3, p[2] / 1e3)).color(DIM),
            None => RichText::new("fix received, no body").color(DIM),
        },
        _ => RichText::new("no position yet · /showlocation, or the overlay reader with r_DisplayInfo").color(DIM),
    };
    ui.label(text.size(12.0));
}

fn big(ui: &mut egui::Ui, label: &str, value: String, unit: &str, ink: Color32) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).color(DIM).size(10.5).strong());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(RichText::new(value).font(FontId::monospace(26.0)).color(ink));
            ui.label(RichText::new(unit).color(DIM).size(11.0));
        });
    });
}

fn small(ui: &mut egui::Ui, label: &str, value: String) {
    ui.label(RichText::new(format!("{label} {value}")).color(DIM).size(12.0));
}

fn fmt_alt(m: f32) -> String {
    if m.abs() >= 10_000.0 { format!("{:.1}k", m / 1000.0) } else { format!("{m:.0}") }
}

/// Steering to a point in space: a view box (crosshair = where you look,
/// dot = the target, ring = where you're moving), the turn in words, the
/// line with distance and closing speed, the cue.
fn space(ui: &mut egui::Ui, s: &Snapshot, debug: bool) {
    let name = s.target.as_deref().unwrap_or("?");
    let Some(g) = &s.space else {
        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 58.0), egui::Sense::hover());
        let msg = format!("{name} (space) · waiting for a Root position (overlay or /showlocation)");
        ui.painter().text(r.center(), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), DIM);
        return;
    };
    ui.horizontal(|ui| {
        view_box(ui, g.target_view, g.travel_view);
        ui.vertical(|ui| {
            let (text, col) = match g.target_view {
                Some((r, u)) if r.abs() < 2.0 && u.abs() < 2.0 => ("ON TARGET".to_string(), OK),
                Some((r, u)) => {
                    let lr = if r.abs() < 1.0 { String::new() } else { format!("{:.0}° {}", r.abs(), if r > 0.0 { "R" } else { "L" }) };
                    let ud = if u.abs() < 1.0 { String::new() } else { format!("{:.0}° {}", u.abs(), if u > 0.0 { "UP" } else { "DOWN" }) };
                    let behind = if r.abs() > 90.0 { "  (behind)" } else { "" };
                    (format!("{lr}  {ud}{behind}").trim().to_string(), if r.abs() > 60.0 || u.abs() > 60.0 { WARN } else { INK })
                }
                None => ("no view · needs r_DisplayInfo 2".to_string(), DIM),
            };
            ui.label(RichText::new(text).font(FontId::monospace(20.0)).color(col));
            let track = match g.track_error {
                Some(e) if debug => format!(" · travel {e:.0}° off"),
                _ => String::new(),
            };
            let eta = g.eta_s.map_or("--".into(), fmt_time);
            ui.label(RichText::new(format!("{name} · {} · ETA {eta} · closing {:.0} m/s{track}", fmt_km(g.distance_m), g.closing)).color(INK).size(13.0));
            ui.horizontal(|ui| {
                cue_box(ui, g.cue, g.time_to_brake_s, g.age_s > 5.0);
                if debug {
                    ui.label(RichText::new(format!("speed {:.0} m/s", g.speed)).color(DIM).size(12.0));
                }
            });
        });
    });
}

/// ±45° of view in a 64 px box; beyond that the dot sits on the edge.
fn view_box(ui: &mut egui::Ui, target: Option<(f64, f64)>, travel: Option<(f64, f64)>) {
    const SPAN: f32 = 45.0;
    let (r, _) = ui.allocate_exact_size(egui::vec2(64.0, 64.0), egui::Sense::hover());
    let p = ui.painter();
    let c = r.center();
    let half = r.width() / 2.0 - 5.0;
    p.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, ACCENT.gamma_multiply(0.45)), egui::StrokeKind::Inside);
    p.line_segment([Pos2::new(c.x - 6.0, c.y), Pos2::new(c.x + 6.0, c.y)], Stroke::new(1.5, INK));
    p.line_segment([Pos2::new(c.x, c.y - 6.0), Pos2::new(c.x, c.y + 6.0)], Stroke::new(1.5, INK));
    let at = |(rt, up): (f64, f64)| {
        let (x, y) = (rt as f32 / SPAN, -(up as f32) / SPAN);
        let m = x.abs().max(y.abs()).max(1.0);
        (Pos2::new(c.x + x / m * half, c.y + y / m * half), m > 1.0)
    };
    if let Some(t) = travel {
        p.circle_stroke(at(t).0, 4.0, Stroke::new(1.5, ACCENT));
    }
    if let Some(t) = target {
        let (pos, edge) = at(t);
        p.circle_filled(pos, if edge { 3.5 } else { 4.5 }, if edge { WARN } else { OK });
    }
}

/// The ground warning, above everything else that steers: it is the one
/// thing that kills. Takes its space always, so the strip doesn't jump.
fn ground(ui: &mut egui::Ui, s: &Snapshot, debug: bool) {
    let Some(w) = &s.ground else {
        if debug {
            ui.add_space(24.0);
        }
        return;
    };
    let blink = (ui.input(|i| i.time) * 4.0) as i64 % 2 == 0;
    let (text, col) = match w.level {
        GroundLevel::PullUp => (
            format!("PULL UP · impact {} · sink {:.0} m/s · ground {}", fmt_time(w.time_to_impact_s), w.sink, fmt_km(w.agl)),
            if blink { ALERT } else { WARN },
        ),
        GroundLevel::Caution => (
            format!("SINK {:.0} m/s · impact {} · ground {} · stop needs {}", w.sink, fmt_time(w.time_to_impact_s), fmt_km(w.agl), fmt_km(w.stop_m)),
            WARN,
        ),
    };
    egui::Frame::new()
        .fill(col.gamma_multiply(0.18))
        .stroke(Stroke::new(1.5, col))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(text).color(col).strong().size(15.0));
        });
}
