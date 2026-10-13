//! Chat and name rules for the community network (docs/chat.md), the same code wherever they're
//! checked: the server (which decides), the coordinator (which decides again) and the launcher
//! (which shows the player what will be sent while they type). Not the game's overlay: it shows
//! the server's verdict.
//!
//! - **Slurs and hate are masked** in chat (`***`, the whole word), in every room; swearing,
//!   crude words and insults are allowed. Words come from `rustrict`'s list (leetspeak,
//!   look-alike letters and spaced-out spellings handled), its *offensive* type at any level,
//!   plus a short list of our own for sexual content involving minors.
//! - **Names follow the same line:** a name with such a word is refused (it can't be masked).
//! - **No links**, in chat or names, but a Discord invite in exactly its own form, one a
//!   message, so players can bring each other to a Discord.
//! - **An allow list** (the coordinator's settings) clears words and names admins have checked,
//!   e.g. a player's name the list takes for something else.

mod links;

use std::collections::HashSet;

pub use links::find_link;
pub use links::Link;
use rustrict::Censor;
use rustrict::Type;

/// The longest message, in characters.
pub const MAX_MESSAGE: usize = 500;

/// Sexual content involving minors: never allowed, and not swearing, so word lists catch
/// little of it. Matched as whole words, case and spacing aside.
const MINORS: &[&str] = &["loli", "lolicon", "shota", "shotacon", "jailbait", "childporn", "cheesepizza"];

/// The rules, with the network's own additions.
#[derive(Debug, Clone, Default)]
pub struct Rules {
    /// Words and names admins checked: never masked or refused (lower case).
    allow: HashSet<String>,
    /// Words to mask besides the list's (lower case).
    extra: HashSet<String>,
    /// Discord invites may be sent (the coordinator's `[chat] discord_invites`).
    pub discord_invites: bool,
}

/// What a message comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The message as it's sent: offending words masked.
    pub text: String,
    /// How many words were masked (counted per player for the admins; the words aren't kept).
    pub masked: u32,
    /// The Discord invite in it, if any.
    pub invite: Option<String>,
}

/// Why a message isn't sent, as the player reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Empty,
    TooLong,
    /// A link other than a Discord invite.
    Link,
    /// More than one Discord invite, or invites are off.
    Invite,
}

impl Refused {
    pub fn says(self) -> &'static str {
        match self {
            Self::Empty => "That message is empty.",
            Self::TooLong => "That message is too long: 500 characters at most.",
            Self::Link => "That message wasn't sent: links aren't allowed in chat (a Discord invite is).",
            Self::Invite => "That message wasn't sent: one Discord invite a message, where invites are allowed.",
        }
    }
}

/// Why a name isn't allowed, as the player reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRefused {
    /// A slur or hate, or sexual content involving minors.
    Offensive,
    /// A link (a web address).
    Link,
}

impl NameRefused {
    pub fn says(self) -> &'static str {
        match self {
            Self::Offensive => "That name isn't allowed. Try another one.",
            Self::Link => "Names can't be web addresses. Try another one.",
        }
    }
}

