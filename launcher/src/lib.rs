//! The 5th Echelon launcher: sets up Splinter Cell: Blacklist for a 5th Echelon server and
//! starts it, and runs or manages a server. Its egui interface (the `gui` feature, main.rs) and
//! launcher-tauri's share what's here; without `gui`, there's no egui in it.

// Without the egui interface, what only it calls goes unused.
#![cfg_attr(not(feature = "gui"), allow(dead_code, unused_imports))]

pub mod activity;
#[cfg(feature = "gui")]
mod app;
pub mod chat;
pub mod checks;
pub mod connect;
pub mod diagnostics;
pub mod dll_utils;
pub mod feedback;
pub mod flow;
pub mod logging;
pub mod maintenance;
pub mod network;
pub mod new_options;
#[cfg(feature = "gui")]
mod play;
pub mod prefs;
pub mod roadmap;
#[cfg(feature = "gui")]
mod scale;
pub mod server;
pub mod server_menu;
pub mod services;
pub mod settings;
#[cfg(all(test, feature = "shots"))]
mod shots;
pub mod start;
pub mod support;
pub mod task;
#[cfg(feature = "gui")]
mod theme;
pub mod update_notice;
pub mod updater;
pub mod version;
pub mod whats_new;
#[cfg(feature = "gui")]
mod window_frame;

#[cfg(feature = "gui")]
pub use gui::run;

#[cfg(feature = "gui")]
mod gui {
    use crate::app;
    use crate::logging;
    use crate::scale;
    use crate::updater;
    use crate::window_frame;

    /// The logo, as egui wants it.
    fn logo() -> eframe::egui::ColorImage {
        let size = [env!("LOGO_WIDTH").parse().unwrap_or(256), env!("LOGO_HEIGHT").parse().unwrap_or(256)];
        eframe::egui::ColorImage::from_rgba_unmultiplied(size, include_bytes!(concat!(env!("OUT_DIR"), "/logo.dat")))
    }

    /// Picks the renderer: `FE_RENDERER=glow` or `FE_RENDERER=wgpu`, or by
    /// default Direct3D 12 (through wgpu) on Windows and OpenGL elsewhere.
    ///
    /// Virtual machines without a GPU driver only offer OpenGL 1.1, which egui
    /// can't use ("egui_glow requires opengl 2.0+"). Direct3D 12 always works
    /// there, through WARP, Windows' own software renderer.
    fn renderer() -> eframe::Renderer {
        match std::env::var(RENDERER_VAR).ok().as_deref() {
            Some("glow") => return eframe::Renderer::Glow,
            #[cfg(target_os = "windows")]
            Some("wgpu") => return eframe::Renderer::Wgpu,
            _ => {}
        }
        #[cfg(target_os = "windows")]
        return eframe::Renderer::Wgpu;
        #[cfg(not(target_os = "windows"))]
        return eframe::Renderer::Glow;
    }

    const RENDERER_VAR: &str = "FE_RENDERER";

    /// The egui launcher: its window, until it closes.
    pub fn run() -> eframe::Result {
        logging::init();
        if scale::gamescope() {
            tracing::info!("Running in gamescope (gaming mode): full screen");
        }
        std::thread::spawn(updater::clean_up);
        let icon = logo();
        let renderer = renderer();
        let options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_title(concat!(env!("FE_PRODUCT"), " ", env!("FE_RELEASE")))
                // The window's first size (smaller on a small screen); the screens fit whatever size
                // it's made (scale.rs).
                .with_inner_size(scale::DESIGN)
                .with_min_inner_size(scale::MIN_WINDOW)
                .with_resizable(true)
                // The launcher draws its own title bar and edges (window_frame.rs).
                .with_decorations(!window_frame::custom())
                .with_fullscreen(scale::gamescope())
                .with_maximize_button(true)
                .with_icon(eframe::egui::IconData {
                    rgba: icon.pixels.iter().flat_map(|c| c.to_array()).collect(),
                    width: icon.size[0] as u32,
                    height: icon.size[1] as u32,
                }),
            renderer,
            ..Default::default()
        };
        let result = eframe::run_native("5th Echelon", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))));
        let Err(error) = result else { return Ok(()) };
        tracing::error!("the {renderer} renderer failed: {error}");

        // A process gets one window event loop, so try the other renderer in a
        // new one, unless a renderer was asked for (or this is that new one).
        #[cfg(target_os = "windows")]
        if std::env::var_os(RENDERER_VAR).is_none() {
            let other = match renderer {
                eframe::Renderer::Wgpu => "glow",
                _ => "wgpu",
            };
            let status = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(std::env::args_os().skip(1)).env(RENDERER_VAR, other).status());
            match status {
                Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                Err(e) => tracing::error!("couldn't start the launcher again: {e}"),
            }
        }
        logging::show_msgbox(
            &format!("The launcher couldn't open its window: {error}\n\nIf this is a virtual machine, turn on its 3D acceleration, or install its graphics driver."),
            "5th Echelon",
        );
        Err(error)
    }
}
