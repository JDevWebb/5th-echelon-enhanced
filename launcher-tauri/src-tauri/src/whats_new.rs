//! What's new: once after the launcher updates (the egui launcher's whats_new.rs, its releases
//! built into the shared library). The version in the side menu opens it again.

use std::sync::Mutex;

use launcher::prefs::Prefs;
use launcher::whats_new as w;
use serde::Serialize;
use tauri::Manager;

use crate::commands::Launcher;

/// Whether the start was dealt with, and the release to note as seen when it closes.
#[derive(Default)]
pub struct WhatsNew {
    inner: Mutex<(bool, Option<String>)>,
}

#[derive(Serialize)]
pub struct ItemView {
    title: &'static str,
    text: &'static str,
    community: bool,
}

#[derive(Serialize)]
pub struct ReleaseView {
    version: &'static str,
    thanks: Option<String>,
    new: Vec<ItemView>,
    fixed: Vec<(&'static str, &'static str)>,
}

/// The window: the releases (newest first; the others' headlines only), updated from and to,
/// and the release notes' link.
#[derive(Serialize)]
pub struct View {
    from: Option<String>,
    to: String,
    releases: Vec<ReleaseView>,
    /// For the older releases, how many headlines show.
    headlines: usize,
    notes: (&'static str, String),
}

fn view(releases: Vec<&'static w::Release>, from: Option<String>, to: String) -> Option<View> {
    let newest = *releases.first()?;
    let notes = if releases.len() > 1 {
        ("All release notes", launcher::updater::RELEASES_PAGE.to_string())
    } else {
        ("Full release notes", format!("{}/tag/v{}", launcher::updater::RELEASES_PAGE, newest.version))
    };
    Some(View {
        from,
        to,
        headlines: w::OLDER_HEADLINES,
        notes,
        releases: releases
            .into_iter()
            .map(|r| ReleaseView {
                version: r.version,
                thanks: (!r.thanks.is_empty()).then(|| w::and_list(r.thanks)),
                new: r
                    .new
                    .iter()
                    .map(|i| ItemView {
                        title: i.title,
                        text: i.text,
                        community: i.community,
                    })
                    .collect(),
                fixed: r.fixed.iter().map(|f| (f.lead, f.rest)).collect(),
            })
            .collect(),
    })
}

/// Once, at start: what's new since the release last seen, if there's something to show.
#[tauri::command(async)]
pub fn whats_new(app: tauri::AppHandle, state: tauri::State<WhatsNew>) -> Option<View> {
    let mut inner = state.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if std::mem::replace(&mut inner.0, true) {
        return None;
    }
    let launcher = app.state::<Launcher>();
    let set_up = launcher.lock().game.as_ref().is_some_and(|g| !g.cfg.profiles.is_empty());
    let seen = Prefs::whats_new_seen();
    let current = env!("FE_RELEASE");
    match w::on_start(w::RELEASES, seen.as_deref(), current, set_up) {
        w::Start::Show(versions) => {
            inner.1 = Some(current.to_string());
            view(versions.into_iter().filter_map(w::release).collect(), seen, current.to_string())
        }
        w::Start::Note => {
            Prefs::set_whats_new_seen(current);
            None
        }
        w::Start::Nothing => None,
    }
}

/// This launcher's release again (the version in the side menu), if it has an entry.
#[tauri::command(async)]
pub fn whats_new_again() -> Option<View> {
    let version = env!("FE_RELEASE").split('-').next().unwrap_or_default();
    view(w::release(version).into_iter().collect(), None, version.to_string())
}

/// It closed: noted as seen when it was shown on start.
#[tauri::command(async)]
pub fn whats_new_seen(state: tauri::State<WhatsNew>) {
    if let Some(release) = state.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner).1.take() {
        Prefs::set_whats_new_seen(&release);
    }
}

/// Back to the community network, for a change that's only there.
#[tauri::command(async)]
pub fn use_community_network(app: tauri::AppHandle, state: tauri::State<Launcher>) {
    Prefs::use_community_network();
    let mut inner = state.lock();
    inner.directory = None;
    inner.directory_note = None;
    inner.notices.info("Back on the community network: its servers are in the server menu.");
    drop(inner);
    crate::commands::changed(&app);
}
