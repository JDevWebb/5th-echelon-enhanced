//! New game options: the ones that change how the game plays, asked about once each.
//!
//! Each is off in `uplay.toml` until the player chose, and offered with its recommended
//! setting already chosen. On a first run they're a step of the guided setup (`play.rs`);
//! after an update brings one, this asks in a window of its own before the game starts (after
//! the question about diagnostics, before What's new). Settings › Game changes them later. An
//! install another tool manages isn't asked: they stay off there.

use std::path::Path;

use eframe::egui;
use eframe::egui::RichText;
use hooks_config::Config;

use crate::app::Game;
use crate::app::Notices;
use crate::app::Prefs;
use crate::theme;

pub struct NewOption {
    /// Noted in the launcher's prefs once the player chose; never reused.
    pub id: &'static str,
    pub title: &'static str,
    pub text: &'static str,
    pub recommended: bool,
    pub set: fn(&mut Config, bool),
    /// Why it can't be used on this install right now, if it can't (it's then left off): a
    /// badge and a line.
    pub blocked: fn(&Path) -> Option<(&'static str, &'static str)>,
}

/// Oldest first. An option stays here once added: a player who skipped releases is asked
/// about every one they haven't been.
pub const OPTIONS: &[NewOption] = &[NewOption {
    id: "mouse-accel",
    title: "Mouse moves 1:1",
    text: "Fast flicks turn as far as slow ones: the game's negative mouse acceleration is gone. A wide limit stays, so a bad input spike can't flip the camera.",
    recommended: true,
    set: |c, on| c.disable_mouse_accel = on,
    blocked: |dir| hooks_config::fusion_fix::removes_mouse_accel(dir).then_some(("Fusion Fix", FUSION_FIX_MOUSE)),
}];

/// Shown for the mouse option while Fusion Fix removes the acceleration itself (here and in
/// Settings › Game).
pub const FUSION_FIX_MOUSE: &str = "Fusion Fix already removes it (DisableNegativeMouseAcceleration = 1 in SplinterCellBlacklist.FusionFix.ini). Set that to 0 to use this one: it keeps the spike limit, which matters under Proton.";

const INTRO_UPDATE: &str = "This update adds options that change how the game plays. The recommended setting is already chosen; change any you don't want.";
const INTRO_SETUP: &str = "These change how the game plays. The recommended setting is already chosen; change any you don't want.";
const LATER: &str = "Change these any time in Settings › Game.";

/// The options in `options` the player hasn't been asked about (`asked`: their ids).
fn not_asked<'a>(options: &'a [NewOption], asked: &[String]) -> Vec<&'a NewOption> {
    options.iter().filter(|o| !asked.iter().any(|a| a == o.id)).collect()
}

/// The options this install still has to be asked about: none before the game is set up on a
/// server (the guided setup asks then), nor where another tool manages it.
pub fn pending(game: Option<&Game>) -> Vec<&'static NewOption> {
    match game {
        Some(g) if g.managed.is_none() && g.cfg.current_profile().is_some_and(|p| !p.server.is_empty()) => not_asked(OPTIONS, &Prefs::options_asked()),
        _ => Vec::new(),
    }
}

/// The choices as first offered: the recommendation, or off where it can't be used.
pub fn offered(options: &[&NewOption], game_dir: &Path) -> Vec<bool> {
    options.iter().map(|o| o.recommended && (o.blocked)(game_dir).is_none()).collect()
}

/// The options as cards with a checkbox each, `choices` in the same order.
pub fn list(ui: &mut egui::Ui, options: &[&NewOption], choices: &mut [bool], game_dir: &Path) {
    for (option, on) in options.iter().zip(choices.iter_mut()) {
        let blocked = (option.blocked)(game_dir);
        egui::Frame::new()
            .fill(theme::SUNKEN)
            .stroke(egui::Stroke::new(1.0, theme::LINE))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if blocked.is_some() {
                        *on = false;
                    }
                    ui.add_enabled(blocked.is_none(), egui::Checkbox::new(on, RichText::new(option.title).family(theme::strong())));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match blocked {
                        Some((badge, _)) => crate::play::badge(ui, badge, false),
                        None if option.recommended => crate::play::badge(ui, "Recommended", true),
                        None => {}
                    });
                });
                ui.add_space(2.0);
                ui.label(RichText::new(blocked.map_or(option.text, |(_, why)| why)).color(theme::SOFT));
            });
        ui.add_space(8.0);
    }
    ui.label(theme::muted(LATER));
}

