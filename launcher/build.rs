#[cfg(feature = "embed-dll")]
use std::env;
#[cfg(feature = "embed-dll")]
use std::fs;
#[cfg(feature = "embed-dll")]
use std::path::Path;
#[cfg(feature = "embed-dll")]
use std::path::PathBuf;
#[cfg(feature = "embed-dll")]
use std::process::Command;

#[cfg(feature = "embed-dll")]
fn root_manifest_dir() -> PathBuf {
    let output = &String::from_utf8(
        Command::new("cargo")
            .arg("locate-project")
            .arg("--workspace")
            .arg("--message-format")
            .arg("plain")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let path = Path::new(output.trim());
    assert_eq!(path.file_name().unwrap(), "Cargo.toml");
    path.parent().unwrap().to_owned()
}

#[cfg(feature = "embed-dll")]
fn embed_dll() {
    // A prebuilt hooks DLL, e.g. from a cross-compile (cargo-xwin), where the
    // nested plain `cargo build` below can't link for MSVC.
    println!("cargo:rerun-if-env-changed=HOOKS_DLL");
    let dll_path = match env::var_os("HOOKS_DLL") {
        Some(path) => PathBuf::from(path),
        None => build_hooks_dll(),
    };
    println!("cargo:rerun-if-changed={}", dll_path.display());
    embed(&dll_path);
}

#[cfg(feature = "embed-dll")]
fn build_hooks_dll() -> PathBuf {
    use jzon::JsonValue;

    let is_release = env::var("PROFILE").unwrap() == "release";
    let dir = root_manifest_dir();
    let target_dir = dir.join("target_i686");
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("-p")
        .arg("hooks")
        .arg("--target")
        .arg("i686-pc-windows-msvc")
        .arg("--target-dir")
        .arg(target_dir.to_str().unwrap())
        .arg("--message-format")
        .arg("json");

    if is_release {
        cmd.arg("--release")
            // With the static C runtime, as .cargo/config.toml gives every other Windows build
            // (this RUSTFLAGS replaces it).
            .env(
                "RUSTFLAGS",
                "-Zremap-cwd-prefix=. --remap-path-prefix=$(PWD)=/ -Clink-arg=/PDBALTPATH:%_PDB% -Ctarget-feature=+crt-static",
            );
    }

    let results = String::from_utf8(cmd.output().unwrap().stdout).unwrap();

    let artifacts = results
        .lines()
        .map(jzon::parse)
        .map(Result::unwrap)
        .filter_map(|event| {
            let JsonValue::Object(obj) = event else {
                return None;
            };
            let JsonValue::Short(reason) = obj.get("reason")? else {
                return None;
            };
            if reason != "compiler-artifact" {
                return None;
            }
            let JsonValue::Object(target) = obj.get("target")? else {
                return None;
            };
            if !matches!(target.get("name")?, JsonValue::Short(s) if s == "hooks") {
                return None;
            }
            let JsonValue::Array(filenames) = obj.get("filenames")? else {
                return None;
            };
            let filenames = filenames
                .iter()
                .filter_map(|entry| {
                    let JsonValue::String(name) = entry else {
                        return None;
                    };
                    Some(PathBuf::from(name))
                })
                .collect::<Vec<PathBuf>>();
            Some(filenames)
        })
        .flatten()
        .filter(|artifact| artifact.extension().map(|ext| ext == "dll").unwrap_or_default())
        .collect::<Vec<PathBuf>>();

    assert_eq!(artifacts.len(), 1);

    artifacts.into_iter().next().unwrap()
}

#[cfg(feature = "embed-dll")]
fn embed(dll_path: &Path) {
    use brotli::enc::BrotliEncoderParams;

    println!("cargo:warning=Dll path: {dll_path:?}");

    let data = fs::read(dll_path).unwrap();
    let dll = dll::parse(&data).unwrap();

    println!("cargo:warning=Dll version: {}", dll.version);
    // The client and the launcher are one release: never carry a DLL from another build.
    let (_, release) = release();
    let release = release.split('-').next().unwrap_or_default();
    assert_eq!(
        dll.version.to_string(),
        release,
        "{} is client {}, but this launcher is {release}: rebuild the hooks DLL",
        dll_path.display(),
        dll.version
    );
    println!("cargo:rustc-env=HOOKS_VERSION={}", dll.version);

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let payload_path = out_dir.join("uplay_r1_loader.dll.brotli");

    let mut in_file = fs::File::open(dll_path).unwrap();
    let mut out_file = fs::File::create(payload_path).unwrap();
    let params = BrotliEncoderParams { quality: 5, ..Default::default() };
    brotli::BrotliCompress(&mut in_file, &mut out_file, &params).unwrap();
}

#[cfg(feature = "embed-dll")]
mod dll {
    include!("src/dll_utils/parse.rs");
}

#[cfg(feature = "embed-dll")]
mod version {
    include!("src/version.rs");
}

include!("../build/release.rs");

fn main() {
    // FE_PRODUCT and FE_RELEASE, from release.toml.
    let _ = release();
    // The hooks DLL the launcher installs, built in (launcher-slint's embed-dll). The program's
    // icon, manifest and version info are launcher-slint's (its build.rs).
    #[cfg(feature = "embed-dll")]
    {
        let dir = root_manifest_dir();
        let hooks_dir = dir.join("hooks");
        println!("cargo:rerun-if-changed={}", hooks_dir.to_str().unwrap());
        embed_dll();
    }
}
