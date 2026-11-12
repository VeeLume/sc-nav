//! The HUD, drawn straight onto a transparent, click-through window over the
//! whole screen and anchored to the game's own HUD. The look follows
//! Squadron 42's: thin glowing lines, markers without boxes, readouts as a
//! large number over a small spaced label, and glass cards with an accent
//! bar only for cues and alerts.
//!
//! - a chevron under the game's compass at the target's bearing (sideways
//!   at the tape's end, with the turn, when it's beyond); below it the
//!   target's name, distance, ETA, and the braking cue when it matters;
//! - in space, the target where it is on screen (a chevron at the edge
//!   when it isn't), from the overlay's view direction and the game's FOV;
//! - PULL UP below the crosshair.
//!
//! Text sits on whole pixels, so a changing number never blurs. Sizes scale
//! with the screen height (`u` is one point at 1080p). With debug on,
//! [`crate::panel`] shows at the top left.

use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Color32, CornerRadius, FontId, Galley, Painter, Pos2, Rect, Shape, Stroke, Vec2, vec2};
use sc_nav_geo::Cue;
use sc_nav_geo::ground::{GroundLevel, GroundWarning};
use sc_nav_geo::space::SpaceGuidance;
use sc_nav_pilot::Snapshot;

use crate::store::{Compass, Prefs};

/// Ours: one accent, apart from the ship HUD's amber and the visor's cyan.
pub const MARK: Color32 = Color32::from_rgb(120, 232, 172);
const TEXT: Color32 = Color32::from_rgb(236, 244, 248);
const MUTED: Color32 = Color32::from_rgb(150, 176, 190);
const AMBER: Color32 = Color32::from_rgb(246, 186, 76);
const RED: Color32 = Color32::from_rgb(232, 64, 56);

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

/// One piece of a card's row: text in a slot at least `slot` wide, right
/// aligned in it (numbers), or left (labels).
struct Seg {
    g: Arc<Galley>,
    slot: f32,
    right: bool,
}

struct Hud {
    p: Painter,
    r: Rect,
    /// Seconds, for pulsing.
    t: f64,
    /// One point at 1080p.
    u: f32,
    ppp: f32,
}

impl Hud {
    fn snap(&self, v: f32) -> f32 {
        (v * self.ppp).round() / self.ppp
    }

    fn snap_pos(&self, p: Pos2) -> Pos2 {
        Pos2::new(self.snap(p.x), self.snap(p.y))
    }

    /// 0..1, about once a second: for the alerts.
    fn pulse(&self) -> f32 {
        (0.5 + 0.5 * (self.t * std::f64::consts::TAU * 1.2).sin()) as f32
    }

    fn text(&self, s: &str, size: f32, color: Color32, spacing: f32) -> Arc<Galley> {
        let mut job = LayoutJob::default();
        job.append(
            s,
            0.0,
            TextFormat {
                font_id: FontId::new((size * self.u).round(), crate::theme::hud_font()),
                color,
                extra_letter_spacing: spacing * self.u,
                ..Default::default()
            },
        );
        self.p.layout_job(job)
    }

    /// Small spaced capitals: names and labels.
    fn label(&self, s: &str, size: f32, color: Color32) -> Arc<Galley> {
        self.text(&s.to_uppercase(), size, color, 0.16 * size)
    }

    /// A galley on whole pixels, with a faint drop shadow so it holds over
    /// bright sky.
    fn put(&self, top_left: Pos2, g: &Arc<Galley>) {
        let at = self.snap_pos(top_left);
        let shadow = Color32::from_black_alpha(120);
        self.p.galley_with_override_text_color(at + vec2(0.0, 1.0 / self.ppp), g.clone(), shadow);
        self.p.galley(at, g.clone(), TEXT);
    }

    /// Centred on `cx`.
    fn put_centred(&self, cx: f32, top: f32, g: &Arc<Galley>) {
        self.put(Pos2::new(cx - g.size().x / 2.0, top), g);
    }

