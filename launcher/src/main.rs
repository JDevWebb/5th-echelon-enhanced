//! The 5th Echelon launcher: sets up Splinter Cell: Blacklist for a 5th
//! Echelon server and starts it, and runs or manages a server.

#![windows_subsystem = "windows"]

mod activity;
mod app;
mod diagnostics;
mod dll_utils;
mod feedback;
mod flow;
mod logging;
mod maintenance;
mod network;
mod play;
mod roadmap;
mod scale;
mod server;
mod server_menu;
mod services;
mod settings;
mod support;
mod task;
mod theme;
mod update_notice;
mod updater;
mod whats_new;

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

fn main() -> eframe::Result {
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
            // The screens are laid out for this size, and scale with the window (scale.rs).
            .with_inner_size(scale::DESIGN)
            .with_min_inner_size(scale::DESIGN * 0.4)
            .with_resizable(true)
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
