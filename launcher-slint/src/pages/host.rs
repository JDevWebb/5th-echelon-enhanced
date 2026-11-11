//! Host (the Tauri launcher's Host.vue): run a 5th Echelon server on this PC, and manage a
//! server (this one or another) through its admin API: players, games and the log.

use serde_json::Value;

use super::*;
use crate::host::CommunityApi;
use crate::host::Host;
use crate::tauri::Manager;

const LEVELS: [(&str, &str); 4] = [
    ("debug", "Debug and above"),
    ("info", "Info and above"),
    ("warn", "Warn and above"),
    ("error", "Error and above"),
];

pub struct HostPage {
    listen: Option<String>,
    public_ip: String,
    trusted: Option<String>,
    api: Option<[bool; 4]>,
    start_error: Option<String>,
    address: String,
    key: String,
    plain: bool,
    connect_error: Option<String>,
    tab: usize,
    confirm: Option<(String, String, String)>,
    min_level: usize,
    /// Managing another server: its address and key shown.
    manage_open: bool,
    /// At compact sizes while it runs: its settings shown.
    settings_open: bool,
}

impl Default for HostPage {
    fn default() -> Self {
        Self {
            listen: None,
            public_ip: String::new(),
            trusted: None,
            api: None,
            start_error: None,
            address: String::new(),
            key: String::new(),
            plain: false,
            connect_error: None,
            tab: 0,
            confirm: None,
            min_level: 1,
            manage_open: false,
            settings_open: false,
        }
    }
}

/// How long it's run, in words.
fn uptime(secs: u64) -> String {
    match secs {
        0..=59 => "under a minute".into(),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs / 60 % 60),
    }
}

impl HostPage {
    /// The page from what the server here is doing (`v`, crate::host's HostView as JSON), the
    /// managed server's players and games (`lists`), the latest log lines (`logs`), and the size
    /// band: the control room. Not downloaded: one card to get it. Else its status and
    /// Start/Stop by the title; at regular and wide sizes two columns (its settings, or a summary
    /// of them while it runs, and how players reach it; the counts and the players, games and
    /// log), at compact one.
    pub fn build(&mut self, v: &Value, lists: Option<&Value>, logs: &[Value], compact: bool) -> PageView {
        let running = bo(v, "running");
        let starting = bo(v, "starting");
        let found = bo(v, "found");
        let managing = st(v, "managing");
        if let Some(api) = v.get("api").filter(|a| !a.is_null()) {
            self.api.get_or_insert([bo(api, "info"), bo(api, "presence"), bo(api, "accounts"), bo(api, "unhandled")]);
        }
        let locked = running || starting;
        let mut view = PageView {
            kicker: "Servers · host on this PC".into(),
            title: "Host a server".into(),
            ..Default::default()
        };
        if !found {
            let mut r = Rows::default();
            r.card("");
            let mut get = mark(
                "Run a server for your group",
                format!(
                    "A LAN party, or friends over the internet. Their launchers find it on your network; players elsewhere connect by your address. The server ({}) goes next to the launcher.",
                    st(v, "exe_name")
                ),
            )
            .sub("Signed like every release, and checked before it runs.");
            if bo(v, "downloading") {
                r.add(get);
                r.add(working("Downloading…"));
            } else {
                get.buttons = vec![
                    super::b("download", "Download the server").kind("primary"),
                    super::b("releases", "Release notes").kind("link"),
                ];
                r.add(get);
            }
            if !st(v, "error").is_empty() {
                r.add(bad(st(v, "error")));
            }
            self.manage(&managing, &mut r);
            view.rows = r.done();
            view.layout = "narrow";
            return view;
        }
        view.status = if starting {
            ("Starting…".into(), "warn")
        } else if running {
            (format!("Running · {}", v.get("uptime").and_then(Value::as_u64).map_or("just now".into(), uptime)), "ok")
        } else {
            ("Stopped".into(), "")
        };
        view.actions = vec![if running {
            super::b("stop", "Stop")
        } else {
            super::b("start", if starting { "Starting…" } else { "Start server" }).kind("primary").enabled(!starting)
        }];
        // The managed server's lists: this PC's while it runs, or another's connected to.
        let live = !managing.is_empty();
        let mut left = Rows::default();
        let mut right = Rows::default();
        if let Some(e) = &self.start_error {
            left.out();
            left.add(bad(e.clone()));
        }
        if !st(v, "error").is_empty() {
            left.out();
            left.add(bad(st(v, "error")));
        }
        if compact {
            // One column: how players reach it (and, while it runs, its settings folded away),
            // then the lists.
            // The addresses and the counts in one strip, its settings behind a link while it
            // runs.
            self.reach(v, true, &mut left);
            if live || locked {
                let mut said = Vec::new();
                if live {
                    let n = |key: &str| lists.and_then(|l| l.get(key)).and_then(|l| l.get("Ok")).and_then(Value::as_array).map(Vec::len);
                    if let Some(n) = n("players") {
                        said.push(format!("{n} player{}", if n == 1 { "" } else { "s" }));
                    }
                    if let Some(n) = n("games") {
                        said.push(format!("{n} game{}", if n == 1 { "" } else { "s" }));
                    }
                    let warnings = logs.iter().filter(|l| matches!(st(l, "tone").as_str(), "warn" | "error")).count();
                    if bo(v, "local") && warnings > 0 {
                        said.push(format!("{warnings} warning{} in the log", if warnings == 1 { "" } else { "s" }));
                    }
                }
                let mut strip = line(said.join(" · "));
                if locked {
                    strip = strip.button(super::b("settings", if self.settings_open { "Hide settings" } else { "Settings" }).kind("link"));
                }
                left.add(strip);
            }
            if !locked {
                self.settings(v, &mut left);
            } else if self.settings_open {
                self.summary(v, &mut left);
            }
            if live {
                self.lists(v, lists, logs, &mut left);
            } else {
                waiting(&mut left);
            }
            self.manage(&managing, &mut left);
            view.rows = left.done();
            view.layout = "wide";
            return view;
        }
        if locked {
            self.reach(v, false, &mut left);
            self.summary(v, &mut left);
        } else {
            self.settings(v, &mut left);
            self.reach(v, false, &mut right);
        }
        if live {
            counts(lists, logs, bo(v, "local"), &mut right);
            self.lists(v, lists, logs, &mut right);
        } else {
            waiting(&mut right);
        }
        self.manage(&managing, &mut right);
        view.rows = left.done();
        view.side = right.done();
        view.split = 0.4;
        view.layout = "wide";
        view
    }

