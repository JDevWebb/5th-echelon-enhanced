//! The README's and the wiki's screenshots (`launcher-slint --shots-docs DIR`, run by
//! `build.sh slint-docs`): the launcher's own pages, dialogs, Play button, checklist and server
//! card, worked out by its own code from made-up state: a game folder with an empty executable,
//! an account on the community network, a made-up identity, the network's directory and news.
//! Run with no network at all, so nothing reaches a real server. Each picture in the dark look at
//! 1280x800, named as the docs use it.
//!
//! The made-up folders are under `XDG_CONFIG_HOME` (build.sh gives it one of its own); what the
//! pages say of them is shown as a Windows install's paths.

use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use slint::ComponentHandle;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;

use crate::commands::Launcher;
use crate::core;
use crate::pages::dialogs::Dialog;
use crate::pages::PageView;
use crate::pages::Screen;
use crate::pages::R;
use crate::tauri::AppHandle;
use crate::tauri::Manager;
use crate::AppWindow;

/// Where the made-up game folder shows as being.
const WIN_GAME: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Splinter Cell Blacklist\src\SYSTEM";
/// Where the launcher's own folder shows as being.
const WIN_CONFIG: &str = r"C:\Users\Kai\AppData\Roaming";
const NAME: &str = "Nightshade";
const COMMUNITY: &str = setup::directory::COMMUNITY_ADDRESS;

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

struct Shot {
    name: &'static str,
    set: fn(&AppWindow, &Docs),
}

/// The made-up install and the app the pages run in.
struct Docs {
    config: PathBuf,
    /// The game folder with an account on the community network, and one with none yet.
    game: PathBuf,
    fresh_game: PathBuf,
    /// One with an account on a group's network of several servers (the server menu).
    group_game: PathBuf,
    app: AppHandle,
}

impl Docs {
    fn core(&self) -> Arc<core::Core> {
        self.app.state::<Launcher>().0.clone()
    }

    /// The game folder the launcher works on now.
    fn use_game(&self, dir: &Path) {
        self.core().lock().game = Some(launcher::prefs::Game::open(dir.to_path_buf()));
    }

    /// A page's rows, with the made-up folders shown as a Windows install's.
    fn rows(&self, rows: Vec<R>) -> ModelRc<crate::Row> {
        let rows: Vec<R> = rows.into_iter().map(|r| self.windows_paths(r)).collect();
        model(rows.iter().map(crate::shell::row).collect())
    }

    fn path_text(&self, text: &str) -> String {
        let mut t = text.to_string();
        for game in [&self.fresh_game, &self.group_game, &self.game] {
            let g = game.to_string_lossy();
            t = t.replace(&format!("{g}/"), &format!("{WIN_GAME}\\")).replace(g.as_ref(), WIN_GAME);
        }
        let c = self.config.to_string_lossy();
        t.replace(&format!("{c}/5th-Echelon/"), &format!("{WIN_CONFIG}\\5th-Echelon\\"))
            .replace(&format!("{c}/"), &format!("{WIN_CONFIG}\\"))
            .replace(c.as_ref(), WIN_CONFIG)
    }

    fn windows_paths(&self, mut r: R) -> R {
        r.label = self.path_text(&r.label);
        r.text = self.path_text(&r.text);
        r.sub = self.path_text(&r.sub);
        r.placeholder = self.path_text(&r.placeholder);
        r.options = r.options.iter().map(|o| self.path_text(o)).collect();
        for b in &mut r.buttons {
            b.label = self.path_text(&b.label);
        }
        r
    }

    fn page(&self, w: &AppWindow, screen: &str, mut view: PageView) {
        w.set_screen(s(screen));
        view.summary = self.path_text(&view.summary);
        view.rows = view.rows.into_iter().map(|r| self.windows_paths(r)).collect();
        view.side = view.side.into_iter().map(|r| self.windows_paths(r)).collect();
        crate::shell::show_page(w, &view);
    }

    fn dialog(&self, w: &AppWindow, mut dialog: Dialog) {
        let v = dialog.view(&self.app);
        w.set_dialog_kicker(s(&v.page.kicker));
        w.set_dialog_title(s(&v.page.title));
        w.set_dialog_width(v.width);
        w.set_dialog_rows(self.rows(v.page.rows));
        w.set_dialog_shown(true);
    }

