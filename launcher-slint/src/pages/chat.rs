//! Chat (the Tauri launcher's Chat.vue): Global, Lobby and the conversations with friends down
//! the side; the line to write at the top of the room and its messages newest first under it
//! (so a new one is in view without scrolling); what can be done about a message under it when
//! it's picked; who's online here beside Global.

use serde_json::Value;

use super::*;
use crate::chat::Chat;
use crate::tauri::Manager;

#[derive(Default)]
pub struct ChatPage {
    tab: String,
    with: Option<String>,
    text: String,
    refused: Option<String>,
    picked: Option<i64>,
    reporting: Option<(i64, String)>,
    invite: Option<String>,
    picked_person: Option<String>,
}

impl ChatPage {
    pub fn new() -> Self {
        Self {
            tab: "global".into(),
            ..Default::default()
        }
    }
}

impl Screen for ChatPage {
    fn refresh_every(&self) -> u64 {
        2
    }

    fn live_edits(&self) -> bool {
        true
    }

    fn section(&mut self, id: &str) {
        if let Some(with) = id.strip_prefix("with:") {
            self.tab = "messages".into();
            self.with = Some(with.to_string());
        } else {
            self.tab = id.into();
            self.with = None;
        }
        self.text.clear();
        self.refused = None;
        self.picked = None;
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        let v = json(&crate::chat::chat(app.clone(), app.state::<Chat>(), self.tab.clone(), self.with.clone(), true));
        let mut r = Rows::default();
        let mut sections = vec![("global".to_string(), "# Global".to_string()), ("lobby".to_string(), "# Lobby".to_string())];
        let state = st(&v, "state");
        let page = |r: Rows, sections: Vec<(String, String)>, section: String, summary: String| PageView {
            kicker: "Community network".into(),
            title: "Chat".into(),
            summary,
            sections,
            section,
            rows: r.done(),
        };
        if state != "open" {
            r.add(match state.as_str() {
                "no-account" => mark("No account yet", "Chat is for players with an account on a community server: set one up on Play."),
                "opening" => working("Opening chat…"),
                _ => note(st(&v, "why")),
            });
            return page(r, sections, self.tab.clone(), "Talk with everyone on the community network".into());
        }
        for c in arr(&v, "conversations") {
            let unread = num(c, "unread") as u32;
            let label = if unread > 0 {
                format!("{} · {}", st(c, "name"), unread.min(99))
            } else {
                st(c, "name")
            };
            sections.push((format!("with:{}", st(c, "with")), label));
        }
        let section = match (&self.tab[..], &self.with) {
            ("messages", Some(w)) => format!("with:{w}"),
            (t, _) => t.to_string(),
        };
        let with_name = st(&v, "with_name");
        let summary = match self.tab.as_str() {
            "global" => "# Global · the whole community network".to_string(),
            "lobby" => "# Lobby · the players in your party or match".to_string(),
            _ => format!("{with_name} · direct messages"),
        };
        if !st(&v, "trouble").is_empty() {
            r.add(warn(format!("Chat is reconnecting: {}", st(&v, "trouble"))));
        }
        r.add_if(bo(&v, "read_only"), || warn("The network can't be reached right now: you can read, not send."));
        if arr(&v, "conversations").is_empty() {
            r.add(note("No conversations yet. Make friends in the game (or from Global) to message them."));
        }
        if self.tab == "lobby" && !bo(&v, "lobby") {
            r.add(note("Join a lobby or a match in the game: its players can talk here, and in the game's overlay."));
            return page(r, sections, section, summary);
        }
        // The line to write.
        let allowed = !bo(&v, "read_only") && (self.tab != "global" || bo(&v, "may_post_global"));
        let checked = json(&crate::chat::chat_check(self.text.clone()));
        let why = st(&checked, "why");
        let placeholder = match self.tab.as_str() {
            "global" => "Say something to everyone…".to_string(),
            "lobby" => "Say something to your lobby…".to_string(),
            _ => format!("Message {with_name}…"),
        };
        r.card("");
        r.add(
            field("text", "", self.text.clone())
                .placeholder(placeholder)
                .enabled(allowed)
                .button(super::b("send", "Send").kind("primary").enabled(allowed && why.is_empty() && !self.text.trim().is_empty())),
        );
        if self.tab == "global" && !bo(&v, "may_post_global") {
            r.add(note(st(&v, "global_wait")));
        }
        if let Some(refused) = &self.refused {
            r.add(warn(refused.clone()));
        } else if !why.is_empty() {
            r.add(warn(why));
        } else if bo(&checked, "masked") {
            r.add(note("Some words will show as ***."));
        }
        // The messages, newest first.
        r.card("");
        let messages = arr(&v, "messages");
        if messages.is_empty() {
            r.add(note("No messages yet."));
        }
        for m in messages.iter().rev() {
            let id = m.get("id").and_then(Value::as_i64).unwrap_or_default();
            let body: String = arr(m, "parts").iter().map(|p| st(p, "text")).collect();
            let link = arr(m, "parts").iter().find(|p| bo(p, "link")).map(|p| st(p, "text"));
            let who = if bo(m, "mine") { "You".to_string() } else { st(m, "from_name") };
            let mut row = message(who, body, st(m, "time")).on(bo(m, "mine")).tone(if bo(m, "mentions_me") { "accent" } else { "" });
            row.buttons
                .push(super::b(format!("pick:{id}"), if self.picked == Some(id) { "close" } else { "…" }).kind("link"));
            r.add(row);
            if let Some(link) = &link {
                r.add(buttons(vec![super::b(format!("invite:{link}"), format!("Open the Discord invite {link}")).kind("link")]));
            }
            if self.picked == Some(id) {
                let name = st(m, "from_name");
                let from = st(m, "from");
                let mut acts = Vec::new();
                if bo(m, "mine") {
                    acts.push(super::b(format!("delete:{id}"), "Delete"));
                } else {
                    if bo(m, "friend") && self.tab != "messages" {
                        acts.push(super::b(format!("message:{from}"), format!("Message {name}")));
                    }
                    if !bo(m, "friend") {
                        acts.push(super::b(format!("request-by-id:{from}:{name}"), "Add friend"));
                    }
                    acts.push(super::b(format!("block-by-id:{from}:{name}"), format!("Block {name}")).kind("danger"));
                    acts.push(super::b(format!("report:{id}"), "Report…"));
                }
                r.add(buttons(acts));
            }
            if let Some((rid, why)) = &self.reporting {
                if *rid == id {
                    r.add(field("report-why", format!("Report {}'s message to the admins", st(m, "from_name")), why.clone()).placeholder("Why (optional)"));
                    r.add(buttons(vec![super::b("report-send", "Report").kind("primary"), super::b("report-cancel", "Cancel")]));
                }
            }
        }
        if let Some(link) = &self.invite {
            r.card("Open a Discord invite?");
            r.add(note(format!("{link} opens in your browser: Discord invites are the only links chat lets through.")));
            r.add(buttons(vec![super::b("invite-open", "Open it").kind("primary"), super::b("invite-cancel", "Cancel")]));
        }
        if self.tab == "global" {
            let online = arr(&v, "online");
            r.card(&format!("Online here · {}", online.len()));
            if online.is_empty() {
                r.add(note("Nobody else is online here right now."));
            }
            for p in online {
                let pid = st(p, "id");
                let name = st(p, "name");
                let mut row = item(name.clone(), st(p, "activity")).tone(if bo(p, "friend") { "accent" } else { "" });
                if !bo(p, "friend") {
                    if self.picked_person.as_deref() == Some(pid.as_str()) {
                        if !bo(p, "asked") {
                            row.buttons.push(super::b(format!("request:{pid}:{name}"), "Add friend").kind("fix"));
                        }
                        row.buttons.push(super::b(format!("invite-game:{pid}:{name}"), "Invite to your game").kind("fix"));
                        row.buttons.push(super::b(format!("block:{pid}:{name}"), "Block").kind("fix"));
                    } else {
                        row.buttons.push(super::b(format!("person:{pid}"), "…").kind("fix"));
                    }
                }
                r.add(row);
            }
        }
        r.out();
        r.add(note(st(&v, "rules")));
        page(r, sections, section, summary)
    }

