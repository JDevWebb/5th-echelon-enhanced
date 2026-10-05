//! A release going out to the network's servers (the coordinator's rollout, in its
//! directory): a notice under the launch bar saying when the player's server updates and
//! is back, in the player's own time, and what to do meanwhile.

use eframe::egui;
use eframe::egui::RichText;
use setup::clock::Clock;
use setup::directory::Listing;
use setup::directory::Rollout;
use setup::directory::Stage;

use crate::theme;

/// About how long a server is down while it installs a release and restarts.
const RESTART: i64 = 120;

/// What the notice is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The release is being tested, or going out; the player's server hasn't updated yet.
    Updating,
    /// The player's server is installing it and restarting.
    Restarting,
    /// The player's server runs a newer release than this launcher, and turns its game away.
    LauncherBehind,
}

/// What a server is at, for its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Waiting,
    Now,
    Done,
}

/// One server in the notice.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub place: String,
    pub mine: bool,
    /// "0.4.1", or "0.4.1 → 0.4.2" while it installs.
    pub version: String,
    pub doing: String,
    /// How far the stage it's in has got (0 to 1).
    pub meter: Option<f32>,
    pub tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    UpdateLauncher,
    PlayOn { host: String, place: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub kind: Kind,
    pub heading: String,
    /// The time that matters, beside the heading.
    pub eta: String,
    pub text: String,
    /// "Launcher 0.4.2 ready", beside the button.
    pub note: Option<String>,
    pub action: Option<(Action, String)>,
    pub servers: Vec<Row>,
    /// What the launch bar says instead of how ready the player is.
    pub status: String,
    /// Why Play waits, when it does.
    pub blocks_play: Option<String>,
}

impl Notice {
    /// How often the directory is read again while this shows: every 15 s while the
    /// player's server restarts, else every minute.
    pub fn refresh_every(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if self.kind == Kind::Restarting { 15 } else { 60 })
    }
}

/// What the notice is worked out from.
pub struct Input<'a> {
    pub rollout: Option<&'a Rollout>,
    /// The network's servers, with this PC's ping to each (None: no answer).
    pub servers: &'a [(Listing, Option<u32>)],
    /// The player's server: its listing, with its ping now (None when it stopped answering,
    /// or left the directory while it restarts).
    pub mine: Option<(&'a Listing, Option<u32>)>,
    /// This launcher's release, and a newer one if there is.
    pub launcher: &'a str,
    pub newer_launcher: Option<&'a str>,
    pub now: i64,
    pub clock: &'a Clock,
}

/// major.minor.patch, as servers compare clients (a `-dev` build counts as its release).
fn release(version: &str) -> Option<[u32; 3]> {
    let mut parts = version.trim().split('-').next()?.split('.').map(|p| p.parse().ok());
    let v = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(v)
}

fn place(s: &Listing) -> String {
    if s.region.is_empty() {
        s.name.clone()
    } else {
        s.region.clone()
    }
}

/// The city alone ("Sydney" of "Sydney, Australia"), for the short lines.
fn city(place: &str) -> &str {
    place.split(',').next().unwrap_or(place).trim()
}

fn players(n: u32) -> String {
    if n == 1 {
        "1 player".into()
    } else {
        format!("{n} players")
    }
}