    fn settings(&self, w: &AppWindow, section: &str, before: impl FnOnce(&mut crate::pages::settings::SettingsPage)) {
        let mut page = crate::pages::settings::SettingsPage::new();
        page.section(section);
        before(&mut page);
        let view = page.view(&self.app);
        self.page(w, "settings", view);
    }
}

/// The facts the checklist is made from: everything as it should be, on the community network.
fn ready_facts(game: &Path) -> setup::diagnose::Facts {
    setup::diagnose::Facts {
        game_dir: Some(game.to_path_buf()),
        client: Some(setup::install::ClientState::Installed),
        server: Some((COMMUNITY.into(), Some([203, 0, 113, 20].into()))),
        server_ports: Some((true, true)),
        account: Some(setup::diagnose::AccountFact::Ok(NAME.into())),
        route_adapter: Some("Ethernet".into()),
        route_ip: Some([192, 168, 1, 20].into()),
        save: Some(setup::save::SaveState::Ok { xp: 182_400 }),
        ..Default::default()
    }
}

/// The Play screen as the launcher works it out from `facts` (the checklist, the button, the
/// server card and menu, the news), with `info` changed by `adjust` first.
fn play(w: &AppWindow, d: &Docs, facts: setup::diagnose::Facts, adjust: impl FnOnce(&mut core::Info)) {
    let core = d.core();
    {
        let mut inner = core.lock();
        inner.checks = setup::diagnose::checklist(&facts);
        inner.support = Some(launcher::flow::Support::Supported);
        inner.checked = Some(Instant::now());
    }
    let list = core::checks(&core, false, false);
    let mut info = core::info(&core);
    // The game's files noted long ago (the first time, Play waits on it a few seconds).
    info.preparing = None;
    adjust(&mut info);
    let (button, _) = crate::ui::play_button(&info, Some(&list), None);
    w.set_screen(s("play"));
    w.set_info(crate::ui::info_view(&info));
    w.set_button(button);
    w.set_initial(s(crate::ui::initial(info.account.as_deref().unwrap_or("?"))));
    w.set_checks(model(list.checks.iter().map(crate::ui::check_row).collect()));
    let (status, title, why) = list.support.clone().unwrap_or_default();
    w.set_support_status(s(status));
    w.set_support_title(s(title));
    w.set_support_why(s(why));
    w.set_fixable(list.fixable);
    w.set_checks_busy(list.busy);
    w.set_trouble(list.tone == Some("fail"));
    w.set_signed_in(list.signed_in);
    w.set_checks_loaded(true);
    let servers = core::servers(&core, false);
    w.set_servers(model(servers.rows.iter().map(crate::ui::server_row).collect()));
    w.set_card(crate::ui::server_card(servers.card));
    w.set_servers_header(s(servers.header));
    w.set_servers_note(s(servers.note.unwrap_or_default()));
    w.set_network(servers.network.is_some());
    w.set_news(model(core::news(&core).into_iter().map(s).collect()));
}

/// The common state of the window: the community network, friends online, nothing open.
fn base(w: &AppWindow, d: &Docs) {
    d.use_game(&d.game);
    write_prefs(d, Prefs::Answered);
    {
        let core = d.core();
        let mut inner = core.lock();
        inner.directory = Some(community_directory());
        inner.news = Some((COMMUNITY.into(), Instant::now(), community_news()));
    }
    w.set_loaded(true);
    w.set_servers_open(false);
    w.set_dialog_shown(false);
    w.set_banner(s(""));
    w.set_activity(crate::ActivityView::default());
    w.set_notice(crate::NoticeView::default());
    w.set_squad_loaded(true);
    w.set_online(model(vec![
        crate::FriendRow {
            id: s("1"),
            name: s("Kestrel"),
            activity: s("In a Spies vs Mercs lobby"),
            joinable: true,
            with_me: false,
        },
        crate::FriendRow {
            id: s("2"),
            name: s("Marlow"),
            activity: s("Co-op · Opium Farm, under way"),
            joinable: false,
            with_me: false,
        },
    ]));
    w.set_others(model(vec![crate::OtherRow {
        id: s("3"),
        name: s("Quillon"),
        activity: s("In the menus"),
        asked: false,
        asked_us: false,
    }]));
    w.set_others_total(1);
    w.set_elsewhere(model(vec![]));
    w.set_offline(4);
    w.set_squad_requests(1);
    w.set_join_offered(true);
    w.set_playing_shown(true);
    w.set_playing(model(vec![
        s("SvM Blacklist · Virus Vault · 5/8"),
        s("SvM Classic · Cartel · 4/4"),
        s("Co-op · Opium Farm · 1/2"),
    ]));
    w.set_playing_more(s("Also: 6 in lobbies"));
    w.set_choose_offered(true);
    w.set_modes_allowed(true);
    w.set_server_blocked(s(""));
    w.set_matches_ready(true);
    w.set_seat_open(s(""));
    play(w, d, ready_facts(&d.game), |_| {});
}

