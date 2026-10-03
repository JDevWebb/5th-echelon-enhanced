//! The community API: a small JSON API on the config server's port (80), for
//! launchers, overlays and tools.
//!
//! The game itself only ever asks this port for its online config, which every
//! path outside `/api/` still returns. Each part is switched in `service.toml`
//! (`[community_api]`); only `info` is on by default:
//!
//! * `GET /api/info` - what this server is and supports.
//! * `GET /api/news` - the server's news (`data/news.json`, as the game's news
//!   screen shows it), for the launcher. On with `info`.
//! * `GET /api/presence` - every registered player, who's online and what
//!   they're playing (`presence = true`).
//! * `POST /api/register` `{"username","password"}` - create an account, as
//!   the launcher's Register does (`accounts = true`). Rate-limited (see
//!   `rate_limit`, shared with the other login routes).
//! * `POST /api/login` `{"username","password"}` - check credentials
//!   (`accounts = true`). Rate-limited.
//! * `GET /api/unhandled` - the game's RMC calls this server couldn't answer,
//!   most frequent first, see `quazal::rmc::unhandled` (`unhandled = true`).

use std::sync::Arc;

use quazal::rmc::unhandled;
use serde_json::json;
use serde_json::Value;

use crate::config::CommunityApiConfig;
use crate::config::PublicPorts;
use crate::game_session::attribute_value;
use crate::rate_limit;
use crate::simple_http::Request;
use crate::simple_http::Response;
use crate::simple_http::Routes;
use crate::storage::LiveSession;
use crate::storage::LoginError;
use crate::storage::Storage;

/// This build's release, from release.toml.
pub const RELEASE: &str = env!("FE_RELEASE");

/// What every server of this release supports, for clients to check. The
/// switchable API parts are added when they're on.
const FEATURES: &[&str] = &[
    "invites",
    "private-matches",
    "trusted-subnet",
    "persistent-logins",
    "friends",
    "identity",
    "identity-login",
    "rename",
];

/// Session attributes (see game_session.rs): 101 map, 102 mode, 113 room kind
/// (0 match, 1 lobby); `match_mode` tells co-op from Spies vs Mercs.
const ATTR_MAP: u32 = 101;
const ATTR_ROOM_KIND: u32 = 113;

/// The ports (and host) players connect to, for `/api/info`; set at start.
static PUBLIC: std::sync::OnceLock<(PublicPorts, Option<String>)> = std::sync::OnceLock::new();

/// Publishes the ports players use in `/api/info`, so the launcher can set
/// players up for a server behind a proxy or with remapped ports.
pub fn publish(ports: PublicPorts, host: Option<String>) {
    let _ = PUBLIC.set((ports, host));
}

/// What `/api/info` says about friends: this server's id (players sign it
/// into identity links and key logins), the friend list mode, and the
/// coordinator it shares friends with, if any.
static FRIENDS: std::sync::OnceLock<(String, crate::config::FriendsMode, Option<String>)> = std::sync::OnceLock::new();

pub fn publish_friends(server_id: String, mode: crate::config::FriendsMode, coordinator: Option<String>) {
    let _ = FRIENDS.set((server_id, mode, coordinator));
}

/// The paths that take a POST body; POSTs anywhere else are refused before it.
pub fn takes_body(path: &str) -> bool {
    matches!(path, "/api/register" | "/api/login")
}

