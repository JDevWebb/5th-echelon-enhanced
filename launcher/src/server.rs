//! The Server screen: run a 5th Echelon server on this PC, and manage a
//! server (this one or another) through its admin API: players, games, logs.

use std::io::BufRead as _;
use std::io::BufReader;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;

use dedicated_server_config::Config as ServerConfig;
use eframe::egui;
use eframe::egui::RichText;
use serde::Deserialize;

use crate::app::App;
use crate::app::Notices;
use crate::services::Admin;
use crate::task::Slot;
use crate::theme;

const SERVER_EXE: &str = crate::updater::SERVER_FILE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Players,
    Games,
    Logs,
}

#[derive(Default)]
pub struct Server {
    loaded: bool,
    exe: Option<PathBuf>,
    cfg: Option<ServerConfig>,
    cfg_error: Option<String>,
    /// None: every address.
    listen: Option<IpAddr>,
    public_ip: String,
    trusted_subnet: String,
    process: Option<Child>,
    starting: Slot<Result<(Child, String), String>>,
    downloading: Slot<anyhow::Result<()>>,

    /// The server being managed: this PC's (after Start) or one connected to.
    admin: Option<Admin>,
    remote_api: String,
    remote_key: String,
    /// A plain-HTTP admin address waiting for the player to agree to send
    /// the key unencrypted, and whether they have.
    plain: Option<(String, bool)>,
    connect_error: Option<String>,
    tab: Tab,
    players: Slot<anyhow::Result<Vec<server_api::users::User>>>,
    player_list: Vec<server_api::users::User>,
    games: Slot<anyhow::Result<Vec<server_api::games::Game>>>,
    game_list: Vec<server_api::games::Game>,
    deleting: Slot<anyhow::Result<()>>,
    confirm: Option<(Tab, String, String)>,
    min_level: LogLevel,
}

impl Drop for Server {
    /// A server started here stops with the launcher.
    fn drop(&mut self) {
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Server {
    /// Finds `dedicated_server.exe` next to the launcher and reads its
    /// `service.toml` (the defaults when there's none yet).
    fn load(&mut self) {
        self.loaded = true;
        self.exe = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(SERVER_EXE))).filter(|p| p.is_file());
        let Some(exe) = &self.exe else { return };
        let path = exe.with_file_name("service.toml");
        let cfg = if path.exists() {
            ServerConfig::load_from_file(&path)
        } else {
            Ok(ServerConfig::default())
        };
        match cfg {
            Ok(cfg) => {
                self.listen = Some(cfg.api_server.ip()).filter(|ip| !ip.is_unspecified());
                self.trusted_subnet = secure_settings(&cfg).and_then(|s| s.get("trusted_subnet").cloned()).unwrap_or_default();
                self.cfg = Some(cfg);
            }
            Err(e) => self.cfg_error = Some(format!("{}: {e}", path.display())),
        }
    }

    fn running(&mut self) -> bool {
        match self.process.as_mut().map(Child::try_wait) {
            Some(Ok(None)) => true,
            Some(_) => {
                self.process = None;
                false
            }
            None => false,
        }
    }

    /// Writes the addresses and switches into `service.toml` and starts the
    /// server. It prints its admin key on start (`--launcher`).
    fn start(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        // Read again first: changes made by hand while the launcher was open stay.
        if let Some(path) = self.exe.as_ref().map(|e| e.with_file_name("service.toml")).filter(|p| p.exists()) {
            match ServerConfig::load_from_file(&path) {
                Ok(fresh) => self.cfg = Some(fresh),
                Err(e) => return notices.error(format!("{}: {e}", path.display())),
            }
        }
        let (Some(exe), Some(cfg)) = (self.exe.clone(), self.cfg.as_mut()) else { return };
        let ip = self.listen.unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let public = match self.public_ip.trim() {
            "" => self.listen.unwrap_or(ip),
            s => match s.parse() {
                Ok(p) => p,
                Err(_) => return notices.error(format!("\"{s}\" isn't an IP address.")),
            },
        };
        set_ips(cfg, ip, public);
        if let Some(settings) = secure_settings_mut(cfg) {
            match self.trusted_subnet.trim() {
                "" => {
                    settings.remove("trusted_subnet");
                }
                s => {
                    settings.insert("trusted_subnet".into(), s.to_string());
                }
            }
        }
        if let Err(e) = cfg.save_to_file(exe.with_file_name("service.toml")) {
            return notices.error(format!("Couldn't save service.toml: {e}"));
        }
        self.starting.start(ctx, move || {
            let mut child = Command::new(&exe)
                .arg("--launcher")
                .current_dir(exe.parent().unwrap_or(std::path::Path::new(".")))
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| format!("Couldn't start the server: {e}"))?;
            let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
            let key = lines.by_ref().map_while(Result::ok).find_map(|l| l.strip_prefix("Admin Key: ").map(str::to_string));
            // Keep reading, so a full pipe never blocks the server.
            std::thread::spawn(move || lines.for_each(drop));
            match key {
                Some(key) => Ok((child, key)),
                None => {
                    let _ = child.kill();
                    Err("The server stopped while starting. See its log below.".into())
                }
            }
        });
    }

    fn local_api(&self) -> Option<String> {
        let mut addr: SocketAddr = self.cfg.as_ref()?.api_server;
        if addr.ip().is_unspecified() {
            addr.set_ip(IpAddr::from([127, 0, 0, 1]));
        }
        Some(format!("http://{addr}"))
    }

    fn reload_lists(&mut self, ctx: &egui::Context) {
        let Some(admin) = self.admin.clone() else { return };
        let a = admin.clone();
        self.players.start(ctx, move || a.users());
        self.games.start(ctx, move || admin.games());
    }
}