/// The notice to show, if any.
pub fn notice(input: &Input) -> Option<Notice> {
    let (mine, my_ping) = input.mine?;
    let my_place = place(mine);
    let my_city = city(&my_place).to_string();
    // The player's server turns away launchers older than its release.
    if let (Some(server), Some(launcher)) = (release(&mine.version), release(input.launcher)) {
        if server > launcher {
            return Some(Notice {
                kind: Kind::LauncherBehind,
                heading: "Your server runs a newer version".into(),
                eta: format!("launcher {} → {}", input.launcher.split('-').next().unwrap_or(input.launcher), mine.version),
                text: format!(
                    "{my_city} now runs {} and turns away older games. Updating takes under a minute: the launcher restarts and puts the new version in the game.",
                    mine.version
                ),
                note: None,
                action: Some((Action::UpdateLauncher, "Update and restart".into())),
                servers: vec![],
                status: format!("Update the launcher to play: {my_city} runs {}", mine.version),
                blocks_play: Some(format!("Update the launcher first: {my_city} runs {}", mine.version)),
            });
        }
    }
    let rollout = input.rollout?;
    let target = rollout.release.as_str();
    if mine.version == target {
        return None;
    }
    let now = input.now;
    let clock = input.clock;
    let is_canary = |s: &Listing| rollout.canary.as_deref() == Some(s.id.as_str());
    // When the test ends and the rest of the servers are told to update.
    let testing_ends = match rollout.stage {
        Stage::Canary => (rollout.stage_started + RESTART).max(now) + rollout.healthy_for,
        Stage::Verifying => rollout.stage_started + rollout.healthy_for,
        Stage::Rolling => rollout.stage_started,
    };
    let latest = testing_ends + rollout.quiet_wait;
    let mine_due = rollout.stage == Stage::Rolling || (is_canary(mine) && rollout.stage == Stage::Canary);

    let rows = rows(input, rollout, testing_ends, latest);
    let update_note = input.newer_launcher.map(|v| format!("Launcher {v} ready"));
    let update_action = input.newer_launcher.map(|_| (Action::UpdateLauncher, "Update launcher".to_string()));

    // Due, and not answering: it's restarting.
    if mine_due && my_ping.is_none() {
        let alternative = input
            .servers
            .iter()
            .filter(|(s, ping)| s.version == target && ping.is_some() && s.host != mine.host)
            .min_by_key(|(_, ping)| *ping)
            .map(|(s, ping)| (s.host.clone(), place(s), ping.unwrap_or_default()));
        let mut text = format!("{my_city} is installing {target} and restarting. Wait here: the launcher checks every 15 seconds and says when it's back.");
        if let Some((_, alt, ms)) = &alternative {
            text.push_str(&format!(" Or play now on {}, which already runs {target} ({ms} ms).", city(alt)));
        }
        return Some(Notice {
            kind: Kind::Restarting,
            heading: "Your server is updating".into(),
            eta: format!("back {}", setup::clock::when(clock, now + RESTART, now).replace(", about", " · about")),
            text,
            note: None,
            action: alternative.map(|(host, alt, _)| {
                let label = format!("Play on {}", city(&alt));
                (Action::PlayOn { host, place: alt }, label)
            }),
            servers: rows,
            status: format!("{my_city} is restarting with {target}"),
            blocks_play: Some(format!("{my_city} is restarting with {target}")),
        });
    }

    let (eta, when_mine) = if mine.players_online > 0 && !(is_canary(mine) && rollout.stage == Stage::Canary) {
        (
            format!("{my_city} by {} at the latest", clock.time(latest, now)),
            format!("once nobody is playing on it, by {} at the latest", clock.time(latest, now)),
        )
    } else {
        let at = if mine_due { now + RESTART } else { testing_ends + RESTART };
        let rel = setup::clock::from_now(at - now);
        (format!("{my_city} {rel} · about {}", clock.time(at, now)), rel)
    };
    let text = match rollout.stage {
        Stage::Canary | Stage::Verifying => format!(
            "{target} is being tested on one server first. {my_city} updates after that ({when_mine}) and is down for about two minutes. A game you're in isn't cut off: servers with players on wait until they're empty, for up to {} hours.",
            rollout.quiet_wait / 3600
        ),
        Stage::Rolling => format!(
            "{target} passed its test and is going out to every server. {my_city} updates {when_mine}, and is down for about two minutes. You can keep playing until then."
        ),
    };
    Some(Notice {
        kind: Kind::Updating,
        heading: "Servers are being updated".into(),
        eta,
        text,
        note: update_note,
        action: update_action,
        servers: rows,
        status: format!("Servers updating to {target} · you can play until {my_city} restarts"),
        blocks_play: None,
    })
}

