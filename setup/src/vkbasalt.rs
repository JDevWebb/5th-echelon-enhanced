//! "Brighten the picture" on Linux, through vkBasalt.
//!
//! Under Proton/DXVK the game's own brightness (`m_fVideoGamma`, the menu's slider, the
//! profile) changes nothing on screen: the game sets the display's gamma ramp, which
//! XWayland doesn't have. So on Linux the launcher brightens the picture with vkBasalt, a
//! Vulkan layer that sits behind DXVK and maps every frame's colours through a lookup table:
//! before each start it writes the table (a `.cube` file, `out = in^(1/strength)` per
//! channel: mid-tones lighter, black and white kept) and a vkBasalt config beside it, and
//! starts the game with [`ENABLE`] and [`CONFIG_FILE`] set. The game is 32-bit, so it needs
//! vkBasalt's 32-bit layer ([`find_layers`]).

use std::ffi::OsString;
use std::fmt::Write as _;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use crate::gamma::MAX_BRIGHTNESS;
use crate::gamma::MIN_BRIGHTNESS;

/// Turns vkBasalt's implicit layer on for the game.
pub const ENABLE: &str = "ENABLE_VKBASALT";
/// The vkBasalt config the game's layer reads.
pub const CONFIG_FILE: &str = "VKBASALT_CONFIG_FILE";
/// vkBasalt's layer, as its manifests name it.
pub const LAYER: &str = "VK_LAYER_VKBASALT_post_processing";
/// The key that turns the effect off and on in the game.
pub const TOGGLE_KEY: &str = "Home";
/// Points per side of the colour cube: a curve this smooth needs no more between them.
pub const LUT_SIZE: usize = 32;

/// The strength of the curve for the launcher's brightness (60 to 100): 60 is 1.0 (neutral)
/// and 100 is 2.0, in a straight line. Measured in the game, 1.3, 1.6 and 2.0 made the
/// mid-tones 1.6, 2.3 and 3.0 times as bright, so the slider runs from untouched to about
/// three times as bright, and the default (90, 1.75) sits near 2.6 times.
pub fn strength(brightness: u8) -> f32 {
    let b = brightness.clamp(MIN_BRIGHTNESS, MAX_BRIGHTNESS);
    1.0 + f32::from(b - MIN_BRIGHTNESS) / f32::from(MAX_BRIGHTNESS - MIN_BRIGHTNESS)
}

/// One channel through the curve: `x^(1/strength)`, for `x` in 0..=1.
pub fn curve(x: f32, strength: f32) -> f32 {
    x.clamp(0.0, 1.0).powf(1.0 / strength.max(0.01))
}

/// A `.cube` colour table (Adobe's format, which vkBasalt reads) for `strength`, `size` points
/// per side, red changing fastest. Every value starts with a digit, as vkBasalt's reader wants.
pub fn lut_cube(strength: f32, size: usize) -> String {
    let size = size.clamp(2, 256);
    let steps: Vec<String> = (0..size).map(|i| format!("{:.6}", curve(i as f32 / (size - 1) as f32, strength))).collect();
    let mut out = String::with_capacity(size * size * size * 27 + 128);
    let _ = writeln!(out, "# 5th Echelon launcher, Brighten the picture: out = in^(1/{strength:.3})");
    let _ = writeln!(out, "LUT_3D_SIZE {size}");
    for b in &steps {
        for g in &steps {
            for r in &steps {
                let _ = writeln!(out, "{r} {g} {b}");
            }
        }
    }
    out
}

/// The vkBasalt config: the colour table at `lut`, on from the start, [`TOGGLE_KEY`] to switch
/// it. The path is quoted (vkBasalt drops unquoted spaces); a path with a quote or a line
/// break in it can't be written.
pub fn config_text(lut: &Path) -> io::Result<String> {
    let path = lut
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the folder's path isn't valid UTF-8"))?;
    if path.contains(['"', '\n', '\r']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("vkBasalt can't read a path with a quote in it: {path}"),
        ));
    }
    Ok(format!(
        "# Written by the 5th Echelon launcher (Brighten the picture) before each game start.\n\
         effects = lut\n\
         lutFile = \"{path}\"\n\
         enableOnLaunch = True\n\
         toggleKey = {TOGGLE_KEY}\n"
    ))
}

/// Writes the colour table and the config for `brightness` into `dir` (made if missing) and
/// gives the config's path.
pub fn write(dir: &Path, brightness: u8) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let lut = dir.join("brighten.cube");
    let config = dir.join("vkBasalt.conf");
    let text = config_text(&lut)?;
    crate::write_atomic(&lut, lut_cube(strength(brightness), LUT_SIZE).as_bytes())?;
    crate::write_atomic(&config, text.as_bytes())?;
    Ok(config)
}