impl Rules {
    /// The rules with the network's additions: `allow` (checked words and names), `extra` (words
    /// to mask too), each compared without case.
    pub fn new<'a>(allow: impl IntoIterator<Item = &'a str>, extra: impl IntoIterator<Item = &'a str>, discord_invites: bool) -> Self {
        Self {
            allow: allow.into_iter().map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).collect(),
            extra: extra.into_iter().map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).collect(),
            discord_invites,
        }
    }

    /// Checks a chat message: refused for a link (a Discord invite excepted) or its length, else
    /// sent with offending words masked.
    pub fn check_message(&self, text: &str) -> Result<Checked, Refused> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Refused::Empty);
        }
        if text.chars().count() > MAX_MESSAGE {
            return Err(Refused::TooLong);
        }
        let invites = links::discord_invites(text);
        if invites.len() > 1 || (!invites.is_empty() && !self.discord_invites) {
            return Err(Refused::Invite);
        }
        // Links, with the invite taken out first (it's the one allowed).
        let without_invite = invites
            .first()
            .map_or_else(|| text.to_string(), |(start, end)| format!("{} {}", &text[..*start], &text[*end..]));
        if find_link(&without_invite).is_some() {
            return Err(Refused::Link);
        }
        let (masked_text, masked) = self.mask(text, invites.first().copied());
        Ok(Checked {
            text: masked_text,
            masked,
            invite: invites.first().map(|(s, e)| text[*s..*e].to_string()),
        })
    }

    /// Checks a player's name: refused for a slur or hate, sexual content involving minors, or a
    /// web address. (Its characters and length are the server's own rule.)
    pub fn check_name(&self, name: &str) -> Result<(), NameRefused> {
        let lower = name.trim().to_lowercase();
        if self.allow.contains(&lower) {
            return Ok(());
        }
        if find_link(name).is_some() {
            return Err(NameRefused::Link);
        }
        // A name is often several words run together ("SirCooms", "kill_the_x"): the list reads
        // it as is, and our own words are looked for inside it.
        let compact: String = lower.chars().filter(char::is_ascii_alphanumeric).collect();
        if MINORS.iter().any(|w| compact.contains(w)) || self.extra.iter().any(|w| compact.contains(w.as_str())) {
            return Err(NameRefused::Offensive);
        }
        if offensive(Censor::from_str(name).analyze()) {
            return Err(NameRefused::Offensive);
        }
        Ok(())
    }

    /// `text` with each word holding a slur, hate or one of our words masked whole (`***`),
    /// except within `keep` (a Discord invite's span) and allowed words; and how many.
    fn mask(&self, text: &str, keep: Option<(usize, usize)>) -> (String, u32) {
        // Which characters the list censors, at the offensive type only.
        let censored: Vec<bool> = {
            let (out, _) = Censor::from_str(text)
                .with_censor_threshold(Type::OFFENSIVE & Type::MILD_OR_HIGHER)
                .with_censor_first_character_threshold(Type::OFFENSIVE & Type::MILD_OR_HIGHER)
                .with_censor_replacement('*')
                .censor_and_analyze();
            let (a, b): (Vec<char>, Vec<char>) = (text.chars().collect(), out.chars().collect());
            if a.len() == b.len() {
                a.iter().zip(&b).map(|(x, y)| x != y && *y == '*').collect()
            } else {
                // The list changed the text's length (it shouldn't): word by word instead.
                let mut flags = Vec::with_capacity(a.len());
                for word in text.split_inclusive(char::is_whitespace) {
                    let bad = offensive(Censor::from_str(word.trim()).analyze());
                    flags.extend(word.chars().map(|c| bad && !c.is_whitespace()));
                }
                flags
            }
        };
        let mut out = String::with_capacity(text.len());
        let mut masked = 0;
        let mut i = 0; // index in chars
        let mut byte = 0;
        for word in text.split_inclusive(char::is_whitespace) {
            let n = word.chars().count();
            let body = word.trim_end();
            let in_keep = keep.is_some_and(|(s, e)| byte >= s && byte < e);
            let lower: String = body.to_lowercase();
            let core: String = lower.chars().filter(|c| c.is_alphanumeric()).collect();
            let allowed = self.allow.contains(&core) || self.allow.contains(&lower);
            let ours = MINORS.contains(&core.as_str()) || self.extra.contains(&core);
            let flagged = censored[i..i + n].iter().any(|&b| b) || ours;
            if flagged && !allowed && !in_keep && !body.is_empty() {
                out.push_str("***");
                out.push_str(&word[body.len()..]);
                masked += 1;
            } else {
                out.push_str(word);
            }
            i += n;
            byte += word.len();
        }
        (out, masked)
    }
}

