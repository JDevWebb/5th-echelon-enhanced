//! What's new: once after the launcher updates, the main changes a player sees, and the
//! fixes they'd notice. Written by hand for each release (the release notes are longer, and
//! have the server side in them too), and built in, so it needs no network.
//!
//! A fresh install isn't shown it: the release is only noted. An install from before this
//! (no release noted yet, but set up with an account) is shown the current release. A player
//! who skipped releases sees the newest in full and the others' headlines. Development
//! builds never show it on their own. The version at the bottom of the side menu opens it
//! again.

use eframe::egui;
use eframe::egui::RichText;
use setup::update::Release as Version;

use crate::theme;

/// A change the player sees.
pub struct Item {
    pub title: &'static str,
    pub text: &'static str,
    /// Only on the community network: badged, and with a way there for a player elsewhere.
    pub community: bool,
}

/// A fix: what works now, and where it mattered.
pub struct Fix {
    pub lead: &'static str,
    pub rest: &'static str,
}

pub struct Release {
    pub version: &'static str,
    pub new: &'static [Item],
    pub fixed: &'static [Fix],
}

/// Newest first. A release with nothing a player would notice has no entry.
pub const RELEASES: &[Release] = &[Release {
    version: "0.4.2",
    new: &[
        Item {
            title: "Pick a server from the Play screen",
            text: "The server card lists the community servers with your ping. One click switches, with your name and friends.",
            community: true,
        },
        Item {
            title: "Know when your server goes down",
            text: "For an update or booked maintenance, a notice under Play says when, in your time zone, and offers a server that's up. The game warns you 10 minutes before.",
            community: false,
        },
        Item {
            title: "Know when other players can't reach you",
            text: "If the server stops hearing from your game, the overlay says so, with what to do. F5 shows your ping to the server.",
            community: false,
        },
        Item {
            title: "Your account is made for you",
            text: "On a community server you haven't played on, with the name you use everywhere.",
            community: true,
        },
        Item {
            title: "The roadmap, and your ideas",
            text: "See what's coming under Roadmap, and send a suggestion. The admins' answer shows there too.",
            community: false,
        },
        Item {
            title: "One bar for everything that takes a while",
            text: "Switching servers, installing, updates and reports show along the bottom. Anything that fails stays, with Try again.",
            community: false,
        },
    ],
    fixed: &[
        Fix {
            lead: "Reports with logs get through",
            rest: ", even from far away.",
        },
        Fix {
            lead: "The overlay's mouse works",
            rest: " outside the SMI map (F5).",
        },
        Fix {
            lead: "You can still find a server",
            rest: " when the server list is down.",
        },
        Fix {
            lead: "Bans and sign-in limits say why",
            rest: ", not \"Error when sending request\".",
        },
        Fix {
            lead: "No dead lobbies after a drop",
            rest: ": friends see only the lobby your game is in now.",
        },
        Fix {
            lead: "Faraway servers get longer to answer",
            rest: ", instead of a port error.",
        },
    ],
}];

/// Older releases shown under the newest, for a player who skipped some.
const OLDER_SHOWN: usize = 2;
/// Headlines shown for each of them.
const OLDER_HEADLINES: usize = 3;

/// What to do on start: the releases to show (newest first), and whether to note the
/// running release as seen without showing anything.
#[derive(Debug, PartialEq)]
enum Start {
    Show(Vec<&'static str>),
    Note,
    Nothing,
}

/// `seen`: the release last shown or noted, if any; `set_up`: the game has an account (so
/// this isn't a fresh install).
fn on_start(releases: &'static [Release], seen: Option<&str>, current: &str, set_up: bool) -> Start {
    let Some(current) = Version::parse(current) else { return Start::Nothing };
    if current.pre.is_some() {
        return Start::Nothing;
    }
    let seen = match seen.and_then(Version::parse) {
        Some(seen) => seen,
        // Installed before releases were noted: the current one only.
        None if set_up => Version { numbers: (0, 0, 0), pre: None },
        None => return Start::Note,
    };
    if !current.newer_than(&seen) {
        return Start::Nothing;
    }
    let mut due = releases
        .iter()
        .filter(|r| Version::parse(r.version).is_some_and(|v| v.newer_than(&seen) && !v.newer_than(&current)));
    let Some(newest) = due.next() else { return Start::Note };
    let skipped = seen.numbers != (0, 0, 0);
    let mut show = vec![newest.version];
    if skipped {
        show.extend(due.take(OLDER_SHOWN).map(|r| r.version));
    }
    Start::Show(show)
}

fn release(version: &str) -> Option<&'static Release> {
    RELEASES.iter().find(|r| r.version == version)
}

