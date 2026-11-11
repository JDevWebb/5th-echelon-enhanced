//! Chat, "Comms" (ui/chat.slint draws it): Global, Lobby and the conversations with friends
//! across the top; the log oldest first, the newest by the line to write, a sender's lines
//! together, a line for each day and one where the unseen messages start; what can be done about
//! a message when it's picked; who's on Global in the roster beside it, and a player's dossier.

use std::collections::HashMap;

use serde_json::Value;

use super::*;
use crate::chat::Chat;
use crate::commands::Launcher;
use crate::tauri::Manager;

/// A sender's lines this close together show as one (their tile and name once).
const GROUP_SECS: i64 = 5 * 60;
/// The roster's groups show this many each, then "and N more".
const ROSTER_GROUP: usize = 8;

#[derive(Default)]
pub struct ChatPage {
    tab: String,
    with: Option<String>,
    text: String,
    refused: Option<String>,
    picked: Option<i64>,
    reporting: Option<(i64, String)>,
    invite: Option<String>,
    /// The player whose dossier shows: an account id from the roster, or "msg:<id>" for a
    /// message's sender.
    picked_person: Option<String>,
    /// Goes up with each message sent: the window empties its line to write then.
    sent: i32,
    /// The newest message seen in each room ("global", "lobby", "with:<identity>"), and the
    /// room the "New traffic" line is for, after which message.
    seen: HashMap<String, i64>,
    marker: Option<(String, Option<i64>)>,
}

impl ChatPage {
    pub fn new() -> Self {
        Self {
            tab: "global".into(),
            ..Default::default()
        }
    }

    /// A message picked (its actions show), and a player's dossier open: for the screenshots.
    pub fn pick(&mut self, message: Option<i64>, person: Option<&str>) {
        self.picked = message;
        self.picked_person = person.map(str::to_string);
    }

    /// The section shown: "global", "lobby" or "with:<identity>".
    fn room(&self) -> String {
        match (&self.tab[..], &self.with) {
            ("messages", Some(w)) => format!("with:{w}"),
            (t, _) => t.to_string(),
        }
    }
}

fn s(t: impl AsRef<str>) -> slint::SharedString {
    slint::SharedString::from(t.as_ref())
}

fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

fn id_of(m: &Value) -> i64 {
    m.get("id").and_then(Value::as_i64).unwrap_or_default()
}

