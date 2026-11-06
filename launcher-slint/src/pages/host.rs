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
        }
    }
}

impl Screen for HostPage {
    fn live_edits(&self) -> bool {
        true
    }

    fn refresh_every(&self) -> u64 {
        3
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        let v = json(&crate::host::host(app.state::<Host>()));
        let mut r = Rows::default();
        let running = bo(&v, "running");
        let starting = bo(&v, "starting");
        let found = bo(&v, "found");
        r.card("Run a server on this PC");
        if !found {
            r.add(note(format!("The server ({}) goes next to the launcher.", st(&v, "exe_name"))));
            if bo(&v, "downloading") {
                r.add(working("Downloading…"));
            } else {
                r.add(buttons(vec![
                    super::b("download", "Download the server").kind("primary"),
                    super::b("releases", "Releases").kind("link"),
                ]));
            }
        } else {
            let locked = running || starting;
            let mut adapters = vec![(String::new(), "Every address".to_string())];
            for a in arr(&v, "adapters") {
                if let (Some(name), Some(ip)) = (a.get(0).and_then(Value::as_str), a.get(1).and_then(Value::as_str)) {
                    adapters.push((ip.to_string(), format!("{ip} · {name}")));
                }
            }
            let listen = self.listen.clone().unwrap_or_else(|| st(&v, "listen"));
            r.add(choice("listen", "Listen on", &adapters, &listen).enabled(!locked));
            r.add(field("public_ip", "Public address", self.public_ip.clone()).placeholder("only behind NAT").enabled(!locked));
            let trusted = self.trusted.clone().unwrap_or_else(|| st(&v, "trusted_subnet"));
            r.add(
                field("trusted", "Trusted subnet", trusted)
                    .placeholder("e.g. 10.8.0.0/16, the VPN players use")
                    .enabled(!locked),
            );
            r.add(note(
                "Players connecting from the trusted subnet always advertise the address the server sees, fixing joins when the game picks the wrong adapter.",
            ));
            if let Some(api) = v.get("api").filter(|a| !a.is_null()) {
                let on = self.api.unwrap_or([bo(api, "info"), bo(api, "presence"), bo(api, "accounts"), bo(api, "unhandled")]);
                self.api = Some(on);
                r.add(note("Community API (port 80, for launchers and the overlay):"));
                r.add(switch("api:0", "Server info", on[0]).enabled(!locked));
                r.add(
                    switch("api:1", "Who's online", on[1])
                        .enabled(!locked)
                        .sub("Shares every username and what online players are doing."),
                );
                r.add(
                    switch("api:2", "Sign-up for tools", on[2])
                        .enabled(!locked)
                        .sub("Lets tools create accounts through /api/register (rate-limited). Refused when every account needs an identity."),
                );
                r.add(switch("api:3", "Unhandled calls (development)", on[3]).enabled(!locked));
            }
            if starting {
                r.add(working("Starting…"));
            } else if running {
                r.add(item("Running", "").tone("ok").button(super::b("stop", "Stop")));
            } else {
                r.add(buttons(vec![super::b("start", "Start server").kind("primary")]));
            }
            if let Some(e) = &self.start_error {
                r.add(bad(e.clone()));
            }
        }
        r.card("Manage a server");
        let managing = st(&v, "managing");
        if managing.is_empty() {
            r.add(note(
                "Start the server above, or connect to one with the admin API on, using its key (admin-key.txt next to the server).",
            ));
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
        } else {
            let mut row = vec![super::b("refresh", "Refresh").kind("fix")];
            if !bo(&v, "local") {
                row.push(super::b("disconnect", "Disconnect").kind("fix"));
            }
            r.add(item(managing.clone(), "").buttons(row));
            r.add(segments(
                "tab",
                "",
                &[("players".into(), "Players".into()), ("games".into(), "Games".into()), ("logs".into(), "Log".into())],
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
                let lists = crate::host::host_lists(app.state::<Host>()).map(|l| json(&l)).unwrap_or(Value::Null);
                let key = if self.tab == 0 { "players" } else { "games" };
                match lists.get(key) {
                    Some(l) if l.get("Err").is_some() => {
                        r.add(bad(st(l, "Err")));
                    }
                    Some(l) => {
                        let list = arr(l, "Ok");
                        if list.is_empty() {
                            r.add(note(if self.tab == 0 { "No players yet." } else { "No games running." }));
                        }
                        for x in list {
                            if self.tab == 0 {
                                r.add(item(st(x, "name"), st(x, "ips")).button(super::b(format!("ask:player:{}:{}", st(x, "id"), st(x, "name")), "Delete…").kind("fix")));
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
            }
        }
        if found && (managing.is_empty() || self.tab == 2) {
            r.card("Log");
            r.add(choice(
                "min_level",
                "Detail",
                &LEVELS.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>(),
                LEVELS[self.min_level].0,
            ));
            let logs = crate::host::host_logs(app.state::<Host>(), LEVELS[self.min_level].0.into());
            if logs.is_empty() {
                r.add(note("Nothing logged yet."));
            }
            for l in logs.iter().rev().take(120).collect::<Vec<_>>().into_iter().rev() {
                let l = json(l);
                let tone = match st(&l, "tone").as_str() {
                    "error" | "bad" => "bad",
                    "warn" => "warn",
                    _ => "",
                };
                r.add(message(st(&l, "time"), st(&l, "msg"), "").tone(if tone.is_empty() { "" } else { tone }));
            }
        }
        PageView {
            kicker: "Host on this PC".into(),
            title: "Host a server".into(),
            summary: "Run a server for your group: a LAN party, or friends over the internet. Their launchers find it on your network.".into(),
            rows: r.done(),
            ..Default::default()
        }
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
                    Ok(said) => self.plain = said == "plain",
                    Err(e) => self.connect_error = Some(e),
                }
            }
            "disconnect" => crate::host::host_disconnect(host),
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