/// Where [`write`] puts its files: `vkBasalt` in the launcher's own folder.
pub fn files_dir() -> Option<PathBuf> {
    crate::app_data_dir().map(|d| d.join("vkBasalt"))
}

/// What the game's process needs in its environment to use the config at `config`.
pub fn env(config: &Path) -> Vec<(&'static str, OsString)> {
    vec![(ENABLE, "1".into()), (CONFIG_FILE, config.as_os_str().to_owned())]
}

/// vkBasalt's layer manifests found, by the width of the library each one loads.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Layers {
    pub bits32: Option<PathBuf>,
    pub bits64: Option<PathBuf>,
}

/// The folders the Vulkan loader reads implicit layers from, as this environment sets them
/// (`VK_ADD_IMPLICIT_LAYER_PATH`, the XDG config and data folders, `/etc`), most specific first.
pub fn layer_dirs() -> Vec<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let home = var("HOME").map(PathBuf::from);
    let split = |v: OsString| std::env::split_paths(&v).collect::<Vec<_>>();
    let mut roots: Vec<PathBuf> = Vec::new();
    roots.extend(var("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".config"))));
    roots.extend(var("XDG_CONFIG_DIRS").map(split).unwrap_or_else(|| vec!["/etc/xdg".into()]));
    roots.push("/etc".into());
    roots.extend(var("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".local/share"))));
    roots.extend(var("XDG_DATA_DIRS").map(split).unwrap_or_else(|| vec!["/usr/local/share".into(), "/usr/share".into()]));
    let mut dirs: Vec<PathBuf> = var("VK_ADD_IMPLICIT_LAYER_PATH").map(split).unwrap_or_default();
    dirs.extend(roots.into_iter().map(|r| r.join("vulkan/implicit_layer.d")));
    let mut unique: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !unique.contains(&d) {
            unique.push(d);
        }
    }
    unique
}

/// The folders a bare library name in a manifest is loaded from: `LD_LIBRARY_PATH`, then
/// the usual system ones, 32- and 64-bit (which is which is read from each library).
pub fn lib_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("LD_LIBRARY_PATH").map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    for d in [
        "/usr/lib32",
        "/usr/lib/i386-linux-gnu",
        "/usr/local/lib32",
        "/lib32",
        "/usr/lib",
        "/usr/lib64",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/local/lib",
        "/usr/local/lib64",
        "/lib",
        "/lib64",
    ] {
        dirs.push(d.into());
    }
    dirs
}

/// vkBasalt's layer manifests in `layer_dirs`, sorted by the width of the library each loads
/// (a bare library name is looked for in `lib_dirs`). A library that can't be found is taken
/// by the manifest's name: `x86`, `i386`, `i686` or `32` in it means 32-bit (Arch's
/// `vkBasalt.x86.json`).
pub fn find_layers(layer_dirs: &[PathBuf], lib_dirs: &[PathBuf]) -> Layers {
    let mut found = Layers::default();
    for dir in layer_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut manifests: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
        manifests.sort();
        for manifest in manifests {
            let Some(library) = vkbasalt_library(&manifest) else { continue };
            let widths = library_widths(&manifest, &library, lib_dirs);
            let widths = if widths.is_empty() { vec![width_from_name(&manifest)] } else { widths };
            for w in widths {
                let slot = if w == Width::Bits32 { &mut found.bits32 } else { &mut found.bits64 };
                slot.get_or_insert_with(|| manifest.clone());
            }
        }
    }
    found
}

/// [`find_layers`] in this system's folders.
pub fn installed() -> Layers {
    find_layers(&layer_dirs(), &lib_dirs())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    Bits32,
    Bits64,
}

/// The library a manifest loads, when it's vkBasalt's layer (`layer`, or one of `layers`).
fn vkbasalt_library(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let layers: Vec<&serde_json::Value> = match (json.get("layer"), json.get("layers")) {
        (Some(l), _) => vec![l],
        (None, Some(serde_json::Value::Array(ls))) => ls.iter().collect(),
        _ => return None,
    };
    layers
        .into_iter()
        .find(|l| l.get("name").and_then(|n| n.as_str()) == Some(LAYER))
        .and_then(|l| l.get("library_path")?.as_str().map(str::to_string))
}