    /// The settings, to change while it's stopped: the network and the community API.
    fn settings(&self, v: &Value, r: &mut Rows) {
        let mut adapters = vec![(String::new(), "Every address".to_string())];
        for a in arr(v, "adapters") {
            if let (Some(name), Some(ip)) = (a.get(0).and_then(Value::as_str), a.get(1).and_then(Value::as_str)) {
                adapters.push((ip.to_string(), format!("{ip} · {name}")));
            }
        }
        let listen = self.listen.clone().unwrap_or_else(|| st(v, "listen"));
        r.card("Network");
        r.add(choice("listen", "Listen on", &adapters, &listen));
        r.add(field("public_ip", "Public address", self.public_ip.clone()).placeholder("only behind NAT"));
        let trusted = self.trusted.clone().unwrap_or_else(|| st(v, "trusted_subnet"));
        r.add(field("trusted", "Trusted subnet", trusted).placeholder("e.g. 10.8.0.0/16, the VPN players use"));
        r.add(note(
            "Players connecting from the trusted subnet always advertise the address the server sees, fixing joins when the game picks the wrong adapter.",
        ));
        if let Some(on) = self.api {
            r.card("Community API · port 80, for launchers and the overlay");
            r.add(switch("api:0", "Server info", on[0]));
            r.add(switch("api:1", "Who's online", on[1]).sub("Shares every username and what online players are doing."));
            r.add(
                switch("api:2", "Sign-up for tools", on[2]).sub("Lets tools create accounts through /api/register (rate-limited). Refused when every account needs an identity."),
            );
            r.add(switch("api:3", "Unhandled calls (development)", on[3]));
        }
    }