/// What the player asked for in the window.
pub enum Action {
    /// Back to the community network, for a community-only change.
    UseCommunityNetwork,
}

#[derive(Default)]
pub struct WhatsNew {
    /// Whether the start was dealt with (once a game is open).
    started: bool,
    /// The releases shown, newest first.
    open: Vec<&'static Release>,
    /// The release updated from, and to (this launcher's).
    from: Option<String>,
    to: String,
    /// Noted as seen when it closes: shown on start, not opened again by the player.
    note: bool,
}

impl WhatsNew {
    /// Decides once, when the game is open, whether to show what's new.
    pub fn start(&mut self, game: Option<&crate::app::Game>) {
        let Some(game) = game else { return };
        if std::mem::replace(&mut self.started, true) {
            return;
        }
        let seen = crate::app::Prefs::whats_new_seen();
        let current = env!("FE_RELEASE");
        match on_start(RELEASES, seen.as_deref(), current, !game.cfg.profiles.is_empty()) {
            Start::Show(versions) => {
                self.open = versions.into_iter().filter_map(release).collect();
                self.from = seen;
                self.to = current.to_string();
                self.note = true;
            }
            Start::Note => crate::app::Prefs::set_whats_new_seen(&current),
            Start::Nothing => {}
        }
    }

    /// Opens it again for this launcher's release (the version in the side menu), or
    /// says there's nothing to show.
    pub fn reopen(&mut self) -> bool {
        let version = env!("FE_RELEASE").split('-').next().unwrap_or_default();
        self.open = release(version).into_iter().collect();
        self.from = None;
        self.to = version.to_string();
        self.note = false;
        !self.open.is_empty()
    }

    /// Draws it while it's open. `waiting`: another question is up first (the game's
    /// diagnostics); `community`: the player uses the community network.
    pub fn show(&mut self, ctx: &egui::Context, waiting: bool, community: bool) -> Option<Action> {
        if self.open.is_empty() || waiting {
            return None;
        }
        let mut action = None;
        let mut close = false;
        let middle = (ctx.content_rect().height() - 230.0).max(140.0);
        let modal = egui::Modal::new(egui::Id::new("whats-new")).show(ctx, |ui| {
            ui.set_width(520.0_f32.min(ctx.content_rect().width() - 80.0));
            ui.horizontal(|ui| {
                ui.label(theme::caps("Updated"));
                let versions = match &self.from {
                    Some(from) => format!("{from} → {}", self.to),
                    None => self.to.clone(),
                };
                ui.label(RichText::new(versions).monospace().size(12.0).color(theme::SOFT));
            });
            ui.label(theme::display("What's new", 30.0));
            ui.add_space(10.0);
            egui::ScrollArea::vertical()
                .id_salt("whats-new-middle")
                .max_height(middle)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 12.0;
                    let newest = self.open[0];
                    let skipped = self.open.len() > 1;
                    if skipped {
                        version_caps(ui, "In", newest.version, None);
                    } else {
                        ui.label(theme::caps("New"));
                    }
                    // Off the community network, the way there once, under its first change.
                    let mut offer = !community;
                    for item in newest.new {
                        let here = offer && item.community;
                        offer &= !here;
                        if new_item(ui, item, here) {
                            action = Some(Action::UseCommunityNetwork);
                        }
                    }
                    if skipped {
                        if let Some(first) = newest.fixed.first() {
                            let n = newest.fixed.len();
                            let fixes = if n == 1 { "1 fix".to_string() } else { format!("{n} fixes") };
                            fix_line(ui, &format!("Plus {fixes}, among them "), first.lead, ".");
                        }
                        ui.add_space(4.0);
                        divider(ui);
                        for older in &self.open[1..] {
                            version_caps(ui, "And in", older.version, Some("which you skipped"));
                            for item in older.new.iter().take(OLDER_HEADLINES) {
                                headline(ui, item.title);
                            }
                        }
                    } else if !newest.fixed.is_empty() {
                        ui.add_space(4.0);
                        ui.label(theme::caps("Fixed"));
                        ui.spacing_mut().item_spacing.y = 8.0;
                        for fix in newest.fixed {
                            fix_line(ui, "", fix.lead, fix.rest);
                        }
                    }
                });
            ui.add_space(12.0);
            divider(ui);
            ui.horizontal(|ui| {
                let (label, link) = if self.open.len() > 1 {
                    ("All release notes", crate::updater::RELEASES_PAGE.to_string())
                } else {
                    ("Full release notes", format!("{}/tag/v{}", crate::updater::RELEASES_PAGE, self.open[0].version))
                };
                ui.hyperlink_to(label, link);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(theme::primary("Got it").min_size(egui::vec2(120.0, 40.0))).clicked() {
                        close = true;
                    }
                });
            });
        });
        if close || modal.should_close() || action.is_some() {
            if std::mem::take(&mut self.note) {
                crate::app::Prefs::set_whats_new_seen(&self.to);
            }
            self.open.clear();
        }
        action
    }
}

