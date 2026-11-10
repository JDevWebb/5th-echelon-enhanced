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
        let mut names: Vec<String> = ["USERNAME", "USER", "COMPUTERNAME", "HOSTNAME", "LOGNAME"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .chain(host_name())
            .map(|n| n.trim().to_string())
            .filter(|n| n.chars().count() >= 3)
            .collect();
        // The host name's first label too ("jons-pc" of "jons-pc.home.lan").
        if let Some(short) = names
            .iter()
            .filter_map(|n| n.split_once('.'))
            .map(|(first, _)| first.to_string())
            .find(|f| f.chars().count() >= 3)
        {
            names.push(short);
        }
        names.sort_by_key(|n| n.to_ascii_lowercase());
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
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

/// The PC's own name as the system has it, not only as the environment says (`HOSTNAME` is
/// often unset on Linux, and a shell's isn't the launcher's).
#[cfg(unix)]
fn host_name() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer and its length; gethostname writes at most that.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).into_owned()).filter(|n| !n.is_empty())
}

/// The PC's DNS host name (`COMPUTERNAME` is its NetBIOS name, cut to 15 characters; under Wine
/// this is the Linux host's name).
#[cfg(windows)]
fn host_name() -> Option<String> {
    use windows::Win32::System::SystemInformation::ComputerNameDnsHostname;
    use windows::Win32::System::SystemInformation::GetComputerNameExW;
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    // SAFETY: the buffer and its length in characters; the call writes at most that.
    unsafe { GetComputerNameExW(ComputerNameDnsHostname, windows::core::PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    let len = (len as usize).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len])).filter(|n| !n.is_empty())
}

#[cfg(not(any(unix, windows)))]
fn host_name() -> Option<String> {
    None
}

/// `text` with the private parts hidden: home folders (`C:\Users\<name>`, `/home/<name>`,
/// Wine's `Z:\home\<name>`, also with doubled backslashes as debug output writes them), Steam
/// account ids in its folders (`userdata\<digits>`), the names in `private`
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
        // Wine's view of a Linux home is `Z:\home\<name>`.
        let home = ["users\\\\", "users\\", "users/", "home\\\\", "home\\", "home/"].into_iter().find(|m| rest.starts_with(m));
        // Steam's folder of an account's own files is named by its account id.
        let steam = ["userdata\\\\", "userdata\\", "userdata/"].into_iter().find(|m| rest.starts_with(m));
        let at_boundary = i == 0 || matches!(text.as_bytes()[i - 1], b'\\' | b'/');
        let ends = |from: usize| text[from..].find(['\\', '/', '"', '\'', '\n', '\r', ',', ';', ')', ']']).map_or(text.len(), |e| from + e);
        if let (Some(m), true) = (home, at_boundary) {
            out.push_str(&text[i..i + m.len()]);
            i += m.len();
            // To the end of the folder's name, spaces and all ("Jo Smith").
            let end = ends(i);
            if end > i {
                out.push_str("<user>");
            }
            i = end;
            continue;
        }
        if let (Some(m), true) = (steam, at_boundary) {
            let from = i + m.len();
            let end = ends(from);
            if end > from && text[from..end].bytes().all(|b| b.is_ascii_digit()) {
                out.push_str(&text[i..from]);
                out.push_str("<id>");
                i = end;
                continue;
            }
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
    fn wine_homes_and_steam_account_ids_are_hidden() {
        let private = Private::default();
        assert_eq!(redact(r"open Z:\home\jo smith\Games\x.ini", &private), r"open Z:\home\<user>\Games\x.ini");
        assert_eq!(redact(r#""Z:\\home\\jo\\.steam""#, &private), r#""Z:\\home\\<user>\\.steam""#);
        assert_eq!(redact("/home/jo/.local/share", &private), "/home/<user>/.local/share");
        assert_eq!(
            redact(
                r"C:\Program Files (x86)\Steam\userdata\123456789\235600\remote, ~/.steam/steam/userdata/42/config",
                &private
            ),
            r"C:\Program Files (x86)\Steam\userdata\<id>\235600\remote, ~/.steam/steam/userdata/<id>/config"
        );
        // Not an account id, nor a folder of that name.
        assert_eq!(
            redact(r"Steam\userdata\anonymous\x and myuserdata\123", &private),
            r"Steam\userdata\anonymous\x and myuserdata\123"
        );
    }

    #[test]
    fn this_pcs_own_name_is_among_the_private_ones() {
        let private = Private::of_this_pc(Vec::new());
        if let Some(name) = super::host_name().filter(|n| n.trim().chars().count() >= 3) {
            assert!(private.names.iter().any(|n| n.eq_ignore_ascii_case(name.trim())), "{name} in {:?}", private.names);
            assert!(redact(&format!("on {name}"), &private).contains("<private>"));
        }
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
