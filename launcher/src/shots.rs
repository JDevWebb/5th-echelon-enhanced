//! Screenshots of the launcher's screens at several window sizes, for checking a layout by eye
//! (`build/build.sh shots`: PNGs in dist/shots). The whole app, as `main` starts it, on a made-up
//! game folder (an empty executable) with a server profile, and no network: what needs a server
//! shows its waiting or failed state (build.sh runs it with no network at all, so nothing
//! reaches the community network it names).

use eframe::egui;

use crate::app::App;
use crate::app::View;

/// The window sizes looked at, in points: the design's, a small laptop's, full HD.
const SIZES: [(&str, egui::Vec2); 3] = [
    ("1280x800", egui::vec2(1280.0, 800.0)),
    ("960x600", egui::vec2(960.0, 600.0)),
    ("1920x1080", egui::vec2(1920.0, 1080.0)),
];

const VIEWS: [(&str, View); 8] = [
    ("play", View::Play),
    ("servers", View::Servers),
    ("chat", View::Chat),
    ("news", View::News),
    ("support", View::Support),
    ("settings", View::Settings),
    ("roadmap", View::Roadmap),
    ("host", View::Server),
];

/// A game folder the launcher takes, with a profile for a made-up server, named in the
/// launcher's settings (`XDG_CONFIG_HOME`, set by build.sh to a folder of its own).
fn made_up_game() -> std::path::PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").expect("build.sh sets XDG_CONFIG_HOME");
    let game = std::path::Path::new(&config).join("game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("Blacklist_DX11_game.exe"), b"").unwrap();
    let mut cfg = setup::config::Config::load(&game);
    cfg.update(|c| {
        c.upsert_profile(setup::config::Profile {
            name: "eu1.example.net".into(),
            server: "eu1.example.net".into(),
            user: hooks_config::User {
                username: "Kiwi".into(),
                ..Default::default()
            },
            ..Default::default()
        });
    })
    .unwrap();
    let launcher = std::path::Path::new(&config).join("5th-Echelon");
    std::fs::create_dir_all(&launcher).unwrap();
    std::fs::write(launcher.join("launcher.toml"), format!("game_dir = {:?}\n{}", game.to_string_lossy(), ANSWERED)).unwrap();
    game
}

/// The first-run questions answered, so none of them covers the screens.
const ANSWERED: &str = "diagnostics_asked = true\nlog_excerpt_told = true\nwhats_new_seen = \"9.9.9\"\n\
                        options_asked = [\"mouse-accel\", \"brighten\", \"autoclimb\"]\n\
                        directory = \"https://scbl.jdevwebb.net\"\n";

#[test]
fn screens_at_each_size() {
    let out = std::path::PathBuf::from(std::env::var("SHOTS_OUT").unwrap_or_else(|_| "shots".into()));
    std::fs::create_dir_all(&out).unwrap();
    made_up_game();
    for (size_name, size) in SIZES {
        for (view_name, view) in VIEWS {
            let mut h = egui_kittest::Harness::builder().with_size(size).wgpu().build_eframe(|cc| {
                let mut app = App::new(cc);
                app.set_view(view);
                app
            });
            // A few frames: the theme, the fonts, the first layout, then what it asked for.
            for _ in 0..6 {
                h.run_steps(1);
            }
            let image = h.render().expect("rendered");
            let path = out.join(format!("{view_name}-{size_name}.png"));
            image.save(&path).unwrap();
            println!("{}", path.display());
        }
    }
}

/// Parts that need a server to fill them, drawn from sample data.
#[test]
fn parts_with_sample_data() {
    let out = std::path::PathBuf::from(std::env::var("SHOTS_OUT").unwrap_or_else(|_| "shots".into()));
    std::fs::create_dir_all(&out).unwrap();
    for (size_name, size) in [("1280x700", egui::vec2(1220.0, 640.0)), ("900x560", egui::vec2(840.0, 520.0))] {
        let mut themed = false;
        let mut h = egui_kittest::Harness::builder().with_size(size).wgpu().build(move |ctx: &egui::Context| {
            if !themed {
                crate::theme::apply(ctx);
                themed = true;
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| crate::chat::sample(ui));
        });
        for _ in 0..4 {
            h.run_steps(1);
        }
        let path = out.join(format!("chat-sample-{size_name}.png"));
        h.render().expect("rendered").save(&path).unwrap();
        println!("{}", path.display());
    }
}
