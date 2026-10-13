pub mod friends {
    tonic::include_proto!("friends"); // The string specified here must match the proto package name
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("friends_descriptor");
}
pub mod users {
    tonic::include_proto!("users"); // The string specified here must match the proto package name
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("users_descriptor");
}
pub mod misc {
    tonic::include_proto!("misc"); // The string specified here must match the proto package name
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("misc_descriptor");
}
pub mod chat {
    tonic::include_proto!("chat");
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("chat_descriptor");
}
pub mod games {
    tonic::include_proto!("games"); // The string specified here must match the proto package name
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("games_descriptor");
}

pub mod notices;
pub mod playing;
/// The game's names for maps and modes.
pub use stat_boards::game_names;

/// The game's data versions (DX11, build V2425.0) that we know, as the server and the
/// launcher both name them. A room carries its host's in attribute 107. Games whose versions
/// differ can't play a match together; the same mods (the same version) play fine.
pub mod data_versions {
    /// The unmodified game's, the same for the Steam and Ubisoft editions in every language.
    pub const STOCK: u32 = 0xe90c_0d2d;

    /// Versions known by what makes them, in a few words for a sentence ("Theusma01's game
    /// is unmodified; yours has the balaclava mod").
    pub const KNOWN: &[(u32, &str)] = &[(STOCK, "unmodified"), (0xdd55_acd3, "balaclava mod")];

    /// What makes `version`, if it's a known one.
    pub fn name(version: u32) -> Option<&'static str> {
        KNOWN.iter().find(|(v, _)| *v == version).map(|(_, name)| *name)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn known_versions_are_named() {
            assert_eq!(super::name(super::STOCK), Some("unmodified"));
            assert_eq!(super::name(0xdd55_acd3), Some("balaclava mod"));
            assert_eq!(super::name(1), None);
        }
    }
}