/// Each server: what it runs, what's happening to it, and how far along.
fn rows(input: &Input, rollout: &Rollout, testing_ends: i64, latest: i64) -> Vec<Row> {
    let (now, clock, target) = (input.now, input.clock, rollout.release.as_str());
    let mine = input.mine.map(|(s, _)| s.host.as_str());
    let is_canary = |s: &Listing| rollout.canary.as_deref() == Some(s.id.as_str());
    let fraction = |from: i64, length: i64| (length > 0).then(|| ((now - from) as f32 / length as f32).clamp(0.0, 1.0));
    let mut servers: Vec<(Listing, Option<u32>)> = input.servers.to_vec();
    if let Some((m, ping)) = input.mine.filter(|(m, _)| !servers.iter().any(|(s, _)| s.host == m.host)) {
        servers.push((m.clone(), ping));
    }
    // The server it's tested on, then the player's, then the rest.
    servers.sort_by_key(|(s, _)| (!is_canary(s), Some(s.host.as_str()) != mine));
    servers
        .iter()
        .take(6)
        .map(|(s, ping)| {
            // The player's server as it answers now.
            let ping = match input.mine {
                Some((m, now_ping)) if m.host == s.host => now_ping,
                _ => *ping,
            };
            let on = s.version == target;
            let due = rollout.stage == Stage::Rolling || (is_canary(s) && rollout.stage == Stage::Canary);
            let installing = format!("{} → {target}", if s.version.is_empty() { "?" } else { s.version.as_str() });
            let (version, doing, meter, tone) = if on && is_canary(s) && rollout.stage == Stage::Verifying {
                let left = (rollout.stage_started + rollout.healthy_for - now).max(0);
                (
                    target.to_string(),
                    format!("Testing the new release · {} min left", (left + 59) / 60),
                    fraction(rollout.stage_started, rollout.healthy_for),
                    Tone::Now,
                )
            } else if on {
                (target.to_string(), "Updated · open".into(), None, Tone::Done)
            } else if due && ping.is_none() {
                (installing, "Restarting · usually two minutes".into(), None, Tone::Now)
            } else if is_canary(s) && rollout.stage == Stage::Canary {
                (installing, "Installing the new release, to test it".into(), None, Tone::Now)
            } else if s.players_online > 0 {
                let doing = if rollout.stage == Stage::Rolling {
                    format!("Waiting for its {} to leave · by {} at the latest", players(s.players_online), clock.time(latest, now))
                } else {
                    format!("Next · {} on: once they leave, by {} at the latest", players(s.players_online), clock.time(latest, now))
                };
                let meter = (rollout.stage == Stage::Rolling).then(|| fraction(rollout.stage_started, rollout.quiet_wait)).flatten();
                (s.version.clone(), doing, meter, Tone::Waiting)
            } else if rollout.stage == Stage::Rolling {
                (s.version.clone(), "Updating in the next few minutes".into(), None, Tone::Waiting)
            } else {
                (
                    s.version.clone(),
                    format!("Next, when testing ends · {}", setup::clock::when(clock, testing_ends + RESTART, now)),
                    None,
                    Tone::Waiting,
                )
            };
            Row {
                place: place(s),
                mine: Some(s.host.as_str()) == mine,
                version,
                doing,
                meter,
                tone,
            }
        })
        .collect()
}

/// The notice, under the launch bar: a warm strip with the heading and time, what's
/// happening, its button, and each server. Answers the button pressed.
pub fn show(ui: &mut egui::Ui, notice: &Notice) -> Option<Action> {
    let mut pressed = None;
    let warm = theme::WARN;
    egui::Frame::new()
        .fill(warm.linear_multiply(0.07))
        .stroke(egui::Stroke::new(1.0, warm.linear_multiply(0.35)))
        .inner_margin(egui::Margin {
            left: 32,
            right: 32,
            top: 14,
            bottom: 16,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let text_width = (ui.available_width() - 240.0).max(300.0);
                ui.allocate_ui_with_layout(egui::vec2(text_width, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(theme::display(&notice.heading, 16.0).color(warm));
                        egui::Frame::new()
                            .fill(warm.linear_multiply(0.16))
                            .corner_radius(6)
                            .inner_margin(egui::Margin::symmetric(8, 2))
                            .show(ui, |ui| {
                                ui.label(RichText::new(&notice.eta).monospace().size(13.0).color(theme::FG));
                            });
                    });
                    ui.label(RichText::new(&notice.text).color(theme::SOFT).size(14.0));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if let Some((action, label)) = &notice.action {
                        let button = if notice.kind == Kind::LauncherBehind {
                            theme::primary(label)
                        } else {
                            theme::secondary(label)
                        };
                        if ui.add(button).clicked() {
                            pressed = Some(action.clone());
                        }
                    }
                    if let Some(note) = &notice.note {
                        ui.label(theme::muted(note.as_str()).small());
                    }
                });
            });
            if notice.servers.is_empty() {
                return;
            }
            ui.add_space(8.0);
            let gap = 8.0;
            let columns = 3;
            let width = ((ui.available_width() - gap * (columns as f32 - 1.0)) / columns as f32).floor();
            for chunk in notice.servers.chunks(columns) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for row in chunk {
                        server_box(ui, row, width);
                    }
                });
                ui.add_space(gap);
            }
        });
    pressed
}