    fn edited(&mut self, id: &str, text: &str) {
        match id {
            "text" => {
                self.text = text.into();
                self.refused = None;
            }
            "report-why" => {
                if let Some((_, why)) = self.reporting.as_mut() {
                    *why = text.chars().take(300).collect();
                }
            }
            _ => {}
        }
    }

    fn accepted(&mut self, app: &AppHandle, id: &str, text: &str) {
        self.edited(id, text);
        self.pressed(app, if id == "text" { "send" } else { "report-send" });
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        let (action, rest) = id.split_once(':').unwrap_or((id, ""));
        let (who, name) = rest.split_once(':').unwrap_or((rest, ""));
        match action {
            "send" => {
                if self.text.trim().is_empty() {
                    return;
                }
                match crate::chat::chat_send(app.clone(), app.state::<Chat>(), self.tab.clone(), self.with.clone().unwrap_or_default(), self.text.clone()) {
                    Ok(()) => self.text.clear(),
                    Err(e) => self.refused = Some(e),
                }
            }
            "pick" => {
                let id: i64 = rest.parse().unwrap_or_default();
                self.picked = if self.picked == Some(id) { None } else { Some(id) };
            }
            "person" => self.picked_person = Some(rest.to_string()),
            "delete" => {
                crate::chat::chat_delete(app.clone(), rest.parse().unwrap_or_default());
                self.picked = None;
            }
            "message" => self.section(&format!("with:{rest}")),
            "report" => {
                self.reporting = Some((rest.parse().unwrap_or_default(), String::new()));
                self.picked = None;
            }
            "report-send" => {
                if let Some((id, why)) = self.reporting.take() {
                    crate::chat::chat_report(app.clone(), id, why);
                }
            }
            "report-cancel" => self.reporting = None,
            // By their identity: a name alone may be another account's on this server.
            "request-by-id" | "block-by-id" => {
                crate::chat::chat_act(app.clone(), action.trim_end_matches("-by-id").into(), String::new(), who.into(), name.into());
                self.picked = None;
            }
            "request" | "block" | "invite-game" => {
                let act = if action == "invite-game" { "invite" } else { action };
                crate::chat::chat_act(app.clone(), act.into(), who.into(), String::new(), name.into());
                self.picked_person = None;
            }
            "invite" => self.invite = Some(rest.to_string()),
            "invite-open" => {
                if let Some(link) = self.invite.take() {
                    let link = if link.starts_with("http") { link } else { format!("https://{link}") };
                    let _ = crate::core::open_url(&link);
                }
            }
            "invite-cancel" => self.invite = None,
            _ => {}
        }
    }
}
