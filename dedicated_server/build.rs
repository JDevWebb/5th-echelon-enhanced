include!("../build/release.rs");

pub fn main() {
    release();
    // The source revision, reported by the community API (/api/info).
    let revision = std::process::Command::new("git")
        .args(["describe", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=FE_REVISION={revision}");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs");

    #[cfg(target_os = "windows")]
    winres::WindowsResource::new().compile().unwrap();
}
