//! The client DLL this launcher carries. (`dll_utils/parse.rs` and
//! `version.rs` read its version at build time; see build.rs.)

/// The client DLL built into this launcher (brotli-compressed at build
/// time), or None in a build without `embed-dll`.
pub fn bundled() -> Option<&'static [u8]> {
    #[cfg(feature = "embed-dll")]
    {
        static DLL: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        static COMPRESSED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/uplay_r1_loader.dll.brotli"));
        Some(DLL.get_or_init(|| {
            let mut out = Vec::new();
            brotli::BrotliDecompress(&mut std::io::Cursor::new(COMPRESSED), &mut out).expect("the bundled DLL decompresses");
            out
        }))
    }
    #[cfg(not(feature = "embed-dll"))]
    None
}

/// The bundled DLL's version, as shown to players.
pub const BUNDLED_VERSION: Option<&str> = option_env!("HOOKS_VERSION");