/// "IN 0.4.2", "AND IN 0.4.1, WHICH YOU SKIPPED".
fn version_caps(ui: &mut egui::Ui, before: &str, version: &str, after: Option<&str>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(theme::caps(before));
        ui.label(RichText::new(version).monospace().size(12.0).color(theme::SOFT));
        if let Some(after) = after {
            ui.label(theme::caps(&format!(", {after}")));
        }
    });
}

fn divider(ui: &mut egui::Ui) {
    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 9.0), egui::Sense::hover());
    ui.painter().hline(line.x_range(), line.center().y, egui::Stroke::new(1.0, theme::LINE));
}

/// A bullet: the accent dot new things have.
fn dot(ui: &mut egui::Ui, top: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, top + 8.0), egui::Sense::hover());
    ui.painter().circle_filled(egui::pos2(rect.left() + 3.5, rect.top() + top + 3.5), 3.5, theme::ACCENT);
}

/// A tick: what fixes have.
fn tick(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 18.0), egui::Sense::hover());
    let p = |x: f32, y: f32| egui::pos2(rect.left() + x, rect.top() + 4.0 + y);
    let s = egui::Stroke::new(2.0, theme::OK);
    ui.painter().line_segment([p(2.5, 7.5), p(5.5, 10.5)], s);
    ui.painter().line_segment([p(5.5, 10.5), p(11.5, 3.5)], s);
}

/// "COMMUNITY NETWORK", beside a change only the community network has.
fn community_badge(ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(theme::ACCENT.linear_multiply(0.14))
        .stroke(egui::Stroke::new(1.0, theme::ACCENT.linear_multiply(0.35)))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(
                RichText::new("COMMUNITY NETWORK")
                    .family(theme::strong())
                    .size(10.5)
                    .extra_letter_spacing(0.8)
                    .color(theme::ACCENT),
            );
        });
}

/// A new thing, with its line, and with `offer`, the way to the community network. Says
/// whether the player took it.
fn new_item(ui: &mut egui::Ui, item: &Item, offer: bool) -> bool {
    let mut switch = false;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        dot(ui, 7.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(item.title).family(theme::strong()));
                if item.community {
                    community_badge(ui);
                }
            });
            ui.label(RichText::new(item.text).size(14.0).color(theme::SOFT));
            if offer {
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(theme::SUNKEN)
                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        // Clear of the scroll bar.
                        ui.set_width(ui.available_width() - 14.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(theme::muted("Your launcher uses another network, which doesn't have these.").size(13.5));
                            if ui.link(RichText::new("Switch to the community network ›").color(theme::ACCENT).size(13.5)).clicked() {
                                switch = true;
                            }
                        });
                    });
            }
        });
    });
    switch
}

