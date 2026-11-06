//! Screenshots of every screen in both looks at several window sizes (`launcher-slint --shots
//! DIR`, run by `build.sh slint-shots` on a virtual screen): the window on made-up data, with no
//! game, no server and no network. For reviewing the design, as `build.sh shots` does for the
//! egui launcher.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;

use crate::AppWindow;

fn s(text: &str) -> SharedString {
    SharedString::from(text)
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// One screen to take: its name, and what to set.
struct Shot {
    name: &'static str,
    set: fn(&AppWindow),
}

const SIZES: [(u32, u32); 3] = [(1280, 800), (960, 600), (1920, 1080)];

pub fn run(dir: PathBuf) -> Result<(), slint::PlatformError> {
    std::fs::create_dir_all(&dir).map_err(|e| slint::PlatformError::Other(e.to_string()))?;
    let w = AppWindow::new()?;
    w.set_release(s(env!("FE_RELEASE")));
    base(&w);
    let shots: Rc<Vec<Shot>> = Rc::new(vec![
        Shot {
            name: "splash",
            set: |w| w.set_loaded(false),
        },
        Shot {
            name: "play-ready",
            set: play_ready,
        },
        Shot { name: "play-fix", set: play_fix },
        Shot {
            name: "play-notice",
            set: play_notice,
        },
        Shot {
            name: "play-checking",
            set: play_checking,
        },
        Shot { name: "matches", set: matches },
        Shot {
            name: "matches-teams",
            set: matches_teams,
        },
        Shot {
            name: "matches-empty",
            set: matches_empty,
        },
        Shot { name: "settings", set: settings },
        Shot { name: "friends", set: friends },
        Shot { name: "dialog", set: dialog },
        Shot { name: "setup", set: setup },
    ]);
    // Every shot in each look and size, one after another on the window's own clock.
    let mut steps = Vec::new();
    for dark in [true, false] {
        for size in SIZES {
            for i in 0..shots.len() {
                steps.push((dark, size, i));
            }
        }
    }
    let steps = Rc::new(steps);
    let at = Rc::new(std::cell::Cell::new(0usize));
    let timer = Rc::new(slint::Timer::default());
    let weak = w.as_weak();
    let next = {
        let (steps, shots, at, dir, timer2) = (steps.clone(), shots.clone(), at.clone(), dir.clone(), timer.clone());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let i = at.get();
            // The one before was set up a moment ago: taken now.
            if i > 0 {
                let (dark, (width, height), shot) = steps[i - 1];
                let name = format!("{}-{}-{width}x{height}.png", shots[shot].name, if dark { "dark" } else { "light" });
                match w.window().take_snapshot() {
                    Ok(px) => {
                        let img = image::RgbaImage::from_raw(px.width(), px.height(), px.as_bytes().to_vec());
                        if let Some(img) = img {
                            let _ = img.save(dir.join(&name));
                            println!("{name} {}x{}", px.width(), px.height());
                        }
                    }
                    Err(e) => eprintln!("{name}: {e}"),
                }
            }
            if i == steps.len() {
                timer2.stop();
                let _ = slint::quit_event_loop();
                return;
            }
            let (dark, (width, height), shot) = steps[i];
            base(&w);
            w.global::<crate::Theme>()
                .set_choice(if dark { crate::ThemeChoice::Dark } else { crate::ThemeChoice::Light });
            w.window().set_size(slint::LogicalSize::new(width as f32, height as f32));
            (shots[shot].set)(&w);
            at.set(i + 1);
        }
    };
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(700), next);
    w.global::<crate::Theme>().set_reduced_motion(true);
    w.run()
}

