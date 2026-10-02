// Reads release.toml at the workspace root for a build script (include!'d by
// hooks, launcher and dedicated_server) and exports it to the crate as
// FE_PRODUCT ("5th Echelon Enhanced") and FE_RELEASE (e.g. "0.3.0").

/// The product name and release number from release.toml.
#[allow(dead_code)]
fn release() -> (String, String) {
    let path = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../release.toml");
    println!("cargo:rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let field = |key: &str| {
        text.lines()
            .filter_map(|l| l.split_once('='))
            .find(|(k, _)| k.trim() == key)
            .map(|(_, v)| v.trim().trim_matches('"').to_string())
            .unwrap_or_else(|| panic!("release.toml has no {key}"))
    };
    let (name, version) = (field("name"), field("version"));
    println!("cargo:rustc-env=FE_PRODUCT={name}");
    println!("cargo:rustc-env=FE_RELEASE={version}");
    (name, version)
}

/// `version` ("0.3.156", any "-dev" suffix ignored) packed as a Windows
/// FILEVERSION/PRODUCTVERSION: major.minor.patch.0.
#[allow(dead_code)]
fn version_number(version: &str) -> u64 {
    let numbers: Vec<u64> = version.split('-').next().unwrap_or_default().split('.').map(|n| n.parse().unwrap_or_else(|_| panic!("release.toml version {version:?} isn't major.minor.patch"))).collect();
    let [major, minor, patch] = numbers[..] else {
        panic!("release.toml version {version:?} isn't major.minor.patch");
    };
    (major << 48) | (minor << 32) | (patch << 16)
}