    /// The settings it runs with, read-only.
    fn summary(&self, v: &Value, r: &mut Rows) {
        let listen = self.listen.clone().unwrap_or_else(|| st(v, "listen"));
        let trusted = self.trusted.clone().unwrap_or_else(|| st(v, "trusted_subnet"));
        r.card("Settings · locked while it runs");
        r.add(pair("Listen on", if listen.is_empty() { "Every address".into() } else { listen }, false));
        if !self.public_ip.trim().is_empty() {
            r.add(pair("Public address", self.public_ip.trim(), true));
        }
        r.add(pair("Trusted subnet", if trusted.trim().is_empty() { "None".into() } else { trusted }, false));
        if let Some(on) = self.api {
            let names: Vec<&str> = ["Server info", "who's online", "sign-up for tools", "unhandled calls"]
                .iter()
                .zip(on)
                .filter(|(_, on)| *on)
                .map(|(n, _)| *n)
                .collect();
            let list = if names.is_empty() { "Off".to_string() } else { names.join(", ") };
            let mut list = list;
            if let Some(first) = list.get(..1) {
                list = first.to_uppercase() + &list[1..];
            }
            r.add(pair("Community API", list, false));
        }
    }

    /// How players reach it: this PC's address on its network, and the public one when set.
    fn reach(&self, v: &Value, short: bool, r: &mut Rows) {
        let listen = self.listen.clone().unwrap_or_else(|| st(v, "listen"));
        let ips: Vec<String> = arr(v, "adapters").iter().filter_map(|a| a.get(1).and_then(Value::as_str).map(String::from)).collect();
        let (local, more) = if !listen.is_empty() {
            (listen, 0)
        } else {
            (ips.first().cloned().unwrap_or_else(|| "This PC".into()), ips.len().saturating_sub(1))
        };
        let local_line = if short {
            String::new()
        } else if more > 0 {
            format!(
                "Or this PC's {more} other address{}. Launchers on the network find it by themselves.",
                if more == 1 { "" } else { "es" }
            )
        } else {
            "Launchers on this network find it by themselves.".into()
        };
        let public = self.public_ip.trim();
        r.card("How players reach you");
        r.add(stats(
            &[
                ("On your network", local, local_line),
                if public.is_empty() {
                    (
                        "Over the internet",
                        "Not set".into(),
                        if short {
                            "Behind a router: set its public address.".into()
                        } else {
                            "Behind a router, set the public address (with its ports forwarded) for players elsewhere.".into()
                        },
                    )
                } else {
                    (
                        "Over the internet",
                        public.to_string(),
                        if short { String::new() } else { "Give friends elsewhere this address.".into() },
                    )
                },
            ],
            true,
        ));
    }

    /// Players, games and the log as tabs, with what can be done to each.
    fn lists(&self, v: &Value, lists: Option<&Value>, logs: &[Value], r: &mut Rows) {
        let managing = st(v, "managing");
        let local = bo(v, "local");
        let list = |key: &str| lists.and_then(|l| l.get(key));
        let ok = |key: &str| list(key).map(|l| arr(l, "Ok").len());
        let label = |name: &str, n: Option<usize>| n.map_or(name.to_string(), |n| format!("{name} · {n}"));
        r.card("");
        if !local {
            let mut row = vec![super::b("refresh", "Refresh").kind("fix")];
            row.push(super::b("disconnect", "Disconnect").kind("fix"));
            r.add(item(managing.clone(), "Managing this server").buttons(row));
        }
        r.add(segments(
            "tab",
            "",
            &[
                ("players".into(), label("Players", ok("players"))),
                ("games".into(), label("Games", ok("games"))),
                ("logs".into(), "Log".into()),
            ],
            ["players", "games", "logs"][self.tab],
        ));
        if let Some((what, _, name)) = &self.confirm {
            r.add(warn(if what == "player" {
                format!("Delete the account {name}? It can't be undone.")
            } else {
                format!("End {name}?")
            }));
            r.add(buttons(vec![super::b("delete", "Yes").kind("danger"), super::b("no", "No")]));
        }
        if self.tab < 2 {
            let key = if self.tab == 0 { "players" } else { "games" };
            // Who's in which game, by name, for each player's line.
            let games = list("games").map(|g| arr(g, "Ok").to_vec()).unwrap_or_default();
            match list(key) {
                Some(l) if l.get("Err").is_some() => {
                    r.add(bad(st(l, "Err")));
                }
                Some(l) => {
                    let items = arr(l, "Ok");
                    if items.is_empty() {
                        r.add(note(if self.tab == 0 { "No players yet." } else { "No games running." }));
                    }
                    for x in items {
                        if self.tab == 0 {
                            let name = st(x, "name");
                            let in_game = games
                                .iter()
                                .find(|g| arr(g, "players").iter().any(|p| p.as_str() == Some(name.as_str())))
                                .map(|g| format!("In game #{}", st(g, "id")))
                                .unwrap_or_default();
                            r.add(
                                item(name.clone(), st(x, "ips"))
                                    .text(in_game)
                                    .button(super::b(format!("ask:player:{}:{}", st(x, "id"), name), "Delete…").kind("fix")),
                            );
                        } else {
                            let id = st(x, "id");
                            r.add(item(format!("#{id} {}", st(x, "kind")), st(x, "who")).button(super::b(format!("ask:game:{id}:game #{id}"), "End…").kind("fix")));
                        }
                    }
                }
                None => {
                    r.add(working("Reading…"));
                }
            }
            // The latest lines of this PC's log under them.
            if local && !logs.is_empty() {
                r.add(caps("Latest log lines"));
                for l in &logs[logs.len().saturating_sub(4)..] {
                    r.add(log_line(l));
                }
            }
        } else {
            r.add(choice(
                "min_level",
                "Detail",
                &LEVELS.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>(),
                LEVELS[self.min_level].0,
            ));
            if logs.is_empty() {
                r.add(note("Nothing logged yet."));
            }
            for l in &logs[logs.len().saturating_sub(120)..] {
                r.add(log_line(l));
            }
        }
    }

