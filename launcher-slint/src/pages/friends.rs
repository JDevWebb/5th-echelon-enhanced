//! Friends (the Tauri launcher's Friends.vue, the 0.5.0 plan's P1): friends, requests both
//! ways, finding players and who's blocked, with the same words as the overlay. Remove, Block
//! and Switch server ask first; a block can be undone at once.

use launcher::flow::Person;

use super::*;
use crate::commands::Launcher;
use crate::tauri::Manager;
use crate::tauri::WindowCall;

const BLOCK_MEANS: &str = "Neither of you can message, invite or friend the other, you don't see each other online, and quick match leaves out matches they're in.";

#[derive(Default)]
pub struct FriendsPage {
    tab: usize,
    query: String,
    searched: Option<String>,
    found: Option<Result<Vec<Person>, String>>,
    /// Asking first: the action, the person's id (or a server's host) and name.
    asking: Option<(String, String, String)>,
    /// What was just done that can be undone: its line, the action back, the id and name.
    undo: Option<(String, &'static str, String, String)>,
}

fn initial_line(p: &Person) -> String {
    let mut line = if p.online {
        p.activity.clone().unwrap_or_else(|| "Online".into())
    } else {
        "Offline".into()
    };
    if let Some(id) = &p.identity {
        line.push_str(&format!(" · {id}"));
    }
    line
}

impl Screen for FriendsPage {
    fn refresh_every(&self) -> u64 {
        20
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        let core = app.state::<Launcher>().0.clone();
        let info = crate::core::info(&core);
        let mut r = Rows::default();
        let page = |rows: Vec<R>| PageView {
            kicker: "Squad".into(),
            title: "Friends".into(),
            rows,
            ..Default::default()
        };
        if info.game_dir.is_none() || info.server.is_none() {
            r.add(
                mark("No server yet", "Set up the game and a server first: your friends are kept by your account there.")
                    .button(super::b("go-play", "Set up on Play").kind("primary")),
            );
            return page(r.done());
        }
        let (people, signed_in, error) = crate::core::people(&core);
        if !signed_in {
            r.add(mark("No account yet", "Make an account on your server first: friends are kept by your account.").button(super::b("go-play", "Go to Play").kind("primary")));
            return page(r.done());
        }
        let Some(p) = people else {
            r.add(bad(format!("Couldn't read your friends: {}", error.unwrap_or_default())));
            r.add(buttons(vec![super::b("reload", "Try again").kind("link")]));
            return page(r.done());
        };
        let online = p.friends.iter().filter(|f| f.online).count();
        let tabs = [
            (
                "friends",
                if p.friends.is_empty() {
                    "Friends".to_string()
                } else {
                    format!("Friends · {online}/{}", p.friends.len())
                },
            ),
            (
                "requests",
                match p.received.len() + p.sent.len() {
                    0 => "Requests".into(),
                    n => format!("Requests · {n}"),
                },
            ),
            ("find", "Find players".into()),
            (
                "blocked",
                if p.blocked.is_empty() {
                    "Blocked".into()
                } else {
                    format!("Blocked · {}", p.blocked.len())
                },
            ),
        ];
        r.add(segments(
            "tab",
            "",
            &tabs.iter().map(|(i, l)| (i.to_string(), l.clone())).collect::<Vec<_>>(),
            tabs[self.tab].0,
        ));

        // Asked first.
        if let Some((action, _, name)) = &self.asking {
            let (heading, means, button) = match action.as_str() {
                "block" => (format!("Block {name}?"), BLOCK_MEANS.to_string(), "Block"),
                "remove" => (
                    format!("Remove {name} from your friends?"),
                    "You can ask them to be friends again later.".to_string(),
                    "Remove",
                ),
                _ => {
                    let server = p.elsewhere.iter().find(|e| e.name == *name).map(|e| e.server.clone()).unwrap_or_default();
                    (
                        format!("Switch to {server}?"),
                        format!("To play with {name}, the launcher sets you up on their server with your identity (your friends come with you), and asks for a name there if you have no account on it yet."),
                        "Switch",
                    )
                }
            };
            r.card(&heading);
            r.add(note(means));
            r.add(buttons(vec![
                super::b("confirm", button).kind(if action == "switch" { "primary" } else { "danger" }),
                super::b("cancel", "Cancel"),
            ]));
        }
        if let Some((line, ..)) = &self.undo {
            r.out();
            r.add(item(line.clone(), "").button(super::b("undo", "Undo").kind("fix")));
        }
        r.out();
        let busy = info.connecting;
        match self.tab {
            0 => {
                if p.friends.is_empty() {
                    r.add(mark("No friends yet", "Find players to add some, or accept a request when one comes.").button(super::b("tab:2", "Find players").kind("primary")));
                }
                for f in &p.friends {
                    let mut acts = Vec::new();
                    if f.with_me {
                        acts.push(super::b("none", "You're in it").kind("fix").enabled(false));
                    } else if f.joinable {
                        acts.push(super::b(format!("join:{}:{}", f.id, f.name), "Join").kind("primary").enabled(!busy));
                    } else if f.online && info.running {
                        acts.push(super::b(format!("invite:{}:{}", f.id, f.name), "Invite").enabled(!busy));
                    }
                    acts.push(super::b(format!("ask-remove:{}:{}", f.id, f.name), "Remove").kind("fix"));
                    acts.push(super::b(format!("ask-block:{}:{}", f.id, f.name), "Block").kind("fix"));
                    r.add(item(f.name.clone(), initial_line(f)).tone(if f.online { "accent" } else { "muted" }).buttons(acts));
                }
                if !p.elsewhere.is_empty() {
                    r.card("Playing on other servers");
                    for e in &p.elsewhere {
                        let region = if e.region.is_empty() { String::new() } else { format!(" ({})", e.region) };
                        r.add(
                            item(e.name.clone(), format!("On {}{region}: to play together, one of you joins the other's server", e.server))
                                .tone("warn")
                                .button(super::b(format!("ask-switch:{}:{}", e.host, e.name), "Switch server").enabled(!busy && !info.running)),
                        );
                    }
                }
            }
            1 => {
                if p.received.is_empty() && p.sent.is_empty() {
                    r.add(mark("No requests", "Nobody's waiting for your answer, and you haven't asked anyone."));
                }
                if !p.received.is_empty() {
                    r.card("Waiting for your answer");
                    for q in &p.received {
                        r.add(item(q.name.clone(), "Wants to be friends").tone(if q.online { "accent" } else { "" }).buttons(vec![
                            super::b(format!("accept:{}:{}", q.id, q.name), "Accept").kind("primary").enabled(!busy),
                            super::b(format!("decline:{}:{}", q.id, q.name), "Decline").enabled(!busy),
                            super::b(format!("ask-block:{}:{}", q.id, q.name), "Block").kind("fix"),
                        ]));
                    }
                }
                if !p.sent.is_empty() {
                    r.card("You asked");
                    for q in &p.sent {
                        r.add(item(q.name.clone(), "Hasn't answered yet").button(super::b(format!("cancel-request:{}:{}", q.id, q.name), "Cancel request").enabled(!busy)));
                    }
                }
            }
            2 => {
                r.add(field("query", "", self.query.clone()).placeholder("A player's name").button(super::b("search", "Search")));
                if self.found.is_none() {
                    self.found = Some(crate::core::find_players(&app.state::<Launcher>().0, ""));
                    self.searched = None;
                }
                match &self.found {
                    Some(Err(e)) => {
                        r.add(bad(e.clone()));
                    }
                    Some(Ok(list)) if list.is_empty() => {
                        r.add(note(match &self.searched {
                            Some(q) => format!("Nobody called \u{201c}{q}\u{201d} here."),
                            None if !p.everyone => "None of your friends are online right now.".into(),
                            None => "Nobody else is online right now.".into(),
                        }));
                    }
                    Some(Ok(list)) => {
                        r.card(&match &self.searched {
                            Some(q) => format!("Players called \u{201c}{q}\u{201d}"),
                            None if !p.everyone => "Friends online now".into(),
                            None => "Online now".into(),
                        });
                        for q in list {
                            let act = match q.relation {
                                "friend" => super::b("none", "Your friend").kind("fix").enabled(false),
                                "sent" => super::b("none", "Asked").kind("fix").enabled(false),
                                "received" => super::b(format!("accept:{}:{}", q.id, q.name), "Accept").kind("primary").enabled(!busy),
                                _ => super::b(format!("request:{}:{}", q.id, q.name), "Add friend").enabled(!busy),
                            };
                            r.add(
                                item(q.name.clone(), initial_line(q))
                                    .tone(if q.online { "accent" } else { "" })
                                    .buttons(vec![act, super::b(format!("ask-block:{}:{}", q.id, q.name), "Block").kind("fix")]),
                            );
                        }
                    }
                    None => {}
                }
            }
            _ => {
                r.add(note(format!(
                    "{BLOCK_MEANS} A block of a player with a 5th Echelon identity holds on every server that shares friends, when you have one too."
                )));
                if p.blocked.is_empty() {
                    r.add(mark("Nobody blocked", "You haven't blocked anyone."));
                }
                for q in &p.blocked {
                    r.add(item(q.name.clone(), q.identity.clone().unwrap_or_default()).button(super::b(format!("unblock:{}:{}", q.id, q.name), "Unblock").enabled(!busy)));
                }
            }
        }
        page(r.done())
    }

