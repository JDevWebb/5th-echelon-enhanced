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

/// One screen to take: its name, what to set, and the one size it's taken at (else all).
struct Shot {
    name: &'static str,
    set: fn(&AppWindow),
    size: Option<(u32, u32)>,
}

/// Sizes only some shots are taken at.
const EXTRA: [(u32, u32); 1] = [(1280, 1900)];

/// From the window's smallest up to 2560x1440: compact (960, the minimum), regular (1280,
/// 1366) and wide (1600 and up).
const SIZES: [(u32, u32); 6] = [(960, 600), (1280, 800), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440)];

pub fn run(dir: PathBuf) -> Result<(), slint::PlatformError> {
    std::fs::create_dir_all(&dir).map_err(|e| slint::PlatformError::Other(e.to_string()))?;
    let w = AppWindow::new()?;
    w.set_release(s(env!("FE_RELEASE")));
    base(&w);
    let shots: Rc<Vec<Shot>> = Rc::new(vec![
        Shot {
            name: "splash",
            set: |w| w.set_loaded(false),
            size: None,
        },
        Shot {
            name: "play-ready",
            set: play_ready,
            size: None,
        },
        Shot {
            name: "play-fix",
            set: play_fix,
            size: None,
        },
        Shot {
            name: "play-notice",
            set: play_notice,
            size: None,
        },
        Shot {
            name: "play-checking",
            set: play_checking,
            size: None,
        },
        Shot {
            name: "matches",
            set: matches,
            size: None,
        },
        Shot {
            name: "matches-teams",
            set: matches_teams,
            size: None,
        },
        Shot {
            name: "matches-empty",
            set: matches_empty,
            size: None,
        },
        Shot {
            name: "settings",
            set: settings,
            size: None,
        },
        Shot {
            name: "friends",
            set: friends,
            size: None,
        },
        Shot {
            name: "dialog",
            set: dialog,
            size: None,
        },
        Shot {
            name: "setup",
            set: setup,
            size: None,
        },
        Shot {
            name: "friends-empty",
            set: friends_empty,
            size: None,
        },
        Shot {
            name: "whats-new",
            set: whats_new,
            size: None,
        },
        Shot {
            name: "banner",
            set: banner,
            size: None,
        },
        Shot {
            name: "settings-save",
            set: settings_save,
            size: None,
        },
        Shot {
            name: "settings-about",
            set: settings_about,
            size: None,
        },
        Shot {
            name: "servers",
            set: servers,
            size: None,
        },
        Shot {
            name: "intel",
            set: intel,
            size: None,
        },
        Shot {
            name: "support",
            set: support,
            size: None,
        },
        Shot {
            name: "chat",
            set: chat,
            size: None,
        },
        Shot {
            name: "chat-direct",
            set: chat_direct,
            size: None,
        },
        Shot {
            name: "chat-lobby",
            set: chat_lobby,
            size: None,
        },
        Shot {
            name: "host-empty",
            set: host_empty,
            size: None,
        },
        Shot {
            name: "host-stopped",
            set: host_stopped,
            size: None,
        },
        Shot {
            name: "host-running",
            set: host_running,
            size: None,
        },
        // About whole, to its small print and Slint's widget, on a window tall enough for it.
        Shot {
            name: "settings-about-full",
            set: settings_about,
            size: Some((1280, 1900)),
        },
    ]);
    // Every shot in each look and size, one after another on the window's own clock; or only
    // some while working on a screen: FE_SHOTS (names starting so, comma-separated),
    // FE_SHOT_SIZES ("1280x800,1920x1080") and FE_SHOT_LOOKS ("dark" or "light").
    let only = |var: &str| -> Vec<String> {
        std::env::var(var)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    };
    let (names, sizes, looks) = (only("FE_SHOTS"), only("FE_SHOT_SIZES"), only("FE_SHOT_LOOKS"));
    let mut steps = Vec::new();
    for dark in [true, false] {
        if !looks.is_empty() && !looks.iter().any(|l| (l == "dark") == dark) {
            continue;
        }
        for size in SIZES.into_iter().chain(EXTRA) {
            let named = format!("{}x{}", size.0, size.1);
            if !sizes.is_empty() && !sizes.contains(&named) {
                continue;
            }
            for (i, shot) in shots.iter().enumerate() {
                if !names.is_empty() && !names.iter().any(|n| shot.name.starts_with(n.as_str())) {
                    continue;
                }
                // A shot for one size only (About's whole page, on a tall window).
                if shot.size.is_some_and(|only| only != size) || (shot.size.is_none() && EXTRA.contains(&size)) {
                    continue;
                }
                steps.push((dark, size, i));
            }
        }
    }
    let steps = Rc::new(steps);
    let at = Rc::new(std::cell::Cell::new(0usize));
    let retries = Rc::new(std::cell::Cell::new(0u32));
    let timer = Rc::new(slint::Timer::default());
    let weak = w.as_weak();
    let next = {
        let (steps, shots, at, dir, timer2) = (steps.clone(), shots.clone(), at.clone(), dir.clone(), timer.clone());
        let retries = retries.clone();
        move || {
            let Some(w) = weak.upgrade() else { return };
            let i = at.get();
            // The one before was set up a moment ago: taken now.
            if i > 0 {
                let (dark, (width, height), shot) = steps[i - 1];
                let name = format!("{}-{}-{width}x{height}.png", shots[shot].name, if dark { "dark" } else { "light" });
                // The screen-change cover off: on the software renderer, with the window resized
                // between shots, it sometimes hadn't cleared, and the shot had the side menu alone
                // (the 1920x1080 ones after Play). The launcher itself clears it.
                w.set_covering(false);
                // The window not at its size yet (the X server sometimes lets a resize go):
                // asked again, and taken on the next tick.
                let size = w.window().size();
                if (size.width != width || size.height != height) && retries.get() < 5 {
                    retries.set(retries.get() + 1);
                    w.window().set_size(slint::LogicalSize::new(width as f32, height as f32));
                    return;
                }
                retries.set(0);
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
            // The size band, as the window reports it to the pages (shell::wire isn't run here).
            crate::pages::set_band(if width < 1100 {
                "compact"
            } else if width >= 1600 {
                "wide"
            } else {
                "regular"
            });
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
    w.set_banner(s(""));
    w.set_page_side(model(vec![]));
    w.set_page_split(0.6);
    w.set_page_layout(s(""));
    w.set_page_status(s(""));
    w.set_page_status_tone(s(""));
    w.set_page_actions(model(vec![]));
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

fn someone(id: &str, name: &str, identity: Option<&str>, activity: Option<&str>, relation: &'static str) -> launcher::flow::Person {
    launcher::flow::Person {
        id: id.into(),
        name: name.into(),
        identity: identity.map(String::from),
        online: activity.is_some(),
        activity: activity.filter(|a| !a.is_empty()).map(String::from),
        joinable: id == "1",
        with_me: false,
        relation,
    }
}

/// Friends, from the page's own builder on made-up people: tabs at compact and regular sizes,
/// two columns at wide.
fn friends(w: &AppWindow) {
    let people = launcher::flow::People {
        friends: vec![
            someone("1", "Kestrel", Some("K7QF-2M9D"), Some("In a Spies vs Mercs lobby"), "friend"),
            someone("2", "Marlow", None, Some("Co-op · Opium Farm, under way"), "friend"),
            someone("5", "Quillon", None, None, "friend"),
        ],
        received: vec![someone("4", "Ravel_77", None, Some(""), "received")],
        sent: vec![],
        blocked: vec![someone("9", "Grief3r", None, None, "blocked")],
        elsewhere: vec![launcher::flow::Elsewhere {
            name: "Hollis".into(),
            server: "EU".into(),
            region: "Falkenstein, Germany".into(),
            host: "eu.example.org".into(),
        }],
        everyone: true,
    };
    let mut page = crate::pages::friends::FriendsPage::default();
    page.set_found(vec![
        someone("6", "Tamsin", None, Some("In the menus"), ""),
        someone("7", "Pell", None, Some("In a Co-op lobby"), ""),
        someone("8", "Orrin", None, Some(""), "sent"),
    ]);
    let wide = crate::pages::wide();
    self::page(w, "friends", page.build(&people, false, false, wide));
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
        // A match under way with two waiting to join it (the matches shot shows the queue).
        waiting: if under_way { 2 } else { 0 },
        queue_place: 0,
        art: Default::default(),
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
    page(w, "setup", view);
}

/// A page's view on the window, as the launcher shows it.
fn page(w: &AppWindow, screen: &str, view: crate::pages::PageView) {
    w.set_screen(s(screen));
    crate::shell::show_page(w, &view);
}

/// An empty page: the mark, its heading, why, and what to do.
fn friends_empty(w: &AppWindow) {
    let mut page = crate::pages::friends::FriendsPage::default();
    page.set_found(vec![]);
    let view = page.build(&launcher::flow::People::default(), false, false, crate::pages::wide());
    self::page(w, "friends", view);
}

/// What's new after an update, from the dialog's own builder.
fn whats_new(w: &AppWindow) {
    play_ready(w);
    let v = serde_json::json!({
        "from": "0.4.7",
        "to": "0.5.0",
        "headlines": 3,
        "notes": ["Release notes", "https://example.org/notes"],
        "releases": [{
            "version": "0.5.0",
            "thanks": "",
            "new": [
                {"title": "One matchmaking server for everyone", "text": "Quick match finds a match on the whole network.", "community": true},
                {"title": "A new launcher", "text": "Faster, in dark and light, the same everywhere.", "community": false}
            ],
            "fixed": [["Stuck searching", ": started matches no longer show as open."]]
        }]
    });
    let view = crate::pages::dialogs::whats_new(&v);
    w.set_dialog_kicker(s(&view.page.kicker));
    w.set_dialog_title(s(&view.page.title));
    w.set_dialog_rows(rows(view.page.rows));
    w.set_dialog_width(view.width);
    w.set_dialog_shown(true);
}

/// The update banner with a newer release out.
fn banner(w: &AppWindow) {
    play_ready(w);
    w.set_banner(s("Version 0.5.1 is out."));
    w.set_banner_button(s("Update now"));
}

/// Settings › Save game, from the page's own builder on a made-up save.
fn settings_about(w: &AppWindow) {
    use crate::pages::*;
    settings(w);
    w.set_page_summary(s("About"));
    w.set_page_section(s("about"));
    let v = serde_json::json!({ "product": "5th Echelon Enhanced", "release": "0.5.0" });
    let mut r = Rows::default();
    crate::pages::settings::SettingsPage::default().about(&v, &mut r);
    w.set_page_rows(rows(r.done()));
}

fn settings_save(w: &AppWindow) {
    use crate::pages::*;
    settings(w);
    w.set_page_summary(s("Save game"));
    w.set_page_section(s("save"));
    let v = serde_json::json!({ "save": {
        "path": "C:\\Users\\Kai\\AppData\\Roaming\\5th-Echelon\\Saves\\00000001.sav",
        "state": "Found.", "missing": false, "ubisoft": "C:\\Program Files (x86)\\Ubisoft\\savegames\\1.save",
        "saved": "2 hours ago", "progress": "Rank 5 or above · 18240 XP", "backups": 3, "last_backup": "yesterday"
    }});
    let mut r = Rows::default();
    crate::pages::settings::SettingsPage::default().save(&v, &mut r);
    w.set_page_rows(rows(r.done()));
}

/// Host a server: what crate::host says, made up (no server here).
fn host_view(found: bool, running: bool) -> serde_json::Value {
    serde_json::json!({
        "exe_name": "dedicated_server.exe",
        "found": found,
        "error": null,
        "running": running,
        "starting": false,
        "downloading": false,
        "listen": null,
        "adapters": [["Ethernet", "192.168.1.20"]],
        "trusted_subnet": "",
        "api": { "info": true, "presence": true, "accounts": false, "unhandled": false },
        "managing": if running { serde_json::json!("http://127.0.0.1:50050") } else { serde_json::Value::Null },
        "local": running,
        "uptime": if running { serde_json::json!(8040) } else { serde_json::Value::Null },
    })
}

fn host_page(w: &AppWindow, v: serde_json::Value, lists: Option<serde_json::Value>, logs: Vec<serde_json::Value>) {
    let mut page = crate::pages::host::HostPage::default();
    let view = page.build(&v, lists.as_ref(), &logs, crate::pages::compact());
    self::page(w, "host", view);
}

/// Not downloaded yet.
fn host_empty(w: &AppWindow) {
    host_page(w, host_view(false, false), None, vec![]);
}

/// Downloaded, stopped: its settings to change.
fn host_stopped(w: &AppWindow) {
    host_page(w, host_view(true, false), None, vec![]);
}

/// Running for 2 h 14 min, four players in two games, a warning in the log.
fn host_running(w: &AppWindow) {
    let lists = serde_json::json!({
        "players": { "Ok": [
            { "id": "1", "name": "Kestrel", "ips": "192.168.1.31" },
            { "id": "2", "name": "Marlow", "ips": "198.51.100.24" },
            { "id": "3", "name": "Quillon", "ips": "192.168.1.44" },
            { "id": "4", "name": "Ravel_77", "ips": "192.168.1.52" },
        ]},
        "games": { "Ok": [
            { "id": 12, "kind": "SvM Classic", "who": "Kestrel with Marlow", "players": ["Kestrel", "Marlow"] },
            { "id": 14, "kind": "Co-op", "who": "Quillon with nobody yet", "players": ["Quillon"] },
        ]},
    });
    let logs = vec![
        serde_json::json!({ "time": "14:02:11", "tone": "", "msg": "Kestrel signed in from 192.168.1.31" }),
        serde_json::json!({ "time": "14:02:40", "tone": "", "msg": "Game #12 created by Kestrel (SvM Classic)" }),
        serde_json::json!({ "time": "14:05:02", "tone": "warn", "msg": "Relay: no answer from 198.51.100.24 for 20 s" }),
        serde_json::json!({ "time": "14:05:09", "tone": "", "msg": "Marlow joined game #12" }),
    ];
    host_page(w, host_view(true, true), Some(lists), logs);
}

fn view(kicker: &str, title: &str, summary: &str, rows: Vec<crate::pages::R>) -> crate::pages::PageView {
    crate::pages::PageView {
        kicker: kicker.into(),
        title: title.into(),
        summary: summary.into(),
        rows,
        ..Default::default()
    }
}

/// Servers, as pages/servers.rs lays it out, on made-up servers.
fn servers(w: &AppWindow) {
    use crate::pages::*;
    let mut r = Rows::default();
    r.card("Network");
    r.add(
        item(
            "scbl.example.org",
            "The 5th Echelon community network · 3 servers · your friends and identity follow you between its servers",
        )
        .tone("accent")
        .text("In use"),
    );
    r.add(rule());
    r.add(bold("Use another network"));
    r.add(
        field("network", "", "")
            .placeholder("its address, e.g. play.mygroup.org")
            .button(b("look-up", "Look it up").enabled(false)),
    );
    r.add(note("Another group's network of servers: its servers then fill the server menu on the Play screen."));
    r.card("A server of your own");
    r.add(
        field("address", "", "")
            .placeholder("play.example.org or 192.168.1.20")
            .button(b("connect", "Connect").kind("primary").enabled(false)),
    );
    r.add(note("A private server for your group, outside the network: the server menu takes you back."));
    r.add(note("Recent"));
    r.add(item("LAN party · 192.168.1.20", "Yesterday").button(b("own", "Connect")));
    r.card("On your network");
    r.add(note("Looks for a server running at a LAN party, on this network."));
    r.add(buttons(vec![b("lan", "Find on my network")]));
    r.card("Host on this PC");
    r.add(note(
        "Run a server for your group: a LAN party, or friends over the internet. Their launchers find it on your network.",
    ));
    r.add(buttons(vec![b("host", "Host a server")]));
    page(
        w,
        "servers",
        view(
            "Network",
            "Servers",
            "The community network is on the Play screen. Here: a server of your own, a LAN party, or another network.",
            r.done(),
        ),
    );
}

/// Intel, as pages/intel.rs lays it out: the server's briefings.
fn intel(w: &AppWindow) {
    use crate::pages::*;
    let mut r = Rows::default();
    r.add(segments("tab", "", &[("news".into(), "News".into()), ("roadmap".into(), "Roadmap".into())], "news"));
    r.add(note("From scbl.example.org, as the game's news screen shows it"));
    r.card("Briefings");
    r.add(
        item(
            "0.5.0: one matchmaking server for everyone",
            "Quick match finds a match on the whole network, and fair play checks",
        )
        .on(true)
        .tone("accent"),
    );
    r.add(item("Weekend SvM night", "Saturday 20:00 UTC on Germany: bring a friend"));
    r.add(item("Maintenance", "Australia restarts Tuesday 03:00 UTC for a few minutes"));
    r.card("0.5.0: one matchmaking server for everyone");
    r.add(note("Briefing 1 of 3").tone("accent"));
    r.add(text("Quick match now looks across the whole network for a match near you, and the relays carry players who can't reach each other directly. Fair play checks run on every server."));
    r.add(buttons(vec![b("link", "Read more on example.org").kind("link")]));
    page(w, "intel", view("From your server", "Intel", "", r.done()));
}

/// Support, as pages/support.rs lays it out: a conversation with the admins.
fn support(w: &AppWindow) {
    use crate::pages::*;
    let mut r = Rows::default();
    r.card("Your conversation");
    r.add(message("You", "My game closes when the match ends, back to the lobby. Log attached.", "Mon 21:14"));
    r.add(
        message(
            "Admins",
            "Thanks: it's the crash on the way back to the lobby, fixed in 0.5.0. Update when it's out and tell us if it happens again.",
            "Tue 09:02",
        )
        .on(true),
    );
    r.card("Message");
    r.add(area("message", "", "").placeholder("What went wrong, when, and on which server?"));
    r.add(note("0/2000"));
    r.add(switch("game_log", "The game's log (its last part, and the one before)", true));
    r.add(switch("launcher_log", "The launcher's log", false));
    r.add(note("Your PC's name, your user folder and your internet address are hidden in them first."));
    r.add(buttons(vec![b("send", "Send").kind("primary").enabled(false)]));
    r.card("Something went wrong?");
    r.add(note("Send a report with the logs, or run every check on the Play screen."));
    r.add(buttons(vec![b("feedback", "Send a report with the logs"), b("checks", "Run all checks")]));
    page(
        w,
        "support",
        view(
            "The network's admins",
            "Support",
            "Write to the community network's admins about anything that goes wrong. They answer here, and you're told when they do, here and in the game.",
            r.done(),
        ),
    );
}

/// A chat message as crate::chat::ChatView has it (`at` in minutes past 14:00 today, or
/// yesterday's below 0), its invite as a part of its own.
#[allow(clippy::too_many_arguments)]
fn said(id: i64, minute: i64, from: &str, name: &str, body: &str, mine: bool, friend: bool, mention: bool) -> serde_json::Value {
    let at = 1_790_000_000 + minute * 60;
    let (day, time) = if minute < 0 {
        // 16 hours 40 before 14:00: from 21:20 the day before.
        ("Yesterday", format!("21:{:02}", (minute + 1240 + 40).rem_euclid(60)))
    } else {
        ("Today", format!("14:{minute:02}"))
    };
    let parts = match body.split_once("discord.gg/") {
        Some((before, link)) => serde_json::json!([{ "text": before, "link": false }, { "text": format!("discord.gg/{link}"), "link": true }]),
        None => serde_json::json!([{ "text": body, "link": false }]),
    };
    serde_json::json!({ "id": id, "at": at, "time": time, "day": day, "from": from, "from_name": name,
        "server": "Singapore", "parts": parts, "mine": mine, "mentions_me": mention, "friend": friend })
}

/// Chat's view on made-up people, as crate::chat::chat gives it.
fn chat_view(messages: Vec<serde_json::Value>, trouble: &str, read_only: bool, with_name: &str) -> serde_json::Value {
    let person = |id: &str, name: &str, activity: &str, friend: bool, joinable: bool, asked: bool| {
        serde_json::json!({ "id": id, "name": name, "activity": if activity.is_empty() { serde_json::Value::Null } else { activity.into() },
            "friend": friend, "asked": asked, "joinable": joinable })
    };
    let mut online = vec![
        person("1", "Kestrel", "SvM lobby · 3/4", true, true, false),
        person("2", "Marlow", "Co-op · Opium Farm, under way", true, false, false),
        person("5", "Pell", "SvM Blacklist · Wake", false, false, false),
        person("6", "Orrin", "Co-op · Dead Drop", false, false, false),
        person("7", "Tamsin", "", false, false, false),
        person("8", "Ravel_77", "", false, false, true),
        person("9", "Sable", "SvM Classic · Penthouse", false, false, false),
    ];
    for i in 0..17 {
        online.push(person(
            &format!("x{i}"),
            &format!("Agent_{:02}", i + 10),
            if i < 3 { "SvM Classic · Station" } else { "" },
            false,
            false,
            false,
        ));
    }
    let conv = |with: &str, name: &str, unread: u32, online: bool, direct: bool| serde_json::json!({ "with": with, "name": name, "what": "Direct", "unread": unread, "direct": direct, "friend": true, "online": online });
    serde_json::json!({
        "state": "open", "trouble": if trouble.is_empty() { serde_json::Value::Null } else { trouble.into() },
        "read_only": read_only, "may_post_global": true, "global_wait": "", "lobby": true,
        "messages": messages,
        "conversations": [conv("K7QF2M9DAAAA", "Kestrel", 2, true, true), conv("M4RL0WAAAAAA", "Marlow", 0, true, true),
            conv("QU1LL0NAAAAA", "Quillon", 0, false, true), conv("T4MS1NAAAAAA", "Tamsin", 0, false, false)],
        "online": online, "with_name": with_name, "unread": 2,
        "rules": launcher::chat::RULES_LINE, "max_message": 500, "join_offered": true, "lobby_ids": [101, 102, 103],
    })
}

/// The shots' chat page: its room, a first look (what was seen before), then the view.
fn chat_page(w: &AppWindow, room: &str, first: Option<serde_json::Value>, v: serde_json::Value, picked: Option<i64>, person: Option<&str>) {
    use crate::pages::Screen;
    let mut page = crate::pages::chat::ChatPage::new();
    page.section(room);
    if let Some(first) = first {
        page.build(&first);
        page.section("lobby");
        page.build(&chat_view(vec![], "", false, ""));
        page.section(room);
    }
    page.pick(picked, person);
    let view = page.build(&v);
    self::page(w, "chat", view);
}

/// Chat's Global: a sender's lines together, a mention, the unseen from "New traffic", a
/// Discord invite, the player's own; the roster, and Tamsin's dossier.
fn chat(w: &AppWindow) {
    let global = |more: bool| {
        let mut m = vec![
            said(1, 2, "K7QF2M9DAAAA", "Kestrel", "anyone up for SvM Classic on Penthouse?", false, true, false),
            said(2, 2, "K7QF2M9DAAAA", "Kestrel", "need one more merc, we're 3/4", false, true, false),
            said(3, 3, "T4MS1NAAAAAA", "Tamsin", "that last round was so *** close", false, false, false),
            said(4, 4, "M4RL0WAAAAAA", "Marlow", "@Nightshade you in? Kestrel's lobby", false, true, true),
        ];
        if more {
            m.push(said(
                5,
                5,
                "P3LLAAAAAAAA",
                "Pell",
                "EU crew plays on our Discord most nights: discord.gg/blacklist-eu",
                false,
                false,
                false,
            ));
            m.push(said(6, 6, "N1GHTAAAAAAA", "Nightshade", "yep, on my way", true, false, false));
            m.push(said(7, 6, "N1GHTAAAAAAA", "Nightshade", "joining now", true, false, false));
        }
        m
    };
    chat_page(
        w,
        "global",
        Some(chat_view(global(false), "", false, "")),
        chat_view(global(true), "", false, ""),
        Some(5),
        Some("7"),
    );
}

/// A direct message: Kestrel's strip with Join, yesterday's lines and today's, a line picked.
fn chat_direct(w: &AppWindow) {
    let k = "K7QF2M9DAAAA";
    let me = "N1GHTAAAAAAA";
    let m = vec![
        said(11, -1240, k, "Kestrel", "gg, that last extraction was clean", false, true, false),
        said(12, -1239, me, "Nightshade", "same time tomorrow?", true, false, false),
        said(13, 1, k, "Kestrel", "lobby's up, Penthouse", false, true, false),
        said(14, 1, k, "Kestrel", "come when you're ready", false, true, false),
        said(15, 2, me, "Nightshade", "2 min", true, false, false),
    ];
    chat_page(w, &format!("with:{k}"), None, chat_view(m, "", false, "Kestrel"), Some(13), None);
}

/// Lobby while chat reconnects: read only, the line to write off.
fn chat_lobby(w: &AppWindow) {
    let m = vec![
        said(101, 8, "K7QF2M9DAAAA", "Kestrel", "mercs: me and Marlow. spies: you two", false, true, false),
        said(102, 8, "0RR1NAAAAAAA", "Orrin", "ready", false, false, false),
        said(103, 9, "N1GHTAAAAAAA", "Nightshade", "ready, start it", true, false, false),
    ];
    chat_page(w, "lobby", None, chat_view(m, "the server didn't answer", true, ""), None, None);
}
