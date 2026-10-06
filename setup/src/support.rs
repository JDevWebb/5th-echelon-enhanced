//! Support: the player's conversation with the network's admins, on the community network's
//! coordinator (`coordinator/src/support.rs`). What the launcher sends and reads, signed
//! with the player's identity for that coordinator.

use base64::Engine as _;
use serde::Deserialize;

use crate::feedback::Attachment;

/// The longest message (the coordinator's limit).
pub const MAX_TEXT: usize = 2000;

/// Who writes: their name (shown to admins), the server they play on, and the launcher's
/// version.
pub struct Sender<'a> {
    pub name: &'a str,
    pub server: Option<&'a str>,
    pub launcher: &'a str,
}

/// The body of `POST /v1/support` at `coordinator`: `text` and `files`, signed with
/// `identity` at `time` for that coordinator's name.
pub fn message_body(coordinator: &str, identity: &identity::Identity, sender: &Sender, text: &str, files: &[Attachment], time: i64) -> serde_json::Value {
    let host = url::Url::parse(coordinator.trim()).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let id = identity.global_id();
    let name = match hooks_config::text::clip(sender.name, 32) {
        name if name.trim().is_empty() => identity::short(&id),
        name => name,
    };
    let text = text.trim();
    let digests: Vec<(&str, [u8; 32])> = files.iter().map(|f| (f.name.as_str(), identity::digest(&f.gzip))).collect();
    serde_json::json!({
        "identity": id,
        "name": name,
        "server": sender.server.map(str::trim).unwrap_or_default(),
        "launcher": sender.launcher,
        "time": time,
        "text": text,
        "files": files.iter().map(|f| serde_json::json!({
            "name": f.name,
            "size": f.size,
            "gzip_base64": base64::engine::general_purpose::STANDARD.encode(&f.gzip),
        })).collect::<Vec<_>>(),
        "signature": identity.sign(&identity::support_message(&host, time, text, &digests)),
    })
}

/// The address of `GET /v1/support/mine` at `coordinator`, signed with `identity` at
/// `time`; with `read`, the admins' answers count as read (the Support page shows them).
pub fn mine_url(coordinator: &str, identity: &identity::Identity, time: i64, read: bool) -> String {
    let base = format!("{}/v1/support/mine", coordinator.trim().trim_end_matches('/'));
    let Ok(mut url) = url::Url::parse(&base) else { return base };
    let host = url.host_str().unwrap_or_default().to_string();
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("identity", &identity.global_id())
            .append_pair("time", &time.to_string())
            .append_pair("signature", &identity.sign(&identity::support_read_message(&host, time)));
        if read {
            q.append_pair("read", "1");
        }
    }
    url.into()
}

/// A message of the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Message {
    pub id: i64,
    #[serde(default)]
    pub at: i64,
    /// "player" or "admin".
    #[serde(default)]
    pub from: String,
    /// The admin who answered.
    #[serde(default)]
    pub admin: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub files: Vec<FileName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileName {
    pub name: String,
}

impl Message {
    pub fn from_admin(&self) -> bool {
        self.from == "admin"
    }
}

/// The conversation as the coordinator gives it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Thread {
    /// open, waiting, resolved; None before the player's first message.
    pub status: Option<String>,
    pub messages: Vec<Message>,
    /// Answers the player hasn't seen on the Support page.
    pub unread: u32,
}

impl Thread {
    /// The newest answer's id (to tell an answer that came since the launcher last looked).
    pub fn last_answer(&self) -> i64 {
        self.messages.iter().filter(|m| m.from_admin()).map(|m| m.id).max().unwrap_or(0)
    }
}

