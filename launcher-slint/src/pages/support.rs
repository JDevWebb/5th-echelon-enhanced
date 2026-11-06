//! Support (the Tauri launcher's Support.vue): the conversation with the community network's
//! admins, the newest at the foot, a message with the logs ticked, and beside it a report, the
//! checks and the help pages.

use super::*;
use crate::support::Support;
use crate::tauri::Manager;
use crate::tauri::WindowCall;

pub struct SupportPage {
    text: String,
    game_log: bool,
    launcher_log: bool,
    error: Option<String>,
}

impl Default for SupportPage {
    fn default() -> Self {
        Self {
            text: String::new(),
            game_log: true,
            launcher_log: true,
            error: None,
        }
    }
}

impl Screen for SupportPage {
    fn refresh_every(&self) -> u64 {
        15
    }

    fn live_edits(&self) -> bool {
        true
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        let v = json(&crate::support::support(app.clone(), app.state::<Support>(), true));
        let draft = st(&v, "draft");
        if !draft.is_empty() {
            self.text = draft;
        }
        let mut r = Rows::default();
        if !bo(&v, "identity") {
            r.add(note("Connect to a server first: your messages are signed with the identity the launcher makes then."));
        } else {
            let error = st(&v, "error");
            r.add_if(!error.is_empty(), || note(error.clone()));
            match v.get("messages").and_then(|m| m.as_array()) {
                None => {
                    r.add(working("Reading your conversation…"));
                }
                Some(list) if list.is_empty() => {
                    r.add(note(
                        "Nothing yet. Say what went wrong: when, on which server, and what you saw. Ticking the logs below helps the admins find it.",
                    ));
                }
                Some(list) => {
                    r.card("Your conversation");
                    for m in list {
                        let mut row = item(st(m, "who"), st(m, "when")).text(st(m, "text")).tone(if bo(m, "admin") { "accent" } else { "" });
                        if !st(m, "files").is_empty() {
                            row.text = format!("{}\n{}", row.text, st(m, "files"));
                        }
                        r.add(row);
                    }
                }
            }
            r.add_if(bo(&v, "resolved"), || note("The admins marked this solved. Write again if it isn't, and it opens again."));
            r.card("Message");
            r.add(area("message", "", self.text.clone()).placeholder("What went wrong, when, and on which server?"));
            r.add(note(format!("{}/{}", self.text.chars().count(), num(&v, "max_text"))));
            r.add(switch("game_log", "The game's log (its last part, and the one before)", self.game_log && bo(&v, "has_game")).enabled(bo(&v, "has_game")));
            r.add(switch("launcher_log", "The launcher's log", self.launcher_log));
            r.add(note("Your PC's name, your user folder and your internet address are hidden in them first."));
            let why = crate::support::support_check(self.text.clone());
            if bo(&v, "sending") {
                r.add(working("Sending…"));
            } else {
                r.add(buttons(vec![super::b("send", "Send").kind("primary").enabled(why.is_none())]));
                if let Some(why) = why.filter(|_| !self.text.trim().is_empty()) {
                    r.add(note(why));
                }
            }
            if let Some(e) = &self.error {
                r.add(bad(e.clone()));
            }
        }
        r.card("Something went wrong?");
        r.add(note(
            "The report goes to your server's admins, with the game's and the launcher's logs; the checks are the Play screen's Status.",
        ));
        r.add(buttons(vec![super::b("feedback", "Send a report with the logs"), super::b("checks", "Run all checks")]));
        r.card("Field manual");
        for m in arr(&v, "manual") {
            if let (Some(name), Some(url)) = (m.get(0).and_then(|x| x.as_str()), m.get(1).and_then(|x| x.as_str())) {
                r.add(buttons(vec![super::b(format!("link:{url}"), name).kind("link")]));
            }
        }
        PageView {
            kicker: "The network's admins".into(),
            title: "Support".into(),
            summary: "Write to the community network's admins about anything that goes wrong. They answer here, and you're told when they do, here and in the game.".into(),
            rows: r.done(),
            ..Default::default()
        }
    }

    fn toggled(&mut self, _app: &AppHandle, id: &str, on: bool) {
        match id {
            "game_log" => self.game_log = on,
            "launcher_log" => self.launcher_log = on,
            _ => {}
        }
    }

    fn edited(&mut self, id: &str, text: &str) {
        if id == "message" {
            self.text = text.into();
        }
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        match id {
            "send" => match crate::support::support_send(app.clone(), self.text.clone(), self.game_log, self.launcher_log) {
                Ok(()) => {
                    self.text.clear();
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            },
            "feedback" => {
                app.window(WindowCall::Feedback);
            }
            "checks" => {
                app.window(WindowCall::Go("play".into()));
            }
            _ if id.starts_with("link:") => {
                let _ = crate::core::open_url(&id[5..]);
            }
            _ => {}
        }
    }
}