fn secure_settings(cfg: &ServerConfig) -> Option<&std::collections::HashMap<String, String>> {
    cfg.quazal.service.values().find_map(|s| match s {
        quazal::Service::Secure(ctx) => Some(&ctx.settings),
        _ => None,
    })
}

fn secure_settings_mut(cfg: &mut ServerConfig) -> Option<&mut std::collections::HashMap<String, String>> {
    cfg.quazal.service.values_mut().find_map(|s| match s {
        quazal::Service::Secure(ctx) => Some(&mut ctx.settings),
        _ => None,
    })
}

/// Listens on `ip` and tells clients to connect to `public` (they differ
/// behind NAT).
fn set_ips(cfg: &mut ServerConfig, ip: IpAddr, public: IpAddr) {
    cfg.set_addresses(ip, public);
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let (server, notices) = app.server_mut();
    if !server.loaded {
        server.load();
    }
    if let Some(result) = server.starting.poll() {
        match result {
            Ok((child, key)) => {
                server.process = Some(child);
                server.admin = server.local_api().map(|api| Admin { api, key });
                notices.info("The server is running.");
                server.reload_lists(&ctx);
            }
            Err(e) => notices.error(e),
        }
    }
    if let Some(result) = server.downloading.poll() {
        match result {
            Ok(()) => notices.info("Downloaded the server."),
            Err(e) => notices.error(format!("Couldn't download the server: {e}")),
        }
        server.load();
    }
    if let Some(result) = server.deleting.poll() {
        if let Err(e) = result {
            notices.error(format!("Couldn't delete: {e}"));
        }
        server.reload_lists(&ctx);
    }
    if let Some(r) = server.players.poll() {
        match r {
            Ok(list) => server.player_list = list,
            Err(e) => notices.error(format!("Couldn't load the players: {e}")),
        }
    }
    if let Some(r) = server.games.poll() {
        match r {
            Ok(list) => server.game_list = list,
            Err(e) => notices.error(format!("Couldn't load the games: {e}")),
        }
    }

    local_server(server, notices, &ctx, ui);
    ui.add_space(10.0);
    manage(server, &ctx, ui);
}

fn local_server(server: &mut Server, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(theme::heading("Run a server on this PC"));
        if server.exe.is_none() {
            ui.label(theme::muted(format!("The server ({SERVER_EXE}) goes next to the launcher.")));
            ui.horizontal(|ui| {
                if server.downloading.running() {
                    ui.spinner();
                    ui.label("Downloading…");
                } else if ui.add(theme::primary("Download the server")).clicked() {
                    server.downloading.start(ctx, || {
                        let latest = crate::updater::latest()?;
                        let to = std::env::current_exe()?.with_file_name(SERVER_EXE);
                        crate::updater::download(&latest, crate::updater::SERVER_ASSET, &to)
                    });
                }
                ui.hyperlink_to("Releases", crate::updater::RELEASES_PAGE);
            });
            return;
        }
        if let Some(e) = &server.cfg_error {
            ui.label(RichText::new(e).color(theme::BAD));
            return;
        }
        let running = server.running();
        let adapters = setup::net::adapters();
        ui.add_enabled_ui(!running && !server.starting.running(), |ui| {
            egui::Grid::new("local").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label("Listen on");
                let label = server.listen.map_or_else(|| "Every address".to_string(), |ip| ip.to_string());
                egui::ComboBox::from_id_salt("listen").selected_text(label).width(260.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut server.listen, None, "Every address");
                    for (name, ip) in &adapters {
                        ui.selectable_value(&mut server.listen, Some(*ip), format!("{ip}  ·  {name}"));
                    }
                });
                ui.end_row();
                ui.label("Public address");
                ui.add(egui::TextEdit::singleline(&mut server.public_ip).hint_text("only behind NAT").desired_width(260.0));
                ui.end_row();
                ui.label("Trusted subnet");
                ui.add(
                    egui::TextEdit::singleline(&mut server.trusted_subnet)
                        .hint_text("e.g. 10.8.0.0/16, the VPN players use")
                        .desired_width(260.0),
                )
                .on_hover_text("Players connecting from this subnet always advertise the address the server sees, fixing joins when the game picks the wrong adapter.");
                ui.end_row();
            });
            if let Some(cfg) = server.cfg.as_mut() {
                ui.add_space(4.0);
                ui.label(theme::muted("Community API (port 80, for launchers and the overlay):"));
                ui.horizontal_wrapped(|ui| {
                    let api = &mut cfg.community_api;
                    ui.checkbox(&mut api.info, "Server info");
                    ui.checkbox(&mut api.presence, "Who's online")
                        .on_hover_text("Shares every username and what online players are doing.");
                    ui.checkbox(&mut api.accounts, "Sign-up for tools")
                        .on_hover_text("Lets tools create accounts through /api/register (rate-limited). Refused when every account needs an identity.");
                    ui.checkbox(&mut api.unhandled, "Unhandled calls (development)");
                });
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if server.starting.running() {
                ui.spinner();
                ui.label("Starting…");
            } else if running {
                theme::status_marker(ui, setup::diagnose::Status::Ok);
                ui.label(RichText::new("Running").color(theme::OK));
                if ui.button("Stop").clicked() {
                    if let Some(mut child) = server.process.take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    server.admin = None;
                    notices.info("The server stopped.");
                }
            } else if ui.add(theme::primary("Start server")).clicked() {
                server.start(ctx, notices);
            }
        });
    });
}