/// The widths of the libraries `library` resolves to: a path (absolute, or relative to the
/// manifest) is that file; a bare name is every match in `lib_dirs`.
fn library_widths(manifest: &Path, library: &str, lib_dirs: &[PathBuf]) -> Vec<Width> {
    let candidates: Vec<PathBuf> = if library.contains('/') {
        let p = Path::new(library);
        vec![if p.is_absolute() {
            p.to_path_buf()
        } else {
            manifest.parent().unwrap_or(Path::new("/")).join(p)
        }]
    } else {
        lib_dirs.iter().map(|d| d.join(library)).collect()
    };
    let mut widths: Vec<Width> = candidates.iter().filter_map(|p| elf_width(p)).collect();
    widths.dedup();
    widths
}

/// An ELF file's class, from its header.
fn elf_width(path: &Path) -> Option<Width> {
    use std::io::Read as _;
    let mut head = [0u8; 5];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    match head {
        [0x7f, b'E', b'L', b'F', 1] => Some(Width::Bits32),
        [0x7f, b'E', b'L', b'F', 2] => Some(Width::Bits64),
        _ => None,
    }
}

fn width_from_name(manifest: &Path) -> Width {
    let name = manifest.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ["x86.", "i386", "i686", "32"].iter().any(|m| name.contains(m)) && !name.contains("x86_64") {
        Width::Bits32
    } else {
        Width::Bits64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slider_maps_from_neutral_to_twice() {
        assert_eq!(strength(60), 1.0);
        assert_eq!(strength(100), 2.0);
        assert!((strength(90) - 1.75).abs() < 1e-6);
        assert!((strength(75) - 1.375).abs() < 1e-6);
        // Outside the slider's range: clamped.
        assert_eq!(strength(0), 1.0);
        assert_eq!(strength(255), 2.0);
    }

    #[test]
    fn the_curve_keeps_black_and_white_and_lifts_the_middle() {
        for s in [1.0, 1.3, 1.75, 2.0] {
            assert_eq!(curve(0.0, s), 0.0);
            assert_eq!(curve(1.0, s), 1.0);
        }
        assert!((curve(0.5, 2.0) - 0.5f32.sqrt()).abs() < 1e-6);
        assert!((curve(0.5, 1.0) - 0.5).abs() < 1e-6, "1.0 is neutral");
    }

    #[test]
    fn the_cube_has_its_size_and_corners() {
        let text = lut_cube(2.0, 4);
        let mut lines = text.lines().filter(|l| !l.starts_with('#'));
        assert_eq!(lines.next(), Some("LUT_3D_SIZE 4"));
        let rows: Vec<[f32; 3]> = lines
            .map(|l| {
                let v: Vec<f32> = l.split(' ').map(|x| x.parse().unwrap()).collect();
                [v[0], v[1], v[2]]
            })
            .collect();
        assert_eq!(rows.len(), 4 * 4 * 4);
        assert_eq!(rows[0], [0.0, 0.0, 0.0], "black stays");
        assert_eq!(rows[63], [1.0, 1.0, 1.0], "white stays");
        // Red changes fastest, then green, then blue.
        assert_eq!(rows[3], [1.0, 0.0, 0.0]);
        assert_eq!(rows[12], [0.0, 1.0, 0.0]);
        assert_eq!(rows[48], [0.0, 0.0, 1.0]);
        // A third of the way: (1/3)^(1/2).
        assert!((rows[1][0] - (1.0f32 / 3.0).sqrt()).abs() < 1e-5);
        // vkBasalt only reads a data line that starts with a digit.
        assert!(text.lines().skip(2).all(|l| l.starts_with(|c: char| c.is_ascii_digit())));
    }

    #[test]
    fn the_full_cube_is_the_usual_size() {
        let text = lut_cube(1.75, LUT_SIZE);
        assert_eq!(text.lines().filter(|l| l.starts_with(|c: char| c.is_ascii_digit())).count(), LUT_SIZE.pow(3));
        assert!(text.contains(&format!("LUT_3D_SIZE {LUT_SIZE}\n")));
    }

    #[test]
    fn the_config_names_the_table_quoted() {
        let text = config_text(Path::new("/home/a b/.config/5th-Echelon/vkBasalt/brighten.cube")).unwrap();
        assert!(text.contains("effects = lut\n"));
        assert!(text.contains("lutFile = \"/home/a b/.config/5th-Echelon/vkBasalt/brighten.cube\"\n"), "{text}");
        assert!(text.contains("enableOnLaunch = True\n"));
        assert!(text.contains("toggleKey = Home\n"));
        assert!(!text.contains("\nlut ="), "the key is lutFile");
        assert!(config_text(Path::new("/tmp/a\"b/brighten.cube")).is_err());
    }

    #[test]
    fn write_puts_both_files_in_the_folder() {
        let dir = crate::testutil::temp_dir("vkbasalt-write").join("vkBasalt");
        let config = write(&dir, 90).unwrap();
        assert_eq!(config, dir.join("vkBasalt.conf"));
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains(&format!("lutFile = \"{}\"", dir.join("brighten.cube").display())));
        let cube = std::fs::read_to_string(dir.join("brighten.cube")).unwrap();
        assert!(cube.starts_with("# 5th Echelon launcher, Brighten the picture: out = in^(1/1.750)\n"));
        let env = env(&config);
        assert_eq!(env[0], (ENABLE, OsString::from("1")));
        assert_eq!(env[1], (CONFIG_FILE, config.into_os_string()));
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    const MANIFEST: &str = r#"{
      "file_format_version" : "1.0.0",
      "layer" : {
        "name": "VK_LAYER_VKBASALT_post_processing",
        "type": "GLOBAL",
        "library_path": "LIB",
        "enable_environment": { "ENABLE_VKBASALT": "1" }
      }
    }"#;

    fn elf(path: &Path, class: u8) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, [0x7f, b'E', b'L', b'F', class, 1, 1, 0]).unwrap();
    }

    #[test]
    fn finds_the_32_bit_layer_by_its_library() {
        let root = crate::testutil::temp_dir("vkbasalt-layers");
        let (layers, lib, lib32) = (root.join("share/vulkan/implicit_layer.d"), root.join("lib"), root.join("lib32"));
        std::fs::create_dir_all(&layers).unwrap();
        let dirs = vec![root.join("missing"), layers.clone()];
        let libs = vec![lib32.clone(), lib.clone()];
        assert_eq!(find_layers(&dirs, &libs), Layers::default(), "nothing installed");

        // Arch: two manifests with the same bare name, one library in each folder.
        std::fs::write(layers.join("vkBasalt.json"), MANIFEST.replace("LIB", "libvkbasalt.so")).unwrap();
        elf(&lib.join("libvkbasalt.so"), 2);
        let only64 = find_layers(&dirs, &libs);
        assert!(only64.bits32.is_none(), "{only64:?}");
        assert_eq!(only64.bits64, Some(layers.join("vkBasalt.json")));
        elf(&lib32.join("libvkbasalt.so"), 1);
        std::fs::write(layers.join("vkBasalt.x86.json"), MANIFEST.replace("LIB", "libvkbasalt.so")).unwrap();
        let both = find_layers(&dirs, &libs);
        assert!(both.bits32.is_some() && both.bits64.is_some(), "{both:?}");

        // Another layer (a local build with its own switch) doesn't count.
        let other = crate::testutil::temp_dir("vkbasalt-other");
        std::fs::write(
            other.join("vkbasalt_live_i386.json"),
            MANIFEST
                .replace("VK_LAYER_VKBASALT_post_processing", "VK_LAYER_VKBASALT_live_32")
                .replace("LIB", "/x/libvkbasalt-live.so"),
        )
        .unwrap();
        assert_eq!(find_layers(&[other.clone()], &libs), Layers::default());

        // An absolute path to a 32-bit library.
        std::fs::write(other.join("vkBasalt.json"), MANIFEST.replace("LIB", &lib32.join("libvkbasalt.so").to_string_lossy())).unwrap();
        let abs = find_layers(&[other.clone()], &[]);
        assert_eq!(abs.bits32, Some(other.join("vkBasalt.json")));
        assert!(abs.bits64.is_none());

        // A library nowhere to be found: the manifest's name says which.
        let named = crate::testutil::temp_dir("vkbasalt-named");
        std::fs::write(named.join("vkBasalt.x86.json"), MANIFEST.replace("LIB", "libvkbasalt.so")).unwrap();
        std::fs::write(named.join("vkBasalt.x86_64.json"), MANIFEST.replace("LIB", "libvkbasalt.so")).unwrap();
        let by_name = find_layers(&[named.clone()], &[root.join("nowhere")]);
        assert_eq!(by_name.bits32, Some(named.join("vkBasalt.x86.json")));
        assert_eq!(by_name.bits64, Some(named.join("vkBasalt.x86_64.json")));

        for d in [root, other, named] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn layer_folders_follow_xdg_with_defaults() {
        // Only checks the shape: the system's defaults are in, each folder once.
        let dirs = layer_dirs();
        let mut unique = dirs.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), dirs.len());
        assert!(dirs
            .iter()
            .all(|d| d.ends_with("vulkan/implicit_layer.d") || std::env::var_os("VK_ADD_IMPLICIT_LAYER_PATH").is_some()));
        assert!(dirs.contains(&PathBuf::from("/etc/vulkan/implicit_layer.d")));
    }
}