    fn chosen(&mut self, _app: &AppHandle, id: &str, index: usize) {
        if id == "tab" {
            self.tab = index.min(3);
            self.asking = None;
        }
    }

    fn edited(&mut self, id: &str, text: &str) {
        if id == "query" {
            self.query = text.chars().take(32).collect();
        }
    }

    fn accepted(&mut self, app: &AppHandle, id: &str, text: &str) {
        self.edited(id, text);
        self.pressed(app, "search");
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        let core = app.state::<Launcher>().0.clone();
        let (action, rest) = id.split_once(':').unwrap_or((id, ""));
        let (who, name) = rest.split_once(':').unwrap_or((rest, ""));
        match action {
            "reload" => {}
            "go-play" => {
                app.window(WindowCall::Go("play".into()));
            }
            "tab" => self.tab = who.parse().unwrap_or(0),
            "search" => {
                let q = self.query.trim().to_string();
                self.found = Some(crate::core::find_players(&core, &q));
                self.searched = Some(q).filter(|q| !q.is_empty());
            }
            "cancel" => self.asking = None,
            "ask-remove" | "ask-block" | "ask-switch" => self.asking = Some((action[4..].to_string(), who.to_string(), name.to_string())),
            "confirm" => {
                if let Some((action, who, name)) = self.asking.take() {
                    if action == "switch" {
                        if crate::core::switch_to_friend(&core, &who) {
                            app.window(WindowCall::Go("play".into()));
                        }
                    } else {
                        crate::core::player_action(&core, &action, &who, &name);
                        if action == "block" {
                            self.undo = Some((format!("Blocked {name}."), "unblock", who, name));
                        }
                    }
                }
            }
            "undo" => {
                if let Some((_, back, who, name)) = self.undo.take() {
                    crate::core::player_action(&core, back, &who, &name);
                }
            }
            "join" => crate::core::join_friend(&core, who, name),
            "cancel-request" => crate::core::player_action(&core, "cancel", who, name),
            "invite" | "accept" | "decline" | "request" | "unblock" => {
                crate::core::player_action(&core, action, who, name);
                // The list as searched, again: "Add friend" turns into "Asked".
                if self.found.is_some() {
                    let q = self.searched.clone().unwrap_or_default();
                    self.found = Some(crate::core::find_players(&core, &q));
                }
            }
            _ => {}
        }
    }
}
