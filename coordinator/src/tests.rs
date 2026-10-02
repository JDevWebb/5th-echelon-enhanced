use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

use super::*;

struct Test {
    router: Router,
    dir: std::path::PathBuf,
}

impl Drop for Test {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(name: &str) -> Test {
    let dir = std::env::temp_dir().join(format!("fe-coord-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let c = Coordinator::open(&dir.join("c.db").to_string_lossy(), "TOKEN".into()).await.unwrap();
    Test {
        router: Arc::new(c).router().layer(axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from(([192, 0, 2, 1], 1)))),
        dir,
    }
}

impl Test {
    async fn call(&self, method: &str, path: &str, secret: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path).header("content-type", "application/json");
        if let Some(s) = secret {
            req = req.header("authorization", format!("Bearer {s}"));
        }
        let req = req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap();
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Joins as `server_id`, going by the host name `server_id` too.
    async fn join(&self, server_id: &str) -> String {
        let (status, v) = self.call("POST", "/v1/join", None, Some(json!({ "token": "TOKEN", "server_id": server_id }))).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        let secret = v["secret"].as_str().unwrap().to_string();
        let (status, v) = self
            .call("POST", "/v1/heartbeat", Some(&secret), Some(json!({ "name": server_id, "host": server_id, "names": [server_id], "listed": false })))
            .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        secret
    }

    async fn changes(&self, secret: &str, changes: Value) -> Vec<Value> {
        let (status, v) = self.call("POST", "/v1/changes", Some(secret), Some(json!({ "changes": changes }))).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        v["results"].as_array().unwrap().clone()
    }