    /// A line with a soft glow of its own colour.
    fn glow(&self, pts: &[Pos2], width: f32, color: Color32, closed: bool) {
        let make = |w: f32, c: Color32| {
            let stroke = Stroke::new(w, c);
            if closed { Shape::closed_line(pts.to_vec(), stroke) } else { Shape::line(pts.to_vec(), stroke) }
        };
        self.p.add(make(width + 7.0 * self.u, color.gamma_multiply(0.06)));
        self.p.add(make(width + 3.5 * self.u, color.gamma_multiply(0.16)));
        self.p.add(make(width, color));
    }

    /// A hairline from `y0` to `y1` at `x`.
    fn divider(&self, x: f32, y0: f32, y1: f32) {
        let x = self.snap(x) + 0.5 / self.ppp;
        self.p.line_segment([Pos2::new(x, y0), Pos2::new(x, y1)], Stroke::new(1.0 / self.ppp, Color32::from_white_alpha(70)));
    }

    fn ring(&self, c: Pos2, radius: f32, width: f32, color: Color32) {
        self.p.circle_stroke(c, radius, Stroke::new(width + 5.0 * self.u, color.gamma_multiply(0.1)));
        self.p.circle_stroke(c, radius, Stroke::new(width, color));
    }

    /// A chevron pointing along `dir` with its tip at `tip`.
    fn chevron(&self, tip: Pos2, dir: Vec2, size: f32, color: Color32) {
        let back = tip - dir * size * 0.8;
        let side = vec2(-dir.y, dir.x) * size;
        self.glow(&[back + side, tip, back - side], 2.0 * self.u, color, false);
    }

    /// A glass card: dark translucent fill, a hairline edge, an accent bar
    /// on the left, then a row of segments. Centred on `cx`, top at `top`.
    fn card(&self, cx: f32, top: f32, segs: &[Seg], bar: Color32, fill: Color32) -> Rect {
        let u = self.u;
        let (pad_l, pad_r, pad_y, gap) = (14.0 * u, 12.0 * u, 6.0 * u, 10.0 * u);
        let row_h = segs.iter().map(|s| s.g.size().y).fold(0.0, f32::max);
        let w = pad_l + segs.iter().map(|s| s.slot).sum::<f32>() + gap * segs.len().saturating_sub(1) as f32 + pad_r;
        let rect = Rect::from_min_size(
            self.snap_pos(Pos2::new(cx - w / 2.0, top)),
            vec2(self.snap(w), self.snap(row_h + 2.0 * pad_y)),
        );
        let radius = CornerRadius::same((3.0 * u) as u8);
        self.p.rect_filled(rect, radius, fill);
        self.p.rect_stroke(rect, radius, Stroke::new(1.0 / self.ppp, Color32::from_rgba_unmultiplied(150, 205, 230, 60)), egui::StrokeKind::Inside);
        let bar_rect = Rect::from_min_size(rect.min + vec2(4.0 * u, 4.0 * u), vec2(self.snap(3.0 * u), rect.height() - 8.0 * u));
        self.p.rect_filled(bar_rect, 0.0, bar);
        let mut x = rect.left() + pad_l;
        for s in segs {
            let gx = if s.right { x + s.slot - s.g.size().x } else { x };
            let gy = rect.top() + pad_y + (row_h - s.g.size().y) / 2.0;
            self.p.galley(self.snap_pos(Pos2::new(gx, gy)), s.g.clone(), TEXT);
            x += s.slot + gap;
        }
        rect
    }

    fn seg(g: Arc<Galley>) -> Seg {
        Seg { slot: g.size().x, g, right: false }
    }

    /// A number in a slot as wide as `template` at the same size.
    fn num(&self, g: Arc<Galley>, template: &str, size: f32) -> Seg {
        let slot = self.text(template, size, TEXT, 0.0).size().x.max(g.size().x);
        Seg { g, slot, right: true }
    }
}

/// The glass of a plain card.
fn glass() -> Color32 {
    Color32::from_rgba_unmultiplied(10, 28, 38, 165)
}

