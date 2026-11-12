//! One look for both windows: dark glass, one accent, and colours that mean
//! one thing each (ok / caution / alert).

use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, Stroke, TextStyle, Visuals};

pub const BG: Color32 = Color32::from_rgb(14, 17, 22);
pub const PANEL: Color32 = Color32::from_rgb(20, 24, 31);
pub const RAISED: Color32 = Color32::from_rgb(29, 34, 43);
pub const LINE: Color32 = Color32::from_rgb(44, 52, 64);
pub const INK: Color32 = Color32::from_rgb(232, 237, 243);
pub const DIM: Color32 = Color32::from_rgb(140, 151, 165);
pub const ACCENT: Color32 = Color32::from_rgb(94, 186, 232);
pub const OK: Color32 = Color32::from_rgb(110, 210, 150);
pub const WARN: Color32 = Color32::from_rgb(242, 182, 72);
pub const ALERT: Color32 = Color32::from_rgb(250, 82, 72);

/// The HUD's glass: the panel colour with alpha.
pub fn glass() -> Color32 {
    Color32::from_rgba_unmultiplied(10, 14, 20, 170)
}

/// The HUD's font family: a heavier face that holds up over the game.
pub fn hud_font() -> egui::FontFamily {
    egui::FontFamily::Name("hud".into())
}

/// Windows' own UI faces when present (neutral, nothing to ship); egui's
/// defaults otherwise.
fn fonts(ctx: &egui::Context) {
    let read = |name: &str| std::fs::read(format!(r"C:\Windows\Fonts\{name}")).ok();
    let mut fonts = egui::FontDefinitions::default();
    if let Some(ui) = read("segoeui.ttf") {
        fonts.font_data.insert("ui".into(), egui::FontData::from_owned(ui).into());
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "ui".into());
    }
    let mut hud = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(bold) = read("seguisb.ttf") {
        fonts.font_data.insert("hud".into(), egui::FontData::from_owned(bold).into());
        hud.insert(0, "hud".into());
    }
    fonts.families.insert(hud_font(), hud);
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    fonts(ctx);
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = RAISED;
    v.override_text_color = Some(INK);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, LINE);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, PANEL),
        (&mut v.widgets.inactive, RAISED),
        (&mut v.widgets.hovered, Color32::from_rgb(38, 45, 57)),
        (&mut v.widgets.active, Color32::from_rgb(46, 56, 70)),
        (&mut v.widgets.open, RAISED),
    ] {
        w.corner_radius = CornerRadius::same(6);
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.window_margin = Margin::same(12);
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(16.0));
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
    });
}

/// A titled card: a raised panel grouping one topic.
pub fn card<R>(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(RAISED.gamma_multiply(0.55))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(egui::RichText::new(title.to_uppercase()).size(11.0).color(DIM).strong());
            ui.add_space(2.0);
            body(ui)
        })
        .inner
}

/// A small coloured status dot with a label.
pub fn dot(ui: &mut egui::Ui, col: Color32, text: impl Into<String>) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, col);
    ui.label(egui::RichText::new(text.into()).color(col).size(12.0));
}

pub fn fmt_km(m: f64) -> String {
    if m >= 10_000.0 {
        format!("{:.1} km", m / 1000.0)
    } else if m >= 1000.0 {
        format!("{:.2} km", m / 1000.0)
    } else {
        format!("{m:.0} m")
    }
}

pub fn fmt_time(s: f64) -> String {
    if s < 0.0 {
        "now".into()
    } else if s < 90.0 {
        format!("{s:.0}s")
    } else {
        format!("{}:{:02}", (s / 60.0) as i64, (s % 60.0) as i64)
    }
}

pub fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n - 1).collect::<String>() + "…" }
}