fn activity(w: &AppWindow, title: &str, state: &str, message: &str, actions: &[(&str, &str)]) {
    w.set_activity(crate::ActivityView {
        shown: true,
        id: 1,
        title: s(title),
        step: s(""),
        state: s(state),
        message: s(message),
        progress: -1.0,
        actions: model(actions.iter().map(|(id, label)| crate::ActionItem { id: s(id), label: s(label) }).collect()),
        waiting: 0,
    });
}

fn shots() -> Vec<Shot> {
    vec![
        Shot {
            name: "play-ready",
            set: |w, _| activity(w, &format!("Signed in to {COMMUNITY}"), "done", "", &[]),
        },
        Shot {
            name: "play-menu",
            set: |w, d| {
                write_prefs(d, Prefs::Group);
                d.use_game(&d.group_game);
                d.core().lock().directory = Some(group_directory());
                play(w, d, group_facts(&d.group_game), |_| {});
                w.set_servers_open(true);
            },
        },
        Shot {
            name: "status-problem",
            set: |w, d| {
                let mut facts = ready_facts(&d.game);
                facts.save = Some(setup::save::SaveState::Missing);
                facts.route_adapter = Some("NordLynx".into());
                play(w, d, facts, |_| {});
            },
        },
        Shot {
            name: "activity-error",
            set: |w, _| {
                activity(
                    w,
                    "Couldn't install the client",
                    "failed",
                    "The game's folder can't be written to: Windows' Controlled folder access, or antivirus software, may be blocking it.",
                    &[("retry", "Try again"), ("copy-details", "Copy details")],
                )
            },
        },
        Shot {
            name: "setup-server",
            set: |w, d| {
                d.use_game(&d.fresh_game);
                let mut page = crate::pages::setup::SetupPage::new(true);
                let view = page.view(&d.app);
                d.page(w, "setup", view);
            },
        },
        Shot {
            name: "setup-name",
            set: |w, d| {
                d.use_game(&d.fresh_game);
                let mut page = crate::pages::setup::SetupPage::new(true);
                page.needs_name(COMMUNITY.into(), NAME.into(), false);
                let view = page.view(&d.app);
                d.page(w, "setup", view);
            },
        },
        Shot {
            name: "name-refused",
            set: |w, d| {
                d.use_game(&d.fresh_game);
                let mut page = crate::pages::setup::SetupPage::new(true);
                page.needs_name(COMMUNITY.into(), String::new(), false);
                let view = page.view(&d.app);
                d.page(w, "setup", view);
                activity(
                    w,
                    &format!("Couldn't create your account on {COMMUNITY}"),
                    "failed",
                    "That name isn't allowed (no slurs or hate). Try another one, or if it's a mistake, ask for a review in the launcher's Support.",
                    &[("copy-details", "Copy details")],
                );
            },
        },
        Shot {
            name: "name-banner",
            set: |w, d| play(w, d, ready_facts(&d.game), |i| i.name_not_allowed = Some(NAME.into())),
        },
        Shot {
            name: "name-play-held",
            set: |w, d| {
                play(w, d, ready_facts(&d.game), |i| i.name_not_allowed = Some(NAME.into()));
                activity(
                    w,
                    "Your name breaks this network's rules. Choose another first: Settings › Servers and accounts › Rename.",
                    "failed",
                    "",
                    &[],
                );
            },
        },
        Shot {
            name: "name-rename-refused",
            set: |w, d| {
                d.settings(w, "servers", |_| {});
                activity(
                    w,
                    "Renaming your account",
                    "failed",
                    "That name isn't allowed (no slurs or hate).",
                    &[("copy-details", "Copy details")],
                );
            },
        },
        Shot {
            name: "name-review",
            set: |w, d| {
                crate::support::draft(
                    &d.app,
                    format!("Please review my name {NAME}: I was asked to choose another, and I think that's a mistake. What it means: "),
                );
                d.app.state::<crate::support::Support>().read_as(setup::support::Thread::default());
                let view = crate::pages::support::SupportPage::default().view(&d.app);
                d.page(w, "support", view);
            },
        },
        Shot {
            name: "servers",
            set: |w, d| {
                let view = crate::pages::servers::ServersPage::default().view(&d.app);
                d.page(w, "servers", view);
            },
        },
        Shot {
            name: "news",
            set: |w, d| {
                let view = crate::pages::intel::IntelPage::default().view(&d.app);
                d.page(w, "intel", view);
            },
        },
        Shot {
            name: "host",
            set: |w, d| {
                let view = crate::pages::host::HostPage::default().view(&d.app);
                d.page(w, "host", view);
            },
        },
        Shot {
            name: "settings-game",
            set: |w, d| d.settings(w, "game", |_| {}),
        },
        Shot {
            name: "settings-network-test",
            set: |w, d| {
                d.settings(w, "network", |p| {
                    p.tested(
                        [
                            ("Config server (port 80)", "ok", ""),
                            ("API and account", "ok", ""),
                            ("Game service sign-in", "ok", ""),
                            ("How players reach this PC", "ok", "Direct, router port opened (UPnP); the game opens it while it runs"),
                            ("Internet play helper", "ok", "the relay Sydney"),
                        ]
                        .iter()
                        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
                        .collect(),
                    )
                })
            },
        },
        Shot {
            name: "settings-accounts",
            set: |w, d| d.settings(w, "servers", |_| {}),
        },
        Shot {
            name: "settings-identity",
            set: |w, d| d.settings(w, "identity", |_| {}),
        },
        Shot {
            name: "settings-save",
            set: |w, d| {
                d.settings(w, "save", |_| {});
                let v = serde_json::json!({ "save": {
                    "path": format!("{WIN_CONFIG}\\5th-Echelon\\Saves\\00000001.sav"),
                    "state": "Found.", "missing": false, "ubisoft": r"C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\savegames\1.save",
                    "saved": "2 hours ago", "progress": "Rank 5 or above · 182400 XP", "backups": 3, "last_backup": "yesterday"
                }});
                let mut r = crate::pages::Rows::default();
                crate::pages::settings::SettingsPage::default().save(&v, &mut r);
                w.set_page_rows(d.rows(r.done()));
            },
        },
        Shot {
            name: "settings-client",
            set: |w, d| d.settings(w, "client", |_| {}),
        },
        Shot {
            name: "settings-advanced",
            set: |w, d| d.settings(w, "advanced", |_| {}),
        },
        Shot {
            name: "settings-display",
            set: |w, d| d.settings(w, "display", |_| {}),
        },
        Shot {
            name: "settings-feedback",
            set: |w, d| d.settings(w, "feedback", |_| {}),
        },
        Shot {
            name: "account-dialog",
            set: |w, d| {
                let mut a = crate::pages::dialogs::Account::new(COMMUNITY.into(), NAME.into(), false, false);
                a.answered(
                    core::AccountServer {
                        place: "Beauharnois, Canada".into(),
                        kind: "Community server".into(),
                        ping: Some(38),
                        ..Default::default()
                    },
                    env!("FE_RELEASE").trim_end_matches("-dev"),
                    NAME,
                );
                d.dialog(w, Dialog::Account(a));
            },
        },
        Shot {
            name: "account-dialog-no-answer",
            set: |w, d| d.dialog(w, Dialog::Account(crate::pages::dialogs::Account::new(COMMUNITY.into(), NAME.into(), false, false))),
        },
        Shot {
            name: "diagnostics-prompt",
            set: |w, d| {
                write_prefs(d, Prefs::DiagnosticsUnasked);
                d.dialog(w, Dialog::Questions(Default::default()));
            },
        },
        Shot {
            name: "feedback-dialog",
            set: |w, d| {
                crate::feedback::ask_feedback(d.app.clone(), d.app.state::<crate::feedback::Feedback>());
                d.dialog(w, Dialog::Feedback(Default::default()));
            },
        },
    ]
}

