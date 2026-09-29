//! The parts of updating that aren't downloading: which release is newer,
//! and the checksums a release publishes.

/// A release number: `0.3.1`, or `0.3.1-dev` (a pre-release sorts before
/// the release it leads to). A leading `v` is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub numbers: (u32, u32, u32),
    pub pre: Option<String>,
}

impl Release {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let (core, pre) = match s.split_once('-') {
            Some((core, pre)) => (core, Some(pre.to_string())),
            None => (s, None),
        };
        let mut parts = core.split('.').map(|p| p.parse::<u32>());
        let numbers = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next().unwrap_or(Ok(0)).ok()?);
        Some(Self { numbers, pre })
    }

    /// Whether `self` is newer than `other`.
    pub fn newer_than(&self, other: &Release) -> bool {
        match self.numbers.cmp(&other.numbers) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            // Same numbers: a release beats its pre-release.
            std::cmp::Ordering::Equal => self.pre.is_none() && other.pre.is_some(),
        }
    }
}

/// The SHA-256 of `file` in a `SHA256SUMS` file (`<hex>  <name>` per line,
/// as `sha256sum` writes it).
pub fn checksum_for(sums: &str, file: &str) -> Option<[u8; 32]> {
    sums.lines().find_map(|line| {
        let (hex, name) = line.trim().split_once(char::is_whitespace)?;
        if name.trim().trim_start_matches('*') != file {
            return None;
        }
        let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect::<Option<_>>()?;
        bytes.try_into().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_order() {
        let r = |s| Release::parse(s).unwrap();
        assert!(r("v0.3.1").newer_than(&r("0.3.0")));
        assert!(r("0.3.0").newer_than(&r("0.3.0-dev")));
        assert!(!r("0.3.0-dev").newer_than(&r("0.3.0")));
        assert!(!r("0.3.0").newer_than(&r("0.3.0")));
        assert!(r("1.0").newer_than(&r("0.9.9")));
        assert!(!r("0.2.5").newer_than(&r("0.3.0-dev")));
        assert_eq!(Release::parse("latest"), None);
    }

    #[test]
    fn checksums() {
        let sums = "0000000000000000000000000000000000000000000000000000000000000001  uplay_r1_loader.dll\n\
                    ab00000000000000000000000000000000000000000000000000000000000002 *launcher.exe\n";
        assert_eq!(checksum_for(sums, "launcher.exe").unwrap()[0], 0xab);
        assert_eq!(checksum_for(sums, "uplay_r1_loader.dll").unwrap()[31], 1);
        assert_eq!(checksum_for(sums, "dedicated_server.exe"), None);
        assert_eq!(checksum_for("zz  launcher.exe", "launcher.exe"), None);
    }
}
