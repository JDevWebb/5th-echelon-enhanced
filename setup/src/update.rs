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
///
/// Strict: one line that isn't exactly that (a leading space, a tab, upper-case hex, a
/// name with a path) refuses the whole file, and so does `file` listed twice. Some
/// `sha256sum -c` skip a loose line with only a warning, so it could pass the signer's
/// check while a looser reading here took it.
pub fn checksum_for(sums: &str, file: &str) -> Option<[u8; 32]> {
    let mut found = None;
    // Not `lines()`, which takes "\r\n" for a line ending too.
    for line in sums.strip_suffix('\n').unwrap_or(sums).split('\n') {
        let (hash, name) = sums_line(line)?;
        if name == file {
            if found.is_some() {
                return None;
            }
            found = Some(hash);
        }
    }
    found
}

/// One `SHA256SUMS` line: 64 lower-case hex digits, two spaces (or a space and `*`, binary
/// mode), and a file name of letters, digits, `.`, `_` and `-`.
fn sums_line(line: &str) -> Option<([u8; 32], &str)> {
    let (hex, rest) = line.split_at_checked(64)?;
    let name = rest.strip_prefix("  ").or_else(|| rest.strip_prefix(" *"))?;
    let name_ok = !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b));
    if !name_ok || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut hash = [0; 32];
    for (i, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some((hash, name))
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

    /// Lines some `sha256sum -c` skip with a warning refuse the whole file here, and so does
    /// a file listed twice (the first line used to win).
    #[test]
    fn loose_checksum_lines_refuse_the_file() {
        let good = "ab00000000000000000000000000000000000000000000000000000000000002  launcher.exe\n";
        let evil = "cd00000000000000000000000000000000000000000000000000000000000003";
        assert!(checksum_for(good, "launcher.exe").is_some());
        for loose in [
            format!(" {evil}  launcher.exe\n{good}"),
            format!("{evil}\tlauncher.exe\n{good}"),
            format!("{}  launcher.exe\n{good}", evil.to_uppercase()),
            format!("{evil}  launcher.exe\n{good}"),
            format!("{evil}  ../launcher.exe\n{good}"),
            format!("{good}{evil}  launcher.exe \n"),
            format!("{good}\n"),
            good.replace('\n', "\r\n"),
        ] {
            assert_eq!(checksum_for(&loose, "launcher.exe"), None, "{loose:?}");
        }
    }
}
