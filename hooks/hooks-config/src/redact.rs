//! Hiding what's private in log text before it leaves the player's PC: their account and PC
//! names, home folders and public addresses. Used by the launcher's reports
//! (`setup::feedback`) and the game's diagnostics sent to the server (the hooks DLL).

use std::net::Ipv4Addr;

/// What to hide in the logs sent: the names of the player's Windows (or Linux) account and
/// PC. Folders under a user's home are hidden whatever the name; so are public addresses
/// other than `keep` (the server's, which isn't the player's).
#[derive(Debug, Clone, Default)]
pub struct Private {
    pub names: Vec<String>,
    pub keep: Vec<Ipv4Addr>,
}

impl Private {
    /// This PC's account and computer names.
    pub fn of_this_pc(keep: Vec<Ipv4Addr>) -> Self {
        let names = ["USERNAME", "USER", "COMPUTERNAME", "HOSTNAME", "LOGNAME"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .map(|n| n.trim().to_string())
            .filter(|n| n.chars().count() >= 3)
            .collect();
        Self { names, keep }
    }
}

/// `text` with the private parts hidden: home folders (`C:\Users\<name>`, `/home/<name>`,
/// also with doubled backslashes as debug output writes them), the names in `private`
/// wherever they appear, and public IPv4 addresses (`203.0.x.x`) other than those kept.
pub fn redact(text: &str, private: &Private) -> String {
    // The names first: a home folder may be the whole of one ("C:\Users\Jo Smith").
    // Longest first: a PC name often holds the account's ("Jons-Laptop" for "Jon"), and
    // hiding the account's first would leave the rest of the PC's ("<private>s-Laptop").
    let mut names: Vec<&str> = private.names.iter().map(String::as_str).collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let mut out = text.to_string();
    for name in names {
        out = replace_ignoring_case(&out, name, "<private>");
    }
    hide_public_addresses(&hide_homes(&out), &private.keep)
}

fn hide_homes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let lower = text.to_ascii_lowercase();
    let mut i = 0;
    while i < text.len() {
        let rest = &lower[i..];
        let marker = ["users\\\\", "users\\", "users/", "home/"].into_iter().find(|m| rest.starts_with(m));
        let at_boundary = i == 0 || matches!(text.as_bytes()[i - 1], b'\\' | b'/');
        if let (Some(m), true) = (marker, at_boundary) {
            out.push_str(&text[i..i + m.len()]);
            i += m.len();
            // To the end of the folder's name, spaces and all ("Jo Smith").
            let end = text[i..].find(['\\', '/', '"', '\'', '\n', '\r', ',', ';', ')', ']']).map_or(text.len(), |e| i + e);
            if end > i {
                out.push_str("<user>");
            }
            i = end;
            continue;
        }
        let c = text[i..].chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn replace_ignoring_case(text: &str, what: &str, with: &str) -> String {
    // ASCII case only: it keeps every byte where it was, so the offsets match `text`.
    let lower = text.to_ascii_lowercase();
    let what = what.to_ascii_lowercase();
    if what.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in lower.match_indices(&what) {
        out.push_str(&text[last..i]);
        out.push_str(with);
        last = i + what.len();
    }
    out.push_str(&text[last..]);
    out
}

fn hide_public_addresses(text: &str, keep: &[Ipv4Addr]) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let starts = bytes[i].is_ascii_digit() && (i == 0 || !(bytes[i - 1].is_ascii_digit() || bytes[i - 1] == b'.'));
        if starts {
            let end = text[i..].find(|c: char| !(c.is_ascii_digit() || c == '.')).map_or(text.len(), |e| i + e);
            let candidate = text[i..end].trim_end_matches('.');
            if let Ok(ip) = candidate.parse::<Ipv4Addr>() {
                let public = !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast());
                if public && !keep.contains(&ip) {
                    let [a, b, ..] = ip.octets();
                    out.push_str(&format!("{a}.{b}.x.x"));
                } else {
                    out.push_str(candidate);
                }
                i += candidate.len();
                continue;
            }
            out.push_str(&text[i..end]);
            i = end;
            continue;
        }
        let c = text[i..].chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}