/// The common state: signed in on a community server, nothing failing.
fn base(w: &AppWindow) {
    w.set_loaded(true);
    w.set_screen(s("play"));
    w.set_info(crate::InfoView {
        game_dir: s("C:\\Games\\Splinter Cell Blacklist"),
        server: s("scbl.example.org"),
        account: s("Nightshade"),
        identity: s("K7QF-2M9D"),
        community: true,
        play_mode: s("svm"),
        managed: false,
        running: false,
        connecting: false,
        beta: 0,
        name_not_allowed: s(""),
        emulator_notice: false,
        new_match: false,
        requests: 1,
    });
    w.set_initial(s("N"));
    w.set_signed_in(true);
    w.set_button(crate::PlayButton {
        label: s("Play SvM"),
        sub: s("Ready to play"),
        tone: s("ok"),
        disabled: false,
        busy: false,
        why: s(""),
        action: s(""),
    });
    w.set_card(crate::ServerCard {
        shown: true,
        caption: s("Server"),
        caption_why: s(""),
        place: s("Singapore"),
        ping: 38,
        listed: true,
        players: 24,
    });
    w.set_servers_header(s("Community servers · scbl.example.org"));
    w.set_network(true);
    w.set_servers(model(vec![
        crate::ServerRow {
            host: s("scbl.example.org"),
            place: s("Singapore"),
            name: s("Matchmaking"),
            ping: 38,
            players: 24,
            playing: s("SvM Classic 6/8 · 3 in lobbies"),
            current: true,
            best: true,
            maintenance: s(""),
        },
        crate::ServerRow {
            host: s("eu.example.org"),
            place: s("Falkenstein, Germany"),
            name: s("EU"),
            ping: 284,
            players: 9,
            playing: s(""),
            current: false,
            best: false,
            maintenance: s("Maintenance 8 pm"),
        },
    ]));
    w.set_checks_loaded(true);
    w.set_checks(model(vec![
        check("ok", "Game found", "", "", ""),
        check("ok", "5th Echelon client installed", "", "", ""),
        check("ok", "Signed in as Nightshade on Singapore", "", "", ""),
    ]));
    w.set_support_title(s(""));
    w.set_trouble(false);
    w.set_fixable(false);
    w.set_checks_busy(false);
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
    w.set_others(model(vec![
        crate::OtherRow {
            id: s("3"),
            name: s("Quillon"),
            activity: s(""),
            asked: false,
            asked_us: false,
        },
        crate::OtherRow {
            id: s("4"),
            name: s("Ravel_77"),
            activity: s("In the menus"),
            asked: true,
            asked_us: false,
        },
    ]));
    w.set_others_total(2);
    w.set_elsewhere(model(vec![]));
    w.set_offline(5);
    w.set_squad_requests(1);
    w.set_join_offered(true);
    w.set_playing_shown(true);
    w.set_playing(model(vec![s("SvM Classic · Penthouse · 3/4"), s("Co-op · Dead Drop · 2/4")]));
    w.set_playing_more(s("Also: 3 in lobbies"));
    w.set_notice(crate::NoticeView::default());
    w.set_news(model(vec![s("0.5.0: one matchmaking server for everyone, and fair play")]));
    w.set_choose_offered(true);
    w.set_modes_allowed(true);
    w.set_server_blocked(s(""));
    w.set_matches_ready(true);
    w.set_activity(crate::ActivityView::default());
    w.set_seat_open(s(""));
    w.set_dialog_shown(false);
}

fn rows(list: Vec<crate::pages::R>) -> ModelRc<crate::Row> {
    model(list.iter().map(crate::shell::row).collect())
}

fn settings(w: &AppWindow) {
    use crate::pages::*;
    w.set_screen(s("settings"));
    let sections = [
        ("game", "Game"),
        ("network", "Network"),
        ("servers", "Servers"),
        ("identity", "Identity"),
        ("notifications", "Notifications"),
        ("save", "Save game"),
        ("client", "5th Echelon client"),
        ("advanced", "Advanced"),
        ("display", "Display"),
        ("feedback", "Feedback"),
        ("about", "About"),
    ];
    w.set_page_kicker(s("Loadout"));
    w.set_page_title(s("Settings"));
    w.set_page_summary(s("Game"));
    w.set_page_sections(model(sections.iter().map(|(_, l)| s(l)).collect()));
    w.set_page_section_ids(model(sections.iter().map(|(i, _)| s(i)).collect()));
    w.set_page_section(s("game"));
    let mut r = Rows::default();
    r.card("Game folder");
    r.add(text("C:\\Games\\Splinter Cell Blacklist"));
    r.add(buttons(vec![b("pick", "Choose folder…"), b("open", "Open folder")]));
    r.card("Options");
    r.add(segments(
        "renderer",
        "Renderer",
        &[("dx11".into(), "DirectX 11 (recommended)".into()), ("dx9".into(), "DirectX 9".into())],
        "dx11",
    ));
    r.add(switch("overlay", "In-game overlay", true));
    r.add(choice("key", "Opened with", &[("F1".into(), "F1".into()), ("F2".into(), "F2".into())], "F2"));
    r.add(note(
        "On most laptops, and some keyboards, hold Fn and press F2. If nothing happens, pick another key here.",
    ));
    let mut m = switch("mouse", "Mouse moves 1:1", true)
        .sub("The game caps how far the view can turn in one frame, so fast flicks feel like the mouse slows down. This widens that cap for a linear, 1:1 feel.");
    m.text = "RECOMMENDED".into();
    r.add(m);
    r.add(switch("brighten", "Brighten the picture", true));
    r.add(slider("brightness", "Brightness", 120.0, 50.0, 200.0, "1.20"));
    r.add(
        field("cmd", "Extra command line", "")
            .placeholder("Unreal Engine options")
            .button(b("save", "Save").enabled(false)),
    );
    r.card("Connection test");
    r.add(result("Server config", "ok", ""));
    r.add(result("Signing in to the game service", "warn", "Slow to answer (900 ms)"));
    r.add(result("The server's helper for playing over the internet", "fail", "No answer on UDP 13000"));
    w.set_page_rows(rows(r.done()));
}

