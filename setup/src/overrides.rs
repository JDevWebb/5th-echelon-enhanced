//! `uplay.override.toml`: settings another tool manages. The launcher only
//! reads it, to know when an install is managed and to show what's pinned.

use std::path::Path;

pub use hooks_config::Managed;
pub use hooks_config::Overrides;
pub use hooks_config::OVERRIDES_FILE;

/// The overrides in `game_dir`, if there are any. An unreadable file is an
/// error, not "no overrides": the game would refuse to start with it too.
pub fn read(game_dir: &Path) -> anyhow::Result<Option<Overrides>> {
    let path = game_dir.join(OVERRIDES_FILE);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(Some(toml::from_str(&s).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The tool that manages `game_dir`, if one does.
pub fn managed_by(game_dir: &Path) -> Option<Managed> {
    read(game_dir).ok().flatten().and_then(|o| o.managed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn managed_installs_are_recognised() {
        let dir = temp_dir("overrides");
        assert!(read(&dir).unwrap().is_none());
        assert!(managed_by(&dir).is_none());
        std::fs::write(dir.join(OVERRIDES_FILE), "ConfigServer = \"10.8.0.10\"\n[Managed]\nBy = \"Community Hub\"\n").unwrap();
        assert_eq!(managed_by(&dir).unwrap().by, "Community Hub");
        std::fs::write(dir.join(OVERRIDES_FILE), "not = = toml").unwrap();
        assert!(read(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
