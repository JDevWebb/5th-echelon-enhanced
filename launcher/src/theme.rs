//! The launcher's look: the dark Echelon theme the in-game overlay uses
//! (slate greys, night-vision green), set in IBM Plex Sans.

use std::sync::Arc;

use eframe::egui;
use eframe::egui::Color32;
use eframe::egui::FontFamily;
use eframe::egui::FontId;
use eframe::egui::Stroke;
use eframe::egui::TextStyle;

pub const BG: Color32 = Color32::from_rgb(0x0f, 0x15, 0x18);
pub const SURFACE: Color32 = Color32::from_rgb(0x1c, 0x25, 0x2a);
/// Buttons and other controls, a step up from the card they sit on.
pub const CONTROL: Color32 = Color32::from_rgb(0x2a, 0x36, 0x3d);
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(0x35, 0x43, 0x4b);
pub const CONTROL_LINE: Color32 = Color32::from_rgb(0x3a, 0x48, 0x50);
pub const LINE: Color32 = Color32::from_rgb(0x2b, 0x36, 0x3c);
pub const FG: Color32 = Color32::from_rgb(0xe6, 0xed, 0xf0);
pub const MUTED: Color32 = Color32::from_rgb(0x93, 0xa4, 0xad);
pub const ACCENT: Color32 = Color32::from_rgb(0x8f, 0xd1, 0x4f);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x0b, 0x14, 0x05);
pub const OK: Color32 = Color32::from_rgb(0x3e, 0xcf, 0x9e);
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x4c);
pub const BAD: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);

/// The semi-bold face, for headings and emphasis.
pub fn strong() -> FontFamily {
    FontFamily::Name("strong".into())
}

pub fn apply(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "plex".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../../hooks/fonts/IBMPlexSans-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "plex-semibold".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../../hooks/fonts/IBMPlexSans-SemiBold.ttf"))),
    );
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "plex".into());
    let mut strong = vec!["plex-semibold".to_string()];
    strong.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(self::strong(), strong);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(22.0, self::strong())),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
        style.spacing.interact_size.y = 30.0;

        let v = &mut style.visuals;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = Color32::from_rgb(0x0a, 0x0f, 0x11);
        v.faint_bg_color = SURFACE;
        v.override_text_color = Some(FG);
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.linear_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.window_stroke = Stroke::new(1.0, LINE);
        v.window_corner_radius = 10.into();
        for (w, fill) in [
            (&mut v.widgets.noninteractive, SURFACE),
            (&mut v.widgets.inactive, CONTROL),
            (&mut v.widgets.hovered, CONTROL_HOVER),
            (&mut v.widgets.active, CONTROL_HOVER),
            (&mut v.widgets.open, CONTROL_HOVER),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.corner_radius = 4.into();
            w.fg_stroke.color = FG;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, CONTROL_LINE);
        v.widgets.noninteractive.fg_stroke.color = MUTED;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.linear_multiply(0.5));
    });
}

/// A status marker: a filled dot for ok, a ring for a warning, a cross for
/// a failure (shapes, so they never depend on a font having the glyph).
pub fn status_marker(ui: &mut egui::Ui, status: setup::diagnose::Status) {
    use setup::diagnose::Status;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    match status {
        Status::Ok => {
            p.circle_filled(c, 5.0, OK);
        }
        Status::Warn => {
            p.circle_stroke(c, 5.0, Stroke::new(2.0, WARN));
        }
        Status::Fail => {
            let d = 4.5;
            let s = Stroke::new(2.2, BAD);
            p.line_segment([c + egui::vec2(-d, -d), c + egui::vec2(d, d)], s);
            p.line_segment([c + egui::vec2(-d, d), c + egui::vec2(d, -d)], s);
        }
    }
}

/// A card: a raised surface for one part of the screen.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(16))
}

/// The one main action on a screen.
pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text).color(ON_ACCENT).family(strong()).size(16.0)).fill(ACCENT)
}

pub fn heading(text: &str) -> egui::RichText {
    egui::RichText::new(text).family(strong()).size(17.0)
}

pub fn muted(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).color(MUTED)
}