/// Whether the list's verdict is a slur or hate, at any level.
fn offensive(t: Type) -> bool {
    t.is(Type::OFFENSIVE & Type::MILD_OR_HIGHER)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules::new([], [], true)
    }

    /// The slurs these tests need, kept reversed so they aren't written out in the source.
    fn rev(s: &str) -> String {
        s.chars().rev().collect()
    }

    #[test]
    fn swearing_and_insults_go_through_as_written() {
        for ok in [
            "gg wp",
            "damn that was close",
            "holy shit nice shot",
            "fuck this map",
            "you're trash lol",
            "noob team, get rekt",
            "kill the merc in the server room",
            "assassin run on charlie",
            "the Lebanese mission is hard",
            "Scunthorpe united",
            "version 1.5.2 at 9.30, 2.5x faster",
        ] {
            let c = rules().check_message(ok).unwrap();
            assert_eq!((c.text.as_str(), c.masked), (ok, 0), "{ok}");
        }
    }

    #[test]
    fn slurs_are_masked_whole_and_the_rest_sent() {
        let c = rules().check_message(&format!("you are a {} lol", rev("regg1n"))).unwrap();
        assert_eq!(c.text, "you are a *** lol");
        assert_eq!(c.masked, 1);
        let c = rules().check_message(&rev("t o g g a f")).unwrap();
        assert!(c.masked >= 1 && !c.text.contains('g'), "spaced out: {:?}", c.text);
        let c = rules().check_message("loli pics").unwrap();
        assert_eq!(c.text, "*** pics", "our own list");
    }

    #[test]
    fn checked_words_and_names_stay() {
        let r = Rules::new(["Bedunga"], [], true);
        assert_eq!(r.check_message("gg Bedunga").unwrap().text, "gg Bedunga");
        assert_eq!(r.check_name("Bedunga"), Ok(()));
        assert_eq!(Rules::new([], ["badword"], true).check_message("a badword here").unwrap().text, "a *** here");
    }

    #[test]
    fn links_are_refused_in_every_disguise() {
        for link in [
            "http://example.com",
            "see www.example.com",
            "go to example.com now",
            "example . com",
            "example dot com",
            "example(.)com",
            "1.2.3.4",
            "join at 203.0.113.9:3074",
            "[::1]",
            "ftp://files.example.org/x",
            "evil.ru",
            "free stuff at steamcommunlty.gift",
            "dlscord.gg/abcd",
            "disc0rd.gg/abcd",
            "discord-gift.com/abcd",
            "dsc.gg/abcd",
            "discord.gift/abcd",
            "xn--discrd-uta.gg/abcd",
            "discord.gg@evil.com",
        ] {
            assert_eq!(rules().check_message(link), Err(Refused::Link), "{link}");
        }
        // Cyrillic "і" in discord: not the invite, and a link.
        assert_eq!(rules().check_message("dіscord.gg/abcd"), Err(Refused::Link));
    }

    #[test]
    fn a_discord_invite_in_its_own_form_is_the_one_link_allowed() {
        for invite in [
            "discord.gg/abc123",
            "https://discord.gg/Ab-12",
            "discord.com/invite/abcd",
            "https://discordapp.com/invite/abcd",
        ] {
            let c = rules().check_message(&format!("our group: {invite} see you")).unwrap();
            assert_eq!(c.invite.as_deref(), Some(invite), "{invite}");
        }
        assert_eq!(rules().check_message("discord.gg/abc/extra"), Err(Refused::Link), "nothing after the code");
        assert_eq!(rules().check_message("discord.gg/a"), Err(Refused::Link), "a code is 2 characters or more");
        assert_eq!(rules().check_message("discord.gg/abcd discord.gg/efgh"), Err(Refused::Invite), "one a message");
        assert_eq!(Rules::new([], [], false).check_message("discord.gg/abcd"), Err(Refused::Invite), "invites off");
        assert_eq!(
            rules().check_message("discord.gg/abcd and example.com"),
            Err(Refused::Link),
            "an invite doesn't bring a link along"
        );
    }

    #[test]
    fn names_follow_the_same_line() {
        for ok in ["SirCooms", "Theusma01", "kill_count", "Shitstorm", "vipex_clapzzz", "d15_HB", "Scunthorpe"] {
            assert_eq!(rules().check_name(ok), Ok(()), "{ok}");
        }
        assert_eq!(rules().check_name(&format!("{}_slayer", rev("regg1n"))), Err(NameRefused::Offensive));
        assert_eq!(rules().check_name("xXloliXx"), Err(NameRefused::Offensive), "our list, inside a name");
        assert_eq!(rules().check_name("evil.com"), Err(NameRefused::Link));
        assert_eq!(rules().check_name("v1.5.2"), Ok(()), "a version isn't a web address");
    }

    #[test]
    fn length_and_emptiness() {
        assert_eq!(rules().check_message("   "), Err(Refused::Empty));
        assert_eq!(rules().check_message(&"a".repeat(MAX_MESSAGE + 1)), Err(Refused::TooLong));
        assert!(rules().check_message(&"a".repeat(MAX_MESSAGE)).is_ok());
    }
}
