//! Links: refused in chat and names, but a Discord invite in exactly its own form.
//!
//! A link is a scheme (`http://`, `ftp://`), `www.`, an IP address, or a word, a dot and a known
//! top-level domain. Dots written out ("dot", "(.)", " . ") count, look-alike letters are read as
//! the letters they look like, and punycode (`xn--`) is a link. Decimals, times and version
//! numbers aren't links, nor short words run together after a full stop ("in.it"), which need a
//! scheme, `www.` or a path to count.

use unicode_normalization::UnicodeNormalization as _;

/// Top-level domains that make a word and a dot a link. Not every one there is: the common
/// ones, those spam uses, and the countries players are in.
const TLDS: &[&str] = &[
    "com", "net", "org", "info", "biz", "io", "gg", "co", "tv", "xyz", "ru", "de", "uk", "fr", "nl", "eu", "ca", "au", "nz", "br", "pl", "es", "jp", "kr", "cn", "su", "ly", "ws",
    "cc", "app", "dev", "link", "site", "online", "store", "shop", "club", "top", "live", "fun", "pro", "gift", "gifts", "click", "page", "win", "vip", "lol", "store", "tk", "ml",
    "ga", "cf", "gq", "ua", "tr", "ro", "pt", "se", "no", "dk", "fi", "cz", "hu", "gr", "ar", "mx", "cl", "za", "sg", "hk", "tw", "id", "ph", "vn", "th", "my", "ch", "at", "be",
    "it", "in", "us", "me", "to", "so",
];

/// TLDs that are also everyday words: a link only with a scheme, `www.` or a path after them.
const WORDLIKE: &[&str] = &[
    "no", "at", "be", "it", "in", "us", "me", "to", "so", "my", "id", "co", "pro", "fun", "win", "live", "shop", "top", "page", "lol", "link", "app", "site", "club",
];

/// A link found in some text, as found (for the logs and the admins).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link(pub String);

/// The Discord invites in `text`, each exactly in its own form, by byte span: `discord.gg/CODE`,
/// `discord.com/invite/CODE` or `discordapp.com/invite/CODE`, with or without `https://` (or
/// `http://`), the code 2 to 32 of `A-Za-z0-9-`, and nothing after it but space or the end.
pub fn discord_invites(text: &str) -> Vec<(usize, usize)> {
    const FORMS: [&str; 3] = ["discord.gg/", "discord.com/invite/", "discordapp.com/invite/"];
    let mut found = Vec::new();
    for (start, _) in text.char_indices() {
        // Only at a word's start.
        if start > 0 && !text[..start].ends_with(char::is_whitespace) {
            continue;
        }
        let rest = &text[start..];
        let lower = rest.to_ascii_lowercase();
        let scheme = ["https://", "http://", ""].into_iter().find(|s| lower.starts_with(s)).unwrap_or_default();
        let after_scheme = &lower[scheme.len()..];
        let Some(form) = FORMS.iter().find(|f| after_scheme.starts_with(*f)) else { continue };
        let code_start = start + scheme.len() + form.len();
        let code: String = text[code_start..].chars().take_while(|c| !c.is_whitespace()).collect();
        let valid = (2..=32).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if valid {
            found.push((start, code_start + code.len()));
        }
    }
    found
}

/// The first link in `text`, if any.
pub fn find_link(text: &str) -> Option<Link> {
    let norm = normalise(text);
    if norm.contains("xn--") {
        return Some(Link("punycode".into()));
    }
    for marker in ["http://", "https://", "ftp://", "www."] {
        if norm.contains(marker) {
            return Some(Link(marker.into()));
        }
    }
    if let Some(ip) = ip_address(&norm) {
        return Some(Link(ip));
    }
    domain(&norm).map(Link)
}

/// `text` as compared: look-alike letters read as Latin ones, accents and widths folded (NFKC),
/// lower case, zero-width and bidi marks gone, and dots written out ("dot", "(.)", "[.]", " . ")
/// made dots.
fn normalise(text: &str) -> String {
    let folded: String = text.nfkc().filter(|c| !invisible(*c)).flat_map(char::to_lowercase).map(lookalike).collect();
    let mut s = format!(" {folded} ");
    // A dot with spaces on both sides, not one ending a sentence ("See you. Net is down").
    for written in ["(.)", "[.]", "{.}", "(dot)", "[dot]", "{dot}", " dot ", " . "] {
        s = s.replace(written, ".");
    }
    s.trim().to_string()
}