fn friends(w: &AppWindow) {
    use crate::pages::*;
    w.set_screen(s("friends"));
    w.set_page_kicker(s("Squad"));
    w.set_page_title(s("Friends"));
    w.set_page_summary(s(""));
    w.set_page_sections(model(vec![]));
    w.set_page_section_ids(model(vec![]));
    let mut r = Rows::default();
    r.add(segments(
        "tab",
        "",
        &[
            ("friends".into(), "Friends · 2/5".into()),
            ("requests".into(), "Requests · 1".into()),
            ("find".into(), "Find players".into()),
            ("blocked".into(), "Blocked".into()),
        ],
        "friends",
    ));
    r.out();
    r.add(item("Kestrel", "In a Spies vs Mercs lobby · K7QF-2M9D").tone("accent").buttons(vec![
        b("join", "Join").kind("primary"),
        b("rm", "Remove").kind("fix"),
        b("bl", "Block").kind("fix"),
    ]));
    r.add(
        item("Marlow", "Co-op · Opium Farm, under way")
            .tone("accent")
            .buttons(vec![b("rm", "Remove").kind("fix"), b("bl", "Block").kind("fix")]),
    );
    r.add(
        item("Quillon", "Offline")
            .tone("muted")
            .buttons(vec![b("rm", "Remove").kind("fix"), b("bl", "Block").kind("fix")]),
    );
    r.card("Playing on other servers");
    r.add(
        item("Ravel_77", "On EU (Falkenstein, Germany): to play together, one of you joins the other's server")
            .tone("warn")
            .button(b("sw", "Switch server")),
    );
    w.set_page_rows(rows(r.done()));
}

fn dialog(w: &AppWindow) {
    use crate::pages::*;
    play_ready(w);
    let mut r = Rows::default();
    r.add(item("Singapore", "Matchmaking · community server · 5th Echelon 0.5.0").text("38 ms").tone("accent"));
    r.add(note(
        "You don't have an account here yet. Pick the name other players see. Your identity signs you in from now on: no password to remember.",
    ));
    r.add(field("name", "Your name", "Nightshade"));
    r.add(result("Taken on this server", "fail", ""));
    r.add(buttons(vec![b("a", "Nightshade7").kind("ghost"), b("b", "Nightshade_42").kind("ghost")]));
    let mut row = buttons(vec![b("close", "Stay on EU"), b("create", "Create account").kind("primary").enabled(false)]);
    row.tone = "end";
    r.add(row);
    w.set_dialog_title(s("Choose your name on this server"));
    w.set_dialog_kicker(s(""));
    w.set_dialog_rows(rows(r.done()));
    w.set_dialog_shown(true);
}

fn check(status: &str, title: &str, detail: &str, fix_id: &str, fix_label: &str) -> crate::CheckRow {
    crate::CheckRow {
        id: s(title),
        status: s(status),
        title: s(title),
        detail: s(detail),
        fix_id: s(fix_id),
        fix_label: s(fix_label),
    }
}

fn play_ready(w: &AppWindow) {
    w.set_activity(crate::ActivityView {
        shown: true,
        id: 1,
        title: s("Signed in to Singapore"),
        step: s(""),
        state: s("done"),
        message: s(""),
        progress: -1.0,
        actions: model(vec![]),
        waiting: 0,
    });
}