/// The made-up game folder, with an account on `server` or none.
fn game_folder(dir: &Path, server: Option<&str>) {
    std::fs::create_dir_all(dir).expect("the made-up game folder");
    std::fs::write(dir.join("Blacklist_DX11_game.exe"), b"").expect("its executable");
    std::fs::write(dir.join("Blacklist_game.exe"), b"").expect("its executable");
    // The client installed: this build's own DLL, beside the game's (made-up) one kept aside.
    if let Some(dll) = launcher::dll_utils::bundled() {
        std::fs::write(dir.join("uplay_r1_loader.dll"), dll).expect("the client");
        std::fs::write(dir.join("uplay_r1_loader.orig.dll"), b"").expect("the game's own DLL");
    }
    let mut cfg = setup::config::Config::load(dir);
    cfg.update(|c| {
        c.profiles.clear();
        if let Some(server) = server {
            c.upsert_profile(setup::config::Profile {
                name: server.into(),
                server: server.into(),
                https: true,
                user: hooks_config::User {
                    username: NAME.into(),
                    // Made up: the account's own random one, as the launcher makes it.
                    password: "made-up-for-the-screenshots".into(),
                    ..Default::default()
                },
                ..Default::default()
            });
        }
    })
    .expect("its settings");
}

#[derive(Clone, Copy)]
enum Prefs {
    Answered,
    DiagnosticsUnasked,
    Group,
}

