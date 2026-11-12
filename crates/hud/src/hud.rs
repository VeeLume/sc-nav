//! The HUD, drawn straight onto a transparent, click-through window over the
//! whole screen and anchored to the game's own HUD:
//!
//! - a marker under the game's compass at the target's bearing (an arrow at
//!   the tape's end with the turn when it's beyond), the target's name,
//!   distance and ETA under it, and the braking cue when it matters;
//! - in space, the target where it is on screen (an arrow at the edge when
//!   it isn't), from the overlay's view direction and the game's FOV;
//! - PULL UP below the crosshair.
//!
//! Positions are fractions of the screen height ([`Compass`]), measured on
//! the game's HUD; the seat and the visor put their compass in different
//! places. With debug on, the old strip ([`crate::panel`]) shows at the top left.

use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, vec2};
use sc_nav_geo::Cue;
use sc_nav_geo::ground::{GroundLevel, GroundWarning};
use sc_nav_geo::space::SpaceGuidance;
use sc_nav_pilot::Snapshot;

use crate::store::{Compass, Prefs};
use crate::theme::{ALERT, WARN};

/// Ours: one colour, apart from the ship HUD's amber and the visor's cyan.
pub const MARK: Color32 = Color32::from_rgb(110, 230, 160);
const SOFT: Color32 = Color32::from_rgb(215, 228, 220);
const DIMMED: Color32 = Color32::from_rgb(150, 165, 158);

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

struct Hud {
    p: Painter,
    r: Rect,
    /// Seconds, for blinking.
    t: f64,
}

impl Hud {
    fn h(&self) -> f32 {
        self.r.height()
    }

    fn at(&self, x: f32, y_frac: f32) -> Pos2 {
        Pos2::new(x, self.r.top() + y_frac * self.h())
    }

    fn blink(&self) -> bool {
        (self.t * 3.0) as i64 % 2 == 0
    }

    /// Text with a dark halo, readable over sky and ground alike.
    fn text(&self, pos: Pos2, anchor: Align2, s: &str, size_frac: f32, col: Color32) -> Rect {
        let font = FontId::new(size_frac * self.h(), crate::theme::hud_font());
        let halo = Color32::from_black_alpha(150);
        for d in [vec2(1.0, 1.0), vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(-1.0, -1.0)] {
            self.p.text(pos + d, anchor, s, font.clone(), halo);
        }
        self.p.text(pos, anchor, s, font, col)
    }

    fn polygon(&self, pts: Vec<Pos2>, col: Color32) {
        self.p.add(Shape::convex_polygon(pts, col, Stroke::new(1.5, Color32::from_black_alpha(140))));
    }
}

pub fn draw(ui: &mut egui::Ui, s: &Snapshot, prefs: &Prefs) {
    let ctx = ui.ctx().clone();
    let hud = Hud { p: ui.painter().clone(), r: ctx.content_rect(), t: ctx.input(|i| i.time) };
    let seated = s.instruments.is_some() && !s.on_foot;
    let compass = if seated { prefs.ship } else { prefs.foot };

    if prefs.preview {
        preview(&hud, &compass);
    } else if s.target.is_some() {
        target(&hud, s, &compass, prefs.fov_deg);
    }
    if let Some(w) = &s.ground {
        ground(&hud, w);
    }
    if prefs.debug {
        // top left, between the ship status and the compass: sky in a seat
        let pos = Pos2::new(hud.r.left() + 0.18 * hud.r.width(), hud.r.top() + 0.02 * hud.h());
        egui::Area::new(egui::Id::unique("debug-panel"))
            .fixed_pos(pos)
            .interactable(false)
            .show(&ctx, |ui| crate::panel::draw(ui, s));
    }
}

/// The rows under the compass: offsets from the tape, fractions of height.
const MARKER_DY: f32 = 0.026;
const ROW1_DY: f32 = 0.048;
const ROW2_DY: f32 = 0.068;
const ROW1_SIZE: f32 = 0.016;
const ROW2_SIZE: f32 = 0.014;

