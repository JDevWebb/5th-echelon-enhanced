//! The dialogs over the window (the Tauri launcher's components): the account dialog, What's
//! new, the questions before a first game (diagnostics, the new game options), how a game went,
//! and closing while a report sends. Each is a page of rows; `closed` says it's done.

use std::collections::HashMap;

use serde_json::Value;

use super::*;
use crate::commands::Launcher;
use crate::feedback::Feedback;
use crate::tauri::Manager;

pub enum Dialog {
    Account(Account),
    WhatsNew { view: Value, on_start: bool, closed: bool },
    Questions(Questions),
    Feedback(FeedbackAsk),
    Closing { waiting: bool },
}

/// What a dialog shows: a page, and whether it's done.
pub struct DialogView {
    pub page: PageView,
    pub width: f32,
    pub closed: bool,
}

#[derive(Default)]
pub struct Account {
    pub server: String,
    pub own: bool,
    name: String,
    probed: Option<Result<crate::core::AccountServer, String>>,
    api: Option<String>,
    version: String,
    ping: Option<u32>,
    /// The server moved its players (one of the network's relays now): where they play.
    moved_to: Option<String>,
    checks: HashMap<String, Result<launcher::flow::NameCheck, String>>,
    suggestions: Option<(String, Vec<String>)>,
    closed: bool,
}

impl Account {
    pub fn new(server: String, suggested: String, taken: bool, own: bool) -> Self {
        let mut a = Account {
            server,
            own,
            name: suggested.clone(),
            ..Default::default()
        };
        if taken {
            a.checks.insert(suggested.clone(), Ok(launcher::flow::NameCheck::Taken(suggested)));
        }
        a
    }

    /// The server as if it had answered, with `name` checked free (the screenshots).
    pub(crate) fn answered(&mut self, server: crate::core::AccountServer, version: &str, name: &str) {
        self.ping = server.ping;
        self.version = version.into();
        self.probed = Some(Ok(server));
        self.name = name.into();
        self.checks.insert(name.into(), Ok(launcher::flow::NameCheck::Free));
    }
}

#[derive(Default)]
pub struct Questions {
    example: Option<Vec<String>>,
    choices: Option<Vec<bool>>,
    closed: bool,
}

#[derive(Default)]
pub struct FeedbackAsk {
    good: Option<bool>,
    problems: Vec<String>,
    comment: String,
    attach: Option<bool>,
    started: bool,
    closed: bool,
}

/// Whether the player has questions to answer before the game starts.
pub fn questions_pending(app: &AppHandle) -> bool {
    let launcher = app.state::<Launcher>();
    let inner = launcher.lock();
    let game = inner.game.as_ref();
    launcher::diagnostics::Ask::pending(game) || !launcher::new_options::pending(game).is_empty()
}

impl Dialog {
    pub fn view(&mut self, app: &AppHandle) -> DialogView {
        match self {
            Dialog::Account(a) => a.view(app),
            Dialog::WhatsNew { view, closed, .. } => DialogView {
                closed: *closed,
                ..whats_new(view)
            },
            Dialog::Questions(q) => q.view(app),
            Dialog::Feedback(f) => f.view(app),
            Dialog::Closing { waiting } => closing(*waiting),
        }
    }

    pub fn live_edits(&self) -> bool {
        matches!(self, Dialog::Account(_) | Dialog::Feedback(_))
    }

