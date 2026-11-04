//! Screenshots of every launcher screen and sub-screen at several window sizes, for checking
//! layouts by eye (`build/build.sh shots`: PNGs in dist/shots). The whole app, as `main` starts
//! it, on a made-up game folder (an empty executable) with a server profile; build.sh runs it
//! with no network at all, so what needs a server shows its waiting or failed state, and nothing
//! reaches the community network it names. Chat, which needs a server to fill it, is drawn from
//! sample messages.

use eframe::egui;

use crate::app::App;
use crate::app::View;

/// The window sizes looked at, in points: the first window's, a small laptop's, full HD.
const SIZES: [(&str, egui::Vec2); 3] = [
    ("1280x800", egui::vec2(1280.0, 800.0)),
    ("960x600", egui::vec2(960.0, 600.0)),
    ("1920x1080", egui::vec2(1920.0, 1080.0)),
];

/// The first-run questions answered, so none of them covers the screens.
const ANSWERED: &str = "diagnostics_asked = true\nlog_excerpt_told = true\nwhats_new_seen = \"9.9.9\"\n";
const OPTIONS_ASKED: &str = "options_asked = [\"mouse-accel\", \"brighten\", \"autoclimb\"]\n";
const COMMUNITY: &str = "directory = \"https://scbl.jdevwebb.net\"\n";

/// One shot: its file's name, the screen, the state set up on it (`App::shot`), and what the
/// launcher's settings say beforehand.
struct Shot {
    name: &'static str,
    view: View,
    what: &'static str,
    prefs: &'static str,
    /// The game has a server profile (else Play is the guided setup).
    profile: bool,
}

const fn shot(name: &'static str, view: View, what: &'static str) -> Shot {
    Shot {
        name,
        view,
        what,
        prefs: "answered",
        profile: true,
    }
}

fn shots() -> Vec<Shot> {
    let mut all = vec![
        shot("play", View::Play, ""),
        shot("play-drawer-squad", View::Play, "drawer-squad"),
        shot("play-drawer-intel", View::Play, "drawer-intel"),
        shot("play-server-menu", View::Play, "server-menu"),
        shot("matches", View::Matches, "matches-sample"),
        shot("friends", View::Friends, "friends-sample"),
        shot("friends-requests", View::Friends, "friends-requests"),
        shot("servers", View::Servers, ""),
        shot("host", View::Server, ""),
        shot("chat", View::Chat, ""),
        shot("intel-briefings", View::News, ""),
        shot("intel-roadmap", View::Roadmap, ""),
        shot("support", View::Support, ""),
        shot("whats-new", View::Play, "whats-new"),
        shot("feedback", View::Play, "feedback"),
        Shot {
            prefs: "options",
            ..shot("first-game-options", View::Play, "")
        },
        Shot {
            prefs: "diagnostics",
            ..shot("first-diagnostics", View::Play, "")
        },
        Shot {
            profile: false,
            ..shot("setup-server", View::Play, "")
        },
        Shot {
            profile: false,
            ..shot("setup-name", View::Play, "setup-name")
        },
    ];
    let sections = [
        "settings-game",
        "settings-network",
        "settings-servers",
        "settings-identity",
        "settings-notifications",
        "settings-save",
        "settings-client",
        "settings-advanced",
        "settings-display",
        "settings-feedback",
        "settings-about",
    ];
    all.extend(sections.into_iter().map(|s| shot(s, View::Settings, s)));
    all
}

/// The made-up game folder, with a profile for a made-up server or none, and the launcher's
/// settings naming it (`XDG_CONFIG_HOME`, set by build.sh to a folder of its own).
fn set_up(profile: bool, prefs: &str) {
    let config = std::env::var_os("XDG_CONFIG_HOME").expect("build.sh sets XDG_CONFIG_HOME");
    let game = std::path::Path::new(&config).join("game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("Blacklist_DX11_game.exe"), b"").unwrap();
    let mut cfg = setup::config::Config::load(&game);
    cfg.update(|c| {
        c.profiles.clear();
        if profile {
            c.upsert_profile(setup::config::Profile {
                name: "eu1.example.net".into(),
                server: "eu1.example.net".into(),
                user: hooks_config::User {
                    username: "Kiwi".into(),
                    ..Default::default()
                },
                ..Default::default()
            });
        }
    })
    .unwrap();
    let answered = match prefs {
        "options" => ANSWERED.to_string(),
        "diagnostics" => format!("whats_new_seen = \"9.9.9\"\n{OPTIONS_ASKED}"),
        _ => format!("{ANSWERED}{OPTIONS_ASKED}"),
    };
    let launcher = std::path::Path::new(&config).join("5th-Echelon");
    std::fs::create_dir_all(&launcher).unwrap();
    std::fs::write(launcher.join("launcher.toml"), format!("game_dir = {:?}\n{answered}{COMMUNITY}", game.to_string_lossy())).unwrap();
}

#[test]
fn screens_at_each_size() {
    let out = std::path::PathBuf::from(std::env::var("SHOTS_OUT").unwrap_or_else(|_| "shots".into()));
    std::fs::create_dir_all(&out).unwrap();
    for s in shots() {
        set_up(s.profile, s.prefs);
        for (size_name, size) in SIZES {
            let mut h = egui_kittest::Harness::builder().with_size(size).wgpu().build_eframe(|cc| {
                let mut app = App::new(cc);
                app.set_view(s.view);
                app
            });
            // A few frames: the theme and fonts, the first layout, then what it asked for; the
            // state is set once the app has drawn itself.
            for i in 0..8 {
                if i == 2 && !s.what.is_empty() {
                    let ctx = h.ctx.clone();
                    h.state_mut().shot(&ctx, s.what);
                }
                h.run_steps(1);
            }
            let path = out.join(format!("{}-{size_name}.png", s.name));
            h.render().expect("rendered").save(&path).unwrap();
            println!("{}", path.display());
        }
    }
}

/// Chat's rooms, drawn from sample messages (it needs a server to fill it otherwise).
#[test]
fn chat_with_sample_messages() {
    let out = std::path::PathBuf::from(std::env::var("SHOTS_OUT").unwrap_or_else(|_| "shots".into()));
    std::fs::create_dir_all(&out).unwrap();
    for tab in ["global", "lobby", "messages"] {
        for (size_name, size) in [("1220x640", egui::vec2(1220.0, 640.0)), ("840x520", egui::vec2(840.0, 520.0))] {
            let mut themed = false;
            let mut h = egui_kittest::Harness::builder().with_size(size).wgpu().build(move |ctx: &egui::Context| {
                if !themed {
                    crate::theme::apply(ctx);
                    themed = true;
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| crate::chat::sample(ui, tab));
            });
            for _ in 0..4 {
                h.run_steps(1);
            }
            let path = out.join(format!("chat-sample-{tab}-{size_name}.png"));
            h.render().expect("rendered").save(&path).unwrap();
            println!("{}", path.display());
        }
    }
}