impl ChatPage {
    /// The screen from chat's view (crate::chat::ChatView as JSON), as the window shows it.
    pub fn build(&mut self, v: &Value) -> PageView {
        let state = st(v, "state");
        let room = self.room();
        let open = state == "open";
        let online = arr(v, "online");
        let convs = arr(v, "conversations");
        let messages = arr(v, "messages");
        let with_name = st(v, "with_name");

        // What's new since the player last looked: the "New traffic" line, and Lobby's count.
        if self.marker.as_ref().is_none_or(|(r, _)| *r != room) {
            self.marker = Some((room.clone(), self.seen.get(&room).copied()));
        }
        let marker = self.marker.as_ref().and_then(|(_, m)| *m);
        if let Some(last) = messages.iter().map(id_of).max() {
            self.seen.insert(room.clone(), last);
        }
        let lobby_ids: Vec<i64> = arr(v, "lobby_ids").iter().filter_map(Value::as_i64).collect();
        let lobby_seen = *self.seen.entry("lobby".into()).or_insert_with(|| lobby_ids.iter().copied().max().unwrap_or(0));
        let lobby_unread = if room == "lobby" {
            0
        } else {
            lobby_ids.iter().filter(|i| **i > lobby_seen).count() as i32
        };

        // The conversations across the top: as many as fit the window, the one open always.
        let room_tab = |id: &str, label: &str, count: String, unread: i32| crate::ChatTab {
            id: s(id),
            label: s(label),
            count: s(count),
            unread,
            ..Default::default()
        };
        let mut tabs = vec![
            room_tab(
                "global",
                "Global",
                if open && !online.is_empty() { format!("{} on", online.len()) } else { String::new() },
                0,
            ),
            room_tab("lobby", "Lobby", String::new(), lobby_unread),
        ];
        let fit = if compact() {
            2
        } else if wide() {
            6
        } else {
            4
        };
        let direct_tab = |c: &Value| crate::ChatTab {
            id: s(format!("with:{}", st(c, "with"))),
            label: s(st(c, "name")),
            direct: true,
            initial: s(initial(&st(c, "name"))),
            count: s(""),
            unread: if format!("with:{}", st(c, "with")) == room { 0 } else { num(c, "unread") as i32 },
            online: bo(c, "online"),
            offline: bo(c, "friend") && !bo(c, "online"),
        };
        let mut picks = Vec::new();
        let mut shown = 0;
        for c in convs.iter() {
            let id = format!("with:{}", st(c, "with"));
            let is_open = id == room;
            if bo(c, "direct") && shown < fit {
                shown += 1;
                tabs.push(direct_tab(c));
            } else if is_open {
                // Past what fits, or a friend not written to yet: across the top while it's open.
                tabs.push(direct_tab(c));
            } else {
                picks.push(crate::ChatPick {
                    id: s(&id),
                    label: s(st(c, "name")),
                    initial: s(initial(&st(c, "name"))),
                    line: s(if bo(c, "direct") {
                        "Conversation".to_string()
                    } else if bo(c, "online") {
                        "Friend · online".to_string()
                    } else {
                        "Friend".to_string()
                    }),
                    unread: num(c, "unread") as i32,
                    online: bo(c, "online"),
                });
            }
        }
        // A conversation open with no entry yet (just started from Global).
        if self.tab == "messages" && !tabs.iter().any(|t| t.id.as_str() == room) {
            tabs.push(crate::ChatTab {
                id: s(&room),
                label: s(&with_name),
                direct: true,
                initial: s(initial(&with_name)),
                ..Default::default()
            });
        }

        let mut view = crate::ChatView {
            state: s(&state),
            tab: s(&room),
            sent: self.sent,
            more_label: s(if wide() { "+ Message a friend" } else { "+ Message" }),
            ..Default::default()
        };
        view.why = s(match state.as_str() {
            "no-account" => "Chat is for players with an account on a community server: set one up on Play.".to_string(),
            "opening" => String::new(),
            _ => st(v, "why"),
        });
        let mut notes = Vec::new();
        let mut lines = Vec::new();
        let mut roster = Vec::new();
        if !open {
            return self.page(view, tabs, lines, roster, notes, picks);
        }

        let trouble = st(v, "trouble");
        let read_only = bo(v, "read_only");
        (view.conn, view.conn_tone) = if !trouble.is_empty() {
            (s("Reconnecting…"), s("warn"))
        } else if read_only {
            (s("Read only"), s("warn"))
        } else {
            (s("Connected"), s("ok"))
        };
        let note = |text: String, tone: &str, top: bool| crate::ChatNote {
            text: s(text),
            tone: s(tone),
            top,
        };
        if !trouble.is_empty() && read_only {
            notes.push(note(format!("Reconnecting… you can read, not send, until it's back ({trouble})."), "warn", true));
        } else if !trouble.is_empty() {
            notes.push(note(format!("Chat is reconnecting: {trouble}"), "warn", true));
        } else if read_only {
            notes.push(note("The network can't be reached right now: you can read, not send.".into(), "warn", true));
        }

        let no_lobby = self.tab == "lobby" && !bo(v, "lobby");
        let allowed = !read_only && !no_lobby && (self.tab != "global" || bo(v, "may_post_global"));
        let checked = json(&crate::chat::chat_check(self.text.clone()));
        let why = st(&checked, "why");
        view.can_type = allowed;
        view.can_send = allowed && why.is_empty() && !self.text.trim().is_empty();
        view.tx = s(match self.tab.as_str() {
            "global" => "TX // GLOBAL".to_string(),
            "lobby" => "TX // LOBBY".to_string(),
            _ => format!("TX // {}", with_name.to_uppercase()),
        });
        view.placeholder = s(match self.tab.as_str() {
            _ if read_only => "Can't send until the network is back".to_string(),
            _ if no_lobby => "Join a lobby or a match first".to_string(),
            "global" if !bo(v, "may_post_global") => "You can read Global; posting opens soon".to_string(),
            "global" => "Say something to everyone…".to_string(),
            "lobby" => "Say something to your lobby…".to_string(),
            _ => format!("Message {with_name}…"),
        });
        view.rules = s(format!("Enter sends. {}", st(v, "rules")));
        if self.tab == "global" && !bo(v, "may_post_global") {
            notes.push(note(st(v, "global_wait"), "", false));
        }
        if let Some(refused) = &self.refused {
            notes.push(note(refused.clone(), "warn", false));
        } else if !why.is_empty() {
            notes.push(note(why, "warn", false));
        } else if bo(&checked, "masked") {
            notes.push(note("Some words will show as ***.".into(), "", false));
        }
        if self.tab == "lobby" && !no_lobby {
            view.room_line = s("The players in your party or match · also in the game's overlay");
        }
        view.empty = s(if no_lobby {
            "Join a lobby or a match in the game: its players can talk here, and in the game's overlay."
        } else {
            "No messages yet."
        });

        // A direct message's friend: where they are, and Join when their game can be joined.
        if self.tab == "messages" {
            let with = self.with.clone().unwrap_or_default();
            let conv = convs.iter().find(|c| st(c, "with") == with);
            let here = online.iter().find(|p| bo(p, "friend") && st(p, "name").eq_ignore_ascii_case(&with_name));
            view.strip = true;
            view.strip_name = s(&with_name);
            view.strip_id = s(identity::short(&with));
            view.strip_online = here.is_some() || conv.is_some_and(|c| bo(c, "online"));
            view.strip_line = s(match here {
                Some(p) if !st(p, "activity").is_empty() => st(p, "activity"),
                _ if view.strip_online => "Online".to_string(),
                _ if conv.is_some_and(|c| bo(c, "friend")) => "Offline · they see your messages when they're back".to_string(),
                _ => "Only the two of you see this".to_string(),
            });
            if let Some(p) = here.filter(|p| bo(p, "joinable") && bo(v, "join_offered")) {
                view.strip_join = s(st(p, "id"));
            }
        }

        // The log, oldest first: a line for each day, the unseen from the marker on.
        let suffix = match self.tab.as_str() {
            "global" => " · Global is everyone on the community network",
            "lobby" => " · the players in your party or match",
            _ => " · only the two of you see this",
        };
        let mut last: Option<&Value> = None;
        let mut marked = false;
        for m in messages {
            let id = id_of(m);
            let (day, at) = (st(m, "day"), num(m, "at") as i64);
            let new_day = last.is_none_or(|l| st(l, "day") != day);
            if new_day {
                let text = if last.is_none() { format!("{day}{suffix}") } else { day.clone() };
                lines.push(crate::ChatLine {
                    kind: s("day"),
                    key: s(format!("day:{id}")),
                    text: s(format!("— {text} —")),
                    ..Default::default()
                });
            }
            let mut new_here = false;
            if !marked && !bo(m, "mine") && marker.is_some_and(|after| id > after) {
                marked = true;
                new_here = true;
                lines.push(crate::ChatLine {
                    kind: s("new"),
                    key: s("new"),
                    text: s("New traffic"),
                    ..Default::default()
                });
            }
            let together = !new_day && !new_here && last.is_some_and(|l| st(l, "from") == st(m, "from") && at - num(l, "at") as i64 <= GROUP_SECS);
            last = Some(m);
            let parts = arr(m, "parts");
            // A Discord invite shows as a card under the text, not in it.
            let invite = parts.iter().find(|p| bo(p, "link")).map(|p| st(p, "text")).unwrap_or_default();
            let body: String = parts.iter().filter(|p| invite.is_empty() || !bo(p, "link")).map(|p| st(p, "text")).collect();
            let body = if invite.is_empty() { body } else { body.trim().to_string() };
            let (mine, friend) = (bo(m, "mine"), bo(m, "friend"));
            let name = st(m, "from_name");
            lines.push(crate::ChatLine {
                kind: s("msg"),
                key: s(id.to_string()),
                time: s(st(m, "time")),
                who: s(if mine { "You".to_string() } else { name.clone() }),
                initial: s(initial(&name)),
                from: s(st(m, "from")),
                tone: s(if mine {
                    "me"
                } else if friend {
                    "friend"
                } else {
                    ""
                }),
                friend: friend && !mine,
                mine,
                mention: bo(m, "mentions_me"),
                head: !together,
                text: s(body),
                invite: s(invite),
                picked: self.picked == Some(id),
                reporting: self.reporting.as_ref().is_some_and(|(r, _)| *r == id),
                can_message: friend && self.tab != "messages",
                can_add: !friend && !mine,
            });
        }
        if let Some((rid, why)) = &self.reporting {
            view.report_who = s(messages.iter().find(|m| id_of(m) == *rid).map(|m| st(m, "from_name")).unwrap_or_default());
            view.report_why = s(why);
        }
        if let Some(link) = &self.invite {
            view.invite = s(link);
        }

        // Who's on Global: friends, then those in a match, then those in the menus.
        if self.tab == "global" {
            view.roster = true;
            view.roster_count = online.len() as i32;
            let in_match = |p: &&Value| !bo(p, "friend") && !st(p, "activity").is_empty() && !st(p, "activity").to_lowercase().contains("menu");
            let groups: [(&str, &str, Vec<&Value>); 3] = [
                ("Friends", "friend", online.iter().filter(|p| bo(p, "friend")).collect()),
                ("In a match", "match", online.iter().filter(in_match).collect()),
                ("In the menus", "menu", online.iter().filter(|p| !bo(p, "friend") && !in_match(p)).collect()),
            ];
            for (head, tone, people) in groups {
                if people.is_empty() {
                    continue;
                }
                roster.push(crate::RosterRow {
                    kind: s("head"),
                    name: s(format!("{head} · {}", people.len())),
                    ..Default::default()
                });
                for p in people.iter().take(ROSTER_GROUP) {
                    let id = st(p, "id");
                    let activity = st(p, "activity");
                    roster.push(crate::RosterRow {
                        kind: s("person"),
                        id: s(&id),
                        name: s(st(p, "name")),
                        line: s(if activity.is_empty() { "Online".to_string() } else { activity }),
                        tone: s(tone),
                        join: bo(p, "joinable") && bo(v, "join_offered"),
                        asked: bo(p, "asked"),
                        picked: self.picked_person.as_deref() == Some(id.as_str()),
                    });
                }
                if people.len() > ROSTER_GROUP {
                    roster.push(crate::RosterRow {
                        kind: s("more"),
                        name: s(format!("and {} more", people.len() - ROSTER_GROUP)),
                        ..Default::default()
                    });
                }
            }
        }

        // The dossier: the player picked in the roster, or a message's sender.
        match self.picked_person.as_deref() {
            Some(p) if p.starts_with("msg:") => {
                let mid: i64 = p[4..].parse().unwrap_or_default();
                if let Some(m) = messages.iter().find(|m| id_of(m) == mid && !bo(m, "mine")) {
                    let (name, from, friend) = (st(m, "from_name"), st(m, "from"), bo(m, "friend"));
                    let here = online.iter().find(|o| st(o, "name").eq_ignore_ascii_case(&name));
                    view.dossier = true;
                    view.dossier_name = s(&name);
                    view.dossier_initial = s(initial(&name));
                    view.dossier_id = s(identity::short(&from));
                    view.dossier_friend = friend;
                    let place = st(m, "server");
                    let doing = here
                        .map(|o| st(o, "activity"))
                        .filter(|a| !a.is_empty())
                        .unwrap_or_else(|| if here.is_some() { "Online".into() } else { "Not online here".into() });
                    view.dossier_line = s(if place.is_empty() { doing } else { format!("{doing} · {place}") });
                    if friend && self.tab != "messages" {
                        view.dossier_message = s(format!("message:{from}"));
                    }
                    if !friend {
                        view.dossier_add = s(format!("request-by-id:{from}:{name}"));
                    }
                    view.dossier_invite = s(format!("invite-by-id:{from}:{name}"));
                    view.dossier_report = s(format!("report:{mid}"));
                    view.dossier_block = s(format!("block-by-id:{from}:{name}"));
                }
            }
            Some(p) => {
                if let Some(o) = online.iter().find(|o| st(o, "id") == p) {
                    let (name, friend) = (st(o, "name"), bo(o, "friend"));
                    view.dossier = true;
                    view.dossier_name = s(&name);
                    view.dossier_initial = s(initial(&name));
                    view.dossier_friend = friend;
                    let activity = st(o, "activity");
                    view.dossier_line = s(if activity.is_empty() { "In the menus".to_string() } else { activity });
                    if friend {
                        // Their identity, to message them, from their conversation or the friends list.
                        if let Some(c) = convs.iter().find(|c| bo(c, "friend") && st(c, "name").eq_ignore_ascii_case(&name)) {
                            view.dossier_id = s(identity::short(&st(c, "with")));
                            view.dossier_message = s(format!("message:{}", st(c, "with")));
                        }
                    } else {
                        if bo(o, "asked") {
                            view.dossier_asked = true;
                        } else {
                            view.dossier_add = s(format!("request:{p}:{name}"));
                        }
                        view.dossier_block = s(format!("block:{p}:{name}"));
                    }
                    view.dossier_invite = s(format!("invite-game:{p}:{name}"));
                }
            }
            None => {}
        }
        self.page(view, tabs, lines, roster, notes, picks)
    }

