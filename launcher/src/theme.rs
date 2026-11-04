//! The launcher's look, "Field ops": near-black panels, one night-vision green, HUD labels in
//! IBM Plex Mono caps, titles in Plex Sans Condensed, the rest in Plex Sans.

use std::sync::Arc;

use eframe::egui;
use eframe::egui::Color32;
use eframe::egui::FontFamily;
use eframe::egui::FontId;
use eframe::egui::Stroke;
use eframe::egui::TextStyle;

pub const BG: Color32 = Color32::from_rgb(0x0a, 0x0e, 0x0f);
pub const SURFACE: Color32 = Color32::from_rgb(0x0c, 0x11, 0x12);
/// Buttons and other controls, a step up from the card they sit on.
pub const CONTROL: Color32 = Color32::from_rgb(0x15, 0x1e, 0x1f);
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(0x1c, 0x28, 0x27);
pub const CONTROL_LINE: Color32 = Color32::from_rgb(0x26, 0x34, 0x34);
pub const LINE: Color32 = Color32::from_rgb(0x17, 0x20, 0x22);
pub const FG: Color32 = Color32::from_rgb(0xe3, 0xec, 0xe6);
pub const MUTED: Color32 = Color32::from_rgb(0x84, 0x96, 0x8f);
pub const ACCENT: Color32 = Color32::from_rgb(0x9b, 0xe0, 0x5a);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x07, 0x10, 0x03);
pub const OK: Color32 = Color32::from_rgb(0x9b, 0xe0, 0x5a);
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x4c);
pub const BAD: Color32 = Color32::from_rgb(0xff, 0x7a, 0x6b);
/// The side menu, a step darker than the page.
pub const RAIL: Color32 = Color32::from_rgb(0x07, 0x09, 0x0a);
/// Wells inside a card (inputs, the launch bar), a step below it.
pub const SUNKEN: Color32 = Color32::from_rgb(0x07, 0x09, 0x0a);
/// Secondary text that should read more strongly than [`MUTED`].
pub const SOFT: Color32 = Color32::from_rgb(0xa9, 0xba, 0xb3);
/// What's chosen or current: a green tint behind it, with a green edge.
pub const SELECTED: Color32 = Color32::from_rgb(0x10, 0x1c, 0x16);

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
        v.extreme_bg_color = SUNKEN;
        v.faint_bg_color = SURFACE;
        v.override_text_color = Some(FG);
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.linear_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.window_corner_radius = 6.into();
        v.menu_corner_radius = 4.into();
        v.window_stroke = Stroke::new(1.0, CONTROL_LINE);
        v.window_shadow = egui::Shadow {
            offset: [0, 16],
            blur: 48,
            spread: 0,
            color: Color32::from_black_alpha(150),
        };
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

/// A card: a panel for one part of the screen.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(6)
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

/// A screen's title: its kicker in green HUD caps over the name in big condensed type.
pub fn title(ui: &mut egui::Ui, kicker: &str, title: &str) {
    ui.label(self::kicker(kicker));
    ui.add_space(-4.0);
    ui.label(display(title, 40.0));
    ui.add_space(2.0);
}

/// A screen's title with tabs on its right (Intel's Briefings and Roadmap): answers the tab
/// clicked, if one was. `tabs` are (value, label); `current` is lit.
pub fn title_with_tabs<T: PartialEq + Copy>(ui: &mut egui::Ui, kicker: &str, title: &str, current: T, tabs: &[(T, &str)]) -> Option<T> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.vertical(|ui| self::title(ui, kicker, title));
        if tabs.len() > 1 {
            // As wide as segmented draws them, at the row's right edge, level with the title.
            let wide = tabs.len() as f32 * (SEGMENT_WIDTH + 3.0) + 9.0;
            ui.add_space((ui.available_width() - wide).max(0.0));
            let mut value = current;
            if segmented(ui, &mut value, tabs) {
                picked = Some(value);
            }
        }
    });
    picked
}

/// A small uppercase label over a value or a group ("SERVER", "PING"), as a HUD reads: Plex
/// Mono caps.
pub fn caps(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).monospace().size(11.5).extra_letter_spacing(1.4).color(MUTED)
}

/// [`caps`] in the accent: a screen's kicker over its title ("NETWORK · SCBL.JDEVWEBB.NET").
pub fn kicker(text: &str) -> egui::RichText {
    caps(text).color(ACCENT)
}

/// The widest a screen's content gets (wider, lines get too long to read); the Play screen,
/// the first-run setup and Chat use the whole window.
pub const MAX_PAGE: f32 = 1180.0;

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
    .corner_radius(4)
    .min_size(egui::vec2(220.0, 58.0))
}

/// A quieter button, for actions beside the main one.
pub fn secondary(text: &str) -> egui::Button<'static> {
    egui::Button::new(text.to_string()).corner_radius(4).min_size(egui::vec2(0.0, 38.0))
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