fn target(hud: &Hud, s: &Snapshot, c: &Compass, fov_deg: f32) {
    let name = s.target.as_deref().unwrap_or_default().to_uppercase();
    let cx = hud.r.center().x;
    let row1 = hud.at(cx, c.y + ROW1_DY);
    let row2 = hud.at(cx, c.y + ROW2_DY);
    if s.target_space {
        let Some(g) = &s.space else {
            hud.text(row1, Align2::CENTER_CENTER, &format!("{name}   NO POSITION"), ROW1_SIZE, WARN);
            return;
        };
        let col = if g.age_s > 5.0 { DIMMED } else { MARK };
        hud.text(row1, Align2::CENTER_CENTER, &line(&name, g.distance_m, g.eta_s), ROW1_SIZE, col);
        cue(hud, row2, g.cue, g.time_to_brake_s);
        on_screen(hud, g, &name, fov_deg);
        return;
    }
    let Some(g) = &s.guidance else {
        let why = if s.provisional {
            "CALIBRATING".to_string()
        } else if let Some(body) = &s.target_elsewhere {
            format!("ON {}", body.to_uppercase())
        } else if s.here.is_none() {
            "NO POSITION".to_string()
        } else {
            "WAITING".to_string()
        };
        hud.text(row1, Align2::CENTER_CENTER, &format!("{name}   {why}"), ROW1_SIZE, WARN);
        return;
    };
    let col = if g.stale { DIMMED } else { MARK };
    marker(hud, c, g.relative as f32, col);
    hud.text(row1, Align2::CENTER_CENTER, &line(&name, g.distance_m, g.eta_s), ROW1_SIZE, col);
    cue(hud, row2, g.cue, g.time_to_brake_s);
}

/// Under the tape at the target's bearing; beyond the tape, an arrow at its
/// end with the turn.
fn marker(hud: &Hud, c: &Compass, rel: f32, col: Color32) {
    let h = hud.h();
    let (cx, ty) = (hud.r.center().x, hud.r.top() + c.y * h);
    let ppd = c.per_deg * h;
    let size = 0.008 * h;
    if rel.abs() <= c.span {
        let (x, tip) = (cx + rel * ppd, ty + MARKER_DY * h - size);
        hud.polygon(
            vec![Pos2::new(x, tip), Pos2::new(x + size, tip + 1.4 * size), Pos2::new(x - size, tip + 1.4 * size)],
            col,
        );
        return;
    }
    let side = rel.signum();
    let x = cx + side * (c.span * ppd + 0.012 * h);
    let back = x - side * 1.4 * size;
    hud.polygon(vec![Pos2::new(x, ty), Pos2::new(back, ty + size), Pos2::new(back, ty - size)], col);
    let anchor = if side > 0.0 { Align2::LEFT_CENTER } else { Align2::RIGHT_CENTER };
    hud.text(Pos2::new(x + side * 0.008 * h, ty), anchor, &format!("{:.0}°", rel.abs()), ROW1_SIZE, col);
}

fn line(name: &str, distance_m: f64, eta_s: Option<f64>) -> String {
    let eta = eta_s.map_or(String::new(), |t| format!("   {}", clock(t)));
    format!("{name}   {}{eta}", distance(distance_m))
}

/// The braking cue, only while it matters.
fn cue(hud: &Hud, pos: Pos2, cue: Cue, time_to_brake_s: Option<f64>) {
    let (text, col) = match (cue, time_to_brake_s) {
        (Cue::Cruise, Some(t)) if t < 90.0 => (format!("BRAKE IN {}", clock(t)), SOFT),
        (Cue::Prepare, t) => (format!("BRAKE IN {}", t.map_or("0:00".into(), clock)), WARN),
        (Cue::Brake, _) => ("BRAKE NOW".into(), if hud.blink() { ALERT } else { WARN }),
        (Cue::Arrived, _) => ("ARRIVED".into(), MARK),
        _ => return,
    };
    hud.text(pos, Align2::CENTER_CENTER, &text, ROW2_SIZE, col);
}

