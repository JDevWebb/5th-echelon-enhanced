//! The 5th Echelon launcher: sets up Splinter Cell: Blacklist for a 5th
//! Echelon server and starts it, and runs or manages a server.

#![windows_subsystem = "windows"]

mod app;
mod dll_utils;
mod flow;
mod logging;
mod network;
mod play;
mod server;
mod services;
mod settings;
mod task;
mod theme;
mod updater;

/// The logo, as egui wants it.
fn logo() -> eframe::egui::ColorImage {
    let size = [env!("LOGO_WIDTH").parse().unwrap_or(256), env!("LOGO_HEIGHT").parse().unwrap_or(256)];
    eframe::egui::ColorImage::from_rgba_unmultiplied(size, include_bytes!(concat!(env!("OUT_DIR"), "/logo.dat")))
}

fn main() -> eframe::Result {
    logging::init();
    std::thread::spawn(updater::clean_up);
    let icon = logo();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(concat!(env!("FE_PRODUCT"), " ", env!("FE_RELEASE")))
            .with_inner_size([860.0, 720.0])
            .with_min_inner_size([640.0, 520.0])
            .with_icon(eframe::egui::IconData {
                rgba: icon.pixels.iter().flat_map(|c| c.to_array()).collect(),
                width: icon.size[0] as u32,
                height: icon.size[1] as u32,
            }),
        ..Default::default()
    };
    eframe::run_native("5th Echelon", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