/// Night vision behind the game's banner and the setup: a soft green glow over the page's
/// colour, with faint scanlines.
pub fn grid_backdrop(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, 0, BG);
    glow(
        painter,
        rect,
        egui::pos2(rect.left() + rect.width() * 0.7, rect.top() + rect.height() * 0.35),
        ACCENT.gamma_multiply(0.09),
    );
    let line = Stroke::new(1.0, ACCENT.gamma_multiply(0.035));
    let mut y = rect.top() + 2.0;
    while y < rect.bottom() {
        painter.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], line);
        y += 4.0;
    }
}

/// A radial glow of `color` at `centre`, fading to nothing at the far edge of `rect`.
pub fn glow(painter: &egui::Painter, rect: egui::Rect, centre: egui::Pos2, color: Color32) {
    let radius = (rect.width().max(rect.height())) * 0.6;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(centre, color);
    let steps = 48;
    for i in 0..=steps {
        let a = i as f32 / steps as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(centre + egui::vec2(a.cos(), a.sin() * 0.8) * radius, Color32::TRANSPARENT);
    }
    for i in 1..=steps as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    let clip = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    clip.add(egui::Shape::mesh(mesh));
}

/// The three lights of the goggles, the side menu's mark: `size` is a light's diameter.
pub fn lights(ui: &mut egui::Ui, size: f32) {
    let gap = size * 0.5;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size * 3.0 + gap * 2.0, size), egui::Sense::hover());
    let painter = ui.painter();
    for i in 0..3 {
        let c = egui::pos2(rect.left() + size / 2.0 + i as f32 * (size + gap), rect.center().y);
        painter.circle_filled(c, size, ACCENT.gamma_multiply(0.12));
        painter.circle_filled(c, size / 2.0, ACCENT);
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

/// The width of each of [`segmented`]'s choices (at least).
const SEGMENT_WIDTH: f32 = 130.0;

/// A row of buttons of which one is chosen (in place of a drop-down when
/// the choices are few and worth seeing at once).
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(SUNKEN)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(4)
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for (option, label) in options {
                    let chosen = *value == *option;
                    let text = egui::RichText::new(*label).color(if chosen { FG } else { MUTED });
                    let text = if chosen { text.family(strong()) } else { text };
                    let button = egui::Button::new(text)
                        .fill(if chosen { SELECTED } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(3)
                        .min_size(egui::vec2(SEGMENT_WIDTH, 36.0));
                    let response = ui.add(button);
                    // The chosen one's green underline.
                    if chosen {
                        let r = response.rect;
                        ui.painter().rect_filled(
                            egui::Rect::from_min_max(egui::pos2(r.left() + 10.0, r.bottom() - 2.0), egui::pos2(r.right() - 10.0, r.bottom())),
                            1,
                            ACCENT,
                        );
                    }
                    if response.clicked() && !chosen {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// An on/off switch with its label to the right, in place of a checkbox: green and to the right
/// when on. Clicking it (or its label) flips `on`; the response says `changed()` then, and reads
/// as a checkbox to screen readers and the tests.
pub fn switch(ui: &mut egui::Ui, on: &mut bool, label: impl Into<egui::WidgetText>) -> egui::Response {
    let galley = label
        .into()
        .into_galley(ui, Some(egui::TextWrapMode::Wrap), ui.available_width() - 56.0, egui::TextStyle::Body);
    let track = egui::vec2(40.0, 22.0);
    let size = egui::vec2(track.x + 12.0 + galley.size().x, galley.size().y.max(track.y) + 6.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let text = galley.text().to_string();
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, &text));
    let enabled = ui.is_enabled();
    let how_far = ui.ctx().animate_bool_responsive(response.id, *on);
    let track_rect = egui::Rect::from_min_size(egui::pos2(rect.left(), rect.center().y - track.y / 2.0), track);
    let painter = ui.painter();
    let (fill, stroke) = if *on { (SELECTED.lerp_to_gamma(ACCENT, 0.25), ACCENT) } else { (CONTROL, CONTROL_LINE) };
    let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    painter.rect(track_rect, track.y / 2.0, dim(fill), Stroke::new(1.0, dim(stroke)), egui::StrokeKind::Inside);
    let knob_x = egui::lerp((track_rect.left() + 11.0)..=(track_rect.right() - 11.0), how_far);
    painter.circle_filled(egui::pos2(knob_x, track_rect.center().y), 7.5, dim(if *on { ACCENT } else { MUTED }));
    let text_pos = egui::pos2(rect.left() + track.x + 12.0, rect.center().y - galley.size().y / 2.0);
    painter.galley(text_pos, galley, if enabled { FG } else { MUTED });
    response
}

/// The side menu's icons, drawn so they never depend on a font's glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Matches,
    Friends,
    Servers,
    News,
    Roadmap,
    Chat,
    Support,
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
        Icon::Matches => {
            // A board with a header row and two lines of entries (the Tauri launcher's).
            painter.rect_stroke(egui::Rect::from_min_max(p(3.0, 4.0), p(21.0, 20.0)), 2, s, egui::StrokeKind::Middle);
            painter.line_segment([p(3.0, 9.0), p(21.0, 9.0)], s);
            painter.line_segment([p(8.0, 13.0), p(16.0, 13.0)], s);
            painter.line_segment([p(8.0, 16.0), p(13.0, 16.0)], s);
        }
        Icon::Friends => {
            // Two people, one behind the other (the Tauri launcher's).
            painter.circle_stroke(p(9.0, 8.0), 3.2 * u, s);
            let arc = |cx: f32, cy: f32, rx: f32, ry: f32| -> Vec<egui::Pos2> {
                (0..=10)
                    .map(|i| {
                        let t = std::f32::consts::PI * (1.0 + i as f32 / 10.0);
                        p(cx + rx * t.cos(), cy + ry * t.sin())
                    })
                    .collect()
            };
            painter.add(egui::Shape::line(arc(9.0, 19.0, 6.0, 5.0), s));
            painter.circle_stroke(p(17.0, 9.0), 2.4 * u, s);
            painter.add(egui::Shape::line(arc(17.5, 18.5, 3.5, 4.8).into_iter().skip(5).collect(), s));
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
        Icon::Roadmap => {
            // A plan: three bars staggered in time, the first one filled (shipping).
            for (i, (from, to)) in [(3.0, 12.0), (8.0, 17.0), (13.0, 21.0)].into_iter().enumerate() {
                let y = 5.0 + 6.0 * i as f32;
                let bar = egui::Rect::from_min_max(p(from, y - 1.8), p(to, y + 1.8));
                if i == 0 {
                    painter.rect_filled(bar, 2, color);
                } else {
                    painter.rect_stroke(bar, 2, s, egui::StrokeKind::Middle);
                }
            }
        }
        Icon::Chat => {
            // Two speech bubbles, one behind the other.
            painter.rect_stroke(egui::Rect::from_min_max(p(2.5, 3.5), p(15.5, 12.5)), 3, s, egui::StrokeKind::Middle);
            painter.add(egui::Shape::line(vec![p(6.0, 12.5), p(5.0, 16.0), p(9.5, 12.5)], s));
            painter.rect_stroke(egui::Rect::from_min_max(p(9.0, 9.5), p(21.5, 18.0)), 3, s, egui::StrokeKind::Middle);
            painter.add(egui::Shape::line(vec![p(15.0, 18.0), p(18.5, 21.5), p(18.0, 18.0)], s));
        }
        Icon::Support => {
            // A question mark in a circle (the Tauri launcher's): not another speech bubble beside Chat.
            painter.circle_stroke(c, 8.0 * u, s);
            let hook: Vec<egui::Pos2> = (0..=10)
                .map(|i| {
                    let t = std::f32::consts::PI * (1.0 + 1.25 * i as f32 / 10.0);
                    p(12.0 + 2.5 * t.cos(), 10.0 + 2.5 * t.sin())
                })
                .collect();
            painter.add(egui::Shape::line(hook, s));
            painter.line_segment([p(12.4, 12.4), p(12.0, 14.2)], s);
            painter.circle_filled(p(12.0, 17.0), 1.1 * u, color);
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
    let (rect, response) = ui.allocate_exact_size(egui::vec2(42.0, 42.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(label);
    let fill = if selected {
        SELECTED
    } else if response.hovered() {
        CONTROL
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
    painter.rect_filled(rect, 8, fill);
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.left_top() + egui::vec2(0.0, 8.0), egui::vec2(2.0, rect.height() - 16.0)),
            1,
            ACCENT,
        );
    }
    paint_icon(painter, egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0)), icon, color);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label));
    response
}

/// Test helpers: a temporary game folder, and egui drawing with the launcher's theme (its fonts
/// are needed before anything with them is laid out, so the first frame only sets the theme).
#[cfg(all(test, feature = "ui-tests"))]
pub(crate) mod ui_test {
    use eframe::egui;

    pub fn game(name: &str) -> (std::path::PathBuf, crate::app::Game) {
        let dir = std::env::temp_dir().join(format!("fe-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (dir.clone(), crate::app::Game::open(dir))
    }

    /// A harness drawing `draw` in a panel, after a first frame that only sets the theme.
    pub fn harness<'a>(size: egui::Vec2, mut draw: impl FnMut(&mut egui::Ui) + 'a) -> egui_kittest::Harness<'a> {
        let mut themed = false;
        let mut h = egui_kittest::Harness::builder().with_size(size).build(move |ctx: &egui::Context| {
            if !themed {
                crate::theme::apply(ctx);
                themed = true;
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| draw(ui));
        });
        h.run();
        h
    }
}