    /// Managing a server elsewhere: a line offering it, or its address and admin key.
    fn manage(&self, managing: &str, r: &mut Rows) {
        if !managing.is_empty() {
            return;
        }
        if !self.manage_open {
            r.out();
            r.add(line("Run a server elsewhere? Manage it from here with its address and admin key.").button(super::b("manage", "Manage another server…")));
            return;
        }
        r.card("Manage another server");
        r.add(note("Its address and admin key (admin-key.txt next to that server), with its admin API on."));
        r.add(field("address", "Server address", self.address.clone()));
        r.add(
            field("key", "Admin key", self.key.clone())
                .secret()
                .button(super::b("connect", "Connect").enabled(!self.address.trim().is_empty() && !self.key.trim().is_empty())),
        );
        if self.plain {
            r.add(warn(format!(
                "{} isn't encrypted: anyone on the way can read the admin key. Use https:// if the server has it.",
                self.address
            )));
            r.add(buttons(vec![super::b("connect-plain", "Allow, then Connect")]));
        }
        if let Some(e) = &self.connect_error {
            r.add(bad(e.clone()));
        }
        r.add(buttons(vec![super::b("manage-close", "Cancel").kind("link")]));
    }
}

/// The counts over the lists: accounts, games, and the log's warnings (this PC's server).
fn counts(lists: Option<&Value>, logs: &[Value], local: bool, r: &mut Rows) {
    let n = |key: &str| {
        lists
            .and_then(|l| l.get(key))
            .and_then(|l| l.get("Ok"))
            .and_then(Value::as_array)
            .map_or("–".to_string(), |a| a.len().to_string())
    };
    let mut tiles = vec![("Players", n("players"), "accounts on it".to_string()), ("Games", n("games"), "running".to_string())];
    let warnings = logs.iter().filter(|l| matches!(st(l, "tone").as_str(), "warn" | "error")).count();
    if local {
        tiles.push(("Log", warnings.to_string(), format!("warning{} in the latest lines", if warnings == 1 { "" } else { "s" })));
    }
    r.out();
    r.add(stats(&tiles, false).tone(if local && warnings > 0 { "warn" } else { "" }));
}

/// The lists' place while nothing's managed.
fn waiting(r: &mut Rows) {
    r.card("Players, games and the log");
    r.add(placeholder("Start the server to see who's on it.", "Its settings can't change while it runs."));
}

fn log_line(l: &Value) -> R {
    let tone = match st(l, "tone").as_str() {
        "error" | "bad" => "bad",
        "warn" => "warn",
        _ => "",
    };
    message(st(l, "time"), st(l, "msg"), "").tone(tone)
}

