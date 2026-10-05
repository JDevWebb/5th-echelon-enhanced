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
/// The side menu, a step darker than the page.
pub const RAIL: Color32 = Color32::from_rgb(0x0b, 0x10, 0x13);
/// Wells inside a card (stat tiles, the launch bar), a step below it.
pub const SUNKEN: Color32 = Color32::from_rgb(0x14, 0x1c, 0x20);
/// Secondary text that should read more strongly than [`MUTED`].
pub const SOFT: Color32 = Color32::from_rgb(0xb8, 0xc6, 0xcd);

/// The semi-bold face, for headings and emphasis.
pub fn strong() -> FontFamily {
    FontFamily::Name("strong".into())
}

/// IBM Plex Sans Condensed Bold, for display headings.
pub fn condensed() -> FontFamily {
    FontFamily::Name("condensed".into())
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
    fonts.font_data.insert(
        "plex-condensed".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../../hooks/fonts/IBMPlexSansCondensed-Bold.ttf"))),
    );
    fonts.font_data.insert(
        "plex-mono".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../../hooks/fonts/IBMPlexMono-Regular.ttf"))),
    );
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "plex".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "plex-mono".into());
    let mut strong = vec!["plex-semibold".to_string()];
    strong.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(self::strong(), strong);
    let mut condensed = vec!["plex-condensed".to_string()];
    condensed.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(self::condensed(), condensed);
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
        .corner_radius(14)
        .inner_margin(egui::Margin::symmetric(20, 18))
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

/// Big uppercase display type, for screen titles and the game's name.
pub fn display(text: &str, size: f32) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).family(condensed()).size(size).extra_letter_spacing(size * 0.02)
}

/// A small uppercase label over a value or a group ("SERVER", "PING").
pub fn caps(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).size(11.5).extra_letter_spacing(1.2).color(MUTED)
}

/// The page's own padding.
pub fn page() -> egui::Frame {
    egui::Frame::new().inner_margin(egui::Margin::symmetric(32, 26))
}

/// The big Play button.
pub fn play_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(text.to_uppercase())
            .color(ON_ACCENT)
            .family(condensed())
            .size(24.0)
            .extra_letter_spacing(3.0),
    )
    .fill(ACCENT)
    .corner_radius(12)
    .min_size(egui::vec2(220.0, 58.0))
}

/// A quieter button, for actions beside the main one.
pub fn secondary(text: &str) -> egui::Button<'static> {
    egui::Button::new(text.to_string()).corner_radius(10).min_size(egui::vec2(0.0, 38.0))
}

/// The launcher's mark (docs/logo.svg): five units in an echelon formation, the fifth
/// out in front, on a tile `size` points square.
pub fn mark(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let at = |x: f32, y: f32| rect.min + egui::vec2(x, y) * (size / 100.0);
    let tile = egui::Rect::from_min_max(at(4.0, 4.0), at(96.0, 96.0));
    let painter = ui.painter();
    painter.rect(
        tile,
        size * 0.18,
        Color32::from_rgb(0x14, 0x1c, 0x20),
        Stroke::new((size / 50.0).max(1.0), LINE),
        egui::StrokeKind::Inside,
    );
    let shades = [
        Color32::from_rgb(0x4a, 0x5a, 0x62),
        Color32::from_rgb(0x5d, 0x70, 0x79),
        Color32::from_rgb(0x73, 0x87, 0x8f),
        MUTED,
        ACCENT,
    ];
    for (i, shade) in shades.into_iter().enumerate() {
        let (x, y) = (16.0 + 14.0 * i as f32, 68.0 - 14.0 * i as f32);
        painter.rect_filled(egui::Rect::from_min_max(at(x, y), at(x + 14.0, y + 14.0)), size * 0.02, shade);
    }
}

/// The faint night-vision grid behind the game's banner.
pub fn grid_backdrop(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, 0, Color32::from_rgb(0x12, 0x1a, 0x1e));
    let line = Stroke::new(1.0, ACCENT.linear_multiply(0.06));
    let step = 48.0;
    let mut x = rect.left() + step;
    while x < rect.right() {
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], line);
        x += step;
    }
    let mut y = rect.top() + step;
    while y < rect.bottom() {
        painter.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], line);
        y += step;
    }
}

/// `texture` filling `rect` like CSS `object-fit: cover`: scaled to cover it,
/// cut to fit, kept centred (a little above, where loading screens put their
/// subject).
pub fn cover_image(painter: &egui::Painter, rect: egui::Rect, texture: &egui::TextureHandle) {
    let [w, h] = texture.size().map(|v| v as f32);
    let (image, area) = (w / h, rect.width() / rect.height());
    let uv = if area > image {
        // Wider than the image: full width, a band of its height.
        let span = image / area;
        let top = (1.0 - span) * 0.4;
        egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(1.0, top + span))
    } else {
        let span = area / image;
        let left = (1.0 - span) / 2.0;
        egui::Rect::from_min_max(egui::pos2(left, 0.0), egui::pos2(left + span, 1.0))
    };
    painter.image(texture.id(), rect, uv, Color32::WHITE);
}

