//! Hiding what's private in log text before it leaves the player's PC: their account and PC
//! names, home folders and public addresses. Used by the launcher's reports
//! (`setup::feedback`) and the game's diagnostics sent to the server (the hooks DLL).

use std::net::Ipv4Addr;
use std::net::Ipv6Addr;

/// What to hide in the logs sent: the names of the player's Windows (or Linux) account and
/// PC. Folders under a user's home are hidden whatever the name; so are public addresses
/// other than `keep` (the server's, which isn't the player's). Names in `shown` (the
/// player's in-game name, which the server knows anyway) stay where they appear whole.
#[derive(Debug, Clone, Default)]
pub struct Private {
    pub names: Vec<String>,
    pub keep: Vec<Ipv4Addr>,
    pub shown: Vec<String>,
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
        Self { names, keep, shown: Vec::new() }
    }

    /// Also keeps `name` (the player's in-game name) where it appears whole, even when it
    /// holds or is the account's name ("Victor" for the account "Victo").
    #[must_use]
    pub fn showing(mut self, name: &str) -> Self {
        let name = name.trim();
        if !name.is_empty() {
            self.shown.push(name.to_string());
        }
        self
    }
}

/// `text` with the private parts hidden: home folders (`C:\Users\<name>`, `/home/<name>`,
/// also with doubled backslashes as debug output writes them), the names in `private`
/// wherever they appear (inside longer words too: `jon1987` is `<private>1987`), except
/// within the player's own in-game name where it appears whole, public IPv4 addresses (`203.0.x.x`) other than those kept, and public IPv6 ones
/// (`2001:db8:x`).
pub fn redact(text: &str, private: &Private) -> String {
    // The names first: a home folder may be the whole of one ("C:\Users\Jo Smith").
    let out = hide_names(text, &private.names, &private.shown);
    hide_public_v6(&hide_public_addresses(&hide_homes(&out), &private.keep))
}

