//! Microsoft's WebView2 runtime, which the launcher's window needs on Windows: part of Windows 11
//! and of Edge, but some Windows 10 PCs lack it (LTSC, stripped-down installs, Edge removed).
//! The launcher carries Microsoft's small Evergreen Bootstrapper (`vendor/`, its checksum pinned
//! in `build.rs`) and runs it when the runtime is missing: it installs the runtime for this
//! Windows user (no administrator needed), and Microsoft keeps it up to date from then on.

/// Microsoft's Evergreen Bootstrapper, as checked in `build.rs`.
const BOOTSTRAPPER: &[u8] = include_bytes!("../vendor/MicrosoftEdgeWebview2Setup.exe");

/// Installs the runtime with Microsoft's installer (its own window shows the progress). True
/// once the runtime is there.
pub fn install() -> bool {
    let dir = std::env::temp_dir().join(format!("5th-echelon-webview2-{}", std::process::id()));
    let setup = dir.join("MicrosoftEdgeWebview2Setup.exe");
    let ran = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&setup, BOOTSTRAPPER))
        .and_then(|()| std::process::Command::new(&setup).arg("/install").status());
    let _ = std::fs::remove_dir_all(&dir);
    match ran {
        Ok(status) if status.success() => tracing::info!("Microsoft's WebView2 installer finished"),
        Ok(status) => tracing::warn!("Microsoft's WebView2 installer ended with {status}"),
        Err(e) => tracing::warn!("Couldn't run Microsoft's WebView2 installer: {e}"),
    }
    match tauri::webview_version() {
        Ok(version) => {
            tracing::info!("WebView2 {version} installed");
            true
        }
        Err(e) => {
            tracing::warn!("Still no WebView2 runtime: {e}");
            false
        }
    }
}