/// A fix: `before`, the lead in bold, then `rest`.
fn fix_line(ui: &mut egui::Ui, before: &str, lead: &str, rest: &str) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        tick(ui);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if !before.is_empty() {
                ui.label(RichText::new(before).size(14.0).color(theme::SOFT));
            }
            ui.label(RichText::new(lead).size(14.0).family(theme::strong()));
            ui.label(RichText::new(rest).size(14.0).color(theme::SOFT));
        });
    });
}

/// An older release's headline.
fn headline(ui: &mut egui::Ui, title: &str) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        dot(ui, 6.0);
        ui.label(RichText::new(title).size(14.0).family(theme::strong()));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM: Item = Item {
        title: "A",
        text: "a",
        community: false,
    };
    static TEST: &[Release] = &[
        Release {
            version: "0.4.4",
            new: &[ITEM],
            fixed: &[],
        },
        Release {
            version: "0.4.3",
            new: &[ITEM],
            fixed: &[],
        },
        Release {
            version: "0.4.1",
            new: &[ITEM],
            fixed: &[],
        },
        Release {
            version: "0.4.0",
            new: &[ITEM],
            fixed: &[],
        },
    ];

    #[test]
    fn once_after_an_update() {
        assert_eq!(on_start(TEST, Some("0.4.3"), "0.4.4", true), Start::Show(vec!["0.4.4"]));
        // Seen already, or not newer.
        assert_eq!(on_start(TEST, Some("0.4.4"), "0.4.4", true), Start::Nothing);
        assert_eq!(on_start(TEST, Some("0.5.0"), "0.4.4", true), Start::Nothing);
    }

    #[test]
    fn a_fresh_install_only_notes_the_release() {
        assert_eq!(on_start(TEST, None, "0.4.4", false), Start::Note);
    }

    #[test]
    fn an_install_from_before_sees_the_current_release_only() {
        assert_eq!(on_start(TEST, None, "0.4.4", true), Start::Show(vec!["0.4.4"]));
    }

    #[test]
    fn skipped_releases_follow_the_newest_up_to_two() {
        assert_eq!(on_start(TEST, Some("0.4.1"), "0.4.4", true), Start::Show(vec!["0.4.4", "0.4.3"]));
        assert_eq!(on_start(TEST, Some("0.3.9"), "0.4.4", true), Start::Show(vec!["0.4.4", "0.4.3", "0.4.1"]));
    }

    #[test]
    fn a_release_without_an_entry_is_noted_and_the_last_one_with_one_is_shown() {
        // 0.4.2 has none: an update to it shows nothing new, only notes it.
        assert_eq!(on_start(TEST, Some("0.4.1"), "0.4.2", true), Start::Note);
        // Skipped past it: the newest with an entry, not newer than this launcher.
        assert_eq!(on_start(TEST, Some("0.4.0"), "0.4.3", true), Start::Show(vec!["0.4.3", "0.4.1"]));
    }

    #[test]
    fn development_builds_never_show_it() {
        assert_eq!(on_start(TEST, Some("0.4.1"), "0.4.4-dev", true), Start::Nothing);
        assert_eq!(on_start(TEST, None, "0.4.4-dev", false), Start::Nothing);
    }

    #[test]
    fn every_release_parses_and_they_are_newest_first() {
        let versions: Vec<Version> = RELEASES.iter().map(|r| Version::parse(r.version).expect(r.version)).collect();
        assert!(versions.iter().all(|v| v.pre.is_none()));
        assert!(versions.windows(2).all(|w| w[0].newer_than(&w[1])));
        assert!(RELEASES.iter().all(|r| !r.new.is_empty()));
    }
}
