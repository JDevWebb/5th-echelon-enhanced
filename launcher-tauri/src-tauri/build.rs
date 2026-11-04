/// Microsoft's WebView2 Evergreen Bootstrapper the Windows launcher carries (vendor/README.md).
const BOOTSTRAPPER: &str = "vendor/MicrosoftEdgeWebview2Setup.exe";
const BOOTSTRAPPER_SHA256: &str = "cf2f2445fef0c5c14e372045f84998771eeb8bb01d59be4afd00692fa160082e";

fn main() {
    // The release, as the egui launcher has it (release.toml at the repository's root).
    let release = std::fs::read_to_string("../../release.toml").unwrap_or_default();
    let version = release
        .lines()
        .find_map(|l| l.trim().strip_prefix("version = ").map(|v| v.trim_matches('"').to_string()))
        .unwrap_or_else(|| "0.0.0".into());
    println!("cargo:rustc-env=FE_RELEASE={version}");
    println!("cargo:rerun-if-changed=../../release.toml");
    // The Windows launcher carries only the bootstrapper it was checked with: a different file
    // stops the build.
    println!("cargo:rerun-if-changed={BOOTSTRAPPER}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        use sha2::Digest as _;
        let file = std::fs::read(BOOTSTRAPPER).unwrap_or_else(|e| panic!("{BOOTSTRAPPER}: {e}"));
        let sum: String = sha2::Sha256::digest(&file).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(sum, BOOTSTRAPPER_SHA256, "{BOOTSTRAPPER} isn't the file checked in vendor/README.md");
    }
    tauri_build::build()
}