/// The launcher's settings: the game folder, the first-run questions answered (or not), the
/// network, and a recent server of the player's own.
fn write_prefs(d: &Docs, which: Prefs) {
    let options = launcher::new_options::OPTIONS.iter().map(|o| format!("{:?}", o.id)).collect::<Vec<_>>().join(", ");
    let (game, diagnostics, directory) = match which {
        Prefs::Answered => (&d.game, true, setup::directory::COMMUNITY),
        Prefs::DiagnosticsUnasked => (&d.game, false, setup::directory::COMMUNITY),
        Prefs::Group => (&d.group_game, true, "https://play.mygroup.org"),
    };
    let now = identity::now();
    let text = format!(
        "game_dir = {:?}\ndirectory = {directory:?}\ndiagnostics_asked = {diagnostics}\nlog_excerpt_told = true\nwhats_new_seen = \"9.9.9\"\noptions_asked = [{options}]\nfeedback_off = false\n\n[[recent_servers]]\naddress = \"192.168.1.20\"\nname = \"LAN party\"\nlast_used = {}\n",
        game.to_string_lossy(),
        now - 3 * 86_400,
    );
    let dir = d.config.join("5th-Echelon");
    std::fs::create_dir_all(&dir).expect("the launcher's folder");
    std::fs::write(dir.join("launcher.toml"), text).expect("the launcher's settings");
    launcher::prefs::Prefs::forget_cached_directory();
}

fn listing(id: &str, name: &str, region: &str, host: &str, players: u32) -> setup::directory::Listing {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": name, "region": region, "host": host, "version": env!("FE_RELEASE").trim_end_matches("-dev"),
        "players_online": players, "players_total": players * 8, "friends_mode": "mutual",
    }))
    .expect("a listing")
}

/// The community network's directory: one server everyone plays on.
fn community_directory() -> Vec<(setup::directory::Listing, Option<u32>)> {
    vec![(listing("community", "5th Echelon Community", "Beauharnois, Canada", COMMUNITY, 87), Some(38))]
}

/// A group's network of several servers, for the server menu.
fn group_directory() -> Vec<(setup::directory::Listing, Option<u32>)> {
    vec![
        (listing("eu", "My Group EU", "Frankfurt, Germany", "eu.mygroup.org", 14), Some(31)),
        (listing("na", "My Group NA", "Montreal, Canada", "na.mygroup.org", 22), Some(118)),
        (listing("oc", "My Group Oceania", "Sydney, Australia", "oc.mygroup.org", 6), Some(296)),
    ]
}

