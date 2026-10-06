//! Asking once whether the game may send its diagnostics to the server (`send_diagnostics`;
//! the hooks DLL sends them, see `hooks_config::diagnostics`). Asked before the game first
//! runs with them: until the player answers, nothing is sent (the DLL's default is off).

use eframe::egui;
use setup::feedback::Private;

use crate::app::Notices;
use crate::app::Prefs;
use crate::theme;

/// Lines of the player's own last game shown as the example.
const EXAMPLE_LINES: usize = 12;

const TITLE: &str = "Help us fix connection problems";
const WHAT: &str = "5th Echelon now sends the server you play on a short diagnostic log while you play: the game's warnings, errors and network messages, and what it's doing (a few lines a minute at most). When something goes wrong (a join that fails, your connection dropping), the server can also ask for the 15 minutes of the game's log before it, at most once an hour; never the whole log. It lets the server's admins see why a join failed or a connection dropped on your side, not just theirs.";
const PRIVATE: &str = "Before anything leaves your PC, your PC's name, your Windows account name, your user folder and your internet address are removed. It goes only to the server you're playing on and is kept for at most 30 days.";
const LATER: &str = "You can change this any time in Settings › Feedback.";

/// The question, while it's open.
#[derive(Default)]
pub struct Ask {
    /// The example's lines, once asked for (redacted).
    example: Option<Vec<String>>,
    /// Whether the player answered, read once (not every frame).
    answered: Option<bool>,
}

impl Ask {
    /// Whether the player still has to answer (an install another tool manages isn't asked:
    /// it stays off).
    pub fn pending(game: Option<&crate::app::Game>) -> bool {
        game.is_some_and(|g| g.managed.is_none()) && !Prefs::diagnostics_asked()
    }

    /// Whether the question is up now (other windows wait for it).
    pub fn asking(&self) -> bool {
        self.answered == Some(false)
    }

    pub fn show(&mut self, ctx: &egui::Context, game: Option<&mut crate::app::Game>, notices: &mut Notices) {
        let Some(game) = game else { return };
        if game.managed.is_some() || *self.answered.get_or_insert_with(Prefs::diagnostics_asked) {
            return;
        }
        let mut answer = None;
        let middle = (ctx.content_rect().height() - 260.0).max(120.0);
        // Not closed by a click beside it: the game waits for the answer.
        let _ = egui::Modal::new(egui::Id::new("diagnostics")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.label(theme::heading(TITLE));
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .id_salt("diagnostics-middle")
                .max_height(middle)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.label(WHAT);
                    ui.add_space(6.0);
                    ui.label(PRIVATE);
                    ui.add_space(6.0);
                    ui.label(theme::muted(LATER));
                    if let Some(lines) = &self.example {
                        ui.add_space(8.0);
                        if lines.is_empty() {
                            ui.label(theme::muted("Nothing yet: your last game didn't write any lines like these."));
                        } else {
                            ui.label(theme::muted("From your last game, as it would be sent:"));
                            egui::ScrollArea::both()
                                .id_salt("diagnostics-example")
                                .max_height((middle * 0.6).min(220.0))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(lines.join("\n")).monospace().size(11.0));
                                });
                        }
                    }
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if self.example.is_none() && ui.add(theme::secondary("Show me an example")).clicked() {
                    self.example = Some(example(&game.dir));
                }
                // The two answers alike: neither is the one to pick.
                if ui.add(theme::secondary("Keep sending")).clicked() {
                    answer = Some(true);
                }
                if ui.add(theme::secondary("Turn it off")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(send) = answer {
            game.update(notices, |c| c.hook_config.send_diagnostics = send);
            Prefs::set_diagnostics_asked();
            self.answered = Some(true);
            self.example = None;
            notices.info(if send {
                "Thanks: the game will send its diagnostics. Settings › Feedback turns them off."
            } else {
                "The game won't send diagnostics. Settings › Feedback turns them on."
            });
        }
    }
}

/// The last lines of the player's own log that would have been sent, private parts hidden.
fn example(game_dir: &std::path::Path) -> Vec<String> {
    let private = Private::of_this_pc(Vec::new());
    ["bl-tracing.log", "bl-tracing.prev.log"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(game_dir.join(f)).ok())
        .map(|log| hooks_config::diagnostics::example(&log, EXAMPLE_LINES))
        .find(|lines| !lines.is_empty())
        .unwrap_or_default()
        .into_iter()
        .map(|l| setup::feedback::redact(&l, &private))
        .collect()
}