/// The conversation from the coordinator's answer, its texts cleaned (an admin's words
/// shown in the launcher: no control characters, at most the coordinator's limit).
pub fn parse_thread(json: &str) -> anyhow::Result<Thread> {
    #[derive(Deserialize)]
    struct Answer {
        status: Option<String>,
        #[serde(default)]
        messages: Vec<serde_json::Value>,
        #[serde(default)]
        unread: u32,
    }
    let a: Answer = serde_json::from_str(json)?;
    let mut messages: Vec<Message> = a.messages.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
    let skip = messages.len().saturating_sub(200);
    messages.drain(..skip);
    for m in &mut messages {
        m.text = hooks_config::text::clip_keeping_lines(&m.text, MAX_TEXT);
        m.admin = m.admin.as_deref().map(|a| hooks_config::text::clip(a, 40));
        m.files.truncate(8);
        for f in &mut m.files {
            f.name = hooks_config::text::clip(&f.name, 64);
        }
    }
    Ok(Thread {
        status: a.status.map(|s| s.trim().to_ascii_lowercase()),
        messages,
        unread: a.unread.min(999),
    })
}

/// Why a message can't go as typed, if it can't.
pub fn check(text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Write what's wrong first.".into());
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("At most {MAX_TEXT} characters ({} now).", text.chars().count()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::attach;
    use crate::feedback::Private;

    #[test]
    fn messages_are_signed_for_the_coordinator_with_their_files() {
        let me = identity::Identity::generate();
        let log = attach(
            "launcher.log",
            "C:\\Users\\jon\\x: dropped\n",
            &Private {
                names: vec!["jon".into()],
                keep: vec![],
            },
        )
        .unwrap();
        let sender = Sender {
            name: "Oni",
            server: Some("eu1.example.net "),
            launcher: "0.4.3",
        };
        let body = message_body("https://play.example.net/", &me, &sender, "  It drops.  ", std::slice::from_ref(&log), 1_800_000_000);
        assert_eq!(
            (body["text"].as_str(), body["server"].as_str(), body["name"].as_str()),
            (Some("It drops."), Some("eu1.example.net"), Some("Oni"))
        );
        assert_eq!(body["files"][0]["name"], "launcher.log");
        assert!(!log.text.contains("jon"), "the file is the redacted text");
        // What the coordinator checks: the text and the file's gzip, for its own name.
        let digests = [("launcher.log", identity::digest(&log.gzip))];
        let message = identity::support_message("play.example.net", 1_800_000_000, "It drops.", &digests);
        assert!(identity::verify(&me.global_id(), &message, body["signature"].as_str().unwrap()));
        let elsewhere = identity::support_message("other.example.net", 1_800_000_000, "It drops.", &digests);
        assert!(!identity::verify(&me.global_id(), &elsewhere, body["signature"].as_str().unwrap()));
        let no_file = identity::support_message("play.example.net", 1_800_000_000, "It drops.", &[]);
        assert!(!identity::verify(&me.global_id(), &no_file, body["signature"].as_str().unwrap()));
    }

    #[test]
    fn the_conversation_is_read_signed_and_cleaned() {
        let me = identity::Identity::generate();
        let url = mine_url("https://play.example.net", &me, 1_800_000_000, true);
        let u = url::Url::parse(&url).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q.get("read").map(String::as_str), Some("1"));
        assert!(identity::verify(
            &me.global_id(),
            &identity::support_read_message("play.example.net", 1_800_000_000),
            &q["signature"]
        ));
        assert!(!mine_url("https://play.example.net", &me, 1, false).contains("read="));

        let json = r#"{"status":"Waiting","unread":1,"messages":[
            {"id":4,"at":10,"from":"player","admin":null,"text":"It drops","files":[{"name":"launcher.log","size":9}]},
            {"id":7,"at":20,"from":"admin","admin":"Jason","text":"Send the game's log\u001b[31m please"},
            {"nonsense":true}]}"#;
        let t = parse_thread(json).unwrap();
        assert_eq!((t.status.as_deref(), t.unread, t.messages.len(), t.last_answer()), (Some("waiting"), 1, 2, 7));
        assert!(t.messages[1].from_admin() && !t.messages[1].text.contains('\u{1b}'));
        assert_eq!(t.messages[0].files[0].name, "launcher.log");
        assert_eq!(parse_thread(r#"{"status":null,"messages":[],"unread":0}"#).unwrap(), Thread::default());
        assert!(check("  ").is_err() && check(&"x".repeat(2001)).is_err() && check("help").is_ok());
    }
}
