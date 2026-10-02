fn quote(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    format!("\"{}\"", s)
}

include!("../build/release.rs");

pub fn main() {
    let (name, version) = release();
    let mut res = winres::WindowsResource::new();
    // Tools that install or update the client recognise this DLL by
    // "for 5th Echelon" in its product name; keep that in any rename.
    res.set("ProductName", &format!("UPlay R1 Loader for {name}"));
    res.set("FileDescription", &format!("{name} {version}"));
    // The release, not the crate's own number: the launcher reads this to say which client
    // it carries, and it must match the launcher's.
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    res.set_version_info(winres::VersionInfo::FILEVERSION, version_number(&version));
    res.set_version_info(winres::VersionInfo::PRODUCTVERSION, version_number(&version));
    res.compile().unwrap();

    let preload = std::fs::read_to_string("preload.dat").unwrap();
    let preload = preload.lines().collect::<Vec<_>>();
    let preload = format!("pub const PRELOAD: [&str; {}] = [\n    {}\n];\n", preload.len(), quote(&preload.join("\",\n    \"")),);
    std::fs::write(format!("{}/preload.rs", std::env::var("OUT_DIR").unwrap()), preload).unwrap();
}