/// Zero-width and bidi characters, which hide or reorder what's shown.
fn invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}' | '\u{00ad}')
}

/// Letters from other scripts that look like Latin ones, read as those (Cyrillic and Greek).
fn lookalike(c: char) -> char {
    match c {
        'а' | 'α' => 'a',
        'с' | 'ϲ' => 'c',
        'е' | 'ε' => 'e',
        'һ' => 'h',
        'і' | 'ι' | 'ı' => 'i',
        'ј' => 'j',
        'κ' | 'к' => 'k',
        'ο' | 'о' | 'σ' => 'o',
        'р' | 'ρ' => 'p',
        'ѕ' => 's',
        'т' | 'τ' => 't',
        'υ' => 'u',
        'ν' => 'v',
        'х' | 'χ' => 'x',
        'у' | 'γ' => 'y',
        _ => c,
    }
}

/// An IPv4 address (four numbers 0-255, dotted) or a bracketed IPv6 one.
fn ip_address(s: &str) -> Option<String> {
    for word in s.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '"' | '\'' | '<' | '>')) {
        let host = word.trim_end_matches(['.', '!', '?']).split(['/', ':']).next().unwrap_or_default();
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() == 4
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n <= 255))
        {
            return Some(host.to_string());
        }
        if let Some(inner) = word.strip_prefix('[').and_then(|w| w.split(']').next()) {
            if inner.contains(':') && inner.parse::<std::net::Ipv6Addr>().is_ok() {
                return Some(inner.to_string());
            }
        }
    }
    None
}

/// A word, a dot and a known top-level domain ("example.com", "steamcommunlty.gift/x").
fn domain(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c != '.' || i == 0 {
            continue;
        }
        // The label before the dot, and the TLD after it.
        let before: String = chars[..i]
            .iter()
            .rev()
            .take_while(|c| c.is_alphanumeric() || **c == '-')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let after: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_alphabetic()).collect();
        if before.is_empty() || after.is_empty() || before.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let next = chars.get(i + 1 + after.chars().count()).copied();
        // The TLD must end there: "example.common" isn't example.com.
        if next.is_some_and(|n| n.is_alphanumeric()) {
            continue;
        }
        if !TLDS.contains(&after.as_str()) {
            continue;
        }
        let path = matches!(next, Some('/' | ':'));
        if WORDLIKE.contains(&after.as_str()) && !path {
            continue;
        }
        return Some(format!("{before}.{after}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_links() {
        for text in [
            "1.5",
            "at 9.30 tonight",
            "v1.5.2 is out",
            "logged in.it was fine",
            "see you. net is laggy",
            "gg. com on, next round",
            "go.to the left",
            "ok.so what now",
            "e.g. this",
            "map.the end",
            "x2.5 damage",
            "example.common",
            "1.2.3",
            "999.1.1.1",
        ] {
            assert_eq!(find_link(text), None, "{text}");
        }
    }

    #[test]
    fn links() {
        for text in [
            "example.com",
            "Example.COM",
            "a.example.co.uk",
            "x.io",
            "try evil.it/path",
            "me.to/x",
            "steamcommunlty.gift",
            "ехаmрlе.com",
            "exa\u{200b}mple.com",
            "example dot com",
            "example[.]com",
            "10.0.0.1",
            "[2001:db8::1]",
        ] {
            assert!(find_link(text).is_some(), "{text}");
        }
    }

    #[test]
    fn invites_are_exact() {
        assert_eq!(discord_invites("hi discord.gg/abc"), vec![(3, 17)]);
        assert!(discord_invites("hidiscord.gg/abc").is_empty(), "only at a word's start");
        assert!(discord_invites("discord.gg/a").is_empty());
        assert!(discord_invites("discord.gg/abc/def").is_empty());
        assert!(discord_invites("discord.gg/ab_c").is_empty());
        assert!(discord_invites("discord.gg@evil.com").is_empty());
        assert_eq!(discord_invites("DISCORD.GG/Abc").len(), 1, "any case");
    }
}
