// The release from release.toml at the workspace root, as FE_RELEASE: bots sign in to the
// API as the launcher and game client of this release.
fn main() {
    let path = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../release.toml");
    println!("cargo:rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let version = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "version")
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
        .expect("release.toml has no version");
    println!("cargo:rustc-env=FE_RELEASE={version}");
}
