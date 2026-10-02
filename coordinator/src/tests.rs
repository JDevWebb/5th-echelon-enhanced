use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

use super::*;

struct Test {
    router: Router,
    c: Arc<Coordinator>,
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
    let c = Arc::new(Coordinator::open(&dir.join("c.db").to_string_lossy(), "TOKEN".into()).await.unwrap());
    Test {
        router: Arc::clone(&c)
            .router()
            .layer(axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from(([192, 0, 2, 1], 1)))),
        c,
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
            .call(
                "POST",
                "/v1/heartbeat",
                Some(&secret),
                Some(json!({ "name": server_id, "host": server_id, "names": [server_id], "listed": false })),
            )
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
    link_at(who, host, username, identity::now())
}

fn link_at(who: &identity::Identity, host: &str, username: &str, time: i64) -> Value {
    json!({ "op": "link", "global_id": who.global_id(), "username": username, "host": host, "time": time, "signature": who.sign_link(host, username, time) })
}

/// A heartbeat for the server `name` (its host the name in lower case), with these players online.
fn beat(name: &str, online: &[&identity::Identity]) -> Value {
    json!({ "name": name, "host": name.to_lowercase(), "region": "Oceania", "online": online.iter().map(|p| p.global_id()).collect::<Vec<_>>() })
}

/// `who`'s entry in a relations answer.
fn rel(v: &Value, who: &identity::Identity) -> Value {
    v["relations"].as_array().unwrap().iter().find(|r| r["other"] == json!(who.global_id())).cloned().unwrap()
}