/// The admin API's URL for what the player typed (`host`, `host:port` or a
/// URL), and whether it's plain HTTP to another machine that isn't on a
/// private network.
fn admin_url(typed: &str) -> Result<(String, bool), String> {
    let typed = typed.trim();
    let with_scheme = if typed.starts_with("http://") || typed.starts_with("https://") {
        typed.to_string()
    } else {
        format!("http://{typed}")
    };
    let mut url = url::Url::parse(&with_scheme).map_err(|_| format!("\"{typed}\" isn't a server address."))?;
    let host = url.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_string();
    if !setup::net::valid_host(&host) || !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
        return Err(format!("\"{typed}\" isn't a server address."));
    }
    if url.port().is_none() && url.scheme() == "http" && !typed.starts_with("http://") {
        let _ = url.set_port(Some(setup::API_PORT));
    }
    let local = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| !setup::net::is_public(ip));
    let plain = url.scheme() == "http" && !local;
    Ok((url.as_str().trim_end_matches('/').to_string(), plain))
}

fn manage(server: &mut Server, ctx: &egui::Context, ui: &mut egui::Ui) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(theme::heading("Manage a server"));
        if server.admin.is_none() {
            ui.label(theme::muted(
                "Start the server above, or connect to one with the admin API on, using its key (admin-key.txt next to the server).",
            ));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut server.remote_api).hint_text("server address").desired_width(180.0));
                ui.add(
                    egui::TextEdit::singleline(&mut server.remote_key)
                        .hint_text("admin key")
                        .password(true)
                        .desired_width(220.0),
                );
                if ui
                    .add_enabled(!server.remote_api.is_empty() && !server.remote_key.is_empty(), egui::Button::new("Connect"))
                    .clicked()
                {
                    server.connect_error = None;
                    match admin_url(&server.remote_api) {
                        Ok((api, plain)) if !plain || server.plain.as_ref().is_some_and(|(a, ok)| *ok && *a == api) => {
                            server.admin = Some(Admin {
                                api,
                                key: server.remote_key.trim().to_string(),
                            });
                            server.plain = None;
                            server.reload_lists(ctx);
                        }
                        Ok((api, _)) => server.plain = Some((api, false)),
                        Err(e) => server.connect_error = Some(e),
                    }
                }
            });
            if let Some((api, ok)) = server.plain.as_mut().filter(|(_, ok)| !*ok) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{api} isn't encrypted: anyone on the way can read the admin key. Use https:// if the server has it."
                        ))
                        .color(theme::WARN),
                    );
                    if ui.button("Allow, then Connect").clicked() {
                        *ok = true;
                    }
                });
            }
            if let Some(e) = &server.connect_error {
                ui.label(RichText::new(e).color(theme::BAD));
            }
            logs(server, ui);
            return;
        }
        ui.horizontal(|ui| {
            ui.label(theme::muted(server.admin.as_ref().map(|a| a.api.clone()).unwrap_or_default()));
            if ui.button("Refresh").clicked() {
                server.reload_lists(ctx);
            }
            if server.process.is_none() && ui.button("Disconnect").clicked() {
                server.admin = None;
                return;
            }
            if server.players.running() || server.games.running() || server.deleting.running() {
                ui.spinner();
            }
        });
        ui.horizontal(|ui| {
            for (tab, label) in [(Tab::Players, "Players"), (Tab::Games, "Games"), (Tab::Logs, "Log")] {
                ui.selectable_value(&mut server.tab, tab, label);
            }
        });
        ui.separator();
        confirm_delete(server, ctx, ui);
        match server.tab {
            Tab::Players => {
                egui::Grid::new("players").num_columns(3).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
                    for user in server.player_list.clone() {
                        ui.label(&user.username);
                        ui.label(theme::muted(user.ips.join(", ")));
                        if ui.button("Delete…").clicked() {
                            server.confirm = Some((Tab::Players, user.id.clone(), user.username.clone()));
                        }
                        ui.end_row();
                    }
                });
                if server.player_list.is_empty() {
                    ui.label(theme::muted("No players yet."));
                }
            }
            Tab::Games => {
                egui::Grid::new("games").num_columns(4).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
                    for game in server.game_list.clone() {
                        ui.label(format!("#{}", game.id));
                        ui.label(&game.game_type);
                        ui.label(theme::muted(format!("{} with {}", game.creator, game.participants.join(", "))));
                        if ui.button("End…").clicked() {
                            server.confirm = Some((Tab::Games, game.id.to_string(), format!("game #{}", game.id)));
                        }
                        ui.end_row();
                    }
                });
                if server.game_list.is_empty() {
                    ui.label(theme::muted("No games running."));
                }
            }
            Tab::Logs => logs(server, ui),
        }
    });
}