/// Darkens art from the bottom and the left, where the banner's text sits.
pub fn scrim(painter: &egui::Painter, rect: egui::Rect) {
    let mut mesh = egui::Mesh::default();
    let mut quad = |r: egui::Rect, tl: Color32, tr: Color32, bl: Color32, br: Color32| {
        let i = mesh.vertices.len() as u32;
        for (pos, color) in [(r.left_top(), tl), (r.right_top(), tr), (r.left_bottom(), bl), (r.right_bottom(), br)] {
            mesh.colored_vertex(pos, color);
        }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 2, i + 3);
    };
    let shade = |a: f32| BG.gamma_multiply(a);
    // Top to bottom: a light veil, deepening to the page's colour at the foot.
    quad(rect, shade(0.25), shade(0.25), shade(0.92), shade(0.92));
    // Left to right: the title's side darker, fading out by two thirds across.
    let left = egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + rect.width() * 0.66, rect.bottom()));
    quad(left, shade(0.55), Color32::TRANSPARENT, shade(0.55), Color32::TRANSPARENT);
    painter.add(egui::Shape::mesh(mesh));
}

/// A row of buttons of which one is chosen (in place of a drop-down when
/// the choices are few and worth seeing at once).
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(SUNKEN)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (option, label) in options {
                    let chosen = *value == *option;
                    let text = egui::RichText::new(*label).color(if chosen { FG } else { MUTED });
                    let text = if chosen { text.family(strong()) } else { text };
                    let button = egui::Button::new(text)
                        .fill(if chosen { CONTROL } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(8)
                        .min_size(egui::vec2(150.0, 38.0));
                    if ui.add(button).clicked() && !chosen {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// The side menu's icons, drawn so they never depend on a font's glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Servers,
    News,
    Host,
    Settings,
}

fn paint_icon(painter: &egui::Painter, rect: egui::Rect, icon: Icon, color: Color32) {
    let s = Stroke::new(1.8, color);
    let c = rect.center();
    let u = rect.width() / 24.0;
    let p = |x: f32, y: f32| egui::pos2(rect.left() + x * u, rect.top() + y * u);
    match icon {
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(vec![p(7.0, 4.0), p(19.0, 12.0), p(7.0, 20.0)], Color32::TRANSPARENT, s));
        }
        Icon::Servers => {
            for top in [4.0, 14.0] {
                painter.rect_stroke(egui::Rect::from_min_max(p(3.0, top), p(21.0, top + 6.0)), 2, s, egui::StrokeKind::Middle);
                painter.circle_filled(p(7.0, top + 3.0), 1.3 * u, color);
            }
        }
        Icon::News => {
            // A page with a headline block and lines of text.
            painter.rect_stroke(egui::Rect::from_min_max(p(4.0, 4.0), p(20.0, 20.0)), 2, s, egui::StrokeKind::Middle);
            painter.rect_filled(egui::Rect::from_min_max(p(7.0, 7.0), p(12.0, 11.5)), 1, color);
            for (y, x) in [(8.0, 17.0), (11.0, 17.0), (14.5, 17.0), (17.0, 13.0)] {
                let from = if y < 12.0 { 13.5 } else { 7.0 };
                painter.line_segment([p(from, y), p(x, y)], s);
            }
        }
        Icon::Host => {
            // A broadcast: a mast with waves either side.
            painter.circle_filled(p(12.0, 10.0), 1.8 * u, color);
            painter.line_segment([p(12.0, 12.0), p(12.0, 21.0)], s);
            for (r, a) in [(5.0, 0.9_f32), (9.0, 0.9)] {
                for side in [-1.0_f32, 1.0] {
                    let pts: Vec<egui::Pos2> = (0..=8)
                        .map(|i| {
                            let t = -a + 2.0 * a * i as f32 / 8.0;
                            egui::pos2(c.x + side * r * u * t.cos(), rect.top() + 10.0 * u + r * u * t.sin())
                        })
                        .collect();
                    painter.add(egui::Shape::line(pts, s));
                }
            }
        }
        Icon::Settings => {
            painter.circle_stroke(c, 3.2 * u, s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (sin, cos) = a.sin_cos();
                painter.line_segment([c + egui::vec2(cos, sin) * 6.0 * u, c + egui::vec2(cos, sin) * 9.5 * u], s);
            }
        }
    }
}

/// A small round button with a chevron, for paging (`left` points left).
pub fn chevron(ui: &mut egui::Ui, left: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    p.circle_filled(rect.center(), 13.0, if response.hovered() { CONTROL_HOVER } else { CONTROL });
    let c = rect.center();
    let d = if left { -1.0 } else { 1.0 };
    let s = Stroke::new(2.0, FG);
    p.line_segment([c + egui::vec2(-2.5 * d, -5.0), c + egui::vec2(2.5 * d, 0.0)], s);
    p.line_segment([c + egui::vec2(2.5 * d, 0.0), c + egui::vec2(-2.5 * d, 5.0)], s);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, if left { "Previous" } else { "Next" }));
    response
}

/// One entry of the side menu: an icon over a small label.
/// A small button in the side menu with another site's mark (white on clear, tinted here).
pub fn mark_button(ui: &mut egui::Ui, mark: &egui::TextureHandle, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let (fill, tint) = if response.hovered() { (SURFACE, FG) } else { (Color32::TRANSPARENT, MUTED) };
    let painter = ui.painter();
    painter.rect_filled(rect, 8, fill);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(mark.id(), egui::Rect::from_center_size(rect.center(), egui::vec2(19.0, 19.0)), uv, tint);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
}

pub fn nav_button(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(66.0, 60.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if selected {
        SURFACE
    } else if response.hovered() {
        SURFACE.linear_multiply(0.6)
    } else {
        Color32::TRANSPARENT
    };
    let color = if selected {
        ACCENT
    } else if response.hovered() {
        FG
    } else {
        MUTED
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 10, fill);
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 21.0), egui::vec2(22.0, 22.0));
    paint_icon(painter, icon_rect, icon, color);
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 13.0),
        egui::Align2::CENTER_CENTER,
        label.to_uppercase(),
        FontId::new(10.5, FontFamily::Proportional),
        color,
    );
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label));
    response
}