pub fn routes(storage: Arc<Storage>, cfg: CommunityApiConfig) -> Routes {
    Arc::new(move |req: &Request| {
        let path = req.path.split('?').next().unwrap_or_default();
        if path != "/api" && !path.starts_with("/api/") {
            return None;
        }
        Some(match (req.method.as_str(), path) {
            ("GET", "/api/info") if cfg.info => Response::json("200 OK", &info(cfg)),
            ("GET", "/api/news") if cfg.info => Response::json("200 OK", &news()),
            ("GET", "/api/presence") if cfg.presence => match storage.presence() {
                Ok((players, sessions)) => Response::json("200 OK", &presence(&players, &sessions)),
                Err(e) => internal(e),
            },
            ("GET", "/api/unhandled") if cfg.unhandled => Response::json("200 OK", &json!({ "calls": unhandled::snapshot() })),
            ("POST", "/api/register" | "/api/login") if cfg.accounts && !req.credentials_allowed() => {
                Response::json("403 Forbidden", &json!({ "error": rate_limit::TLS_REQUIRED }))
            }
            ("POST", "/api/register") if cfg.accounts => match rate_limit::registrations().check(req.peer) {
                true => register(&storage, &req.body),
                false => too_many(),
            },
            ("POST", "/api/login") if cfg.accounts => {
                let name = credentials(&req.body).map(|(u, _)| u).unwrap_or_default();
                if rate_limit::begin_login(req.peer, &name) {
                    let response = login(&storage, &req.body);
                    if response.status_is("401 Unauthorized") {
                        rate_limit::login_failed(req.peer, &name);
                    } else if response.status_is("200 OK") {
                        rate_limit::login_succeeded(req.peer, &name);
                    }
                    response
                } else {
                    too_many()
                }
            }
            _ => Response::status("404 Not Found"),
        })
    })
}

/// The most news items `/api/news` gives, and the longest title, text and link.
const MAX_NEWS: usize = 10;
const MAX_NEWS_TITLE: usize = 80;
const MAX_NEWS_TEXT: usize = 500;
const MAX_NEWS_LINK: usize = 300;

/// The news from `data/news.json`: title, text and link of each item. Read at
/// most once a minute (anyone may ask, as often as they like).
fn news() -> Value {
    static CACHE: std::sync::Mutex<Option<(std::time::Instant, Value)>> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, news)) = cache.as_ref().filter(|(at, _)| at.elapsed() < std::time::Duration::from_secs(60)) {
        return news.clone();
    }
    let items: Vec<Value> = std::fs::read_to_string("data/news.json")
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .unwrap_or_default();
    let text = |item: &Value, key: &str, max: usize| -> String { item[key].as_str().unwrap_or_default().chars().take(max).collect() };
    let news: Vec<Value> = items
        .iter()
        .filter(|item| item["title"].is_string())
        .take(MAX_NEWS)
        .map(|item| {
            json!({
                "title": text(item, "title", MAX_NEWS_TITLE),
                "text": text(item, "description", MAX_NEWS_TEXT),
                "link": text(item, "link", MAX_NEWS_LINK),
            })
        })
        .collect();
    let news = json!({ "news": news });
    *cache = Some((std::time::Instant::now(), news.clone()));
    news
}

fn info(cfg: CommunityApiConfig) -> Value {
    let mut features: Vec<&str> = FEATURES.to_vec();
    features.push("news");
    if cfg.presence {
        features.push("presence");
    }
    if cfg.accounts && !rate_limit::identity_required() {
        features.push("accounts");
    }
    if rate_limit::identity_required() {
        features.push("identity-required");
    }
    let mut info = json!({
        "name": env!("FE_PRODUCT"),
        "version": RELEASE,
        "revision": env!("FE_REVISION"),
        "features": features,
    });
    // The oldest launcher and client it lets play: launchers say so before signing in.
    if let Some(minimum) = crate::clients::minimum() {
        info["minimum_client"] = json!(minimum);
    }
    if let Some((id, mode, coordinator)) = FRIENDS.get() {
        info["id"] = json!(id);
        info["friends_mode"] = json!(mode);
        if let Some(coordinator) = coordinator {
            info["coordinator"] = json!(coordinator);
        }
    }
    if let Some((ports, host)) = PUBLIC.get() {
        info["ports"] = json!(ports);
        if let Some(host) = host {
            info["host"] = json!(host);
        }
    }
    info
}

/// Each registered player, online or not, and for online players what
/// they're doing: the match they're in (preferred) or their lobby.
fn presence(players: &[(String, bool)], sessions: &[LiveSession]) -> Value {
    let players: Vec<Value> = players
        .iter()
        .map(|(name, online)| {
            let mut p = json!({ "name": name, "online": online });
            if *online {
                if let Some(activity) = activity(name, sessions) {
                    p["activity"] = activity;
                }
            }
            p
        })
        .collect();
    json!({ "players": players })
}

/// What an online player is doing, from the live sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Activity {
    /// "svm" or "coop".
    pub mode: &'static str,
    /// "match" or "lobby".
    pub room: &'static str,
    /// The others in it.
    pub with: Vec<String>,
    pub map: Option<u32>,
}