fn play_fix(w: &AppWindow) {
    w.set_checks(model(vec![
        check("ok", "Game found", "", "", ""),
        check(
            "fail",
            "The 5th Echelon client is out of date",
            "The game has 0.4.7's client; this launcher brings 0.5.0's.",
            "install-client",
            "Install the client",
        ),
        check(
            "warn",
            "Graphics adapter set by hand",
            "The game uses adapter 1; Auto picks the one it starts on.",
            "auto-adapter",
            "Set it to Auto",
        ),
    ]));
    w.set_trouble(true);
    w.set_signed_in(false);
    w.set_button(crate::PlayButton {
        label: s("Install the client"),
        sub: s("1 thing to fix before playing"),
        tone: s("fail"),
        disabled: false,
        busy: false,
        why: s(""),
        action: s(""),
    });
    w.set_activity(crate::ActivityView {
        shown: true,
        id: 2,
        title: s("Couldn't install the client"),
        step: s(""),
        state: s("failed"),
        message: s("The game's folder is read-only."),
        progress: -1.0,
        actions: model(vec![
            crate::ActionItem {
                id: s("retry"),
                label: s("Try again"),
            },
            crate::ActionItem {
                id: s("copy-details"),
                label: s("Copy details"),
            },
        ]),
        waiting: 1,
    });
}

fn play_notice(w: &AppWindow) {
    w.set_button(crate::PlayButton {
        label: s("Play SvM"),
        sub: s("0.5.0 is rolling out: your server restarts in 12 min"),
        tone: s("warn"),
        disabled: false,
        busy: false,
        why: s(""),
        action: s(""),
    });
    let row = |place: &str, mine: bool, doing: &str, meter: f32, tone: &str| crate::NoticeRow {
        place: s(place),
        mine,
        version: s("0.5.0"),
        doing: s(doing),
        meter,
        tone: s(tone),
    };
    w.set_notice(crate::NoticeView {
        shown: true,
        heading: s("0.5.0 is rolling out"),
        eta: s("about 12 min"),
        text: s("Each server restarts for a minute or two. Matches under way finish first."),
        note: s(""),
        play_on_host: s("eu.example.org"),
        play_on_label: s("Play on EU meanwhile"),
        servers: model(vec![
            row("EU", false, "Updated", 1.0, "done"),
            row("Singapore", true, "Waiting for 2 matches", 0.4, "now"),
            row("North America", false, "Next", -1.0, "waiting"),
        ]),
    });
    w.set_activity(crate::ActivityView {
        shown: true,
        id: 3,
        title: s("Downloading the game's update"),
        step: s("31 of 120 MB"),
        state: s("working"),
        message: s(""),
        progress: 0.26,
        actions: model(vec![]),
        waiting: 0,
    });
}

fn play_checking(w: &AppWindow) {
    w.set_checks_loaded(false);
    w.set_checks(model(vec![]));
    w.set_button(crate::PlayButton {
        label: s("Play SvM"),
        sub: s("Checking…"),
        tone: s("checking"),
        disabled: true,
        busy: true,
        why: s(""),
        action: s(""),
    });
    w.set_squad_loaded(false);
}

