//! The launcher's size. The window opens at [`DESIGN`] points (smaller on a small screen) and
//! can be resized, maximised or made full screen (F11); the screens lay themselves out for the
//! space they get, at the system's text size, down to [`MIN_WINDOW`]. On top of that comes the
//! player's own size (Settings › Display, or Ctrl + / Ctrl − / Ctrl 0).

use eframe::egui;

/// The window's first size, in points (1120x760 until 0.4.8).
pub const DESIGN: egui::Vec2 = egui::vec2(1280.0, 800.0);
/// The player's size: from this...
pub const MIN_SIZE: f32 = 0.7;
/// ...to this, in steps of [`STEP`].
pub const MAX_SIZE: f32 = 1.6;
const STEP: f32 = 0.1;
/// The window starts no larger than this share of the screen.
const OF_SCREEN: f32 = 0.9;

/// Whether the launcher runs inside gamescope (the Steam Deck's gaming
/// mode). Gamescope stretches a window smaller than the screen to fill it
/// but hands the pointer over unstretched, so clicks land off target, worse
/// the further right and down; the launcher runs full screen there instead.
pub fn gamescope() -> bool {
    std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some() || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.eq_ignore_ascii_case("gamescope"))
}

/// A size the player chose, kept within range and to a step.
pub fn clamp(size: f32) -> f32 {
    if !size.is_finite() {
        return 1.0;
    }
    ((size / STEP).round() * STEP).clamp(MIN_SIZE, MAX_SIZE)
}

/// The zoom for the player's `size`: the screens lay themselves out at real size for whatever
/// window they get, with text as big as the system's scaling says (egui's points), times the
/// player's own size. Until 0.4.8 the design was zoomed to fit the window, so a small window
/// shrank the text and a big one blew it up.
pub fn zoom_for(size: f32) -> f32 {
    if size.is_finite() && size > 0.0 {
        size.clamp(MIN_SIZE, MAX_SIZE)
    } else {
        1.0
    }
}

/// The smallest window the screens are laid out for, in points; below it they scroll.
pub const MIN_WINDOW: egui::Vec2 = egui::vec2(800.0, 560.0);

/// The window's first size: the design's, shrunk (keeping its shape) to fit a
/// screen of `monitor` points, or None when it fits already.
pub fn first_size(window: egui::Vec2, monitor: egui::Vec2) -> Option<egui::Vec2> {
    if monitor.x <= 0.0 || monitor.y <= 0.0 || (window.x <= monitor.x * OF_SCREEN && window.y <= monitor.y * OF_SCREEN) {
        return None;
    }
    let shrink = (monitor.x * OF_SCREEN / DESIGN.x).min(monitor.y * OF_SCREEN / DESIGN.y);
    Some(DESIGN * shrink)
}

/// Per frame: the zoom that fits the window, the window's first size on a
/// small screen, and the size shortcuts. Returns the player's new size when a
/// shortcut changed it.
pub fn apply(ctx: &egui::Context, size: f32, sized: &mut bool) -> Option<f32> {
    let native = ctx.native_pixels_per_point().unwrap_or(1.0);
    if !*sized && gamescope() {
        // Full screen from the start (main.rs): nothing to fit.
        *sized = true;
    }
    if !*sized {
        // The screen's size arrives within the first frames.
        let (inner, monitor) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().monitor_size));
        if let (Some(inner), Some(monitor)) = (inner, monitor) {
            *sized = true;
            // In the screen's own points, whatever the zoom is now.
            let per = ctx.pixels_per_point() / native;
            if let Some(new) = first_size(inner.size() * per, monitor * per) {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(new));
            }
        }
    }

    let changed = shortcut(ctx, size);
    let size = changed.unwrap_or(size);
    let zoom = zoom_for(size);
    if (ctx.zoom_factor() - zoom).abs() > 0.002 {
        ctx.set_zoom_factor(zoom);
    }
    changed
}

/// Ctrl + and Ctrl − change the player's size, Ctrl 0 puts it back; F11
/// toggles full screen.
fn shortcut(ctx: &egui::Context, size: f32) -> Option<f32> {
    use egui::Key;
    use egui::KeyboardShortcut;
    use egui::Modifiers;
    let pressed = |key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, key)));
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F11)) {
        let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
    }
    if pressed(Key::Plus) || pressed(Key::Equals) {
        Some(clamp(size + STEP))
    } else if pressed(Key::Minus) {
        Some(clamp(size - STEP))
    } else if pressed(Key::Num0) {
        Some(1.0)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zoom_is_the_players_size_whatever_the_window() {
        // Real size: no zoom at 100 %, however big or small the window is.
        assert!((zoom_for(1.0) - 1.0).abs() < 1e-6);
        // The player's size on top, within its range.
        assert!((zoom_for(1.3) - 1.3).abs() < 1e-6);
        assert!((zoom_for(9.0) - MAX_SIZE).abs() < 1e-6);
        // Nothing sensible: 100 %.
        assert_eq!(zoom_for(f32::NAN), 1.0);
    }

    #[test]
    fn the_first_window_fits_the_screen() {
        assert_eq!(first_size(DESIGN, egui::vec2(1920.0, 1080.0)), None);
        // 1920×1080 at 150 % is 1280×720 points: shrunk to fit, keeping the shape.
        let s = first_size(DESIGN, egui::vec2(1280.0, 720.0)).expect("too big for it");
        assert!(s.y <= 720.0 * OF_SCREEN + 0.01 && s.x <= 1280.0);
        assert!((s.x / s.y - DESIGN.x / DESIGN.y).abs() < 1e-3);
    }

    #[test]
    fn sizes_stay_in_range() {
        assert_eq!(clamp(5.0), MAX_SIZE);
        assert_eq!(clamp(0.1), MIN_SIZE);
        assert!((clamp(1.04) - 1.0).abs() < 1e-6);
        assert_eq!(clamp(f32::NAN), 1.0);
    }
}