fn confirm_delete(server: &mut Server, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some((tab, id, name)) = server.confirm.clone() else { return };
    ui.horizontal(|ui| {
        let what = if tab == Tab::Players {
            format!("Delete the account {name}? It can't be undone.")
        } else {
            format!("End {name}?")
        };
        ui.label(RichText::new(what).color(theme::WARN));
        if ui.button("Yes").clicked() {
            if let Some(admin) = server.admin.clone() {
                match tab {
                    Tab::Players => server.deleting.start(ctx, move || admin.delete_user(id)),
                    _ => {
                        let id: u32 = id.parse().unwrap_or_default();
                        server.deleting.start(ctx, move || admin.delete_game(id));
                    }
                }
            }
            server.confirm = None;
        }
        if ui.button("No").clicked() {
            server.confirm = None;
        }
    });
}

#[derive(Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default)]
enum LogLevel {
    #[serde(rename = "TRCE")]
    Trace,
    #[serde(rename = "DEBG")]
    Debug,
    #[default]
    #[serde(rename = "INFO")]
    Info,
    #[serde(rename = "WARN")]
    Warn,
    #[serde(rename = "ERRO")]
    Error,
    #[serde(rename = "CRIT")]
    Critical,
}

#[derive(Debug, Deserialize)]
struct LogItem {
    msg: String,
    level: LogLevel,
    ts: String,
}

/// The last lines of this PC's server log (`server.log.json`).
fn logs(server: &mut Server, ui: &mut egui::Ui) {
    let Some(path) = server.exe.as_ref().map(|e| e.with_file_name("server.log.json")) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(theme::heading("Log"));
        egui::ComboBox::from_id_salt("level")
            .selected_text(format!("{:?} and above", server.min_level))
            .show_ui(ui, |ui| {
                for level in [LogLevel::Debug, LogLevel::Info, LogLevel::Warn, LogLevel::Error] {
                    ui.selectable_value(&mut server.min_level, level, format!("{level:?}"));
                }
            });
    });
    let items: Vec<LogItem> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<LogItem>(l).ok())
        .filter(|i| i.level >= server.min_level)
        .collect();
    egui::ScrollArea::vertical().max_height(260.0).stick_to_bottom(true).show(ui, |ui| {
        for item in items.iter().skip(items.len().saturating_sub(300)) {
            let color = match item.level {
                LogLevel::Warn => theme::WARN,
                LogLevel::Error | LogLevel::Critical => theme::BAD,
                _ => theme::FG,
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(theme::muted(item.ts.get(11..19).unwrap_or(&item.ts)).monospace());
                ui.label(RichText::new(&item.msg).color(color));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::admin_url;

    #[test]
    fn admin_addresses() {
        assert_eq!(admin_url("10.8.0.10").unwrap(), ("http://10.8.0.10:50051".to_string(), false));
        assert_eq!(
            admin_url("play.example.org:9000").unwrap(),
            ("http://play.example.org:9000".to_string(), true),
            "plain over the internet"
        );
        assert_eq!(admin_url("https://play.example.org").unwrap(), ("https://play.example.org".to_string(), false));
        assert!(admin_url("localhost").is_ok_and(|(_, plain)| !plain));
        assert!(admin_url("http://user:pw@play.example.org").is_err());
        assert!(admin_url("a b").is_err());
    }
}
