//! The project's roadmap (`GET /v1/roadmap`), always read from the community's coordinator
//! whatever network the player is on, and the suggestions players send it
//! (`POST /v1/suggestions`, signed with their identity) and read back with the admins'
//! replies (`GET /v1/suggestions/mine`).
//!
//! The last roadmap read is kept (`roadmap-cache.json` in the launcher's folder), so it
//! shows offline too.

use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

/// The file, in the launcher's folder, keeping the last roadmap read.
pub const CACHE_FILE: &str = "roadmap-cache.json";
/// The largest answer read, in bytes.
pub const MAX_BYTES: usize = 512 * 1024;

/// What a suggestion is about, as the coordinator takes it.
pub const AREAS: [&str; 5] = ["Launcher", "In the game", "Matches and joining", "Servers", "Other"];
/// A suggestion's title and details, in characters.
pub const TITLE_MIN: usize = 3;
pub const TITLE_MAX: usize = 80;
pub const TEXT_MAX: usize = 1000;

/// The most lanes and items a lane read.
const MAX_LANES: usize = 8;
const MAX_ITEMS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Roadmap {
    pub lanes: Vec<Lane>,
    /// When an admin last changed it (Unix seconds).
    #[serde(default)]
    pub updated: i64,
}

/// A lane: "shipping", "next", "later" or "requested", with the release it's for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Lane {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Item {
    #[serde(default)]
    pub id: i64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub status: String,
}

/// The roadmap's address on a coordinator.
pub fn url(coordinator: &str) -> String {
    format!("{}/v1/roadmap", coordinator.trim().trim_end_matches('/'))
}

/// The roadmap from the coordinator's answer: lanes and items that read as one, their
/// text made safe to show (see `hooks_config::text`).
pub fn parse(json: &str) -> anyhow::Result<Roadmap> {
    #[derive(Deserialize)]
    struct Answer {
        lanes: Vec<serde_json::Value>,
        #[serde(default)]
        updated: i64,
    }
    let answer: Answer = serde_json::from_str(json)?;
    let lanes = answer
        .lanes
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Lane>(v).ok())
        .take(MAX_LANES)
        .map(|lane| Lane {
            id: clip(&lane.id, 24),
            title: clip(&lane.title, 40),
            release: clip(&lane.release, 24),
            items: lane.items.into_iter().take(MAX_ITEMS).map(clean_item).filter(|i| !i.title.is_empty()).collect(),
        })
        .filter(|lane| !lane.title.is_empty())
        .collect();
    Ok(Roadmap { lanes, updated: answer.updated })
}

fn clean_item(item: Item) -> Item {
    Item {
        id: item.id,
        title: clip(&item.title, 80),
        body: block(&item.body, 600),
        tags: item.tags.iter().take(5).map(|t| clip(t, 20)).filter(|t| !t.is_empty()).collect(),
        status: clip(&item.status, 24),
    }
}

fn clip(text: &str, max: usize) -> String {
    hooks_config::text::clip(text, max)
}

/// Text of several lines: each made safe like a one-line text, at most `max` characters.
fn block(text: &str, max: usize) -> String {
    let lines: Vec<String> = text.lines().map(|l| clip(l, max)).collect();
    let joined = lines.join("\n");
    joined.trim().chars().take(max).collect()
}

/// The roadmap kept, and when it was read (Unix seconds).
#[derive(Debug, Clone, Deserialize, Serialize)]
struct Cached {
    saved_at: i64,
    roadmap: Roadmap,
}

/// Keeps `roadmap` as the last one read.
pub fn save_cache(dir: &Path, roadmap: &Roadmap, now: i64) {
    let cached = Cached {
        saved_at: now,
        roadmap: roadmap.clone(),
    };
    if let Ok(json) = serde_json::to_vec(&cached) {
        let _ = std::fs::create_dir_all(dir);
        let _ = crate::write_atomic(&dir.join(CACHE_FILE), &json);
    }
}

/// The last roadmap read, and when.
pub fn read_cache(dir: &Path) -> Option<(Roadmap, i64)> {
    let cached: Cached = serde_json::from_slice(&std::fs::read(dir.join(CACHE_FILE)).ok()?).ok()?;
    Some((cached.roadmap, cached.saved_at))
}