fn person(name: &str, host: bool, friend: bool, team: &str, ping: i32, party: i32, party_line: &str) -> crate::PersonRow {
    crate::PersonRow {
        name: s(name),
        initial: s(&name[..1].to_uppercase()),
        host,
        party,
        party_line: s(party_line),
        friend,
        team: s(team),
        ping,
        me: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn lobby(
    room: i32,
    mode: &str,
    kind: &str,
    map: &str,
    players: i32,
    seats: i32,
    host: &str,
    status: &str,
    under_way: bool,
    full: bool,
    ping: i32,
    line: &str,
    people: Vec<crate::PersonRow>,
    teams: bool,
) -> crate::LobbyRow {
    let spies: Vec<_> = people.iter().filter(|p| p.team == "spy").cloned().collect();
    let mercs: Vec<_> = people.iter().filter(|p| p.team == "merc").cloned().collect();
    crate::LobbyRow {
        room,
        mode: s(mode),
        kind: s(kind),
        map: s(map),
        players,
        seats,
        made_for: -1,
        host: s(host),
        status: s(status),
        tone: s(if under_way || full { "warn" } else { "ok" }),
        under_way,
        full,
        joinable: !under_way && !full,
        open_for: s("4 min"),
        ping,
        line: s(line),
        has_friends: line.contains("friend"),
        teams,
        people: model(people),
        spies: model(spies),
        mercs: model(mercs),
        with_me: false,
        settings: model(vec![
            crate::SettingRow {
                key: s("Time limit"),
                value: s("5 min"),
            },
            crate::SettingRow {
                key: s("Score limit"),
                value: s("3"),
            },
            crate::SettingRow {
                key: s("Spies' respawn"),
                value: s("7 s"),
            },
            crate::SettingRow {
                key: s("Mercs' respawn"),
                value: s("15 s"),
            },
        ]),
    }
}

fn lobbies() -> Vec<crate::LobbyRow> {
    vec![
        lobby(
            4391,
            "SvM Blacklist",
            "svm",
            "Virus Vault",
            5,
            8,
            "Liesel",
            "In lobby · waiting for 3 more",
            false,
            false,
            50,
            "Hosted by Liesel · Kestrel, your friend, is in it",
            vec![
                person("Liesel", true, false, "", 31, 0, ""),
                person("Orrin", false, false, "", 139, 1, "party with Pell"),
                person("Pell", false, false, "", 52, 1, "party with Orrin"),
                person("Kestrel", false, true, "", 44, 0, ""),
                person("Tamsin", false, false, "", 47, 0, ""),
            ],
            false,
        ),
        lobby(
            4375,
            "SvM Classic",
            "svm",
            "Cartel (Classic)",
            4,
            4,
            "Brannock",
            "Round 2 under way · 6 min",
            true,
            true,
            45,
            "Hosted by Brannock",
            vec![
                person("Brannock", true, false, "spy", 45, 0, ""),
                person("Wren", false, false, "spy", 60, 0, ""),
                person("Granite", false, false, "merc", 70, 0, ""),
                person("Ostrava", false, false, "merc", 88, 0, ""),
            ],
            true,
        ),
        lobby(
            4402,
            "Co-op",
            "coop",
            "Opium Farm",
            1,
            2,
            "Lucan",
            "In lobby · waiting for 1 more",
            false,
            false,
            212,
            "Hosted by Lucan",
            vec![person("Lucan", true, false, "", 212, 0, "")],
            false,
        ),
        lobby(
            4410,
            "Team Deathmatch",
            "svm",
            "Silo",
            6,
            8,
            "Sable",
            "Between rounds",
            false,
            false,
            61,
            "Hosted by Sable",
            vec![person("Sable", true, false, "", 61, 0, "")],
            false,
        ),
    ]
}

fn matches(w: &AppWindow) {
    w.set_screen(s("matches"));
    let rows = lobbies();
    w.set_lobbies_loaded(true);
    w.set_lobbies_error(s(""));
    w.set_lobbies_summary(s("5 public · 26 players · 2 private matches · 6 in parties"));
    w.set_lobbies_hidden(s("1 more left out: with a player you blocked."));
    w.set_lobbies_total(5);
    w.set_count_svm(4);
    w.set_count_coop(1);
    w.set_selected(rows[0].clone());
    w.set_has_selected(true);
    w.set_shown(model(rows));
}

fn matches_teams(w: &AppWindow) {
    matches(w);
    let rows = lobbies();
    w.set_selected(rows[1].clone());
    w.set_watching_room(4375);
}

fn matches_empty(w: &AppWindow) {
    w.set_screen(s("matches"));
    w.set_lobbies_loaded(true);
    w.set_lobbies_error(s(""));
    w.set_lobbies_summary(s("0 public · 0 players · 1 private match · 3 in parties"));
    w.set_lobbies_hidden(s(""));
    w.set_lobbies_total(0);
    w.set_shown(model(vec![]));
    w.set_has_selected(false);
}

/// The guided setup's real page (pages/setup.rs) on this machine as it is: no game here, so its
/// first step.
fn setup(w: &AppWindow) {
    use crate::pages::Screen;
    let core = std::sync::Arc::new(crate::core::Core::new(std::sync::Arc::new(|_| {})));
    let app = crate::tauri::Builder::new()
        .manage(crate::commands::Launcher(core))
        .build(std::sync::Arc::new(|_| {}), std::sync::Arc::new(|_| true));
    let view = crate::pages::setup::SetupPage::new(true).view(&app);
    w.set_screen(s("setup"));
    w.set_page_kicker(s(&view.kicker));
    w.set_page_title(s(&view.title));
    w.set_page_summary(s(&view.summary));
    w.set_page_sections(model(vec![]));
    w.set_page_section_ids(model(vec![]));
    w.set_page_rows(rows(view.rows));
}