pub fn draw(ui: &mut egui::Ui, s: &Snapshot, prefs: &Prefs) {
    let ctx = ui.ctx().clone();
    let r = ctx.content_rect();
    let hud = Hud { p: ui.painter().clone(), r, t: ctx.input(|i| i.time), u: r.height() / 1080.0, ppp: ctx.pixels_per_point() };
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
        let pos = Pos2::new(r.left() + 0.18 * r.width(), r.top() + 0.02 * r.height());
        egui::Area::new(egui::Id::unique("debug-panel"))
            .fixed_pos(pos)
            .interactable(false)
            .show(&ctx, |ui| crate::panel::draw(ui, s));
    }
}

/// Below the tape, in points at 1080p: the chevron's tip (clear of the
/// game's heading number), then the block with name and readouts.
const MARKER_DY: f32 = 22.0;
const BLOCK_DY: f32 = 36.0;

fn target(hud: &Hud, s: &Snapshot, c: &Compass, fov_deg: f32) {
    let name = s.target.as_deref().unwrap_or_default();
    let cx = hud.r.center().x;
    let top = hud.r.top() + c.y * hud.r.height() + BLOCK_DY * hud.u;
    if s.target_space {
        let Some(g) = &s.space else {
            status(hud, cx, top, name, "no position");
            return;
        };
        let brake = brake(hud, g.cue, g.time_to_brake_s);
        info(hud, cx, top, name, g.distance_m, g.eta_s, brake, g.age_s > 5.0);
        on_screen(hud, g, name, fov_deg);
        return;
    }
    let Some(g) = &s.guidance else {
        let why = if s.provisional {
            "calibrating".to_string()
        } else if let Some(body) = &s.target_elsewhere {
            format!("on {body}")
        } else if s.here.is_none() {
            "no position".to_string()
        } else {
            "waiting".to_string()
        };
        status(hud, cx, top, name, &why);
        return;
    };
    marker(hud, c, g.relative as f32, if g.stale { MUTED } else { MARK });
    let brake = brake(hud, g.cue, g.time_to_brake_s);
    info(hud, cx, top, name, g.distance_m, g.eta_s, brake, g.stale);
}