/// A suggestion as typed, made ready to send: trimmed, Windows line ends as plain ones,
/// and checked as the coordinator checks it. Answers what's wrong, in the player's words.
pub fn check(area: &str, title: &str, text: &str) -> Result<(String, String), String> {
    if !AREAS.contains(&area) {
        return Err("Choose what it's about.".into());
    }
    let title = title.trim().to_string();
    let text = text.replace("\r\n", "\n").trim().to_string();
    let printable = |s: &str, newlines: bool| s.chars().all(|c| !c.is_control() || (newlines && c == '\n'));
    let title_len = title.chars().count();
    if title_len < TITLE_MIN {
        return Err(format!("Give it a title of at least {TITLE_MIN} characters."));
    }
    if title_len > TITLE_MAX || !printable(&title, false) {
        return Err(format!("Keep the title to one line of {TITLE_MAX} characters."));
    }
    if text.is_empty() {
        return Err("Say a little more in the details.".into());
    }
    if text.chars().count() > TEXT_MAX || !printable(&text, true) {
        return Err(format!("Keep the details to {TEXT_MAX} characters."));
    }
    Ok((title, text))
}

/// Who sends a suggestion, as the coordinator wants to know: their name (shown to admins
/// only), the server they play on, and this launcher's version.
pub struct Sender<'a> {
    pub name: &'a str,
    pub server: Option<&'a str>,
    pub launcher: &'a str,
}

/// The body of `POST /v1/suggestions`: the suggestion signed with `identity` at `time`.
/// `title` and `text` as [`check`] answers them.
pub fn suggestion_body(identity: &identity::Identity, sender: &Sender, area: &str, title: &str, text: &str, time: i64) -> serde_json::Value {
    let id = identity.global_id();
    let name = match clip(sender.name, 32) {
        name if name.is_empty() => identity::short(&id),
        name => name,
    };
    let mut body = serde_json::json!({
        "identity": id,
        "name": name,
        "area": area,
        "title": title,
        "text": text,
        "time": time,
        "signature": identity.sign(&identity::suggestion_message(time, area, title, text)),
        "launcher": sender.launcher,
    });
    if let Some(server) = sender.server.map(str::trim).filter(|s| !s.is_empty()) {
        body["server"] = server.into();
    }
    body
}

/// The address of `GET /v1/suggestions/mine`, signed with `identity` at `time`.
pub fn mine_url(coordinator: &str, identity: &identity::Identity, time: i64) -> String {
    let base = format!("{}/v1/suggestions/mine", coordinator.trim().trim_end_matches('/'));
    let mut url = match url::Url::parse(&base) {
        Ok(url) => url,
        Err(_) => return base,
    };
    url.query_pairs_mut()
        .append_pair("identity", &identity.global_id())
        .append_pair("time", &time.to_string())
        .append_pair("signature", &identity.sign(&identity::suggestions_message(time)));
    url.into()
}

/// One of the player's suggestions, with what the admins made of it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Mine {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub area: String,
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub reply: String,
}

/// The player's suggestions from the coordinator's answer, newest first as it gives them.
pub fn parse_mine(json: &str) -> anyhow::Result<Vec<Mine>> {
    #[derive(Deserialize)]
    struct Answer {
        suggestions: Vec<serde_json::Value>,
    }
    let answer: Answer = serde_json::from_str(json)?;
    Ok(answer
        .suggestions
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Mine>(v).ok())
        .take(50)
        .map(|m| Mine {
            area: clip(&m.area, 24),
            title: clip(&m.title, TITLE_MAX),
            text: block(&m.text, TEXT_MAX),
            status: m.status.trim().to_ascii_lowercase(),
            reply: block(&m.reply, 300),
            ..m
        })
        .collect())
}

/// A suggestion's status in words: "New", "Planned", "Done" or "Declined".
pub fn status_word(status: &str) -> &'static str {
    match status {
        "planned" => "Planned",
        "done" => "Done",
        "declined" => "Declined",
        _ => "New",
    }
}

