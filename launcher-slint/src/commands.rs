//! What the carried-over Tauri modules know as `crate::commands`: the launcher's state (here
//! core.rs's), and how they say the activity bar changed.

use std::sync::Arc;

pub use crate::core::Inner;
use crate::tauri;
use crate::tauri::Manager;

/// The launcher's state, as the modules find it among the app's states.
pub struct Launcher(pub Arc<crate::core::Core>);

impl Launcher {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock()
    }
}

/// Tells the window the activity bar changed.
pub fn changed(app: &tauri::AppHandle) {
    app.state::<Launcher>().0.changed(crate::core::Changed::Activity);
}

/// How an activity reports back while it runs, for `Activities::start_with`.
pub fn notifier(app: &tauri::AppHandle) -> Option<Arc<dyn Fn() + Send + Sync>> {
    app.state::<Launcher>().0.notifier()
}
