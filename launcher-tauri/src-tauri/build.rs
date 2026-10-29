fn main() {
    // The release, as the egui launcher has it (release.toml at the repository's root).
    let release = std::fs::read_to_string("../../release.toml").unwrap_or_default();
    let version = release
        .lines()
        .find_map(|l| l.trim().strip_prefix("version = ").map(|v| v.trim_matches('"').to_string()))
        .unwrap_or_else(|| "0.0.0".into());
    println!("cargo:rustc-env=FE_RELEASE={version}");
    println!("cargo:rerun-if-changed=../../release.toml");
    tauri_build::build()
}