/// A space target where it is on screen, from the view angles: a diamond
/// with its distance, or an arrow at the screen's edge with the turn. The
/// ring is where the ship is moving.
fn on_screen(hud: &Hud, g: &SpaceGuidance, name: &str, fov_deg: f32) {
    let Some((right, up)) = g.target_view else { return };
    let h = hud.h();
    let f = (h / 2.0) / (fov_deg.to_radians() / 2.0).tan();
    let c = hud.r.center();
    let project = |(r, u): (f64, f64)| {
        let (r, u) = (r.to_radians(), u.to_radians());
        let (dr, du, df) = (u.cos() * r.sin(), u.sin(), u.cos() * r.cos());
        (df > 0.05).then(|| Pos2::new(c.x + (dr / df) as f32 * f, c.y - (du / df) as f32 * f))
    };
    let inner = hud.r.shrink(0.05 * h);
    let size = 0.010 * h;
    if let Some(p) = project((right, up)).filter(|p| inner.contains(*p)) {
        let d = |x: f32, y: f32| Pos2::new(p.x + x, p.y + y);
        hud.p.add(Shape::closed_line(
            vec![d(0.0, -size), d(size, 0.0), d(0.0, size), d(-size, 0.0)],
            Stroke::new(2.5, MARK),
        ));
        hud.text(d(0.0, size + 0.012 * h), Align2::CENTER_CENTER, &distance(g.distance_m), ROW2_SIZE, MARK);
    } else {
        // the direction on screen, pushed to the inner edge
        let (r, u) = (right.to_radians() as f32, up.to_radians() as f32);
        let raw = vec2(r.sin(), -u.sin());
        let dir = if raw.length() > 1e-4 { raw.normalized() } else { vec2(1.0, 0.0) };
        let k = ((inner.width() / 2.0) / dir.x.abs()).min((inner.height() / 2.0) / dir.y.abs());
        let tip = c + dir * k;
        let back = tip - dir * 1.4 * size;
        let side = vec2(-dir.y, dir.x) * size;
        hud.polygon(vec![tip, back + side, back - side], MARK);
        let off = (right.to_radians().cos() * up.to_radians().cos()).clamp(-1.0, 1.0).acos().to_degrees();
        hud.text(back - dir * 0.03 * h, Align2::CENTER_CENTER, &format!("{name}  {off:.0}°"), ROW2_SIZE, MARK);
    }
    if let Some(p) = g.travel_view.and_then(project).filter(|p| inner.contains(*p)) {
        hud.p.circle_stroke(p, 0.006 * h, Stroke::new(2.0, MARK.gamma_multiply(0.8)));
    }
}

fn ground(hud: &Hud, w: &GroundWarning) {
    let c = hud.at(hud.r.center().x, 0.62);
    match w.level {
        GroundLevel::PullUp => {
            let col = if hud.blink() { ALERT } else { WARN };
            let r = hud.text(c, Align2::CENTER_CENTER, "PULL UP", 0.048, col);
            let below = Pos2::new(c.x, r.bottom() + 0.016 * hud.h());
            hud.text(below, Align2::CENTER_CENTER, &format!("IMPACT {}", clock(w.time_to_impact_s)), 0.018, col);
        }
        GroundLevel::Caution => {
            let r = hud.text(c, Align2::CENTER_CENTER, &format!("SINK {:.0} M/S", w.sink), 0.024, WARN);
            let below = Pos2::new(c.x, r.bottom() + 0.014 * hud.h());
            hud.text(below, Align2::CENTER_CENTER, &format!("IMPACT {}", clock(w.time_to_impact_s)), 0.016, WARN);
        }
    }
}

/// While adjusting: a marker straight ahead and two at ±20°, ticks where the
/// tape should end, and sample rows.
fn preview(hud: &Hud, c: &Compass) {
    let h = hud.h();
    let (cx, ty) = (hud.r.center().x, hud.r.top() + c.y * h);
    for side in [-1.0, 1.0] {
        let x = cx + side * c.span * c.per_deg * h;
        hud.p.line_segment([Pos2::new(x, ty - 0.012 * h), Pos2::new(x, ty + 0.012 * h)], Stroke::new(2.0, WARN));
    }
    for deg in [-20.0, 20.0] {
        marker(hud, c, deg, MARK.gamma_multiply(0.5));
    }
    marker(hud, c, 0.0, MARK);
    hud.text(hud.at(cx, c.y + ROW1_DY), Align2::CENTER_CENTER, &line("PREVIEW", 1200.0, Some(42.0)), ROW1_SIZE, MARK);
    hud.text(hud.at(cx, c.y + ROW2_DY), Align2::CENTER_CENTER, "BRAKE IN 0:30", ROW2_SIZE, SOFT);
}

fn distance(m: f64) -> String {
    if m >= 10_000.0 {
        format!("{:.1} KM", m / 1000.0)
    } else if m >= 1000.0 {
        format!("{:.2} KM", m / 1000.0)
    } else {
        format!("{m:.0} M")
    }
}

/// `m:ss`, or `h:mm:ss` from an hour.
fn clock(s: f64) -> String {
    let s = s.max(0.0).round() as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