    async fn relations(&self, secret: &str, global_id: &str) -> (StatusCode, Value) {
        self.call("GET", &format!("/v1/relations/{global_id}"), Some(secret), None).await
    }
}

fn link(who: &identity::Identity, host: &str, username: &str) -> Value {
    json!({ "op": "link", "global_id": who.global_id(), "username": username, "host": host, "time": 1000, "signature": who.sign_link(host, username, 1000) })
}

#[tokio::test]
async fn joining_needs_the_token() {
    let t = start("join").await;
    let (status, _) = t.call("POST", "/v1/join", None, Some(json!({ "token": "WRONG", "server_id": "a" }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = t.call("POST", "/v1/heartbeat", Some("nope"), Some(json!({ "name": "A", "host": "a.example" }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let secret = t.join("server-a").await;
    let (status, _) = t.call("POST", "/v1/heartbeat", Some(&secret), Some(json!({ "name": "A", "host": "server-a", "listed": true }))).await;
    assert_eq!(status, StatusCode::OK);
    // Joining again as an existing server needs its current secret.
    let (status, _) = t.call("POST", "/v1/join", None, Some(json!({ "token": "TOKEN", "server_id": "server-a" }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "anyone with the token could take a member over");
    let (status, v) = t.call("POST", "/v1/join", Some(&secret), Some(json!({ "token": "TOKEN", "server_id": "server-a" }))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn listings_are_checked() {
    let t = start("listing").await;
    let secret = t.join("server-a").await;
    for bad in [json!([]), json!("x"), json!({ "name": "A\u{7}", "host": "a.example" }), json!({ "name": "A", "host": "a b" }), json!({ "name": "A", "host": "a.example", "ports": { "api": 0, "login": 1 } })] {
        let (status, _) = t.call("POST", "/v1/heartbeat", Some(&secret), Some(bad.clone())).await;
        assert!(status.is_client_error(), "{bad} was taken ({status})");
    }
    let (status, _) = t.call("GET", "/v1/servers", None, None).await;
    assert_eq!(status, StatusCode::OK, "the directory still answers");
}

#[tokio::test]
async fn a_server_only_uses_its_own_names() {
    let t = start("names-own").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    // B says it's server-a too: ignored, the name is A's.
    t.call("POST", "/v1/heartbeat", Some(&b), Some(json!({ "name": "B", "host": "server-b", "names": ["server-a"] }))).await;
    let kiwi = identity::Identity::generate();
    // A signature players made for A (server-a), brought by B.
    let r = t.changes(&b, json!([link(&kiwi, "server-a", "Kiwi")])).await;
    assert!(r[0]["error"].as_str().unwrap().contains("isn't one of this server's names"), "{r:?}");
    let r = t.changes(&a, json!([link(&kiwi, "server-a", "Kiwi")])).await;
    assert!(r[0].get("error").is_none(), "{r:?}");
}

#[tokio::test]
async fn directory_lists_live_listed_servers_busiest_first() {
    let t = start("directory").await;
    for (id, players, listed) in [("quiet", 1, true), ("busy", 40, true), ("hidden", 99, false)] {
        let secret = t.join(id).await;
        t.call("POST", "/v1/heartbeat", Some(&secret), Some(json!({ "name": id, "host": id, "listed": listed, "players_online": players }))).await;
    }
    t.join("never-heard-from").await;
    let (_, v) = t.call("GET", "/v1/servers", None, None).await;
    let names: Vec<&str> = v["servers"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["busy", "quiet"]);
    assert_eq!(v["servers"][0]["id"], "busy");
}

#[tokio::test]
async fn friends_made_on_one_server_reach_another() {
    let t = start("sync").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    let (kiwi, tank) = (identity::Identity::generate(), identity::Identity::generate());

    // Both play on A and become friends there.
    let r = t
        .changes(&a, json!([link(&kiwi, "server-a", "Kiwi"), link(&tank, "server-a", "Tank"), { "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": true }]))
        .await;
    assert!(r.iter().all(|r| r.get("error").is_none()), "{r:?}");

    // Kiwi links on B: B may ask for Kiwi's friends, and gets Tank.
    let (status, _) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "not linked on B yet");
    t.changes(&b, json!([link(&kiwi, "server-b", "Kiwi")])).await;
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(v["relations"], json!([{ "other": tank.global_id(), "friends": true, "blocked": false, "blocked_by": false }]));

    // B can't act for Tank, who never linked there.
    let r = t.changes(&b, json!([{ "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": false }])).await;
    assert!(r[0]["error"].as_str().unwrap().contains("isn't linked"));

    // A block on A ends the friendship everywhere, and a friendship can't come back past it.
    t.changes(&a, json!([{ "op": "block", "from": tank.global_id(), "to": kiwi.global_id(), "blocked": true }])).await;
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(v["relations"], json!([{ "other": tank.global_id(), "friends": false, "blocked": false, "blocked_by": true }]));
    let r = t.changes(&a, json!([{ "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": true }])).await;
    assert!(r[0]["error"].as_str().unwrap().contains("blocked"));
}

#[tokio::test]
async fn friends_see_which_other_server_a_friend_is_on() {
    let t = start("presence").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    let (kiwi, tank, pest) = (identity::Identity::generate(), identity::Identity::generate(), identity::Identity::generate());
    t.changes(&a, json!([link(&kiwi, "server-a", "Kiwi"), link(&pest, "server-a", "Pest"), { "op": "friends", "a": kiwi.global_id(), "b": pest.global_id(), "friends": true }]))
        .await;
    // Tank plays on B only, under another name there; Kiwi and Tank are friends.
    t.changes(&b, json!([link(&tank, "server-b", "TankB"), link(&kiwi, "server-b", "Kiwi"), { "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": true }]))
        .await;
    let beat = |online: Vec<String>, name: &str| json!({ "name": name, "host": name.to_lowercase(), "region": "Oceania", "online": online });

    // B says Tank is online, and Pest too, who isn't B's to speak for.
    let (status, v) = t.call("POST", "/v1/heartbeat", Some(&b), Some(beat(vec![tank.global_id(), pest.global_id()], "Server-B"))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let (_, v) = t.relations(&a, &kiwi.global_id()).await;
    let rel = |v: &Value, who: &identity::Identity| v["relations"].as_array().unwrap().iter().find(|r| r["other"] == json!(who.global_id())).cloned().unwrap();
    assert_eq!(rel(&v, &tank)["elsewhere"], json!({ "username": "TankB", "server": "Server-B", "region": "Oceania", "host": "server-b" }));
    assert!(rel(&v, &pest).get("elsewhere").is_none(), "B spoke for a player it doesn't have");

    // Asked on B itself, B's own players aren't "elsewhere".
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert!(rel(&v, &tank).get("elsewhere").is_none());

    // Off B's list: offline there.
    t.call("POST", "/v1/heartbeat", Some(&b), Some(beat(vec![], "Server-B"))).await;
    let (_, v) = t.relations(&a, &kiwi.global_id()).await;
    assert!(rel(&v, &tank).get("elsewhere").is_none());

    // Only friends see it.
    t.call("POST", "/v1/heartbeat", Some(&b), Some(beat(vec![tank.global_id()], "Server-B"))).await;
    t.changes(&b, json!([{ "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": false }])).await;
    let (_, v) = t.relations(&a, &kiwi.global_id()).await;
    assert!(rel(&v, &tank).get("elsewhere").is_none());
}

#[tokio::test]
async fn links_need_the_players_signature_for_that_server() {
    let t = start("links").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    let kiwi = identity::Identity::generate();
    // A link signed for server A, replayed by server B: not B's host.
    let r = t.changes(&b, json!([link(&kiwi, "server-a", "Kiwi")])).await;
    assert!(r[0]["error"].as_str().unwrap().contains("isn't one of this server's names"), "{r:?}");
    // Someone else's key can't be claimed for another name either.
    let mut forged = link(&kiwi, "server-a", "Kiwi");
    forged["username"] = json!("Impostor");
    let r = t.changes(&a, json!([forged])).await;
    assert!(r[0]["error"].as_str().unwrap().contains("signature"));
    let r = t.changes(&a, json!([link(&kiwi, "server-a", "Kiwi"), { "op": "unlink", "global_id": kiwi.global_id() }])).await;
    assert!(r.iter().all(|r| r.get("error").is_none()));
    let (status, _) = t.relations(&a, &kiwi.global_id()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "unlinked");
}

#[tokio::test]
async fn names_belong_to_one_player_across_the_group() {
    let t = start("names").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    let (kiwi, other) = (identity::Identity::generate(), identity::Identity::generate());
    let claim = |who: &identity::Identity, host: &str, name: &str| {
        json!({ "name": name, "global_id": who.global_id(), "host": host, "time": 5, "signature": who.sign_link(host, name, 5) })
    };

    // Kiwi takes the name on A; nobody else gets it on B, whatever the case; Kiwi does.
    let (status, _) = t.call("POST", "/v1/names/claim", Some(&a), Some(claim(&kiwi, "server-a", "Kiwi"))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = t.call("POST", "/v1/names/claim", Some(&b), Some(claim(&other, "server-b", "KIWI"))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = t.call("POST", "/v1/names/claim", Some(&b), Some(claim(&kiwi, "server-b", "kiwi"))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, v) = t.call("GET", "/v1/names/kIwI", Some(&b), None).await;
    assert_eq!(v, json!({ "claimed": true }), "taken, but whose isn't said");

    // A claim signed for another server doesn't count.
    let (status, _) = t.call("POST", "/v1/names/claim", Some(&b), Some(claim(&other, "server-a", "Fresh"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Names follow the players' rules.
    let (status, _) = t.call("POST", "/v1/names/claim", Some(&b), Some(claim(&other, "server-b", "bad name"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Linking an account whose name someone else holds: linked, but flagged.
    let r = t.changes(&b, json!([link(&other, "server-b", "Kiwi")])).await;
    assert_eq!(r[0]["conflict"], true);
    let r = t.changes(&a, json!([link(&kiwi, "server-a", "Kiwi")])).await;
    assert_eq!(r[0]["conflict"], false);
}

#[tokio::test]
async fn links_made_before_names_existed_reserve_theirs() {
    let dir = std::env::temp_dir().join(format!("fe-coord-backfill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("c.db").to_string_lossy().to_string();
    let first = identity::Identity::generate();
    {
        let c = Coordinator::open(&db, "T".into()).await.unwrap();
        sqlx::query("INSERT INTO servers (id, secret_hash, joined_at) VALUES ('s', 'h', 0)").execute(&c.pool).await.unwrap();
        sqlx::query("INSERT INTO links VALUES (?, 's', 'Old', 1)").bind(first.global_id()).execute(&c.pool).await.unwrap();
        sqlx::query("INSERT INTO links VALUES ('ZZZ', 's', 'old', 2)").execute(&c.pool).await.unwrap();
        sqlx::query("DELETE FROM names").execute(&c.pool).await.unwrap();
    }
    let c = Coordinator::open(&db, "T".into()).await.unwrap();
    assert_eq!(c.owner("old").await.unwrap(), Some(first.global_id()), "the older link keeps it");
    let _ = std::fs::remove_dir_all(dir);
}