/// `text` with each of `names`, wherever it appears (inside a longer word too: the account's
/// name in `jon1987` or `Jon_Desktop` is still the account's), ignoring (ASCII) case, as
/// `<private>`; a `shown` name that appears as a whole word (no letter, digit or `_` on
/// either side) stays as it is. Where several match at one place the longest wins: a PC name often holds the account's
/// ("Jons-Laptop" for "Jon"), and hiding the account's alone would leave the rest of the
/// PC's ("<private>s-Laptop"); the player's in-game name may hold it too ("Victor" for
/// "Victo"). A shown name wins a tie: it's the player's own, which the server knows.
fn hide_names(text: &str, names: &[String], shown: &[String]) -> String {
    // ASCII case only: it keeps every byte where it was, so the offsets match `text`.
    let lower = text.to_ascii_lowercase();
    let lowered = |list: &[String]| list.iter().map(|n| n.to_ascii_lowercase()).filter(|n| !n.is_empty()).collect::<Vec<_>>();
    let (names, shown) = (lowered(names), lowered(shown));
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let whole_at =
        |i: usize, name: &str| lower[i..].starts_with(name) && !text[..i].chars().next_back().is_some_and(is_word) && !text[i + name.len()..].chars().next().is_some_and(is_word);
    let kept = |i: usize| shown.iter().filter(|n| whole_at(i, n)).map(String::len).max();
    let hidden = |i: usize| names.iter().filter(|n| lower[i..].starts_with(n.as_str())).map(String::len).max();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        match (kept(i), hidden(i)) {
            (Some(keep), hide) if hide.is_none_or(|h| keep >= h) => {
                out.push_str(&text[i..i + keep]);
                i += keep;
            }
            (_, Some(hide)) => {
                out.push_str("<private>");
                i += hide;
            }
            _ => {
                let c = text[i..].chars().next().unwrap_or(' ');
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

/// Public IPv6 addresses, as `2001:db8:x` (the first two groups say the network, as an
/// IPv4 address's first two numbers do). Local ones (loopback, link-local `fe80::`, unique
/// local `fd00::`) stay, as LAN addresses do.
fn hide_public_v6(text: &str) -> String {
    let is_part = |c: char| c.is_ascii_hexdigit() || c == ':';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(is_part) {
        out.push_str(&rest[..start]);
        let token = &rest[start..];
        let len = token.find(|c: char| !is_part(c)).unwrap_or(token.len());
        let word = &token[..len];
        // Part of a longer word ("deadbeef:" in a hash, "ab:cd" in a name) isn't an address.
        let joined = out.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '.');
        let public = |ip: Ipv6Addr| {
            let first = ip.segments()[0];
            !(ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || first & 0xffc0 == 0xfe80 || first & 0xfe00 == 0xfc00)
        };
        match word.parse::<Ipv6Addr>() {
            Ok(ip) if !joined && word.matches(':').count() >= 2 && public(ip) => {
                let [a, b, ..] = ip.segments();
                out.push_str(&format!("{a:x}:{b:x}:x"));
            }
            _ => out.push_str(word),
        }
        rest = &token[len..];
    }
    out.push_str(rest);
    out
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

#[cfg(test)]
mod tests {
    use super::redact;
    use super::Private;

    fn account(names: &[&str]) -> Private {
        Private {
            names: names.iter().map(|n| (*n).to_string()).collect(),
            ..Private::default()
        }
    }

    #[test]
    fn a_username_holding_the_account_name_stays() {
        // The account "Victo", the player "Victor": no "Signed in as <private>r".
        let private = account(&["Victo", "DESKTOP-NASN"]).showing("Victor");
        assert_eq!(redact("Signed in as Victor", &private), "Signed in as Victor");
        assert_eq!(redact("Signed in as VICTOR on DESKTOP-NASN", &private), "Signed in as VICTOR on <private>");
        // Not shown: the account's name is hidden wherever it is, as before.
        assert_eq!(redact("Signed in as Victor", &account(&["Victo"])), "Signed in as <private>r");
        // The in-game name being the account's own: it's the player's, the server knows it.
        assert_eq!(redact("Signed in as Victor", &account(&["victor"]).showing("Victor")), "Signed in as Victor");
        // The account's name alone is still hidden.
        assert_eq!(redact("profile of victo loaded", &private), "profile of <private> loaded");
    }

    #[test]
    fn the_account_name_in_a_path_is_hidden() {
        let private = account(&["Victo"]).showing("Victor");
        let out = redact(r#"save "D:\Games\Victo\Blacklist" and "C:\\Users\\VICTO\\AppData", D:/victo/x"#, &private);
        assert_eq!(out, r#"save "D:\Games\<private>\Blacklist" and "C:\\Users\\<user>\\AppData", D:/<private>/x"#);
    }

    #[test]
    fn the_account_name_is_hidden_inside_other_words_too() {
        let private = account(&["Jon"]).showing("Jonathan");
        // The player's own name stays whole; the account's is hidden everywhere else, glued to
        // digits or underscores too (a folder or PC name made from it).
        assert_eq!(
            redact("by JON, for jon: (Jon) Jonathan jon1987 Jon_Desktop xjon", &private),
            "by <private>, for <private>: (<private>) Jonathan <private>1987 <private>_Desktop x<private>"
        );
        // Not whole, the in-game name doesn't keep the account's name in it.
        assert_eq!(redact("Jonathans-PC", &private), "<private>athans-PC");
    }

    #[test]
    fn public_ipv6_addresses_are_hidden_and_local_ones_stay() {
        let text = "peer [2001:db8:85a3::8a2e:370:7334]:13000, lan fe80::1%3, fd12:3456::1, ::1, at 12:34:56";
        let out = super::hide_public_v6(text);
        assert!(out.contains("peer [2001:db8:x]:13000"), "{out}");
        assert!(!out.contains("7334"), "{out}");
        assert!(out.contains("fe80::1%3") && out.contains("fd12:3456::1") && out.contains("::1,"), "{out}");
        assert!(out.contains("at 12:34:56"), "{out}");
    }
}