    pub fn toggled(&mut self, _app: &AppHandle, id: &str, on: bool) {
        match self {
            Dialog::Questions(q) => {
                if let Some(c) = id
                    .strip_prefix("option:")
                    .and_then(|i| i.parse::<usize>().ok())
                    .and_then(|i| q.choices.as_mut()?.get_mut(i))
                {
                    *c = on;
                }
            }
            Dialog::Feedback(f) => {
                if id == "attach" {
                    f.attach = Some(on);
                } else if let Some(p) = id.strip_prefix("problem:") {
                    f.problems.retain(|x| x != p);
                    if on {
                        f.problems.push(p.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    pub fn chosen(&mut self, _app: &AppHandle, id: &str, index: usize) {
        if let (Dialog::Feedback(f), "good") = (self, id) {
            f.good = Some(index == 0);
        }
    }

    pub fn edited(&mut self, id: &str, text: &str) {
        match (self, id) {
            (Dialog::Account(a), "name") => a.name = text.into(),
            (Dialog::Feedback(f), "comment") => f.comment = text.chars().take(2000).collect(),
            _ => {}
        }
    }

    pub fn pressed(&mut self, app: &AppHandle, id: &str) {
        // Esc: what each dialog's way out is (the questions wait for an answer).
        let id = match (id, &*self) {
            ("escape", Dialog::Account(_)) => "close",
            ("escape", Dialog::WhatsNew { .. }) => "done",
            ("escape", Dialog::Feedback(_)) => "later",
            ("escape", _) => return,
            (id, _) => id,
        };
        match self {
            Dialog::Account(a) => match id {
                "create" | "name" => {
                    let name = a.name.trim().to_string();
                    let ready = !name.is_empty()
                        && !matches!(
                            a.checks.get(&name),
                            Some(Ok(launcher::flow::NameCheck::Taken(_) | launcher::flow::NameCheck::NotAllowed(_)))
                        );
                    if ready {
                        a.closed = true;
                        crate::core::connect(&app.state::<Launcher>().0, &a.server, Some(name));
                    }
                }
                "close" => a.closed = true,
                _ if id.starts_with("use:") => {
                    a.name = id[4..].to_string();
                    a.checks.insert(a.name.clone(), Ok(launcher::flow::NameCheck::Free));
                }
                _ => {}
            },
            Dialog::WhatsNew { on_start, closed, .. } => match id {
                "community" => crate::whats_new::use_community_network(app.clone(), app.state::<Launcher>()),
                super::KOFI_ID => {
                    let _ = crate::core::open_url(super::about_link(id));
                }
                _ if id.starts_with("link:") => {
                    let _ = crate::core::open_url(&id[5..]);
                }
                _ => {
                    if *on_start {
                        crate::whats_new::whats_new_seen(app.state::<crate::whats_new::WhatsNew>());
                    }
                    *closed = true;
                }
            },
            Dialog::Questions(q) => q.pressed(app, id),
            Dialog::Feedback(f) => f.pressed(app, id),
            Dialog::Closing { waiting } => match id {
                "now" => {
                    app.window(crate::tauri::WindowCall::Quit);
                }
                "wait" => *waiting = true,
                _ => {}
            },
        }
    }

    /// Whether it's done.
    pub fn closed(&self) -> bool {
        match self {
            Dialog::WhatsNew { closed, .. } => *closed,
            Dialog::Account(a) => a.closed,
            Dialog::Questions(q) => q.closed,
            Dialog::Feedback(f) => f.closed,
            _ => false,
        }
    }
}

impl Account {
    fn view(&mut self, app: &AppHandle) -> DialogView {
        let core = app.state::<Launcher>().0.clone();
        if self.probed.is_none() {
            let server = crate::core::account_server(&core, &self.server, Some(self.own));
            self.ping = server.ping;
            match crate::core::account_probe(&core, &self.server) {
                Ok(p) => {
                    self.api = p.name_check.then(|| p.api.clone());
                    self.version = p.version.clone();
                    self.ping = self.ping.or(p.ping);
                    self.moved_to = p.moved_to.clone();
                    self.probed = Some(Ok(server));
                }
                Err(e) => self.probed = Some(Err(e)),
            }
        }
        let name = self.name.trim().to_string();
        if let Some(api) = self.api.clone() {
            if !name.is_empty() && !self.checks.contains_key(&name) {
                let checked = launcher::flow::check_name(&api, &name);
                self.checks.insert(name.clone(), checked);
            }
            if matches!(self.checks.get(&name), Some(Ok(launcher::flow::NameCheck::Taken(_)))) && self.suggestions.as_ref().is_none_or(|(n, _)| *n != name) {
                self.suggestions = Some((name.clone(), crate::core::name_suggestions(&api, &name)));
            }
        }
        let mut server = match &self.probed {
            Some(Ok(s)) => s.clone(),
            _ => crate::core::AccountServer {
                place: self.server.clone(),
                ..Default::default()
            },
        };
        // A relay of the network (global matchmaking): the setup goes where its players play,
        // so it isn't a server of the player's own, and nothing's left behind.
        let moved_to = self.moved_to.clone();
        if moved_to.is_some() {
            server.own = false;
            server.kind = "One of the network's relays".into();
        }
        let taken_from_start = matches!(self.checks.values().next(), Some(Ok(launcher::flow::NameCheck::Taken(_)))) && self.checks.len() == 1;
        let mut r = Rows::default();
        let version = if self.version.is_empty() {
            String::new()
        } else {
            format!(" · 5th Echelon {}", self.version)
        };
        let ping = self.ping.map_or("no answer".to_string(), |p| format!("{p} ms"));
        r.add(item(server.place.clone(), format!("{}{version}", server.kind)).text(ping).tone("accent"));
        r.add(note(if let Some(to) = &moved_to {
            format!(
                "{}'s players play on {to} now (it's one of the network's relays). Connecting sets you up there, with your name and identity.",
                self.server
            )
        } else if taken_from_start {
            format!("Someone on this server already has the name {name} (an account from before names were kept for each player), so you need another one here. Your friends still see it's you.")
        } else {
            "You don't have an account here yet. Pick the name other players see. Your identity signs you in from now on: no password to remember.".to_string()
        }));
        if let Some(Err(e)) = &self.probed {
            r.add(warn(e.clone()));
        }
        r.add(field("name", "Your name", self.name.clone()));
        let state = self.checks.get(&name);
        if !name.is_empty() {
            match state {
                Some(Ok(launcher::flow::NameCheck::Free)) => {
                    r.add(result("Free", "ok", ""));
                }
                Some(Ok(launcher::flow::NameCheck::Taken(_))) => {
                    r.add(result("Taken on this server", "fail", ""));
                }
                Some(Ok(launcher::flow::NameCheck::NotAllowed(why))) => {
                    r.add(result(why.clone(), "fail", ""));
                }
                Some(Err(e)) => {
                    r.add(note(format!("Couldn't check it: {e}")));
                }
                None if self.api.is_some() => {
                    r.add(working("Checking…"));
                }
                None => {}
            }
        }
        if let Some((n, list)) = &self.suggestions {
            if *n == name && !list.is_empty() {
                r.add(buttons(list.iter().map(|s| super::b(format!("use:{s}"), s.clone()).kind("ghost")).collect()));
            }
        }
        r.add_if(server.own, || {
            note(format!(
                "This leaves {}: your friends and stats there stay there. The server menu on the Play screen takes you back.",
                server.leaving
            ))
        });
        let ready = !name.is_empty() && !matches!(state, Some(Ok(launcher::flow::NameCheck::Taken(_) | launcher::flow::NameCheck::NotAllowed(_))));
        let cancel = match (&server.current, server.own) {
            (Some(current), false) => format!("Stay on {current}"),
            _ => "Cancel".into(),
        };
        let mut row = buttons(vec![
            super::b("close", cancel),
            super::b(
                "create",
                match (&moved_to, server.own) {
                    (Some(to), _) => format!("Connect to {to}"),
                    (None, true) => "Create account and connect".into(),
                    (None, false) => "Create account".into(),
                },
            )
            .kind("primary")
            .enabled(ready),
        ]);
        row.tone = "end";
        r.add(row);
        DialogView {
            page: PageView {
                title: if let Some(to) = &moved_to {
                    format!("Connect to {to}")
                } else if server.own {
                    format!("Connect to {}", self.server)
                } else {
                    "Choose your name on this server".into()
                },
                rows: r.done(),
                ..Default::default()
            },
            width: 520.0,
            closed: self.closed,
        }
    }
}

pub(crate) fn whats_new(v: &Value) -> DialogView {
    let mut r = Rows::default();
    let releases = arr(v, "releases");
    let Some(newest) = releases.first() else {
        return DialogView {
            page: PageView::default(),
            width: 560.0,
            closed: true,
        };
    };
    let skipped = releases.len() > 1;
    let from = st(v, "from");
    // The mark and the name: the launcher's identity, on the page that says what it is now.
    r.add(mark(
        format!("5th Echelon Enhanced {}", st(newest, "version")),
        if from.is_empty() { st(v, "to") } else { format!("{from} to {}", st(v, "to")) },
    ));
    let thanks = st(newest, "thanks");
    if !thanks.is_empty() {
        r.card("Thanks to your reports");
        r.add(text(format!("{thanks} sent reports from the launcher: much of this release came from them.")));
        r.add(note("Send yours from Settings › Feedback."));
    }
    r.card(&if skipped { format!("In {}", st(newest, "version")) } else { "New".into() });
    let community = launcher::prefs::Prefs::directory().as_deref().is_some_and(launcher::prefs::Prefs::is_community);
    let mut offered = false;
    for it in arr(newest, "new") {
        let mut row = item(st(it, "title"), st(it, "text"));
        if bo(it, "community") {
            row.text = "Community network".into();
        }
        r.add(row);
        if !community && bo(it, "community") && !offered {
            offered = true;
            r.add(note("Your launcher uses another network, which doesn't have these."));
            r.add(buttons(vec![super::b("community", "Switch to the community network ›").kind("link")]));
        }
    }
    let fixed = arr(newest, "fixed");
    if skipped {
        if let Some(first) = fixed.first() {
            let n = fixed.len();
            r.add(note(format!(
                "Plus {}, among them {}.",
                if n == 1 { "1 fix".to_string() } else { format!("{n} fixes") },
                first.get(0).and_then(Value::as_str).unwrap_or_default()
            )));
        }
        let headlines = num(v, "headlines") as usize;
        for older in &releases[1..] {
            r.card(&format!("And in {}, which you skipped", st(older, "version")));
            for it in arr(older, "new").iter().take(headlines.max(1)) {
                r.add(text(format!("• {}", st(it, "title"))));
            }
        }
    } else if !fixed.is_empty() {
        r.card("Fixed");
        for f in fixed {
            r.add(text(format!(
                "• {}{}",
                f.get(0).and_then(Value::as_str).unwrap_or_default(),
                f.get(1).and_then(Value::as_str).unwrap_or_default()
            )));
        }
    }
    let notes = arr(v, "notes");
    let (label, url) = (
        notes.first().and_then(Value::as_str).unwrap_or("Release notes"),
        notes.get(1).and_then(Value::as_str).unwrap_or_default(),
    );
    r.out();
    // Supporting the community servers: under the notes, small, so the release still leads.
    r.add(
        super::support(
            "Support the community servers",
            "Free, always. If you'd like to chip in towards hosting, you can on Ko-fi. Entirely optional, no perks.",
            "",
            "",
            true,
        )
        .button(super::b(super::KOFI_ID, "Ko-fi").kind("plain")),
    );
    r.add(space());
    let mut row = buttons(vec![super::b(format!("link:{url}"), label).kind("link"), super::b("done", "Got it").kind("primary")]);
    row.tone = "end";
    r.add(row);
    DialogView {
        page: PageView {
            kicker: "Updated".into(),
            title: "What's new".into(),
            rows: r.done(),
            ..Default::default()
        },
        width: 600.0,
        closed: false,
    }
}

fn closing(waiting: bool) -> DialogView {
    let mut r = Rows::default();
    r.add(note(
        "Wait and the launcher closes as soon as it's sent, or close now and it's sent the next time you start the launcher.",
    ));
    if waiting {
        r.add(working("Waiting for it to send…"));
    }
    let mut row = buttons(vec![
        super::b("now", "Close now, send it next time"),
        super::b("wait", "Wait, then close").kind("primary").enabled(!waiting),
    ]);
    row.tone = "end";
    r.add(row);
    DialogView {
        page: PageView {
            title: "Your report is still sending".into(),
            rows: r.done(),
            ..Default::default()
        },
        width: 480.0,
        closed: false,
    }
}

impl Questions {
    fn view(&mut self, app: &AppHandle) -> DialogView {
        use launcher::diagnostics as d;
        use launcher::new_options as o;
        let launcher = app.state::<Launcher>();
        let inner = launcher.lock();
        let game = inner.game.as_ref();
        let diagnostics = d::Ask::pending(game);
        let pending = o::pending(game);
        let offered = game.map(|g| o::offered(&pending, &g.dir)).unwrap_or_default();
        let blocked: Vec<Option<(&str, &str)>> = pending.iter().map(|op| game.and_then(|g| (op.blocked)(&g.dir))).collect();
        drop(inner);
        let mut r = Rows::default();
        if !diagnostics && pending.is_empty() {
            self.closed = true;
        }
        let title;
        if diagnostics {
            title = d::TITLE.to_string();
            r.add(text(d::WHAT));
            r.add(text(d::PRIVATE));
            if let Some(example) = &self.example {
                r.add(code(if example.is_empty() {
                    "Nothing yet: your game hasn't written a log here.".to_string()
                } else {
                    example.join("\n")
                }));
            }
            r.add(note(d::LATER));
            let mut row = Vec::new();
            if self.example.is_none() {
                row.push(super::b("example", "Show me an example").kind("link"));
            }
            // The two answers alike: neither is the one to pick.
            row.push(super::b("keep", "Keep sending"));
            row.push(super::b("off", "Turn it off"));
            let mut row = buttons(row);
            row.tone = "end";
            r.add(row);
        } else {
            title = "New game options".to_string();
            r.add(note(o::intro(&pending)));
            let choices = self.choices.get_or_insert_with(|| offered.clone());
            if choices.len() != pending.len() {
                *choices = offered.clone();
            }
            for (i, op) in pending.iter().enumerate() {
                let mut row = switch(format!("option:{i}"), op.title, choices[i]).sub(blocked[i].map_or(op.text, |b| b.1));
                if let Some((badge, _)) = blocked[i] {
                    row = row.enabled(false);
                    row.text = badge.to_uppercase();
                } else if op.recommended {
                    row.text = "RECOMMENDED".into();
                }
                r.add(row);
            }
            r.add(note(o::LATER));
            let mut row = buttons(vec![super::b("save", "Save choices").kind("primary")]);
            row.tone = "end";
            r.add(row);
        }
        DialogView {
            page: PageView {
                kicker: "Before you play".into(),
                title,
                rows: r.done(),
                ..Default::default()
            },
            width: 600.0,
            closed: self.closed,
        }
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        let launcher = app.state::<Launcher>();
        let mut inner = launcher.lock();
        let crate::core::Inner { game, notices, .. } = &mut *inner;
        let Some(game) = game.as_mut() else {
            self.closed = true;
            return;
        };
        match id {
            "example" => self.example = Some(launcher::diagnostics::example(game)),
            "keep" | "off" => launcher::diagnostics::answer_with(game, notices, id == "keep"),
            "save" => {
                let pending = launcher::new_options::pending(Some(game));
                if let Some(choices) = self.choices.take().filter(|c| c.len() == pending.len()) {
                    launcher::new_options::save(game, notices, &pending, &choices);
                    notices.info("Saved. Settings › Game changes them any time.");
                }
            }
            _ => {}
        }
        drop(inner);
        crate::commands::changed(app);
    }
}

impl FeedbackAsk {
    fn view(&mut self, app: &AppHandle) -> DialogView {
        let Some(ask) = crate::feedback::feedback(app.state::<Feedback>()).map(|a| json(&a)) else {
            self.closed = true;
            return DialogView {
                page: PageView::default(),
                width: 520.0,
                closed: true,
            };
        };
        if !self.started {
            self.started = true;
            self.problems = arr(&ask, "ticked").iter().filter_map(|t| t.as_str().map(String::from)).collect();
            self.attach = Some(bo(&ask, "attach"));
        }
        let mut r = Rows::default();
        r.add(note(st(&ask, "why")));
        r.add_if(!st(&ask, "mismatch").is_empty(), || note(st(&ask, "mismatch")));
        r.add(segments(
            "good",
            "",
            &[("good".into(), "Good".into()), ("bad".into(), "Not great".into())],
            match self.good {
                Some(true) => "good",
                Some(false) => "bad",
                None => "",
            },
        ));
        r.add(bold("What went wrong? (tick any)"));
        for (id, label) in pairs(arr(&ask, "problems")) {
            r.add(switch(format!("problem:{id}"), label, self.problems.contains(&id)));
        }
        r.add(area("comment", "Anything else? (who you played with, what you were doing)", self.comment.clone()).lines_height(80.0));
        let attach = self.attach.unwrap_or(true);
        r.add(switch("attach", "Send my logs too", attach));
        r.add(note("Your PC's name, your user folder and your internet address are hidden before they're sent."));
        if attach {
            let files: Vec<String> = arr(&ask, "files")
                .iter()
                .filter_map(|f| Some(format!("{} ({} KB)", f.get(0)?.as_str()?, f.get(1)?.as_u64()?)))
                .collect();
            r.add_if(!files.is_empty(), || note(format!("What's sent: {}", files.join(", "))));
        }
        let can_send = self.good.is_some() || !self.problems.is_empty() || !self.comment.trim().is_empty();
        let mut acts = vec![super::b("send", "Send").kind("primary").enabled(can_send), super::b("later", "Not now")];
        if !bo(&ask, "manual") {
            acts.push(super::b("never", "Don't ask again"));
        }
        let mut row = buttons(acts);
        row.tone = "end";
        r.add(row);
        DialogView {
            page: PageView {
                title: "How did that go?".into(),
                rows: r.done(),
                ..Default::default()
            },
            width: 560.0,
            closed: self.closed,
        }
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        match id {
            "send" => {
                crate::feedback::feedback_send(
                    app.clone(),
                    app.state::<Feedback>(),
                    self.good,
                    self.problems.clone(),
                    self.comment.clone(),
                    self.attach.unwrap_or(true),
                );
                self.closed = true;
            }
            "later" | "never" => {
                crate::feedback::feedback_close(app.state::<Feedback>(), id == "never");
                self.closed = true;
            }
            _ => {}
        }
    }
}
