include!("../build/release.rs");

/// Embeds the admin UI (`admin-ui/dist`, built by `npm run build` there) into
/// the binary as a table of files. Without a build, a page says how to make one.
fn admin_ui() {
    use std::fmt::Write as _;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("admin-ui").join("dist");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();
    let mut out = String::from("pub static FILES: &[(&str, &[u8], &str)] = &[\n");
    if files.is_empty() {
        println!("cargo:warning=coordinator/admin-ui/dist is missing: the admin UI isn't built in (build.sh builds it, or npm ci && npm run build there)");
        out.push_str("    (\"/index.html\", b\"<!doctype html><meta charset=utf-8><title>Admin UI</title><p>This coordinator was built without its admin UI: run npm ci and npm run build in coordinator/admin-ui, then build it again.\", \"text/html; charset=utf-8\"),\n");
    }
    for (path, file) in files {
        println!("cargo:rerun-if-changed={}", file.display());
        let kind = match file.extension().and_then(|e| e.to_str()).unwrap_or_default() {
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "svg" => "image/svg+xml",
            "woff2" => "font/woff2",
            "woff" => "font/woff",
            "ttf" => "font/ttf",
            "png" => "image/png",
            "txt" => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        };
        let _ = writeln!(out, "    ({path:?}, include_bytes!({:?}), {kind:?}),", file.display().to_string());
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("admin_ui.rs");
    std::fs::write(dest, out).expect("write admin_ui.rs");
}

fn collect(root: &std::path::Path, dir: &std::path::Path, files: &mut Vec<(String, std::path::PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files);
        } else if let Ok(rel) = path.strip_prefix(root) {
            files.push((format!("/{}", rel.to_string_lossy().replace('\\', "/")), path));
        }
    }
}

fn main() {
    release();
    admin_ui();
}