/// `name`'s activity: the match they're in (preferred) or their lobby.
pub(crate) fn activity_of(name: &str, sessions: &[LiveSession]) -> Option<Activity> {
    // sessions are newest first; a match wins over the lobby around it.
    let mine = sessions.iter().filter(|s| s.players.iter().any(|p| p == name));
    let best = mine.min_by_key(|s| match attribute_value(&s.attributes, ATTR_ROOM_KIND) {
        Some(0) => 0,
        Some(1) => 1,
        _ => 2,
    })?;
    Some(Activity {
        // A lobby serves either mode; clients up to 0.4.0 show any mode, so lobbies
        // stay "coop" as they always were.
        mode: crate::game_session::match_mode(&best.attributes).unwrap_or("coop"),
        room: match attribute_value(&best.attributes, ATTR_ROOM_KIND) {
            Some(0) => "match",
            _ => "lobby",
        },
        with: best.players.iter().filter(|p| *p != name).cloned().collect(),
        map: attribute_value(&best.attributes, ATTR_MAP),
    })
}

fn activity(name: &str, sessions: &[LiveSession]) -> Option<Value> {
    let a = activity_of(name, sessions)?;
    let mut v = json!({ "mode": a.mode, "room": a.room, "with": a.with });
    if let Some(map) = a.map {
        v["map"] = json!(map);
    }
    Some(v)
}

/// Username and password from a JSON body.
fn credentials(body: &[u8]) -> Result<(String, String), Response> {
    let v: Value = serde_json::from_slice(body).map_err(|_| bad("send JSON {\"username\",\"password\"}"))?;
    let username = v["username"].as_str().unwrap_or_default().trim().to_string();
    let password = v["password"].as_str().unwrap_or_default().to_string();
    if username.is_empty() || username.chars().count() > 32 || username.chars().any(char::is_control) {
        return Err(bad("username must be 1-32 characters"));
    }
    if password.len() < 8 || password.len() > 128 {
        return Err(bad("password must be 8-128 characters"));
    }
    Ok((username, password))
}

fn bad(msg: &str) -> Response {
    Response::json("400 Bad Request", &json!({ "error": msg }))
}

/// An internal error: the details go to the log, never to the client.
fn internal(e: impl std::fmt::Display) -> Response {
    eprintln!("Community API internal error: {e}");
    Response::json("500 Internal Server Error", &json!({ "error": "internal error" }))
}

fn busy() -> Response {
    Response::json("503 Service Unavailable", &json!({ "error": "the server is busy; try again in a moment" }))
}

fn too_many() -> Response {
    Response::json("429 Too Many Requests", &json!({ "error": "too many attempts, try again later" }))
}

fn register(storage: &Storage, body: &[u8]) -> Response {
    let (username, password) = match credentials(body) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(why) = crate::api::check_username(&username) {
        return bad(why);
    }
    if !crate::rate_limit::registration_open() {
        return Response::json("403 Forbidden", &json!({ "error": "this server doesn't take new accounts" }));
    }
    // Accounts here are linked to an identity, which only the launcher has.
    if crate::rate_limit::identity_required() {
        return Response::json("403 Forbidden", &json!({ "error": "accounts on this server are made with the 5th Echelon launcher" }));
    }
    if password.len() > crate::api::MAX_PASSWORD {
        return bad("password must be 8-63 characters (the game's limit)");
    }
    // On servers sharing friends, a name another player holds there is theirs.
    if crate::storage::run(crate::federation::name_holder(&username)).ok() == Some(crate::federation::NameCheck::Taken) {
        return Response::json("409 Conflict", &json!({ "error": "that name belongs to another player on the servers sharing friends" }));
    }
    // The account id is the name, unless a renamed account still has it.
    let ubi_id = match crate::storage::run(storage.free_ubi_id(&username)) {
        Ok(Ok(id)) => id,
        Ok(Err(e)) | Err(e) => return internal(e),
    };
    match storage.register_user(&username, &password, Some(&ubi_id)) {
        Ok(()) => Response::json("200 OK", &json!({ "ok": true })),
        Err(e) if e.downcast_ref::<sqlx::Error>().and_then(|e| e.as_database_error()).is_some_and(|e| e.is_unique_violation()) => {
            Response::json("409 Conflict", &json!({ "error": "username taken" }))
        }
        Err(e) if e.is::<crate::storage::Busy>() => busy(),
        Err(e) => internal(e),
    }
}

