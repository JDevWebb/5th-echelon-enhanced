//! The window's own frame, in place of the system's (0.4.8): a slim Field ops title bar with the
//! launcher's lights and name, dragged to move the window and double-clicked to maximise it,
//! minimise, maximise and close as square buttons on its right, and the window resized from its
//! edges and corners. In gamescope (the Steam Deck's gaming mode) the launcher is full screen and
//! has none of it.

use eframe::egui;
use eframe::egui::Color32;
use eframe::egui::Stroke;

use crate::theme;

/// The title bar's height, in points.
const HEIGHT: f32 = 34.0;
/// How far in from the window's edge a press resizes it.
const GRIP: f32 = 6.0;

/// Whether the launcher draws its own frame (it asks for an undecorated window then, main.rs).
pub fn custom() -> bool {
    !crate::scale::gamescope()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Minimise,
    Maximise,
    Close,
}

/// The title bar, across the top of the window, above the side menu and the screens.
pub fn title_bar(ctx: &egui::Context) {
    if !custom() {
        return;
    }
    let maximised = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    egui::TopBottomPanel::top("title-bar")
        .exact_height(HEIGHT)
        .frame(egui::Frame::new().fill(theme::RAIL))
        .show_separator_line(false)
        .show(ctx, |ui| {
            let bar = ui.max_rect();
            ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, theme::LINE));
            // The whole bar moves the window; the buttons, added after it, are on top.
            let drag = ui.interact(bar, egui::Id::new("title-bar-drag"), egui::Sense::click_and_drag());
            if drag.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximised));
            } else if drag.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            let painter = ui.painter();
            let mut x = bar.left() + 18.0;
            for _ in 0..3 {
                let c = egui::pos2(x, bar.center().y);
                painter.circle_filled(c, 5.0, theme::ACCENT.gamma_multiply(0.12));
                painter.circle_filled(c, 2.6, theme::ACCENT);
                x += 9.0;
            }
            painter.text(
                egui::pos2(x + 8.0, bar.center().y),
                egui::Align2::LEFT_CENTER,
                format!("{} {}", env!("FE_PRODUCT"), env!("FE_RELEASE")).to_uppercase(),
                egui::FontId::monospace(11.0),
                theme::MUTED,
            );
            let mut right = bar.right();
            for control in [Control::Close, Control::Maximise, Control::Minimise] {
                let rect = egui::Rect::from_min_max(egui::pos2(right - 46.0, bar.top()), egui::pos2(right, bar.bottom()));
                right -= 46.0;
                if button(ui, rect, control, maximised).clicked() {
                    ctx.send_viewport_cmd(match control {
                        Control::Minimise => egui::ViewportCommand::Minimized(true),
                        Control::Maximise => egui::ViewportCommand::Maximized(!maximised),
                        Control::Close => egui::ViewportCommand::Close,
                    });
                }
            }
        });
}

/// One of the title bar's buttons: a square of the bar's height, its icon drawn; close turns red
/// under the pointer.
fn button(ui: &mut egui::Ui, rect: egui::Rect, control: Control, maximised: bool) -> egui::Response {
    let label = match control {
        Control::Minimise => "Minimise",
        Control::Maximise if maximised => "Restore",
        Control::Maximise => "Maximise",
        Control::Close => "Close",
    };
    let response = ui.interact(rect, egui::Id::new(("title-bar", label)), egui::Sense::click()).on_hover_text(label);
    let hovered = response.hovered();
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect, 0, if control == Control::Close { theme::BAD } else { theme::CONTROL_HOVER });
    }
    let color = if hovered && control == Control::Close {
        Color32::WHITE
    } else if hovered {
        theme::FG
    } else {
        theme::SOFT
    };
    let s = Stroke::new(1.2, color);
    let c = rect.center();
    match control {
        Control::Minimise => {
            painter.line_segment([c + egui::vec2(-5.0, 0.5), c + egui::vec2(5.0, 0.5)], s);
        }
        Control::Maximise if maximised => {
            // Two squares, the back one showing at the top right.
            painter.rect_stroke(
                egui::Rect::from_center_size(c + egui::vec2(-1.0, 1.0), egui::vec2(8.0, 8.0)),
                1,
                s,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([c + egui::vec2(-2.0, -4.0), c + egui::vec2(5.0, -4.0)], s);
            painter.line_segment([c + egui::vec2(5.0, -4.0), c + egui::vec2(5.0, 3.0)], s);
        }
        Control::Maximise => {
            painter.rect_stroke(egui::Rect::from_center_size(c, egui::vec2(10.0, 10.0)), 1, s, egui::StrokeKind::Middle);
        }
        Control::Close => {
            painter.line_segment([c + egui::vec2(-5.0, -5.0), c + egui::vec2(5.0, 5.0)], s);
            painter.line_segment([c + egui::vec2(-5.0, 5.0), c + egui::vec2(5.0, -5.0)], s);
        }
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
}

/// Resizing from the window's edges and corners (the system's frame did it before), and a thin
/// edge round the window. Not while it's maximised or full screen.
pub fn edges(ctx: &egui::Context) {
    if !custom() {
        return;
    }
    let (maximised, fullscreen) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
    let window = ctx.content_rect();
    if !maximised && !fullscreen {
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-edge"))).rect_stroke(
            window.shrink(0.5),
            0,
            Stroke::new(1.0, theme::CONTROL_LINE),
            egui::StrokeKind::Inside,
        );
    }
    if maximised || fullscreen {
        return;
    }
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    use egui::viewport::ResizeDirection as D;
    let (left, right) = (pos.x - window.left() < GRIP, window.right() - pos.x < GRIP);
    let (top, bottom) = (pos.y - window.top() < GRIP, window.bottom() - pos.y < GRIP);
    let direction = match (left, right, top, bottom) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (.., true) => D::South,
        _ => return,
    };
    ctx.set_cursor_icon(match direction {
        D::North | D::South => egui::CursorIcon::ResizeVertical,
        D::East | D::West => egui::CursorIcon::ResizeHorizontal,
        D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
        D::NorthEast | D::SouthWest => egui::CursorIcon::ResizeNeSw,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}