fn server_box(ui: &mut egui::Ui, row: &Row, width: f32) {
    let stroke = if row.mine { theme::WARN.linear_multiply(0.55) } else { theme::LINE };
    egui::Frame::new()
        .fill(theme::BG.gamma_multiply(0.55))
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(9)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            // The margins and the stroke make up the rest of `width`.
            ui.set_width(width - 26.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    let name = if row.mine { format!("{} · yours", row.place) } else { row.place.clone() };
                    ui.label(RichText::new(hooks_config::text::clip(&name, 34)).family(theme::strong()).size(13.5));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&row.version).monospace().size(12.0).color(theme::MUTED));
                    });
                });
                let color = match row.tone {
                    Tone::Now => theme::WARN,
                    Tone::Done => theme::OK,
                    Tone::Waiting => theme::MUTED,
                };
                ui.add(egui::Label::new(RichText::new(&row.doing).size(12.5).color(color)).wrap());
                if let Some(f) = row.meter {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2, theme::LINE);
                    let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * f, rect.height()));
                    ui.painter().rect_filled(filled, 2, theme::WARN);
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_791_189_000;

    fn listing(id: &str, region: &str, version: &str, players: u32) -> Listing {
        Listing {
            id: id.into(),
            name: format!("Example {id}"),
            region: region.into(),
            host: format!("{id}.example.net"),
            ports: None,
            version: version.into(),
            players_online: players,
            players_total: 0,
            friends_mode: String::new(),
        }
    }

    fn clock() -> Clock {
        Clock {
            offset: 13 * 3600,
            zone: "NZDT".into(),
            twelve_hour: true,
        }
    }

    fn rollout(stage: Stage, started: i64) -> Rollout {
        Rollout {
            release: "0.4.2".into(),
            stage,
            stage_started: started,
            canary: Some("eu".into()),
            healthy_for: 600,
            quiet_wait: 7200,
        }
    }

    fn network(eu: &str, oce: &str, na_players: u32) -> Vec<(Listing, Option<u32>)> {
        vec![
            (listing("eu", "Falkenstein, Germany", eu, 0), Some(366)),
            (listing("oce", "Sydney, Australia", oce, 0), Some(45)),
            (listing("na", "Beauharnois, Canada", "0.4.1", na_players), Some(213)),
        ]
    }

    fn work(rollout: Option<&Rollout>, servers: &[(Listing, Option<u32>)], mine: &str, my_ping: Option<u32>, newer: Option<&str>) -> Option<Notice> {
        let clock = clock();
        let mine = servers.iter().find(|(s, _)| s.id == mine).map(|(s, _)| (s, my_ping));
        notice(&Input {
            rollout,
            servers,
            mine,
            launcher: "0.4.1",
            newer_launcher: newer,
            now: NOW,
            clock: &clock,
        })
    }

    #[test]
    fn testing_says_when_the_players_server_updates() {
        // Four minutes into the ten-minute test.
        let r = rollout(Stage::Verifying, NOW - 4 * 60);
        let servers = network("0.4.2", "0.4.1", 2);
        let n = work(Some(&r), &servers, "oce", Some(45), Some("0.4.2")).unwrap();
        assert_eq!(n.kind, Kind::Updating);
        // The test ends in 6 minutes; Sydney about 2 minutes after: 8 minutes from 9:30 pm.
        assert_eq!(n.eta, "Sydney in ~8 min · about 9:38 pm NZDT");
        assert_eq!(n.note.as_deref(), Some("Launcher 0.4.2 ready"));
        assert_eq!(n.action, Some((Action::UpdateLauncher, "Update launcher".into())));
        assert_eq!(n.blocks_play, None, "playing on until the server restarts");
        assert_eq!(n.servers[0].place, "Falkenstein, Germany");
        assert_eq!(n.servers[0].doing, "Testing the new release · 6 min left");
        assert_eq!(n.servers[0].meter, Some(0.4));
        assert!(n.servers[1].mine);
        assert_eq!(n.servers[1].doing, "Next, when testing ends · in ~8 min, about 9:38 pm NZDT");
        // Players on: once they leave, by the time the last stage began plus two hours.
        assert_eq!(n.servers[2].doing, "Next · 2 players on: once they leave, by 11:36 pm NZDT at the latest");
    }

    #[test]
    fn a_server_with_players_waits_for_them() {
        let r = rollout(Stage::Rolling, NOW - 30 * 60);
        let servers = network("0.4.2", "0.4.2", 2);
        let n = work(Some(&r), &servers, "na", Some(213), None).unwrap();
        assert_eq!(n.eta, "Beauharnois by 11:00 pm NZDT at the latest");
        assert!(n.text.contains("updates once nobody is playing on it"));
        assert_eq!(n.action, None, "no newer launcher");
        let mine = n.servers.iter().find(|r| r.mine).unwrap();
        assert_eq!(mine.meter, Some(0.25));
    }

    #[test]
    fn the_players_server_restarting() {
        let r = rollout(Stage::Rolling, NOW - 60);
        let servers = network("0.4.2", "0.4.1", 0);
        let n = work(Some(&r), &servers, "oce", None, None).unwrap();
        assert_eq!(n.kind, Kind::Restarting);
        assert_eq!(n.eta, "back in ~2 min · about 9:32 pm NZDT");
        assert_eq!(
            n.action,
            Some((
                Action::PlayOn {
                    host: "eu.example.net".into(),
                    place: "Falkenstein, Germany".into()
                },
                "Play on Falkenstein".into()
            ))
        );
        assert!(n.blocks_play.is_some());
        assert_eq!(n.refresh_every(), std::time::Duration::from_secs(15));
        let mine = n.servers.iter().find(|r| r.mine).unwrap();
        assert_eq!((mine.version.as_str(), mine.doing.as_str()), ("0.4.1 → 0.4.2", "Restarting · usually two minutes"));
    }

    #[test]
    fn a_launcher_behind_its_server() {
        // No rollout needed: the server runs a release newer than this launcher.
        let servers = network("0.4.2", "0.4.2", 0);
        let n = work(None, &servers, "oce", Some(45), Some("0.4.2")).unwrap();
        assert_eq!(n.kind, Kind::LauncherBehind);
        assert_eq!(n.eta, "launcher 0.4.1 → 0.4.2");
        assert_eq!(n.action, Some((Action::UpdateLauncher, "Update and restart".into())));
        assert!(n.blocks_play.is_some());
    }

    #[test]
    fn nothing_to_say() {
        let servers = network("0.4.2", "0.4.2", 0);
        // The player's server runs it already (and this launcher is current).
        let r = rollout(Stage::Rolling, NOW);
        let clock = clock();
        let mine = servers.iter().find(|(s, _)| s.id == "oce").map(|(s, _)| (s, Some(45)));
        let input = Input {
            rollout: Some(&r),
            servers: &servers,
            mine,
            launcher: "0.4.2-dev",
            newer_launcher: None,
            now: NOW,
            clock: &clock,
        };
        assert_eq!(notice(&input), None);
        // No rollout, and nothing newer.
        assert_eq!(notice(&Input { rollout: None, ..input }), None);
    }
}