impl Screen for HostPage {
    fn live_edits(&self) -> bool {
        true
    }

    fn refresh_every(&self) -> u64 {
        3
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        let host = app.state::<Host>();
        let v = json(&crate::host::host(host));
        let lists = if st(&v, "managing").is_empty() {
            None
        } else {
            crate::host::host_lists(app.state::<Host>()).map(|l| json(&l))
        };
        let logs: Vec<Value> = if bo(&v, "found") {
            crate::host::host_logs(app.state::<Host>(), LEVELS[self.min_level].0.into()).iter().map(json).collect()
        } else {
            Vec::new()
        };
        self.build(&v, lists.as_ref(), &logs, super::compact())
    }

    fn toggled(&mut self, _app: &AppHandle, id: &str, on: bool) {
        if let Some(i) = id.strip_prefix("api:").and_then(|i| i.parse::<usize>().ok()) {
            if let Some(api) = self.api.as_mut() {
                api[i.min(3)] = on;
            }
        }
    }

    fn chosen(&mut self, app: &AppHandle, id: &str, index: usize) {
        match id {
            "listen" => {
                let v = json(&crate::host::host(app.state::<Host>()));
                self.listen = Some(if index == 0 {
                    String::new()
                } else {
                    arr(&v, "adapters")
                        .get(index - 1)
                        .and_then(|a| a.get(1))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                });
            }
            "tab" => self.tab = index.min(2),
            "min_level" => self.min_level = index.min(3),
            _ => {}
        }
    }

    fn edited(&mut self, id: &str, text: &str) {
        match id {
            "public_ip" => self.public_ip = text.into(),
            "trusted" => self.trusted = Some(text.into()),
            "address" => {
                self.address = text.into();
                self.plain = false;
            }
            "key" => self.key = text.into(),
            _ => {}
        }
    }

    fn accepted(&mut self, app: &AppHandle, id: &str, text: &str) {
        self.edited(id, text);
        if id == "key" {
            self.pressed(app, "connect");
        }
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        let host = app.state::<Host>();
        match id {
            "download" => crate::host::host_download(app.clone(), host),
            "releases" => {
                let _ = crate::core::open_url("https://github.com/JDevWebb/5th-echelon-enhanced/releases");
            }
            "start" => {
                let v = json(&crate::host::host(host));
                let listen = self.listen.clone().unwrap_or_else(|| st(&v, "listen"));
                let trusted = self.trusted.clone().unwrap_or_else(|| st(&v, "trusted_subnet"));
                let [info, presence, accounts, unhandled] = self.api.unwrap_or([true, true, false, false]);
                self.start_error = crate::host::host_start(
                    app.clone(),
                    host,
                    Some(listen).filter(|l| !l.is_empty()),
                    self.public_ip.clone(),
                    trusted,
                    CommunityApi {
                        info,
                        presence,
                        accounts,
                        unhandled,
                    },
                )
                .err();
            }
            "stop" => crate::host::host_stop(app.clone(), host),
            "connect" | "connect-plain" => {
                self.connect_error = None;
                match crate::host::host_connect(host, self.address.clone(), self.key.clone(), id == "connect-plain") {
                    Ok(said) => {
                        self.plain = said == "plain";
                        self.manage_open = self.plain;
                    }
                    Err(e) => self.connect_error = Some(e),
                }
            }
            "disconnect" => crate::host::host_disconnect(host),
            "manage" => self.manage_open = true,
            "manage-close" => {
                self.manage_open = false;
                self.plain = false;
                self.connect_error = None;
            }
            "settings" => self.settings_open = !self.settings_open,
            "refresh" => {}
            "no" => self.confirm = None,
            "delete" => {
                if let Some((what, id, _)) = self.confirm.take() {
                    crate::host::host_delete(app.clone(), host, what, id);
                }
            }
            _ if id.starts_with("ask:") => {
                let mut parts = id[4..].splitn(3, ':');
                if let (Some(what), Some(id), Some(name)) = (parts.next(), parts.next(), parts.next()) {
                    self.confirm = Some((what.into(), id.into(), name.into()));
                }
            }
            _ => {}
        }
    }
}