/// A readout's value, its colour and its label.
type Readout = (String, Color32, &'static str);

/// The target's name behind an accent bar, then readouts either side of a
/// hairline at the centre: distance to the left, ETA and the braking cue to
/// the right. Returns the block's bottom.
#[allow(clippy::too_many_arguments)]
fn info(hud: &Hud, cx: f32, top: f32, name: &str, distance_m: f64, eta_s: Option<f64>, brake: Option<Readout>, stale: bool) -> f32 {
    let u = hud.u;
    let ink = if stale { MUTED } else { TEXT };
    let title = hud.label(name, 12.0, ink);
    let bar_w = 3.0 * u;
    let tw = bar_w + 7.0 * u + title.size().x;
    let tx = hud.snap(cx - tw / 2.0);
    let bar = Rect::from_min_size(Pos2::new(tx, top + 1.0 * u), vec2(hud.snap(bar_w), title.size().y - 2.0 * u));
    hud.p.rect_filled(bar, 0.0, if stale { MUTED } else { MARK });
    hud.put(Pos2::new(tx + bar_w + 7.0 * u, top), &title);

    let y = top + title.size().y + 6.0 * u;
    let gap = 14.0 * u;
    let (value, unit) = split_distance(distance_m);
    let left = [hud.text(&value, 26.0, ink, 0.0), hud.label(unit, 9.5, MUTED)];
    let label_y = y + left[0].size().y - 2.0 * u;
    let bottom = label_y + left[1].size().y;
    hud.put(Pos2::new(cx - gap - left[0].size().x, y), &left[0]);
    hud.put(Pos2::new(cx - gap - left[1].size().x, label_y), &left[1]);

    let mut right: Vec<Readout> = vec![];
    if let Some(t) = eta_s {
        right.push((clock(t), ink, "eta"));
    }
    right.extend(brake);
    let mut x = cx;
    for (value, color, label) in right {
        hud.divider(x, y + 4.0 * u, bottom - 2.0 * u);
        let v = hud.text(&value, 26.0, color, 0.0);
        let l = hud.label(label, 9.5, if color == ink { MUTED } else { color });
        hud.put(Pos2::new(x + gap, y), &v);
        hud.put(Pos2::new(x + gap, label_y), &l);
        // a fixed slot, so the next column doesn't move as digits change
        let slot = hud.text("88:88", 26.0, TEXT, 0.0).size().x.max(v.size().x).max(l.size().x);
        x += gap + slot + gap;
    }
    bottom
}

/// The braking cue as a readout, only while it matters.
fn brake(hud: &Hud, cue: Cue, time_to_brake_s: Option<f64>) -> Option<Readout> {
    match (cue, time_to_brake_s) {
        (Cue::Cruise, Some(t)) if t < 90.0 => Some((clock(t), MUTED, "brake")),
        (Cue::Prepare, t) => Some((clock(t.unwrap_or(0.0)), AMBER, "brake")),
        (Cue::Brake, _) => Some(("NOW".into(), RED.gamma_multiply(0.6 + 0.4 * hud.pulse()), "brake")),
        (Cue::Arrived, _) => Some(("HERE".into(), MARK, "arrived")),
        _ => None,
    }
}

fn status(hud: &Hud, cx: f32, top: f32, name: &str, why: &str) {
    let title = hud.label(name, 12.0, TEXT);
    hud.put_centred(cx, top, &title);
    let segs = [Hud::seg(hud.label(why, 11.0, AMBER))];
    hud.card(cx, top + title.size().y + 8.0 * hud.u, &segs, AMBER, glass());
}

/// Under the tape at the target's bearing; beyond the tape, sideways at its
/// end with the turn.
fn marker(hud: &Hud, c: &Compass, rel: f32, color: Color32) {
    let h = hud.r.height();
    let (cx, ty) = (hud.r.center().x, hud.r.top() + c.y * h);
    let ppd = c.per_deg * h;
    let size = 7.0 * hud.u;
    if rel.abs() <= c.span {
        let tip = hud.snap_pos(Pos2::new(cx + rel * ppd, ty + MARKER_DY * hud.u));
        hud.chevron(tip, vec2(0.0, -1.0), size, color);
        return;
    }
    let side = rel.signum();
    let tip = hud.snap_pos(Pos2::new(cx + side * (c.span * ppd + 16.0 * hud.u), ty));
    hud.chevron(tip, vec2(side, 0.0), size, color);
    let g = hud.text(&format!("{:.0}°", rel.abs()), 18.0, color, 0.0);
    let x = if side > 0.0 { tip.x + 8.0 * hud.u } else { tip.x - 8.0 * hud.u - g.size().x };
    hud.put(Pos2::new(x, ty - g.size().y / 2.0), &g);
}

/// A space target where it is on screen, from the view angles: a thin
/// diamond with its name and distance under it, or a chevron at the
/// screen's edge with the turn. The ring is where the ship is moving.
fn on_screen(hud: &Hud, g: &SpaceGuidance, name: &str, fov_deg: f32) {
    let Some((right, up)) = g.target_view else { return };
    let (h, u) = (hud.r.height(), hud.u);
    let f = (h / 2.0) / (fov_deg.to_radians() / 2.0).tan();
    let c = hud.r.center();
    let project = |(r, u): (f64, f64)| {
        let (r, u) = (r.to_radians(), u.to_radians());
        let (dr, du, df) = (u.cos() * r.sin(), u.sin(), u.cos() * r.cos());
        (df > 0.05).then(|| Pos2::new(c.x + (dr / df) as f32 * f, c.y - (du / df) as f32 * f))
    };
    let inner = hud.r.shrink(0.07 * h);
    let size = 9.0 * u;
    if let Some(p) = project((right, up)).filter(|p| inner.contains(*p)) {
        let p = hud.snap_pos(p);
        let d = |x: f32, y: f32| Pos2::new(p.x + x, p.y + y);
        hud.glow(&[d(0.0, -size), d(size, 0.0), d(0.0, size), d(-size, 0.0)], 1.5 * u, MARK, true);
        let title = hud.label(name, 11.0, TEXT);
        let dist = hud.text(&distance(g.distance_m), 12.0, MUTED, 0.3);
        let y = p.y + size + 8.0 * u;
        hud.put_centred(p.x, y, &title);
        hud.put_centred(p.x, y + title.size().y + 1.0 * u, &dist);
    } else {
        // the direction on screen, pushed to the inner edge
        let (r, up_r) = (right.to_radians() as f32, up.to_radians() as f32);
        let raw = vec2(r.sin(), -up_r.sin());
        let dir = if raw.length() > 1e-4 { raw.normalized() } else { vec2(1.0, 0.0) };
        let k = ((inner.width() / 2.0) / dir.x.abs()).min((inner.height() / 2.0) / dir.y.abs());
        let tip = hud.snap_pos(c + dir * k);
        hud.chevron(tip, dir, size, MARK);
        let off = (right.to_radians().cos() * up.to_radians().cos()).clamp(-1.0, 1.0).acos().to_degrees();
        let at = tip - dir * 46.0 * u;
        let title = hud.label(name, 11.0, TEXT);
        let deg = hud.text(&format!("{off:.0}°"), 16.0, MARK, 0.0);
        hud.put_centred(at.x, at.y - title.size().y, &title);
        hud.put_centred(at.x, at.y + 1.0 * u, &deg);
    }
    if let Some(p) = g.travel_view.and_then(project).filter(|p| inner.contains(*p)) {
        hud.ring(hud.snap_pos(p), 6.0 * u, 1.5 * u, MARK.gamma_multiply(0.85));
    }
}

fn ground(hud: &Hud, w: &GroundWarning) {
    let u = hud.u;
    let (cx, top) = (hud.r.center().x, hud.r.top() + 0.6 * hud.r.height());
    let impact = |label_color: Color32| {
        [Hud::seg(hud.label("impact", 11.0, label_color)), hud.num(hud.text(&clock(w.time_to_impact_s), 16.0, TEXT, 0.0), "8:88", 16.0)]
    };
    match w.level {
        GroundLevel::PullUp => {
            let fill = Color32::from_rgba_unmultiplied(160, 30, 26, (160.0 + 80.0 * hud.pulse()) as u8);
            let rect = hud.card(cx, top, &[Hud::seg(hud.label("pull up", 30.0, TEXT))], Color32::WHITE, fill);
            hud.card(cx, rect.bottom() + 6.0 * u, &impact(MUTED), RED, glass());
        }
        GroundLevel::Caution => {
            let mut segs = vec![Hud::seg(hud.label("sink", 11.0, AMBER)), hud.num(hud.text(&format!("{:.0} m/s", w.sink), 16.0, TEXT, 0.0), "888 m/s", 16.0)];
            segs.extend(impact(MUTED));
            hud.card(cx, top, &segs, AMBER, glass());
        }
    }
}

/// While adjusting: a chevron straight ahead and two at ±20°, ticks where
/// the tape should end, and the sample block.
fn preview(hud: &Hud, c: &Compass) {
    let h = hud.r.height();
    let (cx, ty) = (hud.r.center().x, hud.r.top() + c.y * h);
    for side in [-1.0, 1.0] {
        let x = hud.snap(cx + side * c.span * c.per_deg * h);
        hud.glow(&[Pos2::new(x, ty - 10.0 * hud.u), Pos2::new(x, ty + 10.0 * hud.u)], 2.0 * hud.u, AMBER, false);
    }
    for deg in [-20.0, 20.0] {
        marker(hud, c, deg, MARK.gamma_multiply(0.5));
    }
    marker(hud, c, 0.0, MARK);
    let brake = brake(hud, Cue::Cruise, Some(30.0));
    info(hud, cx, ty + BLOCK_DY * hud.u, "preview", 1200.0, Some(42.0), brake, false);
}

/// The number and its unit, apart: `("22.5", "km")`.
fn split_distance(m: f64) -> (String, &'static str) {
    if m >= 10_000.0 {
        (format!("{:.1}", m / 1000.0), "km")
    } else if m >= 1000.0 {
        (format!("{:.2}", m / 1000.0), "km")
    } else {
        (format!("{m:.0}"), "m")
    }
}

fn distance(m: f64) -> String {
    let (v, unit) = split_distance(m);
    format!("{v} {unit}")
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