/// Writes the choices and notes the options as asked.
pub fn save(game: &mut Game, notices: &mut Notices, options: &[&NewOption], choices: &[bool]) {
    game.update(notices, |c| {
        for (option, on) in options.iter().zip(choices) {
            (option.set)(&mut c.hook_config, *on);
        }
    });
    Prefs::add_options_asked(options.iter().map(|o| o.id));
}

/// The guided setup's step: the intro over the list.
pub fn setup_step(ui: &mut egui::Ui, options: &[&NewOption], choices: &mut [bool], game_dir: &Path) {
    ui.label(RichText::new(INTRO_SETUP).color(theme::SOFT));
    ui.add_space(14.0);
    list(ui, options, choices, game_dir);
}

/// The window after an update, while it's open.
#[derive(Default)]
pub struct Ask {
    /// The options asked about and the choices so far, once it opened.
    open: Option<(Vec<&'static NewOption>, Vec<bool>)>,
    /// Nothing to ask (worked out once, not every frame).
    done: bool,
}

impl Ask {
    /// Whether the window is up now (What's new waits for it).
    pub fn asking(&self) -> bool {
        self.open.is_some()
    }

    /// `waiting`: another question is up first, or the guided setup asks itself.
    pub fn show(&mut self, ctx: &egui::Context, game: Option<&mut Game>, notices: &mut Notices, waiting: bool) {
        let Some(game) = game else { return };
        if self.done || waiting {
            return;
        }
        let (options, choices) = self.open.get_or_insert_with(|| {
            let options = pending(Some(game));
            let choices = offered(&options, &game.dir);
            (options, choices)
        });
        if options.is_empty() {
            self.open = None;
            self.done = true;
            return;
        }
        let mut save_now = false;
        let middle = (ctx.content_rect().height() - 240.0).max(120.0);
        // Not closed by a click beside it: the game waits for the answer.
        let _ = egui::Modal::new(egui::Id::new("new-options")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.label(theme::heading("New game options"));
            ui.add_space(6.0);
            ui.label(RichText::new(INTRO_UPDATE).color(theme::SOFT));
            ui.add_space(12.0);
            egui::ScrollArea::vertical()
                .id_salt("new-options-list")
                .max_height(middle)
                .auto_shrink([false, true])
                .show(ui, |ui| list(ui, options, choices, &game.dir));
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                save_now = ui.add(theme::primary("Save choices")).clicked();
            });
        });
        if save_now {
            if let Some((options, choices)) = self.open.take() {
                save(game, notices, &options, &choices);
            }
            self.done = true;
            notices.info("Saved. Settings › Game changes them any time.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_options_not_asked_about_are_pending() {
        let all: Vec<&str> = not_asked(OPTIONS, &[]).iter().map(|o| o.id).collect();
        assert_eq!(all.len(), OPTIONS.len());
        assert!(not_asked(OPTIONS, &OPTIONS.iter().map(|o| o.id.to_string()).collect::<Vec<_>>()).is_empty());
        assert_eq!(not_asked(OPTIONS, &["something-else".into()]).len(), OPTIONS.len());
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = OPTIONS.iter().map(|o| o.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), OPTIONS.len());
    }

    #[test]
    fn the_recommendation_is_offered_unless_it_cant_be_used() {
        let dir = std::env::temp_dir().join(format!("new-options-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        let mouse: Vec<&NewOption> = OPTIONS.iter().filter(|o| o.id == "mouse-accel").collect();
        assert_eq!(offered(&mouse, &dir), vec![true]);
        // Fusion Fix with its own fix on: offered off.
        std::fs::write(dir.join("scripts/SplinterCellBlacklist.FusionFix.asi"), b"").unwrap();
        assert_eq!(offered(&mouse, &dir), vec![false]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_mouse_option_sets_its_key() {
        let mouse = OPTIONS.iter().find(|o| o.id == "mouse-accel").unwrap();
        let mut c = hooks_config::default();
        assert!(!c.disable_mouse_accel, "off until chosen");
        (mouse.set)(&mut c, true);
        assert!(c.disable_mouse_accel);
    }
}