/// What to tell the player when the coordinator turned a request down: its own words
/// (`{ "error": "..." }`) when it gave some, else its status.
pub fn error_text(status: u16, body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(|e| clip(e, 200)))
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| format!("The project's server answered {status}; try again later."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_roadmap() {
        let json = r#"{"lanes":[
            {"id":"shipping","title":"Shipping","release":"0.4.2","items":[
                {"id":12,"title":"Global matchmaking","body":"One matchmaking server\nfor every region\u202e","tags":["Server","Network"],"status":"Planned"},
                {"id":13,"title":"","body":"no title"}]},
            {"id":"next","title":"Next","release":"0.4.3","items":[]},
            {"id":"later","title":"Later"},
            {"title":42}],
            "updated":1791230000}"#;
        let roadmap = parse(json).unwrap();
        assert_eq!(roadmap.lanes.len(), 3, "a lane that doesn't read is let go");
        assert_eq!(roadmap.updated, 1_791_230_000);
        let item = &roadmap.lanes[0].items[0];
        assert_eq!(roadmap.lanes[0].items.len(), 1, "an item without a title is let go");
        assert_eq!(item.body, "One matchmaking server\nfor every region", "lines kept, tricks dropped");
        assert_eq!(item.tags, ["Server", "Network"]);
        assert!(roadmap.lanes[2].items.is_empty() && roadmap.lanes[2].release.is_empty());
        assert!(parse("<html>").is_err());
        assert_eq!(url("https://play.example.net/"), "https://play.example.net/v1/roadmap");
    }

    #[test]
    fn kept_to_show_offline() {
        let dir = crate::testutil::temp_dir("roadmap-cache");
        assert_eq!(read_cache(&dir), None);
        let roadmap = parse(r#"{"lanes":[{"id":"next","title":"Next","items":[{"title":"Chat"}]}]}"#).unwrap();
        save_cache(&dir, &roadmap, 1000);
        assert_eq!(read_cache(&dir), Some((roadmap, 1000)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn suggestions_checked_as_the_server_does() {
        assert_eq!(
            check("Launcher", "  Dark mode  ", "Please\r\nthanks ").unwrap(),
            ("Dark mode".into(), "Please\nthanks".into())
        );
        assert!(check("Graphics", "Dark mode", "Please").is_err(), "an area it doesn't take");
        assert!(check("Launcher", "Hi", "Please").is_err(), "title too short");
        assert!(check("Launcher", &"a".repeat(81), "Please").is_err());
        assert!(check("Launcher", "Two\nlines", "Please").is_err());
        assert!(check("Launcher", "Dark mode", "   ").is_err());
        assert!(check("Launcher", "Dark mode", &"a".repeat(1001)).is_err());
        assert!(check("Other", "Dark mode", &"é".repeat(1000)).is_ok(), "characters, not bytes");
    }

    #[test]
    fn signed_with_the_players_identity() {
        let me = identity::Identity::generate();
        let sender = Sender {
            name: "Kiwi",
            server: Some("oceania.example.net"),
            launcher: "0.4.2",
        };
        let body = suggestion_body(&me, &sender, "Servers", "More servers", "In Asia", 1_791_230_000);
        assert_eq!(body["identity"], me.global_id());
        assert_eq!((body["name"].as_str(), body["server"].as_str()), (Some("Kiwi"), Some("oceania.example.net")));
        let message = identity::suggestion_message(1_791_230_000, "Servers", "More servers", "In Asia");
        assert!(identity::verify(&me.global_id(), &message, body["signature"].as_str().unwrap()));
        // No name: the short identity; no server: not sent.
        let body = suggestion_body(
            &me,
            &Sender {
                name: " ",
                server: None,
                ..sender
            },
            "Other",
            "Title",
            "Text",
            1,
        );
        assert_eq!(body["name"], identity::short(&me.global_id()));
        assert!(body.get("server").is_none());

        let url = mine_url("https://play.example.net/", &me, 1_791_230_000);
        assert!(url.starts_with("https://play.example.net/v1/suggestions/mine?identity="), "{url}");
        let signature = url::Url::parse(&url).unwrap().query_pairs().find(|(k, _)| k == "signature").unwrap().1.into_owned();
        assert!(identity::verify(&me.global_id(), &identity::suggestions_message(1_791_230_000), &signature));
    }

    #[test]
    fn reads_the_players_suggestions() {
        let json = r#"{"suggestions":[
            {"id":41,"area":"Launcher","title":"Dark mode","text":"Please","created_at":1791230000,"status":"planned","reply":"In 0.4.3"},
            {"id":40,"area":"Other","title":"Old one","text":"x","created_at":1791000000,"status":"weird","reply":""},
            {"id":"bad"}]}"#;
        let mine = parse_mine(json).unwrap();
        assert_eq!(mine.len(), 2);
        assert_eq!((status_word(&mine[0].status), mine[0].reply.as_str()), ("Planned", "In 0.4.3"));
        assert_eq!(status_word(&mine[1].status), "New");
        assert_eq!(status_word("declined"), "Declined");
        assert_eq!(status_word("done"), "Done");
    }

    #[test]
    fn the_servers_own_words_on_failure() {
        assert_eq!(
            error_text(429, r#"{"error":"You've sent 3 suggestions today; try again tomorrow."}"#),
            "You've sent 3 suggestions today; try again tomorrow."
        );
        assert_eq!(error_text(502, "<html>Bad gateway</html>"), "The project's server answered 502; try again later.");
    }
}
