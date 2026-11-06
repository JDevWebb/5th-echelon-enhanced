//! Servers (the Tauri launcher's Servers.vue): the network the server menu shows and another to
//! use, a server of the player's own (the account dialog confirms it), a LAN server, and the way
//! to hosting one.

use super::*;
use crate::commands::Launcher;
use crate::tauri::Manager;
use crate::tauri::WindowCall;

#[derive(Default)]
pub struct ServersPage {
    typed_network: String,
    network_error: Option<String>,
    address: String,
    lan: Option<Result<Vec<String>, String>>,
}

fn host_of(url: &str) -> String {
    url.trim_start_matches("https://").trim_end_matches('/').to_string()
}

/// The account dialog for a server of the player's own (it's also the confirmation).
pub fn own(app: &AppHandle, server: &str) {
    let suggested = crate::core::usual_name(&app.state::<Launcher>().0);
    app.window(WindowCall::Account {
        server: server.trim().into(),
        suggested,
        taken: false,
        own: true,
    });
}

impl Screen for ServersPage {
    fn live_edits(&self) -> bool {
        true
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        let core = app.state::<Launcher>().0.clone();
        let info = crate::core::info(&core);
        let mut r = Rows::default();
        let summary = "Community servers are in the server menu on the Play screen. Here: a server of your own, a LAN party, or another network.";
        if info.game_dir.is_none() {
            r.add(note("Find the game first (Play)."));
        } else if info.managed {
            r.add(note("Another tool manages this install: change the server there."));
        } else {
            let servers = crate::core::servers(&core, false);
            let community = servers.network.as_deref().is_some_and(launcher::prefs::Prefs::is_community);
            r.card("Network");
            match &servers.network {
                Some(url) => {
                    let n = servers.rows.len();
                    let count = match n {
                        0 => String::new(),
                        1 => "1 server · ".into(),
                        n => format!("{n} servers · "),
                    };
                    let lead = if community { "The 5th Echelon community network · " } else { "" };
                    r.add(
                        item(host_of(url), format!("{lead}{count}your friends and identity follow you between its servers"))
                            .tone("accent")
                            .text("In use"),
                    );
                }
                None => {
                    r.add(item("No network", "The server menu on the Play screen is empty until you use one."));
                }
            }
            r.add(rule());
            r.add(bold("Use another network"));
            r.add(
                field("network", "", self.typed_network.clone())
                    .placeholder("its address, e.g. play.mygroup.org")
                    .button(super::b("look-up", "Look it up").enabled(!self.typed_network.trim().is_empty())),
            );
            if let Some(e) = &self.network_error {
                r.add(bad(e.clone()));
            }
            r.add(note("Another group's network of servers: its servers then fill the server menu on the Play screen."));
            r.add_if(!community, || buttons(vec![super::b("community", "Back to the community network").kind("link")]));

            let busy = info.connecting || info.running;
            r.card("A server of your own");
            r.add(
                field("address", "", self.address.clone())
                    .placeholder("play.example.org or 192.168.1.20")
                    .button(super::b("connect", "Connect").kind("primary").enabled(!self.address.trim().is_empty() && !busy)),
            );
            r.add(note("A private server for your group, outside the network: the server menu takes you back."));
            let recent = crate::core::recent_servers(&core);
            if !recent.is_empty() {
                r.add(note("Recent"));
                for (address, label, when, in_use) in recent {
                    r.add(item(label, when).button(super::b(format!("own:{address}"), "Connect").enabled(!busy && !in_use)));
                }
            }
            r.card("On your network");
            r.add(note("Looks for a server running at a LAN party, on this network."));
            r.add(buttons(vec![super::b("lan", "Find on my network")]));
            match &self.lan {
                Some(Err(e)) => {
                    r.add(note(e.clone()));
                }
                Some(Ok(list)) => {
                    if list.is_empty() {
                        r.add(note("No server answered on this network."));
                    }
                    for ip in list {
                        r.add(item(ip.clone(), "found on your network").button(super::b(format!("own:{ip}"), "Connect").enabled(!busy)));
                    }
                }
                None => {}
            }
        }
        r.card("Host on this PC");
        r.add(note(
            "Run a server for your group: a LAN party, or friends over the internet. Their launchers find it on your network.",
        ));
        r.add(buttons(vec![super::b("host", "Host a server")]));
        PageView {
            kicker: "Network".into(),
            title: "Servers".into(),
            summary: summary.into(),
            rows: r.done(),
            ..Default::default()
        }
    }

    fn edited(&mut self, id: &str, text: &str) {
        match id {
            "network" => self.typed_network = text.into(),
            "address" => self.address = text.into(),
            _ => {}
        }
    }

    fn accepted(&mut self, app: &AppHandle, id: &str, text: &str) {
        self.edited(id, text);
        self.pressed(app, if id == "network" { "look-up" } else { "connect" });
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        let core = app.state::<Launcher>().0.clone();
        match id {
            "look-up" => match crate::core::use_network(&core, &self.typed_network) {
                Ok(()) => {
                    self.typed_network.clear();
                    self.network_error = None;
                }
                Err(e) => self.network_error = Some(e),
            },
            "community" => crate::whats_new::use_community_network(app.clone(), app.state::<Launcher>()),
            "connect" => {
                if !self.address.trim().is_empty() {
                    own(app, &self.address.clone());
                }
            }
            "lan" => self.lan = Some(crate::core::find_on_network()),
            "host" => {
                app.window(WindowCall::Go("host".into()));
            }
            _ if id.starts_with("own:") => own(app, &id[4..]),
            _ => {}
        }
    }
}
