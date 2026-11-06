//! New game options: the ones that change how the game plays, asked about once each.
//!
//! Each is off until the player chose, and offered with its recommended setting already
//! chosen (on for some, off for others). On a first run they're a step of the guided setup (`play.rs`);
//! after an update brings one, this asks in a window of its own before the game starts (after
//! the question about diagnostics, before What's new). Settings › Game changes them later. An
//! install another tool manages isn't asked: they stay off there.
//!
//! What was asked is noted in the game folder ([`ASKED_FILE`], beside `uplay.toml`, where the
//! choices go too): a reinstalled game or another copy starts without both, so it's asked
//! again, rather than quietly playing with every option off.

use std::path::Path;

#[cfg(feature = "gui")]
use eframe::egui;
#[cfg(feature = "gui")]
use eframe::egui::RichText;

use crate::activity::Activities as Notices;
use crate::prefs::Game;
use crate::prefs::Prefs;
#[cfg(feature = "gui")]
use crate::theme;

/// The game folder's note of the options asked about there.
pub const ASKED_FILE: &str = "5th-echelon-options.json";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct AskedHere {
    asked: Vec<String>,
}

/// The options asked about in this game folder; None when it has no note.
fn read_asked_here(dir: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(dir.join(ASKED_FILE)).ok()?;
    Some(serde_json::from_str::<AskedHere>(&text).map(|a| a.asked).unwrap_or_default())
}

fn write_asked_here(dir: &Path, asked: &[String]) {
    let note = AskedHere { asked: asked.to_vec() };
    if let Ok(text) = serde_json::to_string_pretty(&note) {
        if let Err(e) = std::fs::write(dir.join(ASKED_FILE), text) {
            tracing::warn!("new options: couldn't note them as asked in {} ({e})", dir.display());
        }
    }
}

/// What a folder counts as asked: its own note; else, the first time this version runs, the
/// launcher-wide list from before 0.5.0 (the folder in use then is the one it was asked for),
/// which is then written to the folder (`true`); else nothing.
fn asked_for(here: Option<Vec<String>>, moved: bool, launcher_wide: &[String]) -> (Vec<String>, bool) {
    match here {
        Some(asked) => (asked, false),
        None if !moved && !launcher_wide.is_empty() => (launcher_wide.to_vec(), true),
        None => (Vec::new(), false),
    }
}

/// The options asked about for `dir`, moving the launcher-wide list onto it the first time.
fn asked_in(dir: &Path) -> Vec<String> {
    let moved = Prefs::options_asked_moved();
    let (asked, write) = asked_for(read_asked_here(dir), moved, &Prefs::options_asked());
    if write {
        write_asked_here(dir, &asked);
    }
    if !moved {
        Prefs::set_options_asked_moved();
    }
    asked
}

pub struct NewOption {
    /// Noted in the game folder once the player chose; never reused.
    pub id: &'static str,
    pub title: &'static str,
    pub text: &'static str,
    pub recommended: bool,
    pub setting: Setting,
    /// Why it can't be used on this install right now, if it can't (it's then left off): a
    /// badge and a line.
    pub blocked: fn(&Path) -> Option<(&'static str, &'static str)>,
}

/// Where an option's choice goes.
pub enum Setting {
    /// A key of `uplay.toml` the hooks read when the game starts.
    Hook(fn(&mut hooks_config::Config, bool)),
    /// Something the launcher does itself (and notes), such as editing the game's own files.
    Launcher(fn(&mut Game, &mut Notices, bool)),
}