fn group_facts(game: &Path) -> setup::diagnose::Facts {
    setup::diagnose::Facts {
        server: Some(("eu.mygroup.org".into(), Some([198, 51, 100, 7].into()))),
        ..ready_facts(game)
    }
}

pub fn run(dir: PathBuf) -> Result<(), slint::PlatformError> {
    let err = |e: String| slint::PlatformError::Other(e);
    std::fs::create_dir_all(&dir).map_err(|e| err(e.to_string()))?;
    let config = PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").ok_or_else(|| err("XDG_CONFIG_HOME isn't set (build.sh slint-docs sets it)".into()))?);
    let game = config.join("Splinter Cell Blacklist");
    let fresh_game = config.join("A new install");
    let group_game = config.join("Another install");
    game_folder(&game, Some(COMMUNITY));
    game_folder(&fresh_game, None);
    game_folder(&group_game, Some("eu.mygroup.org"));
    setup::player_identity::load_or_create().map_err(|e| err(e.to_string()))?;

    let core = Arc::new(core::Core::new(Arc::new(|_| {})));
    let app = crate::tauri::Builder::new()
        .manage(Launcher(core.clone()))
        .manage(crate::feedback::Feedback::default())
        .manage(crate::whats_new::WhatsNew::default())
        .manage(crate::intel::Intel::default())
        .manage(crate::support::Support::default())
        .manage(crate::chat::Chat::default())
        .manage(crate::host::Host::default())
        .manage(crate::update::Update::default())
        .build(Arc::new(|_| {}), Arc::new(|_| true));
    let d = Rc::new(Docs {
        config,
        game,
        fresh_game,
        group_game,
        app,
    });
    write_prefs(&d, Prefs::Answered);
    {
        let mut inner = core.lock();
        inner.directory = Some(community_directory());
        inner.browsed_at = Some(Instant::now());
        inner.news = Some((COMMUNITY.into(), Instant::now(), community_news()));
    }

    let w = AppWindow::new()?;
    w.set_release(s(env!("FE_RELEASE").trim_end_matches("-dev")));
    w.global::<crate::Theme>().set_choice(crate::ThemeChoice::Dark);
    w.global::<crate::Theme>().set_reduced_motion(true);
    w.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    let shots = Rc::new(shots());
    let at = Rc::new(std::cell::Cell::new(0usize));
    let timer = Rc::new(slint::Timer::default());
    let weak = w.as_weak();
    let next = {
        let (shots, at, timer2) = (shots.clone(), at.clone(), timer.clone());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let i = at.get();
            if i > 0 {
                let name = format!("{}.png", shots[i - 1].name);
                w.set_covering(false);
                match w.window().take_snapshot() {
                    Ok(px) => {
                        if let Some(img) = image::RgbaImage::from_raw(px.width(), px.height(), px.as_bytes().to_vec()) {
                            let _ = img.save(dir.join(&name));
                            println!("{name} {}x{}", px.width(), px.height());
                        }
                    }
                    Err(e) => eprintln!("{name}: {e}"),
                }
            }
            if i == shots.len() {
                timer2.stop();
                let _ = slint::quit_event_loop();
                return;
            }
            base(&w, &d);
            (shots[i].set)(&w, &d);
            at.set(i + 1);
        }
    };
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(900), next);
    w.run()
}

/// The community server's made-up news.
fn community_news() -> Vec<setup::server_info::NewsItem> {
    vec![
        news("0.5.0: one server for everyone", "Every region now plays on the same server, so every search finds the whole community. Your name, friends and stats came with you; the game uses the relay nearest you by itself."),
        news("Fair play", "Report a player from the overlay (F5 › Match): the admins see the match with it. Match recordings are kept a week for that."),
        news("SvM night, Saturday 8 pm UTC", "Blacklist and Classic, all evening. Bring a friend."),
    ]
}

fn news(title: &str, text: &str) -> setup::server_info::NewsItem {
    setup::server_info::NewsItem {
        title: title.into(),
        text: text.into(),
        link: String::new(),
    }
}