#[tokio::test]
async fn joining_needs_the_token() {
    let t = start("join").await;
    let (status, _) = t.call("POST", "/v1/join", None, Some(json!({ "token": "WRONG", "server_id": "a" }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = t.call("POST", "/v1/heartbeat", Some("nope"), Some(json!({ "name": "A", "host": "a.example" }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let secret = t.join("server-a").await;
    let (status, _) = t
        .call("POST", "/v1/heartbeat", Some(&secret), Some(json!({ "name": "A", "host": "server-a", "listed": true })))
        .await;
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
    for bad in [
        json!([]),
        json!("x"),
        json!({ "name": "A\u{7}", "host": "a.example" }),
        json!({ "name": "A\u{202e}B", "host": "a.example" }),
        json!({ "name": "A", "host": "a b" }),
        json!({ "name": "A", "host": "a.example", "ports": { "api": 0, "login": 1 } }),
        json!({ "name": "A", "host": "a.example", "ports": { "api": 80, "login": 1, "api_tls": 0 } }),
        json!({ "name": "A", "host": "a.example", "online": ["not-an-identity"] }),
    ] {
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
    t.call("POST", "/v1/heartbeat", Some(&b), Some(json!({ "name": "B", "host": "server-b", "names": ["server-a"] })))
        .await;
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
        t.call(
            "POST",
            "/v1/heartbeat",
            Some(&secret),
            Some(json!({ "name": id, "host": id, "listed": listed, "players_online": players })),
        )
        .await;
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
        .changes(
            &a,
            json!([link(&kiwi, "server-a", "Kiwi"), link(&tank, "server-a", "Tank"), { "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": true }]),
        )
        .await;
    assert!(r.iter().all(|r| r.get("error").is_none()), "{r:?}");

    // Kiwi links on B: B may ask for Kiwi's friends, and gets Tank.
    let (status, _) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "not linked on B yet");
    t.changes(&b, json!([link(&kiwi, "server-b", "Kiwi")])).await;
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(
        v["relations"],
        json!([{ "other": tank.global_id(), "friends": true, "blocked": false, "blocked_by": false }])
    );

    // B can't act for Tank, who never linked there.
    let r = t
        .changes(&b, json!([{ "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": false }]))
        .await;
    assert!(r[0]["error"].as_str().unwrap().contains("isn't linked"));

    // A block on A ends the friendship everywhere, and a friendship can't come back past it.
    t.changes(&a, json!([{ "op": "block", "from": tank.global_id(), "to": kiwi.global_id(), "blocked": true }]))
        .await;
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(
        v["relations"],
        json!([{ "other": tank.global_id(), "friends": false, "blocked": false, "blocked_by": true }])
    );
    let r = t
        .changes(&a, json!([{ "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": true }]))
        .await;
    assert!(r[0]["error"].as_str().unwrap().contains("blocked"));
}

#[tokio::test]
async fn friends_see_which_other_server_a_friend_is_on() {
    let t = start("presence").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    let (kiwi, tank, pest) = (identity::Identity::generate(), identity::Identity::generate(), identity::Identity::generate());
    t.changes(
        &a,
        json!([link(&kiwi, "server-a", "Kiwi"), link(&pest, "server-a", "Pest"), { "op": "friends", "a": kiwi.global_id(), "b": pest.global_id(), "friends": true }]),
    )
    .await;
    // Tank plays on B only, under another name there; Kiwi and Tank became friends there.
    t.changes(
        &b,
        json!([link(&tank, "server-b", "TankB"), link(&kiwi, "server-b", "Kiwi"), { "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": true }]),
    )
    .await;
    t.call("POST", "/v1/heartbeat", Some(&b), Some(beat("Server-B", &[&kiwi, &tank]))).await;

    // B says Tank is online, and Pest too, who isn't B's to speak for.
    let (status, v) = t.call("POST", "/v1/heartbeat", Some(&b), Some(beat("Server-B", &[&tank, &pest]))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let (_, v) = t.relations(&a, &kiwi.global_id()).await;
    assert_eq!(
        rel(&v, &tank)["elsewhere"],
        json!({ "username": "TankB", "server": "Server-B", "region": "Oceania", "host": "server-b" })
    );
    assert!(rel(&v, &pest).get("elsewhere").is_none(), "B spoke for a player it doesn't have");

    // Asked on B itself, B's own players aren't "elsewhere".
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert!(rel(&v, &tank).get("elsewhere").is_none());

    // Off B's list: offline there.
    t.call("POST", "/v1/heartbeat", Some(&b), Some(beat("Server-B", &[]))).await;
    let (_, v) = t.relations(&a, &kiwi.global_id()).await;
    assert!(rel(&v, &tank).get("elsewhere").is_none());

    // Only friends see it.
    t.call("POST", "/v1/heartbeat", Some(&b), Some(beat("Server-B", &[&tank]))).await;
    t.changes(&b, json!([{ "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": false }]))
        .await;
    let (_, v) = t.relations(&a, &kiwi.global_id()).await;
    assert!(rel(&v, &tank).get("elsewhere").is_none());
}

#[tokio::test]
async fn another_server_cant_hide_where_a_friend_plays() {
    let t = start("presence-rogue").await;
    let (a, b, r) = (t.join("server-a").await, t.join("server-b").await, t.join("server-r").await);
    let (kiwi, tank) = (identity::Identity::generate(), identity::Identity::generate());
    // Tank once visited R; plays on A, where Kiwi and Tank are friends. Kiwi is on B now.
    t.changes(&r, json!([link(&tank, "server-r", "Tank")])).await;
    t.changes(
        &a,
        json!([link(&kiwi, "server-a", "Kiwi"), link(&tank, "server-a", "Tank"), { "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": true }]),
    )
    .await;
    t.changes(&b, json!([link(&kiwi, "server-b", "Kiwi")])).await;
    sqlx::query("UPDATE links SET linked_at = linked_at - 100 WHERE server_id = 'server-r'")
        .execute(&t.c.pool)
        .await
        .unwrap();
    t.call("POST", "/v1/heartbeat", Some(&a), Some(beat("Server-A", &[&kiwi, &tank]))).await;
    // R says Tank is on R: A's word stands too, and Tank linked on A last.
    t.call("POST", "/v1/heartbeat", Some(&r), Some(beat("Server-R", &[&tank]))).await;
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(rel(&v, &tank)["elsewhere"]["server"], "Server-A");
    // Once A no longer lists Tank, R's word is all there is.
    t.call("POST", "/v1/heartbeat", Some(&a), Some(beat("Server-A", &[&kiwi]))).await;
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(rel(&v, &tank)["elsewhere"]["server"], "Server-R");
    // An unlisted server isn't given out.
    let mut hidden = beat("Server-R", &[&tank]);
    hidden["listed"] = json!(false);
    t.call("POST", "/v1/heartbeat", Some(&r), Some(hidden)).await;
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert!(rel(&v, &tank).get("elsewhere").is_none());
}

#[tokio::test]
async fn friendships_count_where_both_were_seen_online() {
    let t = start("presence-vouched").await;
    let (a, b, r) = (t.join("server-a").await, t.join("server-b").await, t.join("server-r").await);
    let (kiwi, tank) = (identity::Identity::generate(), identity::Identity::generate());
    // Both linked on R once, so R can say they're friends.
    t.changes(
        &r,
        json!([link(&kiwi, "server-r", "Kiwi"), link(&tank, "server-r", "Tank"), { "op": "friends", "a": kiwi.global_id(), "b": tank.global_id(), "friends": true }]),
    )
    .await;
    t.changes(&a, json!([link(&tank, "server-a", "Tank")])).await;
    t.changes(&b, json!([link(&kiwi, "server-b", "Kiwi")])).await;
    t.call("POST", "/v1/heartbeat", Some(&a), Some(beat("Server-A", &[&tank]))).await;
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(rel(&v, &tank)["friends"], true);
    assert!(rel(&v, &tank).get("elsewhere").is_none(), "R never had them online");
    // R can still say they were: this only makes it say more.
    t.call("POST", "/v1/heartbeat", Some(&r), Some(beat("Server-R", &[&kiwi, &tank]))).await;
    t.call("POST", "/v1/heartbeat", Some(&r), Some(beat("Server-R", &[]))).await;
    let (_, v) = t.relations(&b, &kiwi.global_id()).await;
    assert_eq!(rel(&v, &tank)["elsewhere"]["server"], "Server-A");
}

#[tokio::test]
async fn links_are_limited_per_server() {
    let t = start("link-limits").await;
    let a = t.join("server-a").await;
    // An old signature isn't taken, nor one from the future.
    let kiwi = identity::Identity::generate();
    let r = t.changes(&a, json!([link_at(&kiwi, "server-a", "Kiwi", identity::now() - LINK_SIGNED_FOR - 1)])).await;
    assert!(r[0]["error"].as_str().unwrap().contains("too old"), "{r:?}");
    let r = t.changes(&a, json!([link_at(&kiwi, "server-a", "Kiwi", identity::now() + 3600)])).await;
    assert!(r[0]["error"].as_str().unwrap().contains("too old"), "{r:?}");
    let first = link(&kiwi, "server-a", "Kiwi");
    let mut many = vec![first.clone()];
    for i in 1..MAX_NEW_LINKS_PER_HOUR {
        many.push(link(&identity::Identity::generate(), "server-a", &format!("P{i}")));
    }
    let r = t.changes(&a, json!(many)).await;
    assert!(r.iter().all(|r| r.get("error").is_none()));
    // One more this hour: refused whole, so the server sends it again later.
    let (status, _) = t
        .call(
            "POST",
            "/v1/changes",
            Some(&a),
            Some(json!({ "changes": [link(&identity::Identity::generate(), "server-a", "Late")] })),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // A link it has already (a retry) is no new one.
    let r = t.changes(&a, json!([first])).await;
    assert!(r[0].get("error").is_none(), "{r:?}");
}

#[tokio::test]
async fn unused_names_can_be_released() {
    let t = start("purge").await;
    let a = t.join("server-a").await;
    let (kiwi, squat) = (identity::Identity::generate(), identity::Identity::generate());
    t.changes(&a, json!([link(&kiwi, "server-a", "Kiwi"), link(&squat, "server-a", "Squat")])).await;
    t.call("POST", "/v1/heartbeat", Some(&a), Some(beat("server-a", &[&kiwi]))).await;
    assert_eq!(t.c.purge_unused_names("server-a").await.unwrap(), 0, "only links over an hour old");
    sqlx::query("UPDATE links SET linked_at = linked_at - 7200").execute(&t.c.pool).await.unwrap();
    sqlx::query("UPDATE names SET claimed_at = claimed_at - 7200").execute(&t.c.pool).await.unwrap();
    assert_eq!(t.c.purge_unused_names("server-a").await.unwrap(), 1);
    assert_eq!(t.c.owner("squat").await.unwrap(), None, "the name is free again");
    assert_eq!(t.c.owner("kiwi").await.unwrap(), Some(kiwi.global_id()), "a player seen online keeps theirs");
}

#[tokio::test]
async fn heartbeats_and_metrics_are_rate_limited() {
    let t = start("rates").await;
    let a = t.join("server-a").await;
    let mut statuses = Vec::new();
    for _ in 0..6 {
        statuses.push(t.call("POST", "/v1/heartbeat", Some(&a), Some(beat("server-a", &[]))).await.0);
    }
    assert_eq!(statuses.iter().filter(|s| **s == StatusCode::OK).count(), 5, "the join's heartbeat counts: {statuses:?}");
    assert_eq!(statuses[5], StatusCode::TOO_MANY_REQUESTS);
    let report = json!({ "metrics": { "players": { "online": 1 } }, "update": {} });
    assert_eq!(t.call("POST", "/v1/metrics", Some(&a), Some(report.clone())).await.0, StatusCode::OK);
    assert_eq!(t.call("POST", "/v1/metrics", Some(&a), Some(report.clone())).await.0, StatusCode::OK);
    assert_eq!(t.call("POST", "/v1/metrics", Some(&a), Some(report)).await.0, StatusCode::TOO_MANY_REQUESTS);
    let chart = t.c.series(3600).await.unwrap();
    assert_eq!(chart["points"]["server-a"][0]["players"], 1.0, "{chart}");
}

#[tokio::test]
async fn a_servers_names_are_limited_and_clashes_reported() {
    let t = start("server-names").await;
    let (_a, b) = (t.join("server-a").await, t.join("server-b").await);
    let (_, v) = t
        .call("POST", "/v1/heartbeat", Some(&b), Some(json!({ "name": "B", "host": "server-b", "names": ["server-a"] })))
        .await;
    assert!(v["warnings"][0].as_str().unwrap().contains("server-a is another member server's name"), "{v}");
    assert!(t.c.name_clashes.lock().unwrap()["server-b"][0].contains("server-a"));
    for round in 0..2 {
        let names: Vec<String> = (0..16).map(|i| format!("b{round}-{i}.example")).collect();
        let (_, v) = t
            .call("POST", "/v1/heartbeat", Some(&b), Some(json!({ "name": "B", "host": "server-b", "names": names })))
            .await;
        assert_eq!(v.get("warnings").is_some(), round == 1, "{v}");
    }
    let held: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM server_names WHERE server_id = 'server-b'")
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(held, MAX_SERVER_NAMES);
}

#[test]
fn addresses_are_counted_by_network() {
    assert_eq!(limit_key("203.0.113.9".parse().unwrap()), "203.0.113.9");
    assert_eq!(limit_key("2001:db8:1:2:aaaa::1".parse().unwrap()), limit_key("2001:db8:1:2:bbbb::2".parse().unwrap()));
    assert_ne!(limit_key("2001:db8:1:2::1".parse().unwrap()), limit_key("2001:db8:1:3::1".parse().unwrap()));
    assert_eq!(limit_key("::ffff:203.0.113.9".parse().unwrap()), "203.0.113.9");
    for private in [
        "127.0.0.1",
        "10.1.2.3",
        "192.168.1.1",
        "172.16.0.1",
        "100.64.0.1",
        "169.254.1.1",
        "0.0.0.0",
        "::1",
        "fd00::1",
        "fe80::1",
        "::ffff:10.0.0.1",
    ] {
        assert!(!public_ip(private.parse().unwrap()), "{private}");
    }
    for public in ["1.1.1.1", "2606:4700::1111"] {
        assert!(public_ip(public.parse().unwrap()), "{public}");
    }
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
    let r = t
        .changes(&a, json!([link(&kiwi, "server-a", "Kiwi"), { "op": "unlink", "global_id": kiwi.global_id() }]))
        .await;
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
        let now = identity::now();
        json!({ "name": name, "global_id": who.global_id(), "host": host, "time": now, "signature": who.sign_link(host, name, now) })
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

    // Claims are made while the player waits: an old signature isn't one.
    let mut old = claim(&other, "server-b", "Fresh");
    old["time"] = json!(5);
    old["signature"] = json!(other.sign_link("server-b", "Fresh", 5));
    let (status, _) = t.call("POST", "/v1/names/claim", Some(&b), Some(old)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
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
        sqlx::query("INSERT INTO servers (id, secret_hash, joined_at) VALUES ('s', 'h', 0)")
            .execute(&c.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO links VALUES (?, 's', 'Old', 1)")
            .bind(first.global_id())
            .execute(&c.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO links VALUES ('ZZZ', 's', 'old', 2)").execute(&c.pool).await.unwrap();
        sqlx::query("DELETE FROM names").execute(&c.pool).await.unwrap();
    }
    let c = Coordinator::open(&db, "T".into()).await.unwrap();
    assert_eq!(c.owner("old").await.unwrap(), Some(first.global_id()), "the older link keeps it");
    let _ = std::fs::remove_dir_all(dir);
}

/// The admin UI's router, for a site at https://admin.example.
fn admin_router(t: &Test) -> Router {
    let _ = t.c.admin.set(admin::Config::new("https://admin.example", false).unwrap());
    admin::router(Arc::clone(&t.c)).layer(axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from(([192, 0, 2, 1], 1))))
}

async fn admin_get(router: &Router, path: &str, headers: &[(&str, &str)]) -> axum::response::Response {
    let mut req = Request::builder().method("GET").uri(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    router.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
}

#[tokio::test]
async fn admin_ui_is_served_with_its_assets() {
    let t = start("admin-ui").await;
    let r = admin_router(&t);
    let page = admin_get(&r, "/", &[]).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    assert!(page.headers()["content-security-policy"].to_str().unwrap().contains("script-src 'self'"));
    assert_eq!(page.headers()["cache-control"], "no-store, no-transform");
    // A built asset (when the UI was built in): cached for good, its name being its hash.
    if let Some(asset) = admin::ui_asset_paths().into_iter().find(|p| p.starts_with("/assets/")) {
        let resp = admin_get(&r, asset, &[]).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers()["cache-control"].to_str().unwrap().contains("immutable"));
    }
    assert_eq!(admin_get(&r, "/nothing-here.js", &[]).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn live_updates_want_the_admin_site_and_a_session() {
    let t = start("admin-live").await;
    let r = admin_router(&t);
    let upgrade = [
        ("connection", "upgrade"),
        ("upgrade", "websocket"),
        ("sec-websocket-version", "13"),
        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
    ];
    // Another site's page can't open it, even with the admin's cookie.
    let mut h = upgrade.to_vec();
    h.push(("origin", "https://evil.example"));
    assert_eq!(admin_get(&r, "/api/live", &h).await.status(), StatusCode::FORBIDDEN);
    // No origin at all (not a browser page) is refused too.
    assert_eq!(admin_get(&r, "/api/live", &upgrade).await.status(), StatusCode::FORBIDDEN);
    // The admin site, but nobody signed in.
    let mut h = upgrade.to_vec();
    h.push(("origin", "https://admin.example"));
    assert_eq!(admin_get(&r, "/api/live", &h).await.status(), StatusCode::UNAUTHORIZED);
    // Telling nobody is fine.
    t.c.publish(admin::live::Event::Metrics);
}

#[tokio::test]
async fn reports_count_anonymised_players_traffic_and_alerts() {
    let t = start("reports").await;
    let a = t.join("server-a").await;
    // Two players online, a bad id (ignored), and traffic counters.
    let report = json!({ "metrics": {
        "players": { "online": 2 },
        "active": ["0123456789abcdef", "fedcba9876543210", "<script>"],
        "system": { "net_rx_bytes": 1000, "net_tx_bytes": 500 },
        "counters": { "failed_logins": 0 },
    }, "update": {} });
    assert_eq!(t.call("POST", "/v1/metrics", Some(&a), Some(report.clone())).await.0, StatusCode::OK);
    // The same two a minute later: still two players, two minutes each.
    assert_eq!(t.call("POST", "/v1/metrics", Some(&a), Some(report)).await.0, StatusCode::OK);
    let r = t.c.players_report(86_400).await.unwrap();
    assert_eq!(r["totals"]["players"], 2);
    assert_eq!(r["totals"]["new"], 2);
    assert_eq!(r["days"][0]["minutes"], 4);
    // The bandwidth report and the alert check run on what's there.
    let b = t.c.bandwidth(86_400, None).await.unwrap();
    assert!(b["allowances"].as_array().unwrap().iter().any(|a| a["server"] == "server-a"));
    t.c.check_alerts().await.unwrap();
    assert!(t.c.alerts().await.unwrap()["active"].as_array().unwrap().is_empty());
    // A pulse: kept for the live view, rate-limited like the rest.
    let pulse = json!({ "players": { "online": 2 }, "matches": 1, "counters": {} });
    assert_eq!(t.call("POST", "/v1/pulse", Some(&a), Some(pulse.clone())).await.0, StatusCode::OK);
    assert_eq!(t.call("POST", "/v1/pulse", None, Some(pulse.clone())).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(t.call("POST", "/v1/pulse", Some(&a), Some(json!({ "nonsense": true }))).await.0, StatusCode::BAD_REQUEST);
    assert!(t.c.pulses.lock().unwrap().contains_key("server-a"));
}

#[tokio::test]
async fn strangers_learn_nothing_from_bad_bodies() {
    let t = start("bad-bodies").await;
    // Without a server's secret: refused before the body is looked at.
    for path in ["/v1/heartbeat", "/v1/metrics", "/v1/pulse", "/v1/changes", "/v1/names/claim"] {
        let (status, v) = t.call("POST", path, None, Some(json!({ "nonsense": 1 }))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}: {v}");
    }
    // A bad body gets one plain answer, naming no fields.
    let (status, v) = t.call("POST", "/v1/join", None, Some(json!({ "nonsense": 1 }))).await;
    assert_eq!((status, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("not a valid request")));
    let secret = t.join("srv-a").await;
    let (status, v) = t.call("POST", "/v1/heartbeat", Some(&secret), Some(json!({ "nonsense": 1 }))).await;
    assert_eq!((status, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("not a valid request")));
}

#[tokio::test]
async fn an_address_adds_one_ping_per_server_in_a_while() {
    let t = start("ping-samples").await;
    t.join("srv-a").await;
    let report = json!({ "pings": [{ "server": "srv-a", "ms": 40 }, { "server": "srv-a", "ms": 41 }, { "server": "nowhere", "ms": 5 }] });
    let (status, v) = t.call("POST", "/v1/pings", None, Some(report.clone())).await;
    assert_eq!((status, v["recorded"].as_u64()), (StatusCode::OK, Some(1)), "{v}");
    let (status, v) = t.call("POST", "/v1/pings", None, Some(report)).await;
    assert_eq!((status, v["recorded"].as_u64()), (StatusCode::OK, Some(0)), "a second report counted again: {v}");
}