/// Oldest first. An option stays here once added: a player who skipped releases is asked
/// about every one they haven't been.
pub const OPTIONS: &[NewOption] = &[
    NewOption {
        id: "mouse-accel",
        title: "Mouse moves 1:1",
        text: "Fast flicks turn as far as slow ones: the game's negative mouse acceleration is gone. A wide limit stays, so a bad input spike can't flip the camera.",
        recommended: true,
        setting: Setting::Hook(|c, on| c.disable_mouse_accel = on),
        blocked: |dir| hooks_config::fusion_fix::removes_mouse_accel(dir).then_some(("Fusion Fix", FUSION_FIX_MOUSE)),
    },
    NewOption {
        id: "brighten",
        title: "Brighten the picture",
        text: if BRIGHTEN_WITH_VKBASALT {
            "The game's own brightness does nothing under Proton, so this brightens the picture with vkBasalt when the launcher starts the game: mid-tones lighter, black and white kept. Home turns it off and on in the game; Settings › Game sets how bright."
        } else {
            "The game caps its brightness low, which can look dark (under Proton especially). This lifts the cap and sets a brighter picture by editing Blacklist.ini; turning it off puts the original back. Settings › Game sets how bright."
        },
        recommended: false,
        setting: Setting::Launcher(set_brighten),
        blocked: |_| (BRIGHTEN_WITH_VKBASALT && vkbasalt_missing()).then_some(("Needs vkBasalt", VKBASALT_MISSING)),
    },
    NewOption {
        id: "autoclimb",
        title: "No automatic climbing",
        text: "Sam stops climbing and vaulting over edges by himself when you sprint into them (Active Sprint): climbing is only ever manual.",
        recommended: false,
        setting: Setting::Hook(|c, on| c.disable_autoclimb = on),
        blocked: |_| None,
    },
];

/// Shown for the mouse option while Fusion Fix removes the acceleration itself (here and in
/// Settings › Game).
pub const FUSION_FIX_MOUSE: &str = "Fusion Fix already removes it (DisableNegativeMouseAcceleration = 1 in SplinterCellBlacklist.FusionFix.ini). Set that to 0 to use this one: it keeps the spike limit, which matters under Proton.";

pub const INTRO_UPDATE: &str = "This update adds options that change how the game plays. Each is set as we recommend; tick or untick any you like.";
/// The same, for options the player already chose about for another game folder (a reinstall,
/// another copy): the update didn't bring them.
pub const INTRO_FOLDER: &str =
    "This copy of the game doesn't have your game options yet (a new install, or another copy). Each is set as we recommend; tick or untick any you like.";
pub const INTRO_SETUP: &str = "These change how the game plays. Each is set as we recommend; tick or untick any you like.";
pub const LATER: &str = "Change these any time in Settings › Game.";

/// The options in `options` the player hasn't been asked about (`asked`: their ids).
fn not_asked<'a>(options: &'a [NewOption], asked: &[String]) -> Vec<&'a NewOption> {
    options.iter().filter(|o| !asked.iter().any(|a| a == o.id)).collect()
}

/// The options this install still has to be asked about: none before the game is set up on a
/// server (the guided setup asks then), nor where another tool manages it.
pub fn pending(game: Option<&Game>) -> Vec<&'static NewOption> {
    match game {
        Some(g) if g.managed.is_none() && g.cfg.current_profile().is_some_and(|p| !p.server.is_empty()) => not_asked(OPTIONS, &asked_in(&g.dir)),
        _ => Vec::new(),
    }
}

/// The choices as first offered: the recommendation, or off where it can't be used.
pub fn offered(options: &[&NewOption], game_dir: &Path) -> Vec<bool> {
    options.iter().map(|o| o.recommended && (o.blocked)(game_dir).is_none()).collect()
}

#[cfg(feature = "gui")]
/// The options as cards with a switch each, `choices` in the same order.
pub fn list(ui: &mut egui::Ui, options: &[&NewOption], choices: &mut [bool], game_dir: &Path) {
    for (option, on) in options.iter().zip(choices.iter_mut()) {
        let blocked = (option.blocked)(game_dir);
        egui::Frame::new()
            .fill(theme::SUNKEN)
            .stroke(egui::Stroke::new(1.0, theme::LINE))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if blocked.is_some() {
                        *on = false;
                    }
                    ui.add_enabled_ui(blocked.is_none(), |ui| theme::switch(ui, on, RichText::new(option.title).family(theme::strong())));
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
            if let Setting::Hook(set) = option.setting {
                set(&mut c.hook_config, *on);
            }
        }
    });
    for (option, on) in options.iter().zip(choices) {
        // Off is how they start: only an option turned on has anything to do.
        if let (Setting::Launcher(set), true) = (&option.setting, *on) {
            set(game, notices, true);
        }
    }
    let mut asked = read_asked_here(&game.dir).unwrap_or_default();
    for option in options {
        if !asked.iter().any(|a| a == option.id) {
            asked.push(option.id.to_string());
        }
    }
    write_asked_here(&game.dir, &asked);
    // And as asked on this PC at all: only for how the question is worded (`intro`).
    Prefs::add_options_asked(options.iter().map(|o| o.id));
}

