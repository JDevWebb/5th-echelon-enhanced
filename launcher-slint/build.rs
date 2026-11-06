//! The screens (ui/*.slint) compiled in, the release from release.toml, and for Windows the
//! program's icon and version details (as the egui launcher's build.rs sets them).

fn main() {
    let release = std::fs::read_to_string("../release.toml").unwrap_or_default();
    let field = |key: &str| {
        release
            .lines()
            .find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().trim_start_matches('=').trim().trim_matches('"').to_string()))
    };
    let version = field("version").unwrap_or_else(|| "0.0.0".into());
    println!("cargo:rustc-env=FE_RELEASE={version}");
    println!("cargo:rerun-if-changed=../release.toml");
    slint_build::compile_with_config("ui/app.slint", slint_build::CompilerConfiguration::new().with_style("fluent".into())).expect("the launcher's screens (ui/*.slint)");

    // Decided by the target, not the build host, so a cross-build gets them too.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winres::WindowsResource::new()
            // The product name stays "5th Echelon Launcher": tools find launchers by it.
            .set("ProductName", "5th Echelon Launcher")
            .set("FileDescription", &format!("5th Echelon Enhanced {version}"))
            .set("FileVersion", &version)
            .set("ProductVersion", &version)
            .set_icon("../launcher/logo.ico")
            .compile()
            .expect("the Windows resources");
        println!("cargo:rerun-if-changed=../launcher/logo.ico");
    }
}