    fn page(
        &self,
        view: crate::ChatView,
        tabs: Vec<crate::ChatTab>,
        lines: Vec<crate::ChatLine>,
        roster: Vec<crate::RosterRow>,
        notes: Vec<crate::ChatNote>,
        picks: Vec<crate::ChatPick>,
    ) -> PageView {
        PageView {
            kicker: "Comms".into(),
            title: "Chat".into(),
            section: self.room(),
            chat: Some(Box::new(ChatModel {
                view,
                tabs,
                lines,
                roster,
                notes,
                picks,
            })),
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
        self.reporting = None;
        self.picked_person = None;
        // The field empties with the room.
        self.sent += 1;
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        let v = json(&crate::chat::chat(app.clone(), app.state::<Chat>(), self.tab.clone(), self.with.clone(), true));
        self.build(&v)
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
                    Ok(()) => {
                        self.text.clear();
                        self.sent += 1;
                    }
                    Err(e) => self.refused = Some(e),
                }
            }
            "pick" => {
                let id: i64 = rest.parse().unwrap_or_default();
                self.picked = if self.picked == Some(id) { None } else { Some(id) };
            }
            "who" => {
                let to = format!("msg:{rest}");
                self.picked_person = if self.picked_person.as_deref() == Some(to.as_str()) { None } else { Some(to) };
            }
            "person" => {
                self.picked_person = if self.picked_person.as_deref() == Some(rest) { None } else { Some(rest.to_string()) };
            }
            "dossier-close" => self.picked_person = None,
            "delete" => {
                crate::chat::chat_delete(app.clone(), rest.parse().unwrap_or_default());
                self.picked = None;
            }
            "message" => self.section(&format!("with:{rest}")),
            "report" => {
                self.reporting = Some((rest.parse().unwrap_or_default(), String::new()));
                self.picked = None;
                self.picked_person = None;
            }
            "report-send" => {
                if let Some((id, why)) = self.reporting.take() {
                    crate::chat::chat_report(app.clone(), id, why);
                }
            }
            "report-cancel" => self.reporting = None,
            // By their identity: a name alone may be another account's on this server.
            "request-by-id" | "block-by-id" | "invite-by-id" => {
                crate::chat::chat_act(app.clone(), action.trim_end_matches("-by-id").into(), String::new(), who.into(), name.into());
                self.picked = None;
                self.picked_person = None;
            }
            "request" | "block" | "invite-game" => {
                let act = if action == "invite-game" { "invite" } else { action };
                crate::chat::chat_act(app.clone(), act.into(), who.into(), String::new(), name.into());
                self.picked_person = None;
            }
            "join" => {
                let core = app.state::<Launcher>().0.clone();
                crate::core::join_friend(&core, who, name);
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn msg(id: i64, at: i64, from: &str, name: &str, body: &str) -> Value {
        json!({ "id": id, "at": at, "time": "14:0", "day": "Today", "from": from, "from_name": name,
                "server": "Singapore", "parts": [{ "text": body, "link": false }], "mine": false,
                "mentions_me": false, "friend": false })
    }

    fn view(messages: Vec<Value>) -> Value {
        json!({ "state": "open", "messages": messages, "conversations": [], "online": [], "rules": "",
                "may_post_global": true, "lobby_ids": [] })
    }

    fn lines(p: &PageView) -> Vec<(String, bool)> {
        p.chat.as_ref().unwrap().lines.iter().map(|l| (l.kind.to_string(), l.head)).collect()
    }

    #[test]
    fn a_senders_lines_together_share_one_head() {
        let mut page = ChatPage::new();
        let v = view(vec![
            msg(1, 1000, "a", "Kestrel", "hi"),
            msg(2, 1060, "a", "Kestrel", "again"),
            msg(3, 2000, "a", "Kestrel", "later"),
        ]);
        let p = page.build(&v);
        let l = lines(&p);
        assert_eq!(l, vec![("day".into(), false), ("msg".into(), true), ("msg".into(), false), ("msg".into(), true)]);
    }

    #[test]
    fn new_traffic_marks_what_came_while_away() {
        let mut page = ChatPage::new();
        page.build(&view(vec![msg(1, 1000, "a", "Kestrel", "hi")]));
        page.section("lobby");
        page.build(&view(vec![]));
        page.section("global");
        let p = page.build(&view(vec![msg(1, 1000, "a", "Kestrel", "hi"), msg(2, 1010, "a", "Kestrel", "there")]));
        let kinds: Vec<String> = lines(&p).into_iter().map(|(k, _)| k).collect();
        assert_eq!(kinds, vec!["day", "msg", "new", "msg"]);
        // Its line starts the sender's lines again, with their tile.
        assert!(p.chat.as_ref().unwrap().lines[3].head);
    }
}