/// How the question after an update starts: the update brought these options, or (every one
/// already chosen about on this PC, for another game folder) this game folder lacks them.
pub fn intro(options: &[&NewOption]) -> &'static str {
    let ever = Prefs::options_asked();
    if !options.is_empty() && options.iter().all(|o| ever.iter().any(|a| a == o.id)) {
        INTRO_FOLDER
    } else {
        INTRO_UPDATE
    }
}

#[cfg(feature = "gui")]
/// The guided setup's step: the intro over the list.
pub fn setup_step(ui: &mut egui::Ui, options: &[&NewOption], choices: &mut [bool], game_dir: &Path) {
    ui.label(RichText::new(INTRO_SETUP).color(theme::SOFT));
    ui.add_space(14.0);
    list(ui, options, choices, game_dir);
}

#[cfg(feature = "gui")]
/// The window after an update, while it's open.
#[derive(Default)]
pub struct Ask {
    /// The options asked about and the choices so far, once it opened.
    open: Option<(Vec<&'static NewOption>, Vec<bool>)>,
    /// Nothing to ask (worked out once, not every frame).
    done: bool,
}

#[cfg(feature = "gui")]
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
            ui.label(RichText::new(intro(options)).color(theme::SOFT));
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

/// Whether "Brighten the picture" goes through vkBasalt (Linux: the game's own gamma does
/// nothing under Proton) rather than Blacklist.ini (Windows).
pub const BRIGHTEN_WITH_VKBASALT: bool = cfg!(target_os = "linux");

/// What "Brighten the picture" does, on this system.
pub const BRIGHTEN_HOVER: &str = if BRIGHTEN_WITH_VKBASALT {
    "Blacklist's own brightness does nothing under Proton. This brightens the picture with \
     vkBasalt, a Vulkan layer, when the launcher starts the game: the mid-tones get lighter, \
     black and white stay. In the game, Home turns it off and on."
} else {
    "Blacklist caps its brightness low, which looks dark under Proton. This lifts the cap and \
     sets a brighter picture by editing Blacklist.ini while the game is closed; turning it off \
     puts the original back."
};

/// Said where "Brighten the picture" can't be used on Linux.
pub const VKBASALT_MISSING: &str = "Install vkBasalt (32-bit) to use this: the game is 32-bit and needs vkBasalt's 32-bit layer (on Arch, lib32-vkbasalt).";

/// Whether vkBasalt's 32-bit layer is missing. Looked up at most every few seconds, so
/// installing it shows without a restart.
pub fn vkbasalt_missing() -> bool {
    static SEEN: std::sync::Mutex<Option<(std::time::Instant, bool)>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    match *seen {
        Some((at, missing)) if at.elapsed() < std::time::Duration::from_secs(5) => missing,
        _ => {
            let missing = setup::vkbasalt::installed().bits32.is_none();
            *seen = Some((std::time::Instant::now(), missing));
            missing
        }
    }
}

/// On Linux with "Brighten the picture" on: writes vkBasalt's colour table and config for the
/// current brightness and gives the game's environment to use them. Nothing otherwise, or
/// when vkBasalt's 32-bit layer is missing or the files can't be written (then a notice says so;
/// the game starts anyway).
pub fn brighten_env(game: &Game, notices: &mut Notices) -> Vec<(&'static str, std::ffi::OsString)> {
    if !BRIGHTEN_WITH_VKBASALT || !game.cfg.brighten {
        return Vec::new();
    }
    if vkbasalt_missing() {
        notices.error(format!("The picture isn't brightened. {VKBASALT_MISSING}"));
        return Vec::new();
    }
    let written = setup::vkbasalt::files_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home folder"))
        .and_then(|dir| setup::vkbasalt::write(&dir, game.cfg.brightness));
    match written {
        Ok(config) => {
            tracing::info!(
                "brightening with vkBasalt: {} (strength {:.2})",
                config.display(),
                setup::vkbasalt::strength(game.cfg.brightness)
            );
            setup::vkbasalt::env(&config)
        }
        Err(e) => {
            tracing::warn!("couldn't write vkBasalt's files: {e}");
            notices.error(format!("The picture isn't brightened: couldn't write vkBasalt's files ({e})."));
            Vec::new()
        }
    }
}

/// Turns "Brighten the picture" on or off, then notes it (here and in Settings › Game). On
/// Windows that edits Blacklist.ini (the game must be closed). On Linux turning it on changes no
/// file (vkBasalt is set up when the game starts); turning it off still puts back what an earlier
/// version changed in Blacklist.ini. On a failure the setting stays as it was.
pub fn set_brighten(game: &mut Game, notices: &mut Notices, on: bool) {
    if on && BRIGHTEN_WITH_VKBASALT && vkbasalt_missing() {
        notices.error(VKBASALT_MISSING);
        return;
    }
    let mut backup = game.cfg.gamma_backup.clone();
    let res = if on && BRIGHTEN_WITH_VKBASALT {
        Ok(())
    } else if on {
        setup::gamma::enable(&game.dir, game.cfg.brightness, &mut backup)
    } else {
        setup::gamma::disable(&game.dir, &mut backup)
    };
    match res {
        Ok(()) => game.update(notices, |c| {
            c.brighten = on;
            c.gamma_backup = backup;
        }),
        Err(e) => notices.error(format!("Couldn't change the brightness: {e}")),
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

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_folder_counts_what_was_asked_there() {
        let here = ids(&["mouse-accel"]);
        // Its own note wins, whatever the launcher kept from before.
        assert_eq!(asked_for(Some(here.clone()), false, &ids(&["mouse-accel", "brighten", "autoclimb"])), (here.clone(), false));
        assert_eq!(asked_for(Some(here.clone()), true, &[]), (here, false));
    }

    #[test]
    fn the_folder_in_use_when_0_5_0_first_runs_takes_the_old_list_over() {
        let old = ids(&["mouse-accel", "brighten", "autoclimb"]);
        // Nobody who answered before is asked again: the list is written to that folder.
        assert_eq!(asked_for(None, false, &old), (old.clone(), true));
        // Any folder after that without a note (a reinstall, another copy) is asked.
        assert_eq!(asked_for(None, true, &old), (Vec::new(), false));
        // A launcher that never asked: nothing to move.
        assert_eq!(asked_for(None, false, &[]), (Vec::new(), false));
    }

    #[test]
    fn the_note_reads_back_and_a_damaged_one_counts_as_nothing_asked() {
        let dir = std::env::temp_dir().join(format!("fe-options-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_asked_here(&dir), None);
        write_asked_here(&dir, &ids(&["autoclimb"]));
        assert_eq!(read_asked_here(&dir), Some(ids(&["autoclimb"])));
        std::fs::write(dir.join(ASKED_FILE), "not json").unwrap();
        assert_eq!(read_asked_here(&dir), Some(Vec::new()));
        let _ = std::fs::remove_dir_all(&dir);
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
        let Setting::Hook(set) = mouse.setting else { panic!("a uplay.toml key") };
        set(&mut c, true);
        assert!(c.disable_mouse_accel);
    }

    #[test]
    fn brightening_and_no_auto_climbing_are_offered_off() {
        let dir = std::env::temp_dir().join(format!("new-options-off-{}", std::process::id()));
        let options: Vec<&NewOption> = OPTIONS.iter().filter(|o| o.id == "brighten" || o.id == "autoclimb").collect();
        assert_eq!(options.len(), 2);
        assert_eq!(offered(&options, &dir), vec![false, false]);
        let c = hooks_config::default();
        assert!(!c.disable_autoclimb, "off until chosen");
        let autoclimb = options.iter().find(|o| o.id == "autoclimb").unwrap();
        let Setting::Hook(set) = autoclimb.setting else { panic!("a uplay.toml key") };
        let mut c = c;
        set(&mut c, true);
        assert!(c.disable_autoclimb);
    }
}