fn login(storage: &Storage, body: &[u8]) -> Response {
    let (username, password) = match credentials(body) {
        Ok(c) => c,
        Err(r) => return r,
    };
    match storage.login_user(&username, &password) {
        // The server's own accounts don't sign in here.
        Ok(Ok(id)) if !crate::storage::run(storage.is_player_account(id)).ok().and_then(Result::ok).unwrap_or(false) => {
            Response::json("401 Unauthorized", &json!({ "error": "wrong username or password" }))
        }
        Ok(Ok(_)) => Response::json("200 OK", &json!({ "ok": true })),
        // One answer for unknown user and wrong password.
        Ok(Err(LoginError::NotFound | LoginError::InvalidPassword)) => Response::json("401 Unauthorized", &json!({ "error": "wrong username or password" })),
        Err(e) if e.is::<crate::storage::Busy>() => busy(),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;
    use crate::storage::tests::temp_storage;

    const ALL_ON: CommunityApiConfig = CommunityApiConfig {
        info: true,
        presence: true,
        accounts: true,
        unhandled: true,
    };

    fn req(method: &str, path: &str, body: &str) -> Request {
        Request {
            line: format!("{method} {path} HTTP/1.1\r\n"),
            method: method.into(),
            path: path.into(),
            body: body.as_bytes().to_vec(),
            peer: Some(IpAddr::from([192, 0, 2, 1])),
            conn_peer: Some(IpAddr::from([192, 0, 2, 1])),
            forwarded_proto: None,
        }
    }

    fn call(routes: &Routes, r: Request) -> (String, Value) {
        let resp = routes(&r).expect("route");
        let bytes = resp.to_bytes_for_test();
        let text = String::from_utf8(bytes).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (head.lines().next().unwrap().to_string(), serde_json::from_str(body).unwrap_or(Value::Null))
    }

    #[test]
    fn info_reports_the_ports_players_use() {
        let (storage, dir) = temp_storage("api-ports");
        publish(
            PublicPorts {
                api: 80,
                login: 31126,
                secure: 31127,
                content: 80,
                nat: Some(31128),
                api_tls: None,
            },
            Some("blacklist.example.com".into()),
        );
        let routes = routes(Arc::new(storage), CommunityApiConfig::default());
        let (_, v) = call(&routes, req("GET", "/api/info", ""));
        assert_eq!(v["ports"], json!({ "api": 80, "login": 31126, "secure": 31127, "content": 80, "nat": 31128 }));
        assert_eq!(v["host"], "blacklist.example.com");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn info_names_release_and_features() {
        let (storage, dir) = temp_storage("api-info");
        let routes = routes(Arc::new(storage), ALL_ON);
        let (status, v) = call(&routes, req("GET", "/api/info", ""));
        assert!(status.contains("200"));
        assert_eq!(v["version"], RELEASE);
        let features = v["features"].as_array().unwrap();
        assert!(features.iter().any(|f| f == "invites"));
        assert!(features.iter().any(|f| f == "presence") && features.iter().any(|f| f == "accounts"));
        // Everything else is still the game's config.
        assert!(routes(&req("GET", "/OnlineConfigService.svc/GetOnlineConfig", "")).is_none());
        assert!(routes(&req("GET", "/apiary", "")).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_info_is_on_by_default() {
        let (storage, dir) = temp_storage("api-default");
        let routes = routes(Arc::new(storage), CommunityApiConfig::default());
        let (status, v) = call(&routes, req("GET", "/api/info", ""));
        assert!(status.contains("200"));
        let features = v["features"].as_array().unwrap();
        assert!(!features.iter().any(|f| f == "presence" || f == "accounts"), "switched-off parts aren't advertised");
        let creds = r#"{"username":"Kiwi","password":"correct horse battery"}"#;
        for (method, path, body) in [
            ("GET", "/api/presence", ""),
            ("GET", "/api/unhandled", ""),
            ("POST", "/api/register", creds),
            ("POST", "/api/login", creds),
        ] {
            assert!(call(&routes, req(method, path, body)).0.contains("404"), "{path} is off by default");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unhandled_lists_the_calls_the_server_could_not_answer() {
        let (storage, dir) = temp_storage("api-unhandled");
        let routes = routes(Arc::new(storage), ALL_ON);
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        unhandled::record(&logger, 64998, 7, None, None, unhandled::Kind::UnknownProtocol);
        let (status, v) = call(&routes, req("GET", "/api/unhandled", ""));
        assert!(status.contains("200"));
        let call = v["calls"].as_array().unwrap().iter().find(|c| c["protocol_id"] == 64998).unwrap().clone();
        assert_eq!(call["method_id"], 7);
        assert_eq!(call["kind"], "unknown_protocol");
        assert_eq!(call["count"], 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn one_click_accounts() {
        let (storage, dir) = temp_storage("api-accounts");
        let storage = Arc::new(storage);
        let routes = routes(Arc::clone(&storage), ALL_ON);
        let creds = r#"{"username":"Kiwi","password":"correct horse battery"}"#;
        assert!(call(&routes, req("POST", "/api/register", creds)).0.contains("200"));
        assert!(call(&routes, req("POST", "/api/register", creds)).0.contains("409"), "names are unique");
        assert!(call(&routes, req("POST", "/api/login", creds)).0.contains("200"));
        let wrong = r#"{"username":"Kiwi","password":"wrong password"}"#;
        let unknown = r#"{"username":"Nobody","password":"wrong password"}"#;
        let (s1, v1) = call(&routes, req("POST", "/api/login", wrong));
        let (s2, v2) = call(&routes, req("POST", "/api/login", unknown));
        assert!(s1.contains("401") && s1 == s2 && v1 == v2, "same answer for wrong password and unknown user");
        assert!(call(&routes, req("POST", "/api/register", r#"{"username":"x","password":"short"}"#)).0.contains("400"));
        // The account works like one registered in the launcher: ubi id = username.
        assert!(storage.find_user_by_ubi_id("Kiwi").unwrap().is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn registering_is_rate_limited_per_address() {
        let (storage, dir) = temp_storage("api-ratelimit");
        let routes = routes(Arc::new(storage), ALL_ON);
        let status = |name: &str, peer: [u8; 4]| {
            let mut r = req("POST", "/api/register", &format!(r#"{{"username":"{name}","password":"correct horse battery"}}"#));
            r.peer = Some(IpAddr::from(peer));
            call(&routes, r).0
        };
        for i in 0..rate_limit::registrations().max {
            assert!(status(&format!("P{i}"), [198, 51, 100, 1]).contains("200"));
        }
        assert!(status("Late", [198, 51, 100, 1]).contains("429"));
        assert!(status("Other", [198, 51, 100, 2]).contains("200"), "another address has its own budget");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn presence_prefers_the_match_over_its_lobby() {
        let sessions = vec![
            LiveSession {
                id: 9,
                attributes: "113 => 1;103 => 1;101 => 4".into(),
                players: vec!["Kiwi".into()],
            },
            LiveSession {
                id: 8,
                attributes: "113 => 0;103 => 1;101 => 7".into(),
                players: vec!["Kiwi".into(), "Tank".into()],
            },
            LiveSession {
                id: 3,
                attributes: "113 => 1;101 => 2".into(),
                players: vec!["Nexus".into()],
            },
        ];
        let players = vec![
            ("Kiwi".to_string(), true),
            ("Nexus".to_string(), true),
            ("Tank".to_string(), false),
            ("Zed".to_string(), true),
        ];
        let v = presence(&players, &sessions);
        let p = |name: &str| v["players"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap().clone();
        assert_eq!(p("Kiwi")["activity"], json!({"mode": "svm", "room": "match", "map": 7, "with": ["Tank"]}));
        assert_eq!(p("Nexus")["activity"]["mode"], "coop");
        assert_eq!(p("Nexus")["activity"]["room"], "lobby");
        assert!(p("Tank").get("activity").is_none(), "offline players show no activity");
        assert!(p("Zed").get("activity").is_none(), "online but not in a session");
    }
}
