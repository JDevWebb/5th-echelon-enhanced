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
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header("host", "coordinator.test");
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
async fn a_database_a_newer_release_migrated_still_opens() {
    let t = start("newer").await;
    sqlx::query("INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (29991231000000, 'from a newer release', 1, x'00', 0)")
        .execute(&t.c.pool)
        .await
        .unwrap();
    Coordinator::open(&t.dir.join("c.db").to_string_lossy(), "TOKEN".into())
        .await
        .expect("the release before opens it");
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
async fn the_directory_carries_a_rollout_through_its_stages() {
    let t = start("rollout").await;
    let beat = |id: &str, version: &str, players: u32| json!({ "name": id, "host": id, "listed": true, "auto_update": true, "version": version, "players_online": players });
    let (quiet, busy) = (t.join("quiet").await, t.join("busy").await);
    t.call("POST", "/v1/heartbeat", Some(&quiet), Some(beat("quiet", "1.0.0", 0))).await;
    t.call("POST", "/v1/heartbeat", Some(&busy), Some(beat("busy", "1.0.0", 3))).await;
    let rollout = || async { t.call("GET", "/v1/servers", None, None).await.1 };
    assert!(rollout().await.get("rollout").is_none(), "nothing going out");

    // Canary: the quietest server first.
    t.c.start_rollout("1.1.0", "a test").await.unwrap();
    let v = rollout().await;
    let r = &v["rollout"];
    assert_eq!(
        (r["release"].as_str(), r["stage"].as_str(), r["canary"].as_str()),
        (Some("1.1.0"), Some("canary"), Some("quiet"))
    );
    assert_eq!((r["healthy_for"].as_i64(), r["quiet_wait"].as_i64()), (Some(600), Some(7200)));
    let started = r["stage_started"].as_i64().unwrap();
    assert!((identity::now() - started).abs() < 5, "Unix seconds, now");
    // Each server's version and players stay in its listing.
    let busy_listing = v["servers"].as_array().unwrap().iter().find(|s| s["id"] == "busy").cloned().unwrap();
    assert_eq!((busy_listing["version"].as_str(), busy_listing["players_online"].as_u64()), (Some("1.0.0"), Some(3)));

    // Verifying: the canary runs it.
    t.call("POST", "/v1/heartbeat", Some(&quiet), Some(beat("quiet", "1.1.0", 0))).await;
    t.c.tick_rollout().await.unwrap();
    assert_eq!(rollout().await["rollout"]["stage"], "verifying");

    // Rolling: healthy long enough.
    sqlx::query("UPDATE rollout SET stage_since = stage_since - 601").execute(&t.c.pool).await.unwrap();
    t.call("POST", "/v1/heartbeat", Some(&quiet), Some(beat("quiet", "1.1.0", 0))).await;
    t.c.tick_rollout().await.unwrap();
    assert_eq!(rollout().await["rollout"]["stage"], "rolling");

    // Paused: not shown.
    t.c.set_paused(true).await.unwrap();
    assert!(rollout().await.get("rollout").is_none(), "paused");
    t.c.set_paused(false).await.unwrap();

    // Done: every server runs it, and the directory says nothing more.
    t.call("POST", "/v1/heartbeat", Some(&busy), Some(beat("busy", "1.1.0", 3))).await;
    t.c.tick_rollout().await.unwrap();
    assert!(rollout().await.get("rollout").is_none(), "done");
}

/// A heartbeat from a server that installs updates (unless `manual`), and what the
/// coordinator answers it to install, if anything.
async fn heartbeat_asks(t: &Test, secret: &str, id: &str, version: &str, players: u32, manual: bool) -> Option<String> {
    let body = json!({ "name": id, "host": id, "listed": true, "auto_update": !manual, "version": version, "players_online": players });
    // More heartbeats than a server may send in a minute: the limit isn't what's tested here.
    t.c.heartbeats.seen.lock().unwrap().clear();
    let (status, answer) = t.call("POST", "/v1/heartbeat", Some(secret), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer["update"]["version"].as_str().map(str::to_string)
}

async fn stage(t: &Test) -> (String, String) {
    let r = t.c.rollout().await.unwrap();
    (r.stage, r.note)
}

#[tokio::test]
async fn heartbeats_tell_each_server_when_to_update() {
    let t = start("rollout-asks").await;
    let (quiet, busy, manual) = (t.join("quiet").await, t.join("busy").await, t.join("manual").await);
    heartbeat_asks(&t, &quiet, "quiet", "1.0.0", 0, false).await;
    heartbeat_asks(&t, &busy, "busy", "1.0.0", 3, false).await;
    heartbeat_asks(&t, &manual, "manual", "1.0.0", 0, true).await;
    t.c.start_rollout("1.1.0", "a test").await.unwrap();
    assert_eq!(t.c.rollout().await.unwrap().previous.as_deref(), Some("1.0.0"), "what the network ran");

    // The canary (the quietest that installs updates) is asked; nobody else yet.
    assert_eq!(heartbeat_asks(&t, &quiet, "quiet", "1.0.0", 0, false).await.as_deref(), Some("1.1.0"));
    assert_eq!(heartbeat_asks(&t, &busy, "busy", "1.0.0", 3, false).await, None);
    assert_eq!(
        heartbeat_asks(&t, &manual, "manual", "1.0.0", 0, true).await,
        None,
        "a server that doesn't install updates is never picked"
    );

    // Verifying: nobody else is asked while the canary proves it.
    assert_eq!(heartbeat_asks(&t, &quiet, "quiet", "1.1.0", 0, false).await, None, "it runs it already");
    t.c.tick_rollout().await.unwrap();
    assert_eq!(stage(&t).await.0, "verifying");
    assert_eq!(heartbeat_asks(&t, &busy, "busy", "1.0.0", 3, false).await, None);

    // Rolling: a server with players on waits for them, up to QUIET_WAIT.
    sqlx::query("UPDATE rollout SET stage_since = stage_since - 601").execute(&t.c.pool).await.unwrap();
    t.c.tick_rollout().await.unwrap();
    assert_eq!(stage(&t).await.0, "rolling");
    assert_eq!(heartbeat_asks(&t, &busy, "busy", "1.0.0", 3, false).await, None, "players are on");
    assert_eq!(heartbeat_asks(&t, &busy, "busy", "1.0.0", 0, false).await.as_deref(), Some("1.1.0"), "they left");
    sqlx::query("UPDATE rollout SET stage_since = stage_since - ?")
        .bind(updates::QUIET_WAIT)
        .execute(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(
        heartbeat_asks(&t, &busy, "busy", "1.0.0", 3, false).await.as_deref(),
        Some("1.1.0"),
        "after QUIET_WAIT, whatever"
    );
    // Offered to a server that doesn't install updates too: its own setting declines it
    // (dedicated_server's self_update::request), and it can update by hand.
    assert_eq!(heartbeat_asks(&t, &manual, "manual", "1.0.0", 0, true).await.as_deref(), Some("1.1.0"));

    // Done once every server that installs updates runs it (the manual one aside).
    heartbeat_asks(&t, &busy, "busy", "1.1.0", 3, false).await;
    t.c.tick_rollout().await.unwrap();
    assert_eq!(stage(&t).await.0, "done");
    // Paused or halted: nobody is asked.
    t.c.set_paused(true).await.unwrap();
    assert_eq!(heartbeat_asks(&t, &manual, "manual", "1.0.0", 0, false).await, None, "paused");
    t.c.set_paused(false).await.unwrap();
    t.c.halt("an admin's test").await.unwrap();
    assert_eq!(heartbeat_asks(&t, &manual, "manual", "1.0.0", 0, false).await, None, "halted");
    assert_eq!(stage(&t).await, ("halted".to_string(), "an admin's test".to_string()));
}

#[tokio::test]
async fn a_canary_that_rolls_back_or_never_updates_halts_the_rollout() {
    let t = start("rollout-halts").await;
    let (a, b) = (t.join("a").await, t.join("b").await);
    heartbeat_asks(&t, &a, "a", "1.0.0", 0, false).await;
    heartbeat_asks(&t, &b, "b", "1.0.0", 1, false).await;
    t.c.start_rollout("1.1.0", "a test").await.unwrap();
    assert_eq!(t.c.rollout().await.unwrap().canary.as_deref(), Some("a"));

    // Its updater rolled the release back (it didn't come back healthy there).
    let report = json!({ "metrics": {}, "update": { "auto_update": true, "running": "1.0.0", "updater": { "state": "rolled-back", "version": "1.1.0" } } });
    let (status, _) = t.call("POST", "/v1/metrics", Some(&a), Some(report)).await;
    assert_eq!(status, StatusCode::OK);
    t.c.tick_rollout().await.unwrap();
    let (stage_now, note) = stage(&t).await;
    assert_eq!(stage_now, "halted");
    assert!(note.contains("a couldn't install 1.1.0 (rolled-back)"), "{note}");
    assert_eq!(heartbeat_asks(&t, &b, "b", "1.0.0", 0, false).await, None, "nobody else is asked");

    // A canary that never installs it halts the rollout after CANARY_TIMEOUT.
    t.c.start_rollout("1.2.0", "a test").await.unwrap();
    let canary = t.c.rollout().await.unwrap().canary.unwrap();
    sqlx::query("UPDATE rollout SET stage_since = stage_since - 3 * 3600 - 1").execute(&t.c.pool).await.unwrap();
    t.c.tick_rollout().await.unwrap();
    let (stage_now, note) = stage(&t).await;
    assert_eq!(stage_now, "halted");
    assert!(note.contains(&format!("{canary} hasn't installed 1.2.0")), "{note}");

    // One that stops reporting in while it's verified halts it too.
    t.c.start_rollout("1.3.0", "a test").await.unwrap();
    let canary = t.c.rollout().await.unwrap().canary.unwrap();
    let secret = if canary == "a" { &a } else { &b };
    heartbeat_asks(&t, secret, &canary, "1.3.0", 0, false).await;
    t.c.tick_rollout().await.unwrap();
    assert_eq!(stage(&t).await.0, "verifying");
    sqlx::query("UPDATE servers SET last_seen = last_seen - 600 WHERE id = ?")
        .bind(&canary)
        .execute(&t.c.pool)
        .await
        .unwrap();
    t.c.tick_rollout().await.unwrap();
    let (stage_now, note) = stage(&t).await;
    assert_eq!(stage_now, "halted");
    assert!(note.contains("stopped reporting in on 1.3.0"), "{note}");
}

#[tokio::test]
async fn new_releases_start_a_rollout_unless_pinned_or_held_back() {
    let t = start("rollout-found").await;
    let found = |v: &'static str| {
        let c = std::sync::Arc::clone(&t.c);
        async move { c.release_found(v, "https://github.com/x/y/releases/tag/v1", "2026-01-01T00:00:00Z").await.unwrap() }
    };
    // This coordinator runs 0.x: the next major version is fine, two ahead isn't.
    assert!(matches!(found("2.0.0").await, updates::Found::Held(why) if why.contains("skips a major version")));
    assert_eq!(found("1.1.0").await, updates::Found::Started);
    assert_eq!(t.c.rollout().await.unwrap().target.as_deref(), Some("1.1.0"));
    assert_eq!(found("1.1.0").await, updates::Found::Kept, "the target already");
    assert!(matches!(found("1.0.5").await, updates::Found::Held(why) if why.contains("isn't newer than 1.1.0")));
    // Pinned: a newer release is only recorded, for an admin to roll out.
    t.c.set_pinned(true).await.unwrap();
    assert_eq!(found("1.2.0").await, updates::Found::Kept);
    assert_eq!(t.c.rollout().await.unwrap().target.as_deref(), Some("1.1.0"));
    let versions: Vec<String> = t.c.releases().await.unwrap().into_iter().map(|r| r.0).collect();
    assert_eq!(versions, ["2.0.0", "1.2.0", "1.1.0", "1.0.5"], "every signed release is recorded, newest first");
}

#[tokio::test]
async fn rolling_back_goes_to_the_release_before_everywhere_at_once() {
    let t = start("rollout-back").await;
    assert!(t.c.roll_back().await.unwrap_err().contains("no release before"), "nothing to go back to yet");
    let (a, b) = (t.join("a").await, t.join("b").await);
    heartbeat_asks(&t, &a, "a", "1.0.0", 0, false).await;
    heartbeat_asks(&t, &b, "b", "1.0.0", 0, false).await;
    t.c.start_rollout("1.1.0", "a test").await.unwrap();
    t.c.promote().await.unwrap();
    heartbeat_asks(&t, &a, "a", "1.1.0", 0, false).await;
    heartbeat_asks(&t, &b, "b", "1.1.0", 0, false).await;

    assert_eq!(t.c.roll_back().await.unwrap(), "1.0.0");
    let r = t.c.rollout().await.unwrap();
    assert_eq!(
        (r.target.as_deref(), r.previous.as_deref(), r.stage.as_str(), r.pinned),
        (Some("1.0.0"), Some("1.1.0"), "rolling", true)
    );
    // Every server is asked now, players on or not, with no canary.
    assert_eq!(heartbeat_asks(&t, &a, "a", "1.1.0", 9, false).await.as_deref(), Some("1.0.0"));
    assert_eq!(heartbeat_asks(&t, &b, "b", "1.1.0", 0, false).await.as_deref(), Some("1.0.0"));
    // Pinned: the release it went back from isn't rolled out again on its own.
    assert_eq!(t.c.release_found("1.1.0", "", "").await.unwrap(), updates::Found::Kept);
}

#[tokio::test]
async fn an_admin_rolls_out_a_recorded_release_and_promotes_it() {
    let t = start("rollout-admin").await;
    let (quiet, busy) = (t.join("quiet").await, t.join("busy").await);
    heartbeat_asks(&t, &quiet, "quiet", "1.1.0", 0, false).await;
    heartbeat_asks(&t, &busy, "busy", "1.1.0", 2, false).await;
    assert!(
        t.c.roll_out("1.0.9").await.unwrap_err().contains("isn't a signed release"),
        "only releases the coordinator has seen"
    );
    t.c.release_found("1.0.9", "", "").await.unwrap();
    // An older release, by hand: a canary first, and pinned.
    t.c.roll_out("1.0.9").await.unwrap();
    let r = t.c.rollout().await.unwrap();
    assert_eq!((r.target.as_deref(), r.stage.as_str(), r.pinned), (Some("1.0.9"), "canary", true));
    assert_eq!(t.c.rollout().await.unwrap().canary.as_deref(), Some("quiet"), "the one without players");
    assert_eq!(heartbeat_asks(&t, &busy, "busy", "1.1.0", 0, false).await, None, "not the canary");
    // Promoted: no canary, every quiet server now.
    t.c.promote().await.unwrap();
    assert_eq!(stage(&t).await.0, "rolling");
    assert_eq!(heartbeat_asks(&t, &busy, "busy", "1.1.0", 0, false).await.as_deref(), Some("1.0.9"));
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
    for path in [
        "/v1/heartbeat",
        "/v1/metrics",
        "/v1/pulse",
        "/v1/changes",
        "/v1/names/claim",
        "/v1/players",
        "/v1/actions/1",
        "/v1/reports",
    ] {
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

/// A players report entry.
fn player(id: i64, name: &str, identity: Option<&str>) -> Value {
    json!({ "id": id, "name": name, "identity": identity, "created_at": identity::now() - 86_400, "last_seen": identity::now(),
            "online": true, "play_seconds": 5400, "sessions": 12, "matches": 3, "banned": null })
}

#[tokio::test]
async fn players_and_sessions_are_taken_and_a_full_roster_deletes() {
    let t = start("players").await;
    let a = t.join("server-a").await;
    let now = identity::now();
    let body = json!({ "full": true,
        "players": [player(1007, "Exo", Some("ab12")), player(1008, "Kiwi", None), player(1009, "Tank", None), { "id": "nope" }],
        "sessions": [{ "id": 1, "player": 1007, "start": now - 600, "end": null }, { "id": 2, "player": 4242, "start": now - 60, "end": now }] });
    let (status, v) = t.call("POST", "/v1/players", Some(&a), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        (v["ok"].as_bool(), v["players"].as_i64(), v["sessions"].as_i64(), v["skipped"].as_i64()),
        (Some(true), Some(3), Some(1), Some(2)),
        "{v}"
    );
    // Changes only: the session ends, a player is renamed; nobody is removed.
    let mut renamed = player(1009, "Tanker", None);
    renamed["online"] = json!(false);
    let body = json!({ "full": false, "players": [renamed.clone()], "sessions": [{ "id": 1, "player": 1007, "start": now - 600, "end": now - 10 }] });
    assert_eq!(t.call("POST", "/v1/players", Some(&a), Some(body)).await.0, StatusCode::OK);
    let ended: Option<i64> = sqlx::query_scalar("SELECT ended FROM play_sessions WHERE server_id = 'server-a' AND id = 1")
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(ended, Some(now - 10));
    let list =
        t.c.player_list(&players::ListQuery {
            sort: "name".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let names: Vec<&str> = list["players"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Exo", "Kiwi", "Tanker"]);
    assert_eq!(list["players"][0]["week_seconds"], 590);
    // The whole roster again, without Kiwi: deleted there, so gone here; the sessions stay.
    let body = json!({ "full": true, "players": [player(1007, "Exo", Some("ab12")), renamed] });
    let (_, v) = t.call("POST", "/v1/players", Some(&a), Some(body)).await;
    assert_eq!(v["removed"], 1, "{v}");
    let list = t.c.player_list(&players::ListQuery::default()).await.unwrap();
    assert_eq!(list["total"], 2);
    // Search, filters.
    let found =
        t.c.player_list(&players::ListQuery {
            q: "xo".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(found["players"][0]["id"], 1007);
    let online =
        t.c.player_list(&players::ListQuery {
            online: "1".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(online["total"], 1, "Tanker went offline: {online}");
    // The same identity on another server: "also on".
    let b = t.join("server-b").await;
    t.call("POST", "/v1/players", Some(&b), Some(json!({ "full": true, "players": [player(5, "Exo", Some("ab12"))] })))
        .await;
    // Only an identity linked where each account is counts.
    let unlinked = t.c.player_detail("server-a", 1007).await.unwrap().unwrap();
    assert_eq!(unlinked["others"], json!([]), "{unlinked}");
    sqlx::query("INSERT INTO links (global_id, server_id, username, linked_at) VALUES ('ab12', 'server-a', 'Exo', 0), ('ab12', 'server-b', 'Exo', 0)")
        .execute(&t.c.pool)
        .await
        .unwrap();
    let found =
        t.c.player_list(&players::ListQuery {
            q: "ab12".into(),
            server: "server-a".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    // The row is the account seen last (server B's, if its roster came a second later).
    let shown = &found["players"][0];
    let other = if shown["server"] == "server-a" { "server-b" } else { "server-a" };
    assert_eq!(shown["also_on"], json!([other]), "{shown}");
    // One row for the person: both servers, their numbers added up; before the links, two.
    let all = t.c.player_list(&players::ListQuery::default()).await.unwrap();
    let exo: Vec<&Value> = all["players"].as_array().unwrap().iter().filter(|p| p["name"] == "Exo").collect();
    assert_eq!(exo.len(), 1, "{all}");
    assert_eq!(exo[0]["servers"], json!(["server-a", "server-b"]));
    assert_eq!(
        (exo[0]["accounts"].as_i64(), exo[0]["play_seconds"].as_i64(), exo[0]["sessions"].as_i64()),
        (Some(2), Some(10_800), Some(24))
    );
    assert_eq!(all["total"], 2, "Exo and Tanker: {all}");
    // A filter matching one of the accounts shows the person, with both servers.
    let on_b =
        t.c.player_list(&players::ListQuery {
            server: "server-b".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        (on_b["total"].as_i64(), on_b["players"][0]["servers"].as_array().map(Vec::len)),
        (Some(1), Some(2)),
        "{on_b}"
    );
    let d = t.c.player_detail("server-a", 1007).await.unwrap().unwrap();
    assert_eq!((d["sessions"][0]["seconds"].as_i64(), d["others"][0]["server"].as_str()), (Some(590), Some("server-b")));
    assert_eq!(d["days"].as_array().unwrap().len(), 30);
    assert_eq!(d["days"].as_array().unwrap().iter().map(|x| x["seconds"].as_i64().unwrap()).sum::<i64>(), 590);
}

#[tokio::test]
async fn players_reports_are_checked_and_capped() {
    let t = start("players-caps").await;
    let a = t.join("server-a").await;
    let (status, _) = t.call("POST", "/v1/players", None, Some(json!({ "players": [] }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = t.call("POST", "/v1/players", Some(&a), Some(json!({ "nonsense": 1 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let many: Vec<Value> = (0..=players::MAX_PLAYERS as i64).map(|i| json!({ "id": i, "name": "P" })).collect();
    let (status, _) = t.call("POST", "/v1/players", Some(&a), Some(json!({ "players": many }))).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    // 2000 long-named players fit in one request (bigger than other calls may be).
    let many: Vec<Value> = (0..players::MAX_PLAYERS as i64).map(|i| player(i, &"N".repeat(64), Some(&"a".repeat(64)))).collect();
    let (status, v) = t.call("POST", "/v1/players", Some(&a), Some(json!({ "full": true, "players": many }))).await;
    assert_eq!((status, v["players"].as_i64()), (StatusCode::OK, Some(2000)), "{v}");
    // Names are cleaned, not refused; a name too long is cut.
    let body = json!({ "players": [{ "id": 1, "name": format!("Bad\u{202e}{}", "x".repeat(100)), "play_seconds": -3 }] });
    t.call("POST", "/v1/players", Some(&a), Some(body)).await;
    let (name, secs): (String, i64) = sqlx::query_as("SELECT name, play_seconds FROM players WHERE server_id = 'server-a' AND id = 1")
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!((name.chars().count(), name.starts_with("Badx"), secs), (64, true, 0));
}

impl Test {
    /// The pending actions in a pulse's answer.
    async fn pulse_actions(&self, secret: &str) -> Vec<Value> {
        let (status, v) = self
            .call("POST", "/v1/pulse", Some(secret), Some(json!({ "players": { "online": 1 }, "counters": {} })))
            .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        v["actions"].as_array().unwrap().clone()
    }

    async fn action_done(&self, secret: &str, id: i64, result: Value) -> StatusCode {
        self.call("POST", &format!("/v1/actions/{id}"), Some(secret), Some(result)).await.0
    }
}

#[tokio::test]
async fn actions_go_with_the_pulse_and_only_their_server_answers() {
    let t = start("actions").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    t.call("POST", "/v1/players", Some(&a), Some(json!({ "full": true, "players": [player(1007, "Exo", None)] })))
        .await;
    let ban =
        t.c.queue_action("server-a", 1007, "ban", &json!({ "reason": "cheating", "until": null }), "admin1")
            .await
            .unwrap();
    let reset = t.c.queue_action("server-a", 1007, "reset_password", &json!({ "reason": "" }), "admin1").await.unwrap();
    let sent = t.pulse_actions(&a).await;
    assert_eq!(sent.len(), 2);
    assert_eq!(
        (sent[0]["id"].as_i64(), sent[0]["kind"].as_str(), sent[0]["player"].as_i64()),
        (Some(ban), Some("ban"), Some(1007))
    );
    assert_eq!(sent[0]["reason"], "cheating");
    assert!(t.pulse_actions(&b).await.is_empty(), "server-b has none");
    // Sent again until answered; only server-a may answer.
    let result = json!({ "ok": true, "message": "Banned Exo", "password": null });
    assert_eq!(t.action_done(&b, ban, result.clone()).await, StatusCode::NOT_FOUND);
    assert_eq!(t.action_done(&a, ban, result).await, StatusCode::OK);
    let sent = t.pulse_actions(&a).await;
    assert_eq!(sent.len(), 1, "the ban is done: {sent:?}");
    let banned =
        t.c.player_list(&players::ListQuery {
            banned: "1".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(banned["players"][0]["banned_accounts"], 1, "shown before the server's next report: {banned}");
    let d = t.c.player_detail("server-a", 1007).await.unwrap().unwrap();
    assert_eq!(d["player"]["banned"]["reason"], "cheating", "{d}");
    // A reset password: shown once, to the admin who asked.
    let result = json!({ "ok": true, "message": "Reset", "password": "temp-pass-123" });
    assert_eq!(t.action_done(&a, reset, result).await, StatusCode::OK);
    assert_eq!(t.c.read_action(reset, "admin2").await.unwrap().unwrap()["password"], Value::Null);
    let first = t.c.read_action(reset, "admin1").await.unwrap().unwrap();
    assert_eq!((first["status"].as_str(), first["password"].as_str()), (Some("done"), Some("temp-pass-123")));
    assert_eq!(t.c.read_action(reset, "admin1").await.unwrap().unwrap()["password"], Value::Null, "only once");
    // An hour without an answer: expired, and no longer sent.
    let kick = t.c.queue_action("server-a", 1007, "kick", &json!({}), "admin1").await.unwrap();
    sqlx::query("UPDATE player_actions SET created_at = created_at - 7200 WHERE id = ?")
        .bind(kick)
        .execute(&t.c.pool)
        .await
        .unwrap();
    assert!(t.pulse_actions(&a).await.is_empty());
    assert_eq!(t.c.read_action(kick, "admin1").await.unwrap().unwrap()["status"], "expired");
    // Results are checked.
    assert_eq!(t.action_done(&a, kick, json!({ "message": "?" })).await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_rename_moves_the_players_name_across_the_network() {
    let t = start("rename").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    let (kiwi, tank) = (identity::Identity::generate(), identity::Identity::generate());
    t.changes(&a, json!([link(&kiwi, "server-a", "Kiwi")])).await;
    t.changes(&b, json!([link(&tank, "server-b", "Tank")])).await;
    let roster = json!({ "full": true, "players": [player(1, "Kiwi", Some(&kiwi.global_id()))] });
    t.call("POST", "/v1/players", Some(&a), Some(roster)).await;
    // Another player's name is refused before anything is queued.
    assert!(t.c.rename_refused("server-a", 1, "TANK").await.unwrap().is_some());
    assert!(t.c.rename_refused("server-a", 1, "Kiwi2").await.unwrap().is_none());
    let id = t.c.queue_action("server-a", 1, "rename", &json!({ "name": "Kiwi2" }), "admin1").await.unwrap();
    assert_eq!(t.action_done(&a, id, json!({ "ok": true, "message": "Renamed", "password": null })).await, StatusCode::OK);
    assert_eq!(t.c.owner("kiwi2").await.unwrap(), Some(kiwi.global_id()));
    assert_eq!(t.c.owner("kiwi").await.unwrap(), None, "the old name is free");
    let username: String = sqlx::query_scalar("SELECT username FROM links WHERE server_id = 'server-a'")
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(username, "Kiwi2");
    // A failed one changes nothing.
    let id = t.c.queue_action("server-a", 1, "rename", &json!({ "name": "Kiwi3" }), "admin1").await.unwrap();
    t.action_done(&a, id, json!({ "ok": false, "message": "name taken here", "password": null })).await;
    assert_eq!(t.c.owner("kiwi3").await.unwrap(), None);
}

#[tokio::test]
async fn matches_are_stored_once_and_reported() {
    let t = start("matches").await;
    let a = t.join("server-a").await;
    let now = identity::now();
    let report = json!({ "metrics": { "players": { "online": 4 }, "counters": { "matches_started": 2, "failed_joins": 1 }, "matches": [
        { "mode": "svm", "map": 3_578_398_534_u32, "game_mode": 3, "started": now - 1800, "ended": now - 600, "players": 4, "private": false },
        { "mode": "coop", "map": 7, "game_mode": 1, "started": now - 1200, "ended": now - 300, "players": 2, "private": true },
        { "mode": "svm", "map": 7, "game_mode": 1, "started": now, "ended": now - 300, "players": 2 },
    ] }, "update": {} });
    // Sent twice (a retry): stored once. The bad one (ends before it starts) isn't.
    assert_eq!(t.call("POST", "/v1/metrics", Some(&a), Some(report.clone())).await.0, StatusCode::OK);
    assert_eq!(t.call("POST", "/v1/metrics", Some(&a), Some(report)).await.0, StatusCode::OK);
    let r = t.c.matches_report(7).await.unwrap();
    assert_eq!(r["totals"]["matches"], 2, "{r}");
    assert_eq!(r["totals"]["avg_seconds"], 1050.0);
    assert_eq!(r["totals"]["avg_players"], 3.0);
    assert_eq!((r["private"]["matches"].as_i64(), r["public"]["matches"].as_i64()), (Some(1), Some(1)));
    assert_eq!(r["days"].as_array().unwrap().len(), 7);
    let svm: i64 = r["days"].as_array().unwrap().iter().map(|d| d["svm"].as_i64().unwrap()).sum();
    assert_eq!(svm, 1);
    assert!(r["maps"].as_array().unwrap().iter().any(|m| m["map"] == 3_578_398_534_u32));
    // Older servers send no matches: nothing changes.
    t.c.record_matches("server-a", &Value::Null).await.unwrap();
}

#[tokio::test]
async fn play_time_comes_from_sessions_when_servers_send_them() {
    let t = start("play-time").await;
    let a = t.join("server-a").await;
    // Yesterday at noon (UTC): no session crosses midnight, whatever time the test runs.
    let now = identity::now();
    let noon = now - now % 86_400 - 86_400 + 43_200;
    let body = json!({ "full": true, "players": [player(1, "Exo", None), player(2, "Kiwi", None)], "sessions": [
        // Days ago: sessions are used from the day after a server's first.
        { "id": 1, "player": 1, "start": noon - 2 * 86_400, "end": noon - 2 * 86_400 + 600 },
        { "id": 2, "player": 1, "start": noon - 3600, "end": noon - 1800 },
        { "id": 3, "player": 2, "start": noon - 3000, "end": noon - 2400 },
    ] });
    assert_eq!(t.call("POST", "/v1/players", Some(&a), Some(body)).await.0, StatusCode::OK);
    let r = t.c.players_report(7 * 86_400).await.unwrap();
    let minutes: i64 = r["days"].as_array().unwrap().iter().map(|d| d["minutes"].as_i64().unwrap()).sum();
    assert_eq!(minutes, 40, "{r}");
    assert!(r["sessions_from"]["server-a"].is_i64());
    assert_eq!(r["totals"]["peak"], 2.0, "both played at once");
    assert_eq!(r["totals"]["players"], 2, "from sessions, though they cover only part of the week");
}

#[tokio::test]
async fn live_points_and_events_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("fe-coord-live-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("c.db").to_string_lossy().to_string();
    {
        let c = Coordinator::open(&db, "T".into()).await.unwrap();
        sqlx::query("INSERT INTO servers (id, secret_hash, joined_at) VALUES ('s', 'h', 0)")
            .execute(&c.pool)
            .await
            .unwrap();
        c.record_pulse("s", &json!({ "players": { "online": 1 }, "matches": 0, "counters": { "game_logins": 1 } }))
            .await
            .unwrap();
        // Ten seconds on (a point a second apart replaces the last).
        sqlx::query("UPDATE pulses SET at = at - 10").execute(&c.pool).await.unwrap();
        let last = c.pulses.lock().unwrap().get_mut("s").unwrap().last.as_mut().map(|l| l.0 -= 10);
        assert!(last.is_some());
        c.record_pulse("s", &json!({ "players": { "online": 3 }, "matches": 1, "counters": { "game_logins": 3 } }))
            .await
            .unwrap();
    }
    let c = Coordinator::open(&db, "T".into()).await.unwrap();
    assert_eq!(c.pulses.lock().unwrap()["s"].points.len(), 2);
    let feed = c.feed.lock().unwrap().clone();
    assert!(feed.iter().any(|e| e["text"] == "1 match started"), "{feed:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A signed-in admin's cookie for the admin router, their second factor proved `verified_ago`
/// seconds ago.
async fn admin_cookie(t: &Test, name: &str, verified_ago: i64) -> String {
    let now = identity::now();
    let id: i64 = sqlx::query_scalar("INSERT INTO admins (username, created_at) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(now)
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    let token = admin::auth::random(32);
    sqlx::query(
        "INSERT INTO admin_sessions (token_hash, admin_id, stage, created_at, last_seen, verified_at, ip, country, user_agent)
         VALUES (?, ?, 'full', ?, ?, ?, '192.0.2.1', '', 'test')",
    )
    .bind(admin::auth::digest(&token))
    .bind(id)
    .bind(now)
    .bind(now)
    .bind(now - verified_ago)
    .execute(&t.c.pool)
    .await
    .unwrap();
    format!("__Host-fes-admin={token}")
}

async fn admin_call(router: &Router, method: &str, path: &str, cookie: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", cookie)
        .header("x-fes-admin", "1")
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn admins_manage_players_through_the_api() {
    let t = start("admin-players").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    t.call(
        "POST",
        "/v1/players",
        Some(&a),
        Some(json!({ "full": true, "players": [player(1007, "Exo", Some("ab12"))] })),
    )
    .await;
    assert_eq!(admin_call(&r, "GET", "/api/players", "", None).await.0, StatusCode::UNAUTHORIZED);
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let (status, v) = admin_call(&r, "GET", "/api/players?q=exo&sort=play_time", &cookie, None).await;
    assert_eq!((status, v["total"].as_i64()), (StatusCode::OK, Some(1)), "{v}");
    let (status, v) = admin_call(&r, "GET", "/api/players/server-a/1007", &cookie, None).await;
    assert_eq!((status, v["player"]["name"].as_str()), (StatusCode::OK, Some("Exo")), "{v}");
    assert_eq!(admin_call(&r, "GET", "/api/players/server-a/1", &cookie, None).await.0, StatusCode::NOT_FOUND);
    // A kick needs a session; a ban a second factor proved lately.
    let (status, v) = admin_call(&r, "POST", "/api/players/server-a/1007/actions", &cookie, Some(json!({ "kind": "kick" }))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let kick = v["actions"][0]["id"].as_i64().unwrap();
    let ban = json!({ "kind": "ban", "reason": "cheating", "until": identity::now() + 86_400 });
    let (status, v) = admin_call(&r, "POST", "/api/players/server-a/1007/actions", &cookie, Some(ban.clone())).await;
    assert_eq!((status, v["reverify"].as_bool()), (StatusCode::FORBIDDEN, Some(true)));
    let fresh = admin_cookie(&t, "admin2", 0).await;
    let (status, v) = admin_call(&r, "POST", "/api/players/server-a/1007/actions", &fresh, Some(ban)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    // Checked: kinds, names, reasons, ban ends.
    for bad in [
        json!({ "kind": "explode" }),
        json!({ "kind": "rename", "name": "x" }),
        json!({ "kind": "ban", "reason": "r".repeat(201) }),
        json!({ "kind": "ban", "until": 5 }),
    ] {
        let (status, _) = admin_call(&r, "POST", "/api/players/server-a/1007/actions", &fresh, Some(bad.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    // Every account of theirs, with all_servers.
    let b = t.join("server-b").await;
    t.call("POST", "/v1/players", Some(&b), Some(json!({ "full": true, "players": [player(9, "Exo", Some("ab12"))] })))
        .await;
    let all = json!({ "kind": "delete", "all_servers": true });
    // A server saying its player has someone's identity doesn't reach that person's accounts.
    let (_, v) = admin_call(&r, "POST", "/api/players/server-a/1007/actions", &fresh, Some(all.clone())).await;
    assert_eq!(v["actions"].as_array().unwrap().len(), 1, "{v}");
    sqlx::query("INSERT INTO links (global_id, server_id, username, linked_at) VALUES ('ab12', 'server-a', 'Exo', 0), ('ab12', 'server-b', 'Exo', 0)")
        .execute(&t.c.pool)
        .await
        .unwrap();
    let (_, v) = admin_call(&r, "POST", "/api/players/server-a/1007/actions", &fresh, Some(all)).await;
    assert_eq!(v["actions"].as_array().unwrap().len(), 2, "{v}");
    let (status, v) = admin_call(&r, "GET", &format!("/api/actions/{kick}"), &cookie, None).await;
    assert_eq!((status, v["status"].as_str(), v["kind"].as_str()), (StatusCode::OK, Some("pending"), Some("kick")), "{v}");
    let audited: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit WHERE event LIKE 'player: %'")
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(audited, 4);
    let (status, v) = admin_call(&r, "GET", "/api/matches-report?days=30", &cookie, None).await;
    assert_eq!((status, v["days"].as_array().map(Vec::len)), (StatusCode::OK, Some(30)));
}

/// A stat write: `value` to `stat` on `board` in `context`, for `who` (named after them).
fn sw(id: i64, who: &str, board: u32, context: u32, stat: u32, value: f64) -> Value {
    json!({ "id": id, "global_id": who, "name": format!("Name{who}"), "board": board, "context": context, "stat": stat, "value": value })
}

const EPOCH: &str = "0123456789abcdef";

impl Test {
    /// Sends stat writes, for people linked on every server; answers the last applied id.
    async fn stats(&self, secret: &str, epoch: &str, writes: Vec<Value>) -> i64 {
        for w in &writes {
            if let Some(who) = w["global_id"].as_str() {
                sqlx::query("INSERT OR IGNORE INTO links (global_id, server_id, username, linked_at) SELECT ?, id, ?, 0 FROM servers")
                    .bind(who)
                    .bind(format!("Name{who}"))
                    .execute(&self.c.pool)
                    .await
                    .unwrap();
            }
        }
        self.unlinked_stats(secret, epoch, writes).await
    }

    /// Sends stat writes as they are; answers the last applied id.
    async fn unlinked_stats(&self, secret: &str, epoch: &str, writes: Vec<Value>) -> i64 {
        let (status, v) = self.call("POST", "/v1/stats", Some(secret), Some(json!({ "epoch": epoch, "writes": writes }))).await;
        assert_eq!((status, v["ok"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
        v["last_id"].as_i64().unwrap()
    }

    async fn stat(&self, who: &str, board: u32, context: u32, stat: u32) -> Option<f64> {
        sqlx::query_scalar("SELECT value FROM global_stats WHERE global_id = ? AND board = ? AND context = ? AND stat = ?")
            .bind(who)
            .bind(board)
            .bind(context)
            .bind(stat)
            .fetch_optional(&self.c.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn stats_only_for_people_linked_on_the_server() {
    let t = start("stats-linked").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    sqlx::query("INSERT INTO links (global_id, server_id, username, linked_at) VALUES ('EXO', 'server-b', 'Exo', 0)")
        .execute(&t.c.pool)
        .await
        .unwrap();
    // Server A makes up stats for a player linked only on B: skipped, but done with.
    assert_eq!(t.unlinked_stats(&a, EPOCH, vec![sw(1, "EXO", 17, 1, 100, 5.0)]).await, 1);
    assert_eq!(t.stat("EXO", 17, 1, 100).await, None);
    assert_eq!(t.unlinked_stats(&b, EPOCH, vec![sw(1, "EXO", 17, 1, 100, 7.0)]).await, 1);
    assert_eq!(t.stat("EXO", 17, 1, 100).await, Some(7.0));
}

#[tokio::test]
async fn stat_writes_apply_once_per_sequence() {
    let t = start("stats-seq").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    let writes = vec![sw(1, "EXO", 17, 1, 100, 5.0), sw(2, "EXO", 17, 1, 100, 5.0), sw(3, "EXO", 17, 1, 100, 5.0)];
    assert_eq!(t.stats(&a, EPOCH, writes.clone()).await, 3);
    assert_eq!(t.stat("EXO", 17, 1, 100).await, Some(15.0));
    // Sent again (the answer was lost): nothing changes.
    assert_eq!(t.stats(&a, EPOCH, writes.clone()).await, 3);
    assert_eq!(t.stat("EXO", 17, 1, 100).await, Some(15.0));
    // Partly new, out of order: only the new ones, in order.
    assert_eq!(
        t.stats(&a, EPOCH, vec![sw(5, "EXO", 17, 1, 100, 1.0), sw(3, "EXO", 17, 1, 100, 5.0), sw(4, "EXO", 17, 1, 100, 1.0)])
            .await,
        5
    );
    assert_eq!(t.stat("EXO", 17, 1, 100).await, Some(17.0));
    // A server whose database was reset starts again, and another server has its own sequence.
    assert_eq!(t.stats(&a, "fedcba9876543210", writes[..2].to_vec()).await, 2);
    assert_eq!(t.stats(&b, EPOCH, writes[..1].to_vec()).await, 1);
    assert_eq!(t.stat("EXO", 17, 1, 100).await, Some(32.0));
    // Nothing to apply: the last id as it was.
    assert_eq!(t.stats(&a, EPOCH, vec![]).await, 5);
    assert_eq!(t.stats(&a, "00000000000000aa", vec![]).await, 0);
}

#[tokio::test]
async fn stat_writes_add_up_as_the_board_says() {
    let t = start("stats-agg").await;
    let a = t.join("server-a").await;
    t.stats(
        &a,
        EPOCH,
        vec![
            // Add: kills on a ladder.
            sw(1, "EXO", 17, 2, 100, 3.0),
            sw(2, "EXO", 17, 2, 100, 4.0),
            // Maximum and Minimum: longest and shortest life in Spies vs Mercs.
            sw(3, "EXO", 10, 228, 212, 50.0),
            sw(4, "EXO", 10, 228, 212, 30.0),
            sw(5, "EXO", 10, 228, 213, 50.0),
            sw(6, "EXO", 10, 228, 213, 30.0),
            sw(7, "EXO", 10, 228, 213, 40.0),
            // Overwrite: a mission's high score.
            sw(8, "EXO", 22, 100, 153, 900.0),
            sw(9, "EXO", 22, 100, 153, 700.0),
        ],
    )
    .await;
    assert_eq!(t.stat("EXO", 17, 2, 100).await, Some(7.0));
    assert_eq!(t.stat("EXO", 10, 228, 212).await, Some(50.0));
    assert_eq!(t.stat("EXO", 10, 228, 213).await, Some(30.0));
    assert_eq!(t.stat("EXO", 22, 100, 153).await, Some(700.0));
    let name: String = sqlx::query_scalar("SELECT name FROM global_names WHERE global_id = 'EXO'")
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(name, "NameEXO");
    // A write naming them otherwise doesn't rename them: the name is the one they linked by.
    let mut posing = sw(10, "EXO", 17, 2, 100, 1.0);
    posing["name"] = json!("JDevWebb (admin)");
    t.stats(&a, EPOCH, vec![posing]).await;
    let name: String = sqlx::query_scalar("SELECT name FROM global_names WHERE global_id = 'EXO'")
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(name, "NameEXO");
}

#[tokio::test]
async fn invalid_stat_writes_are_skipped() {
    let t = start("stats-invalid").await;
    let a = t.join("server-a").await;
    let mut bad_name = sw(10, "EXO", 17, 1, 100, 1.0);
    bad_name["name"] = json!("Ex\u{202e}o");
    let last = t
        .stats(
            &a,
            EPOCH,
            vec![
                sw(1, "EXO", 99, 0, 100, 1.0),        // no such board
                sw(2, "EXO", 17, 0, 100, 1.0),        // no such context on it
                sw(3, "EXO", 17, 1, 999, 1.0),        // no such stat on it
                sw(4, "EXO", 10, 227, 102, 1.0),      // a ratio
                sw(5, "EXO", 17, 1, 100, 2e12),       // too large
                sw(6, "not an id!", 17, 1, 100, 1.0), // not a global id
                sw(7, &"A".repeat(129), 17, 1, 100, 1.0),
                json!({ "id": 8, "global_id": "EXO" }), // not a write
                json!({ "id": 9, "global_id": "EXO", "board": -1, "context": 1, "stat": 100, "value": 1 }),
                bad_name, // applied, but the name isn't taken
            ],
        )
        .await;
    assert_eq!(last, 10, "skipped writes are done with too");
    let stored: Vec<(String, u32, u32, u32, f64)> = sqlx::query_as("SELECT global_id, board, context, stat, value FROM global_stats")
        .fetch_all(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(stored, [("EXO".to_string(), 17, 1, 100, 1.0)]);
    let names: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM global_names").fetch_one(&t.c.pool).await.unwrap();
    assert_eq!(names, 0);
    // Whole requests that aren't right.
    for (bad, want) in [
        (json!({ "epoch": "xyz", "writes": [] }), StatusCode::BAD_REQUEST),
        (json!({ "epoch": "0123456789abcdeg", "writes": [] }), StatusCode::BAD_REQUEST),
        (json!({ "epoch": EPOCH }), StatusCode::BAD_REQUEST),
        (
            json!({ "epoch": EPOCH, "writes": (1..=1001).map(|i| sw(i, "EXO", 17, 1, 100, 1.0)).collect::<Vec<_>>() }),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let (status, _) = t.call("POST", "/v1/stats", Some(&a), Some(bad)).await;
        assert_eq!(status, want);
    }
    assert_eq!(t.stat("EXO", 17, 1, 100).await, Some(1.0));
}

#[tokio::test]
async fn leaderboards_rank_across_servers() {
    let t = start("stats-boards").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    // Kills on ladder 1 (leaderboard 10): EXO plays on both servers, so 7 + 6 = 13.
    t.stats(
        &a,
        EPOCH,
        vec![sw(1, "EXO", 17, 1, 100, 7.0), sw(2, "KIWI", 17, 1, 100, 10.0), sw(3, "ZED", 17, 1, 100, 10.0)],
    )
    .await;
    t.stats(&b, EPOCH, vec![sw(1, "EXO", 17, 1, 100, 6.0), sw(2, "EXO", 17, 1, 122, 2.0)]).await;
    // Best times on solo mission 101 (leaderboard 2): lowest first, times never set left out.
    t.stats(
        &a,
        "1111111111111111",
        vec![
            sw(1, "EXO", 23, 101, 154, 90.0),
            sw(2, "KIWI", 23, 101, 154, 60.0),
            sw(3, "ZED", 23, 101, 154, 120.0),
            sw(4, "NONE", 23, 101, 154, f64::from(i32::MAX)),
            sw(5, "ZERO", 23, 101, 154, 0.0),
        ],
    )
    .await;
    let (status, v) = t.call("GET", "/v1/leaderboards", Some(&a), None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let list = |v: &Value, l: u64, c: u64| v["lists"].as_array().unwrap().iter().find(|x| x["leaderboard"] == l && x["context"] == c).cloned();
    let kills = list(&v, 10, 1).unwrap();
    assert_eq!(kills["total"], 3);
    let order: Vec<(&str, i64, f64)> = kills["top"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["global_id"].as_str().unwrap(), r["rank"].as_i64().unwrap(), r["value"].as_f64().unwrap()))
        .collect();
    assert_eq!(order, [("EXO", 1, 13.0), ("KIWI", 2, 10.0), ("ZED", 3, 10.0)]);
    assert_eq!(kills["top"][0]["name"], "NameEXO");
    assert_eq!(kills["top"][0]["stats"], json!([[100, 13.0], [122, 2.0]]));
    let times = list(&v, 2, 101).unwrap();
    let order: Vec<&str> = times["top"].as_array().unwrap().iter().map(|r| r["global_id"].as_str().unwrap()).collect();
    assert_eq!((order, times["total"].as_i64()), (vec!["KIWI", "EXO", "ZED"], Some(3)));
    // Empty lists are left out; wins on ladder 1 has EXO's 2.
    assert!(list(&v, 10, 2).is_none() && list(&v, 1, 100).is_none());
    assert_eq!(list(&v, 7, 1).unwrap()["total"], 1);
    // count, at most 100.
    let (_, v) = t.call("GET", "/v1/leaderboards?count=1", Some(&a), None).await;
    assert_eq!(list(&v, 10, 1).unwrap()["top"].as_array().unwrap().len(), 1);
    assert_eq!(list(&v, 10, 1).unwrap()["total"], 3);
    for bad in ["0", "101", "x"] {
        let (status, _) = t.call("GET", &format!("/v1/leaderboards?count={bad}"), Some(&a), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // Where people are, and every list's size.
    let (status, v) = t.call("POST", "/v1/leaderboards/players", Some(&b), Some(json!({ "ids": ["ZED", "NOBODY", 5] }))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let ranks = v["ranks"].as_array().unwrap();
    assert_eq!(ranks.len(), 2, "{v}");
    let zed_kills = ranks.iter().find(|r| r["leaderboard"] == 10).unwrap();
    assert_eq!(
        (&zed_kills["global_id"], &zed_kills["context"], &zed_kills["rank"], &zed_kills["value"], &zed_kills["stats"]),
        (&json!("ZED"), &json!(1), &json!(3), &json!(10.0), &json!([[100, 10.0]]))
    );
    let zed_time = ranks.iter().find(|r| r["leaderboard"] == 2).unwrap();
    assert_eq!((&zed_time["rank"], &zed_time["context"], &zed_time["name"]), (&json!(3), &json!(101), &json!("NameZED")));
    let totals = v["totals"].as_array().unwrap();
    assert!(totals.contains(&json!({ "leaderboard": 10, "context": 1, "total": 3 })));
    assert!(totals.contains(&json!({ "leaderboard": 2, "context": 101, "total": 3 })));
    assert_eq!(totals.len(), 3, "{v}");
    let (status, _) = t.call("POST", "/v1/leaderboards/players", Some(&b), Some(json!({ "ids": vec!["X"; 201] }))).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    // Everything kept for them.
    let (status, v) = t.call("POST", "/v1/stats/players", Some(&b), Some(json!({ "ids": ["EXO", "ZERO"] }))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        v["stats"],
        json!([
            { "global_id": "EXO", "board": 17, "context": 1, "stat": 100, "value": 13.0 },
            { "global_id": "EXO", "board": 17, "context": 1, "stat": 122, "value": 2.0 },
            { "global_id": "EXO", "board": 23, "context": 101, "stat": 154, "value": 90.0 },
            { "global_id": "ZERO", "board": 23, "context": 101, "stat": 154, "value": 0.0 },
        ])
    );
}

#[tokio::test]
async fn stats_are_for_members_only() {
    let t = start("stats-auth").await;
    let a = t.join("server-a").await;
    t.stats(&a, EPOCH, vec![sw(1, "EXO", 17, 1, 100, 1.0)]).await;
    for secret in [None, Some("not-a-secret")] {
        for (method, path, body) in [
            ("POST", "/v1/stats", Some(json!({ "epoch": EPOCH, "writes": [sw(2, "EXO", 17, 1, 100, 100.0)] }))),
            ("GET", "/v1/leaderboards", None),
            ("POST", "/v1/leaderboards/players", Some(json!({ "ids": ["EXO"] }))),
            ("POST", "/v1/stats/players", Some(json!({ "ids": ["EXO"] }))),
        ] {
            let (status, v) = t.call(method, path, secret, body).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
            assert!(v.get("stats").is_none() && v.get("lists").is_none());
        }
    }
    assert_eq!(t.stat("EXO", 17, 1, 100).await, Some(1.0));
}

#[tokio::test]
async fn admins_see_leaderboards_and_remove_stats() {
    let t = start("stats-admin").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    t.stats(
        &a,
        EPOCH,
        vec![sw(1, "EXO", 17, 1, 100, 7.0), sw(2, "CHEAT", 17, 1, 100, 99999.0), sw(3, "CHEAT", 17, 1, 122, 500.0)],
    )
    .await;
    assert_eq!(admin_call(&r, "GET", "/api/leaderboards", "", None).await.0, StatusCode::UNAUTHORIZED);
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let (status, v) = admin_call(&r, "GET", "/api/leaderboards?leaderboard=10&context=1&count=10", &cookie, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["leaderboards"].as_array().unwrap().len(), stat_boards::LEADERBOARDS.len());
    assert_eq!(v["leaderboards"][1]["contexts"].as_array().unwrap().len(), 13);
    assert_eq!(v["list"]["top"][0]["global_id"], "CHEAT");
    assert_eq!(v["list"]["total"], 2);
    assert!(v["lists"].as_array().unwrap().contains(&json!({ "leaderboard": 7, "context": 1, "total": 1 })));
    for bad in [
        "leaderboard=99&context=1",
        "leaderboard=10&context=4",
        "leaderboard=10",
        "leaderboard=10&context=1&count=101",
    ] {
        assert_eq!(
            admin_call(&r, "GET", &format!("/api/leaderboards?{bad}"), &cookie, None).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    // The servers' answer is made now, and forgotten when stats are removed.
    let (_, before) = t.call("GET", "/v1/leaderboards", Some(&a), None).await;
    let kills = before["lists"].as_array().unwrap().iter().find(|l| l["leaderboard"] == 10).unwrap();
    assert_eq!(kills["top"].as_array().unwrap().len(), 2, "{before}");

    // Removing wants a second factor proved lately.
    let (status, v) = admin_call(&r, "DELETE", "/api/stats/CHEAT", &cookie, None).await;
    assert_eq!((status, v["reverify"].as_bool()), (StatusCode::FORBIDDEN, Some(true)));
    let fresh = admin_cookie(&t, "admin2", 0).await;
    let (status, v) = admin_call(&r, "DELETE", "/api/stats/CHEAT", &fresh, None).await;
    assert_eq!((status, v["removed"].as_u64()), (StatusCode::OK, Some(2)), "{v}");
    assert_eq!(admin_call(&r, "DELETE", "/api/stats/CHEAT", &fresh, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(admin_call(&r, "DELETE", "/api/stats/not%20an%20id", &fresh, None).await.0, StatusCode::BAD_REQUEST);
    let detail: String = sqlx::query_scalar("SELECT detail FROM audit WHERE event = 'stats: removed'")
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(detail, "NameCHEAT (CHEAT): 2 stats");
    let (_, v) = admin_call(&r, "GET", "/api/leaderboards?leaderboard=10&context=1", &cookie, None).await;
    assert_eq!((v["list"]["total"].as_i64(), v["list"]["top"][0]["global_id"].as_str()), (Some(1), Some("EXO")));
    let (_, after) = t.call("GET", "/v1/leaderboards", Some(&a), None).await;
    assert!(
        after["lists"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["top"].as_array().unwrap().iter().all(|p| p["global_id"] != "CHEAT")),
        "{after}"
    );
}

/// `text` gzipped and in base64, as a report's file carries it.
fn gz64(text: &[u8]) -> String {
    use std::io::Write as _;

    use base64::Engine as _;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(text).unwrap();
    base64::engine::general_purpose::STANDARD.encode(e.finish().unwrap())
}

/// Player 1011's identity (a real one: reports keep only those).
static GV7: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| identity::Identity::generate().global_id());

/// A report from player 1011 on its server, with one file.
fn report_body(id: &str) -> Value {
    json!({
        "id": id,
        "created_at": identity::now() - 60,
        "player": { "id": 1011, "name": "ijsman5530", "identity": *GV7 },
        "rating": "bad",
        "problems": ["join", "lag"],
        "comment": "couldn't join my friend",
        "triggers": ["failed_join", "relayed"],
        "client": { "launcher": "0.4.1", "client": "0.4.1", "build": "Steam DX11", "os": "Windows 11", "language": "pt-BR" },
        "summary": { "session": { "joins_failed": 2 }, "relayed": true },
        "files": [{ "name": "bl-tracing.log", "gzip_base64": gz64(b"line one\nline two\n"), "size": 18 }],
        "server_log": "12:00:01 1011 join refused\n",
    })
}

fn report_id(n: u32) -> String {
    format!("{n:032x}")
}

#[tokio::test]
async fn reports_are_taken_once_and_checked() {
    let t = start("reports-ingest").await;
    let body = report_body(&report_id(1));
    let (status, _) = t.call("POST", "/v1/reports", None, Some(body.clone())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "members only");
    let a = t.join("server-a").await;
    let (status, v) = t.call("POST", "/v1/reports", Some(&a), Some(body.clone())).await;
    assert_eq!((status, &v), (StatusCode::OK, &json!({ "ok": true })));
    // Stored as received: gzip on disk under the coordinator's folder.
    let stored = std::fs::read(t.dir.join("reports").join(report_id(1)).join("bl-tracing.log.gz")).unwrap();
    assert_eq!(reports::gunzip(&stored, 100).unwrap(), b"line one\nline two\n");
    // The same report again (a retry) is the same report.
    let (status, v) = t.call("POST", "/v1/reports", Some(&a), Some(body.clone())).await;
    assert_eq!((status, &v), (StatusCode::OK, &json!({ "ok": true })));
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM player_reports").fetch_one(&t.c.pool).await.unwrap();
    assert_eq!(n, 1);
    let server: String = sqlx::query_scalar("SELECT server_id FROM player_reports").fetch_one(&t.c.pool).await.unwrap();
    assert_eq!(server, "server-a", "the server is the one whose secret it is");

    let big = vec![b'x'; reports::MAX_FILE_SIZE + 1];
    let file = |name: &str, data: &[u8], size: usize| json!({ "name": name, "gzip_base64": gz64(data), "size": size });
    let mut n = 100;
    for (field, value, why) in [
        ("id", json!("not-hex"), "id"),
        ("created_at", json!(identity::now() + 3 * 86_400), "created_at"),
        ("created_at", json!(identity::now() - 100 * 86_400), "created_at"),
        ("player", json!({ "id": -1, "name": "x" }), "player.id"),
        ("player", json!({ "id": 1, "name": "" }), "player.name"),
        ("player", json!({ "id": 1, "name": "a\u{202e}b" }), "player.name"),
        ("player", json!({ "id": 1, "name": "x", "identity": "no spaces" }), "player.identity"),
        ("rating", json!("meh"), "rating"),
        ("problems", json!(["join", "explode"]), "problems"),
        ("problems", json!("join"), "problems"),
        ("comment", json!("c".repeat(2001)), "comment"),
        ("triggers", json!(vec!["t"; 17]), "triggers"),
        ("triggers", json!(["t".repeat(41)]), "triggers"),
        ("client", json!({ "os": "o".repeat(65) }), "client"),
        ("client", json!({ "bad key": "x" }), "client"),
        ("client", json!({ "os": 11 }), "client"),
        ("summary", json!([1, 2]), "summary"),
        ("summary", json!({ "big": "s".repeat(64 * 1024) }), "summary"),
        ("server_log", json!("l".repeat(1024 * 1024 + 1)), "server_log"),
        ("files", json!([file("../evil", b"x", 1)]), "name"),
        ("files", json!([file("a b.log", b"x", 1)]), "name"),
        ("files", json!([file("a.log", b"x", 1), file("a.log", b"y", 1)]), "same name"),
        ("files", json!((0..9).map(|i| file(&format!("f{i}"), b"x", 1)).collect::<Vec<_>>()), "at most 8"),
        ("files", json!([file("a.log", b"four", 5)]), "size isn't"),
        ("files", json!([file("a.log", &big, big.len())]), "4 MB"),
        ("files", json!([file("a.log", &big, 4)]), "4 MB"),
        ("files", json!([{ "name": "a.log", "gzip_base64": "%%%", "size": 1 }]), "base64"),
        (
            "files",
            json!([{ "name": "a.log", "gzip_base64": "aGVsbG8gdGhlcmUgdGhpcyBpc24ndCBnemlw", "size": 1 }]),
            "gzip",
        ),
        ("files", json!([{ "name": "a.log", "gzip_base64": "", "size": 0 }]), "gzip"),
    ] {
        n += 1;
        let mut body = report_body(&report_id(n));
        body[field] = value;
        let (status, v) = t.call("POST", "/v1/reports", Some(&a), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {v}");
        assert!(v["error"].as_str().unwrap().contains(why), "{field}: {v}");
    }
    let (status, _) = t.call("POST", "/v1/reports", Some(&a), Some(json!("nonsense"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM player_reports").fetch_one(&t.c.pool).await.unwrap();
    assert_eq!(n, 1, "nothing refused was kept");
    // The least a report needs, and characters that hide in text taken out of what's kept.
    let mut lean = json!({ "id": report_id(2).to_uppercase(), "created_at": identity::now(), "player": { "id": 5, "name": "Kiwi" } });
    lean["comment"] = json!("fine\u{202e} really");
    let (status, v) = t.call("POST", "/v1/reports", Some(&a), Some(lean)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let comment: String = sqlx::query_scalar("SELECT comment FROM player_reports WHERE id = ?")
        .bind(report_id(2))
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(comment, "fine really");
}

#[tokio::test]
async fn reports_are_rate_limited_per_server() {
    let t = start("reports-limit").await;
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    let lean = |n: u32| json!({ "id": report_id(n), "created_at": identity::now(), "player": { "id": 5, "name": "Kiwi" } });
    for n in 0..reports::PER_HOUR as u32 {
        assert_eq!(t.call("POST", "/v1/reports", Some(&a), Some(lean(n))).await.0, StatusCode::OK, "{n}");
    }
    assert_eq!(t.call("POST", "/v1/reports", Some(&a), Some(lean(1000))).await.0, StatusCode::TOO_MANY_REQUESTS);
    // A retry of one it has is still answered; another server has its own allowance.
    assert_eq!(t.call("POST", "/v1/reports", Some(&a), Some(lean(3))).await.0, StatusCode::OK);
    assert_eq!(t.call("POST", "/v1/reports", Some(&b), Some(lean(1001))).await.0, StatusCode::OK);
}

#[tokio::test]
async fn admins_read_resolve_and_delete_reports() {
    let t = start("reports-admin").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    let b = t.join("server-b").await;
    t.call(
        "POST",
        "/v1/players",
        Some(&a),
        Some(json!({ "full": true, "players": [player(1011, "ijsman5530", Some(GV7.as_str()))] })),
    )
    .await;
    let (one, two, three, four) = (report_id(1), report_id(2), report_id(3), report_id(4));
    let mut first = report_body(&one);
    first["created_at"] = json!(identity::now() - 600);
    first["comment"] = json!("c".repeat(300));
    t.call("POST", "/v1/reports", Some(&a), Some(first)).await;
    t.call("POST", "/v1/reports", Some(&a), Some(report_body(&two))).await;
    // The same person on another server (their identity), and someone else.
    let mut elsewhere = report_body(&three);
    elsewhere["player"] = json!({ "id": 7, "name": "ijsman", "identity": *GV7 });
    elsewhere["problems"] = json!(["crash"]);
    elsewhere["created_at"] = json!(identity::now() - 30);
    t.call("POST", "/v1/reports", Some(&b), Some(elsewhere)).await;
    let mut other = report_body(&four);
    other["player"] = json!({ "id": 8, "name": "Kiwi", "identity": null });
    other["rating"] = json!("good");
    other["problems"] = json!([]);
    other["comment"] = json!("great games");
    t.call("POST", "/v1/reports", Some(&b), Some(other)).await;

    assert_eq!(admin_call(&r, "GET", "/api/reports", "", None).await.0, StatusCode::UNAUTHORIZED);
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let (status, v) = admin_call(&r, "GET", "/api/reports", &cookie, None).await;
    assert_eq!((status, v["total"].as_i64(), v["per_page"].as_i64()), (StatusCode::OK, Some(4), Some(reports::PAGE)), "{v}");
    let row = v["reports"].as_array().unwrap().iter().find(|x| x["id"] == json!(one)).unwrap();
    assert_eq!(row["comment"].as_str().unwrap().chars().count(), 200);
    assert_eq!(row["player"], json!({ "id": 1011, "name": "ijsman5530", "identity": *GV7 }));
    assert_eq!(row["files"], json!([{ "name": "bl-tracing.log", "size": 18, "dropped": false }]));
    assert_eq!((row["status"].as_str(), row["server"].as_str()), (Some("open"), Some("server-a")));
    assert!(row.get("server_log").is_none() && row.get("summary").is_none(), "only in the detail");
    let by_id = format!("q={four}");
    for (query, total) in [
        ("server=server-b", 2),
        ("problem=crash", 1),
        ("problem=join", 2),
        ("q=kiwi", 1),
        ("q=great", 1),
        (&*format!("q={}", *GV7), 3),
        (by_id.as_str(), 1),
        ("status=resolved", 0),
        ("status=all", 4),
    ] {
        let (_, v) = admin_call(&r, "GET", &format!("/api/reports?{query}"), &cookie, None).await;
        assert_eq!(v["total"].as_i64(), Some(total), "{query}: {v}");
    }

    // The detail: everything, and the player's other reports, newest first.
    let (status, v) = admin_call(&r, "GET", &format!("/api/reports/{one}"), &cookie, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["comment"].as_str().unwrap().len(), 300);
    assert_eq!(v["summary"]["session"]["joins_failed"], 2);
    assert_eq!(v["server_log"], "12:00:01 1011 join refused\n");
    assert_eq!(v["client"]["language"], "pt-BR");
    assert_eq!(v["triggers"], json!(["failed_join", "relayed"]));
    assert_eq!(v["player_known"], true);
    let others: Vec<&str> = v["others"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert_eq!(others, [three.as_str(), two.as_str()]);
    assert_eq!(
        admin_call(&r, "GET", &format!("/api/reports/{}", report_id(9)), &cookie, None).await.0,
        StatusCode::NOT_FOUND
    );

    // A file: plain text, to save, never a page.
    let resp = admin_get(&r, &format!("/api/reports/{one}/files/bl-tracing.log"), &[("cookie", cookie.as_str())]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(resp.headers()["content-disposition"], format!("attachment; filename=\"{one}-bl-tracing.log\"").as_str());
    assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    let text = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&text[..], b"line one\nline two\n");
    for path in [format!("/api/reports/{one}/files/other.log"), format!("/api/reports/{one}/files/..")] {
        assert_eq!(admin_get(&r, &path, &[("cookie", cookie.as_str())]).await.status(), StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(
        admin_get(&r, &format!("/api/reports/{one}/files/bl-tracing.log"), &[]).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // Resolving notes who and a note; reopening clears who.
    let (status, v) = admin_call(
        &r,
        "POST",
        &format!("/api/reports/{one}"),
        &cookie,
        Some(json!({ "status": "resolved", "note": "port forwarding" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        (v["status"].as_str(), v["note"].as_str(), v["resolved_by"].as_str()),
        (Some("resolved"), Some("port forwarding"), Some("admin1"))
    );
    let (_, v) = admin_call(&r, "GET", "/api/reports?status=resolved", &cookie, None).await;
    assert_eq!(v["total"], 1);
    let (_, v) = admin_call(&r, "POST", &format!("/api/reports/{one}"), &cookie, Some(json!({ "status": "open" }))).await;
    assert_eq!(
        (v["status"].as_str(), v["note"].as_str(), v["resolved_by"].as_str()),
        (Some("open"), Some("port forwarding"), None)
    );
    // A note alone (resolved twice: still who resolved it first).
    let (_, v) = admin_call(&r, "POST", &format!("/api/reports/{two}"), &cookie, Some(json!({ "status": "resolved" }))).await;
    let at = v["resolved_at"].clone();
    let fresh_eyes = admin_cookie(&t, "admin3", 3600).await;
    let (_, v) = admin_call(
        &r,
        "POST",
        &format!("/api/reports/{two}"),
        &fresh_eyes,
        Some(json!({ "status": "resolved", "note": "known issue" })),
    )
    .await;
    assert_eq!(
        (v["resolved_by"].as_str(), &v["resolved_at"], v["note"].as_str()),
        (Some("admin1"), &at, Some("known issue"))
    );
    admin_call(&r, "POST", &format!("/api/reports/{two}"), &cookie, Some(json!({ "status": "open" }))).await;
    for bad in [
        json!({ "status": "closed" }),
        json!({ "status": "open", "note": "n".repeat(501) }),
        json!({ "status": "open", "note": "a\u{7}" }),
    ] {
        assert_eq!(
            admin_call(&r, "POST", &format!("/api/reports/{one}"), &cookie, Some(bad.clone())).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    assert_eq!(
        admin_call(&r, "POST", &format!("/api/reports/{}", report_id(9)), &cookie, Some(json!({ "status": "open" })))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(t.c.open_reports().await.unwrap(), 4, "the nav's badge");

    // Deleting wants a second factor proved lately, and takes the files with it.
    let (status, v) = admin_call(&r, "DELETE", &format!("/api/reports/{one}"), &cookie, None).await;
    assert_eq!((status, v["reverify"].as_bool()), (StatusCode::FORBIDDEN, Some(true)));
    let fresh = admin_cookie(&t, "admin2", 0).await;
    assert_eq!(admin_call(&r, "DELETE", &format!("/api/reports/{one}"), &fresh, None).await.0, StatusCode::OK);
    assert!(!t.dir.join("reports").join(&one).exists());
    assert_eq!(admin_call(&r, "DELETE", &format!("/api/reports/{one}"), &fresh, None).await.0, StatusCode::NOT_FOUND);
    let events: Vec<String> = sqlx::query_scalar("SELECT event FROM audit WHERE event LIKE 'report: %' ORDER BY id")
        .fetch_all(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(
        events,
        [
            "report: resolved",
            "report: reopened",
            "report: resolved",
            "report: noted",
            "report: reopened",
            "report: deleted"
        ]
    );

    // Which reports go to the webhook.
    let (_, v) = admin_call(&r, "GET", "/api/alerts", &cookie, None).await;
    assert_eq!(v["report_alerts"], "problems");
    assert_eq!(
        admin_call(&r, "PUT", "/api/alerts/reports", &cookie, Some(json!({ "mode": "all" }))).await.0,
        StatusCode::OK
    );
    assert_eq!(
        admin_call(&r, "PUT", "/api/alerts/reports", &cookie, Some(json!({ "mode": "loud" }))).await.0,
        StatusCode::BAD_REQUEST
    );
    let (_, v) = admin_call(&r, "GET", "/api/alerts", &cookie, None).await;
    assert_eq!(v["report_alerts"], "all");
}

#[tokio::test]
async fn game_logs_sent_on_their_own_are_kept_apart_from_reports() {
    let t = start("reports-auto").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    let (log, mine) = (report_id(1), report_id(2));
    let mut auto = report_body(&log);
    auto["triggers"] = json!(["auto", "auto:join_failed"]);
    auto["rating"] = json!(null);
    auto["problems"] = json!([]);
    auto["comment"] = json!("");
    assert_eq!(t.call("POST", "/v1/reports", Some(&a), Some(auto)).await.0, StatusCode::OK);
    assert_eq!(t.call("POST", "/v1/reports", Some(&a), Some(report_body(&mine))).await.0, StatusCode::OK);
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let (_, v) = admin_call(&r, "GET", "/api/reports", &cookie, None).await;
    assert_eq!(
        (v["total"].as_i64(), v["reports"][0]["id"].as_str()),
        (Some(1), Some(mine.as_str())),
        "only the player's own are open: {v}"
    );
    let (_, v) = admin_call(&r, "GET", "/api/reports?status=auto", &cookie, None).await;
    assert_eq!((v["total"].as_i64(), v["reports"][0]["status"].as_str()), (Some(1), Some("auto")), "{v}");
    assert_eq!(t.c.open_reports().await.unwrap(), 1);
}

#[tokio::test]
async fn players_shadownet_snapshots_are_kept_for_admins() {
    use std::io::Write as _;

    use base64::Engine as _;
    let t = start("content").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    let gz = |text: &str| {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(text.as_bytes()).unwrap();
        base64::engine::general_purpose::STANDARD.encode(e.finish().unwrap())
    };
    let body = |updated_at: i64, text: &str| json!({ "player": { "id": 1020, "name": "PlaySkill" }, "type": content::SHADOWNET, "size": text.len(), "updated_at": updated_at, "gzip_base64": gz(text) });
    let now = identity::now();
    let first = r#"{"Loadout:Items":{"1":["2"]},"Purchase":{},"Challenges":{"9":{"T":1,"P":5}}}"#;
    // Only for a player the server has told of (any id could be sent): later, not dropped.
    assert_eq!(
        t.call("POST", "/v1/content", Some(&a), Some(body(now - 60, first))).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    sqlx::query("INSERT INTO players (server_id, id, name, updated_at) VALUES ('server-a', 1020, 'PlaySkill', 0)")
        .execute(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(t.call("POST", "/v1/content", Some(&a), Some(body(now - 60, first))).await.0, StatusCode::OK);
    assert_eq!(t.call("POST", "/v1/content", None, Some(body(now, first))).await.0, StatusCode::UNAUTHORIZED);
    // Not JSON, or another type: refused.
    assert_eq!(t.call("POST", "/v1/content", Some(&a), Some(body(now, "not json"))).await.0, StatusCode::BAD_REQUEST);
    let mut other = body(now, first);
    other["type"] = json!(0x8000_0002_i64);
    assert_eq!(t.call("POST", "/v1/content", Some(&a), Some(other)).await.0, StatusCode::BAD_REQUEST);
    // An older one doesn't replace a newer.
    assert_eq!(
        t.call("POST", "/v1/content", Some(&a), Some(body(now - 600, r#"{"Purchase":{"x":1}}"#))).await.0,
        StatusCode::OK
    );

    let cookie = admin_cookie(&t, "admin1", 3600).await;
    assert_eq!(admin_call(&r, "GET", "/api/players/server-a/1020/content", "", None).await.0, StatusCode::UNAUTHORIZED);
    let (status, v) = admin_call(&r, "GET", "/api/players/server-a/1020/content", &cookie, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!((v["summary"]["items"].as_u64(), v["summary"]["challenges_started"].as_u64()), (Some(1), Some(1)));
    assert_eq!(v["snapshot"]["Challenges"]["9"]["P"], 5);
    assert_eq!(admin_call(&r, "GET", "/api/players/server-a/7/content", &cookie, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn players_read_the_admins_replies_to_their_reports() {
    let t = start("reports-replies").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    let me = identity::Identity::generate();
    let one = report_id(1);
    let mut body = report_body(&one);
    body["player"] = json!({ "id": 1032, "name": "Oni", "identity": me.global_id() });
    assert_eq!(t.call("POST", "/v1/reports", Some(&a), Some(body)).await.0, StatusCode::OK);

    let mine = |who: &identity::Identity, signer: &identity::Identity| {
        let time = identity::now();
        format!(
            "/v1/reports/mine?identity={}&time={time}&signature={}",
            who.global_id(),
            signer.sign(&identity::reports_message("coordinator.test", time))
        )
    };
    let (status, v) = t.call("GET", &mine(&me, &me), None, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!((v["reports"][0]["id"].as_str(), v["reports"][0]["reply"].as_str()), (Some(one.as_str()), Some("")));
    assert!(
        v["reports"][0].get("note").is_none() && v["reports"][0].get("files").is_none(),
        "only what the player said, and the reply"
    );
    // Someone else's signature, or the suggestions' message, reads nothing.
    assert_eq!(t.call("GET", &mine(&me, &identity::Identity::generate()), None, None).await.0, StatusCode::FORBIDDEN);
    let time = identity::now();
    let wrong = format!(
        "/v1/reports/mine?identity={}&time={time}&signature={}",
        me.global_id(),
        me.sign(&identity::suggestions_message(time))
    );
    assert_eq!(t.call("GET", &wrong, None, None).await.0, StatusCode::FORBIDDEN);

    // An admin replies (and notes something only admins see).
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let (status, v) = admin_call(
        &r,
        "POST",
        &format!("/api/reports/{one}"),
        &cookie,
        Some(json!({ "status": "resolved", "note": "false banner", "reply": "Thanks Oni! That warning was wrong; 0.4.3 fixes it." })),
    )
    .await;
    assert_eq!((status, v["replied_by"].as_str()), (StatusCode::OK, Some("admin1")), "{v}");
    let (_, v) = t.call("GET", &mine(&me, &me), None, None).await;
    let r0 = &v["reports"][0];
    assert_eq!(
        (r0["reply"].as_str(), r0["status"].as_str()),
        (Some("Thanks Oni! That warning was wrong; 0.4.3 fixes it."), Some("resolved"))
    );
    assert!(r0["replied_at"].as_i64().is_some());
    // Too long a reply is refused.
    let (status, _) = admin_call(
        &r,
        "POST",
        &format!("/api/reports/{one}"),
        &cookie,
        Some(json!({ "status": "resolved", "reply": "r".repeat(1001) })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reports_files_keep_under_the_cap_and_reports_go_after_90_days() {
    let t = start("reports-storage").await;
    let a = t.join("server-a").await;
    let ids: Vec<String> = (1..=3).map(report_id).collect();
    for id in &ids {
        assert_eq!(t.call("POST", "/v1/reports", Some(&a), Some(report_body(id))).await.0, StatusCode::OK);
    }
    let one: i64 = sqlx::query_scalar("SELECT stored FROM player_report_files LIMIT 1").fetch_one(&t.c.pool).await.unwrap();
    // Room for two reports' files: the oldest one's go, the report stays.
    sqlx::query("UPDATE player_reports SET received_at = received_at - 100 WHERE id = ?")
        .bind(&ids[0])
        .execute(&t.c.pool)
        .await
        .unwrap();
    t.c.report_storage_cap.store(u64::try_from(one * 2).unwrap(), std::sync::atomic::Ordering::Relaxed);
    t.c.cap_report_storage().await.unwrap();
    assert!(!t.dir.join("reports").join(&ids[0]).exists());
    assert!(t.dir.join("reports").join(&ids[1]).exists() && t.dir.join("reports").join(&ids[2]).exists());
    assert_eq!(t.c.report_file(&ids[0], "bl-tracing.log").await.unwrap(), Some(None), "gone, said so");
    let r = admin_router(&t);
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let resp = admin_get(&r, &format!("/api/reports/{}/files/bl-tracing.log", ids[0]), &[("cookie", cookie.as_str())]).await;
    assert_eq!(resp.status(), StatusCode::GONE);
    let (_, v) = admin_call(&r, "GET", &format!("/api/reports/{}", ids[0]), &cookie, None).await;
    assert_eq!(v["files"][0]["dropped"], true);
    assert_eq!(v["comment"], "couldn't join my friend", "the report itself stays");
    // A new report over the cap pushes out the next oldest.
    t.call("POST", "/v1/reports", Some(&a), Some(report_body(&report_id(4)))).await;
    assert!(!t.dir.join("reports").join(&ids[1]).exists());
    assert!(t.dir.join("reports").join(report_id(4)).exists());

    // Past 90 days: the report and its files go, with the hourly rollup.
    sqlx::query("UPDATE player_reports SET received_at = received_at - 91 * 86400 WHERE id = ?")
        .bind(&ids[2])
        .execute(&t.c.pool)
        .await
        .unwrap();
    t.c.roll_up().await.unwrap();
    assert!(!t.c.report_exists(&ids[2]).await.unwrap());
    assert!(!t.dir.join("reports").join(&ids[2]).exists());
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM player_report_files").fetch_one(&t.c.pool).await.unwrap();
    assert_eq!(left, 3, "its file rows went with it");
}

#[tokio::test]
async fn all_servers_silent_is_one_alert_about_the_coordinator() {
    let t = start("all-silent").await;
    t.join("srv-a").await;
    t.join("srv-b").await;
    let now = identity::now();
    let seen = |id: &'static str, at: i64| sqlx::query("UPDATE servers SET last_seen = ? WHERE id = ?").bind(at).bind(id).execute(&t.c.pool);
    let active = || async {
        t.c.alerts().await.unwrap()["active"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["kind"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    // Both silent: the coordinator can't be reached, not two servers down.
    seen("srv-a", now - 600).await.unwrap();
    seen("srv-b", now - 600).await.unwrap();
    t.c.check_alerts().await.unwrap();
    assert_eq!(active().await, ["unreachable"]);
    // One back: the other is the one down.
    seen("srv-a", now).await.unwrap();
    t.c.check_alerts().await.unwrap();
    assert_eq!(active().await, ["offline"]);
    // srv-a sends heartbeats, but its API port stopped answering the coordinator's checks.
    for ago in [0, 60, 120] {
        sqlx::query("INSERT INTO server_pings (server_id, at, ms) VALUES ('srv-a', ?, NULL)")
            .bind(now - ago)
            .execute(&t.c.pool)
            .await
            .unwrap();
    }
    t.c.check_alerts().await.unwrap();
    let mut kinds = active().await;
    kinds.sort();
    assert_eq!(kinds, ["api", "offline"]);
    // It answers again.
    sqlx::query("UPDATE server_pings SET ms = 1.5 WHERE server_id = 'srv-a' AND at = ?")
        .bind(now)
        .execute(&t.c.pool)
        .await
        .unwrap();
    t.c.check_alerts().await.unwrap();
    assert_eq!(active().await, ["offline"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn servers_reporting_at_once_dont_lock_each_other_out() {
    // In WAL mode a transaction that reads and then writes can't take the write lock once
    // another connection has written since it read: "database is locked", at once, without
    // waiting. Transactions begin IMMEDIATE, so they wait their turn.
    let t = start("concurrent-writes").await;
    for i in 0..6 {
        sqlx::query("INSERT INTO servers (id, secret_hash, joined_at) VALUES (?, ?, 0)")
            .bind(format!("s{i}"))
            .bind(format!("h{i}"))
            .execute(&t.c.pool)
            .await
            .unwrap();
    }
    let mut set = tokio::task::JoinSet::new();
    for round in 0..30i64 {
        for i in 0..6 {
            let c = Arc::clone(&t.c);
            set.spawn(async move {
                let players: Vec<Value> = (0..5).map(|p| json!({ "id": p, "name": format!("P{p}"), "matches": round })).collect();
                c.record_players(&format!("s{i}"), &json!({ "full": true, "players": players }))
                    .await
                    .err()
                    .map(|e| e.to_string())
            });
        }
    }
    let mut failures = Vec::new();
    while let Some(r) = set.join_next().await {
        failures.extend(r.unwrap());
    }
    assert!(failures.is_empty(), "{} of 180 failed, e.g. {:?}", failures.len(), failures.first());
}

#[tokio::test]
async fn the_database_is_in_wal_mode_for_the_live_backup() {
    let t = start("wal").await;
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode").fetch_one(&t.c.pool).await.unwrap();
    assert_eq!(mode, "wal");
}

#[tokio::test]
async fn session_events_are_taken_once_and_shown_with_their_problems() {
    let t = start("session-events").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    let now = identity::now();
    t.call(
        "POST",
        "/v1/players",
        Some(&a),
        Some(json!({ "full": true, "players": [player(11, "Viper", None)], "sessions": [{ "id": 1, "player": 11, "start": now - 1800, "end": now - 60 }] })),
    )
    .await;
    let coop = json!({ "room": 16, "room_kind": "match", "mode": "coop", "private": true, "host": 5, "host_name": "Theusma" });
    let events = json!({ "events": [
        { "id": 1, "at": now - 1700, "last_at": now - 1700, "player": 11, "name": "Viper", "kind": "join", "detail": coop, "count": 1 },
        { "id": 2, "at": now - 300, "last_at": now - 300, "player": 11, "name": "Viper", "kind": "relay_drop",
          "detail": { "direction": "sending", "before": 65, "after": 8 }, "count": 1 },
        { "id": 3, "at": now - 240, "last_at": now - 240, "player": 11, "name": "Viper", "kind": "leave",
          "detail": { "room": 16, "how": "left", "ended": false, "room_kind": "match", "mode": "coop" }, "count": 1 },
        { "id": 4, "at": now - 900, "last_at": now - 100, "player": null, "name": "Renegade", "kind": "signin_refused",
          "detail": { "reason": "outdated", "via": "api", "client": "game/0.4.0" }, "count": 300 },
        { "id": 5, "at": now, "last_at": now, "player": 11, "name": "Viper", "kind": "made_up", "detail": {}, "count": 1 },
    ] });
    let (status, v) = t.call("POST", "/v1/events", Some(&a), Some(events)).await;
    assert_eq!((status, v["kept"].as_i64()), (StatusCode::OK, Some(4)), "{v}");
    // A repeat counted since comes again with the same id.
    let again = json!({ "events": [{ "id": 4, "at": now - 900, "last_at": now - 50, "player": null, "name": "Renegade", "kind": "signin_refused",
        "detail": { "reason": "outdated", "via": "api", "client": "game/0.4.0" }, "count": 351 }] });
    assert_eq!(t.call("POST", "/v1/events", Some(&a), Some(again)).await.0, StatusCode::OK);
    assert_eq!(t.call("POST", "/v1/events", None, Some(json!({ "events": [] }))).await.0, StatusCode::UNAUTHORIZED);

    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let (status, v) = admin_call(&r, "GET", "/api/sessions", &cookie, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let titles: Vec<&str> = v["problems"].as_array().unwrap().iter().filter_map(|p| p["title"].as_str()).collect();
    assert!(titles.contains(&"Viper dropped out of a co-op match"), "{titles:?}");
    assert!(titles.contains(&"Renegade couldn't sign in"), "{titles:?}");
    let renegade = v["problems"].as_array().unwrap().iter().find(|p| p["name"] == "Renegade").unwrap();
    assert!(renegade["text"].as_str().unwrap().contains("351 times"), "{renegade}");
    let viper = v["players"].as_array().unwrap().iter().find(|p| p["name"] == "Viper").unwrap();
    assert_eq!(viper["rooms"][0]["with"], json!([]), "{viper}");
    assert_eq!(viper["rooms"][0]["to"].as_i64(), Some(now - 240));
    assert_eq!(admin_call(&r, "GET", "/api/sessions", "", None).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_map_lists_who_is_online_from_the_last_pulse_only() {
    let t = start("online-now").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    let pulse = json!({ "players": { "online": 1, "total": 3 }, "online": [
        { "id": 1011, "name": "Viper", "country": "AR", "country_name": "Argentina", "region": "Buenos Aires", "city": "Buenos Aires",
          "lat": -34.6, "lon": -58.4, "status": "match", "mode": "coop", "with": ["Theusma"], "since": 1, "network": "relayed", "extra": "dropped" },
    ] });
    assert_eq!(t.call("POST", "/v1/pulse", Some(&a), Some(pulse)).await.0, StatusCode::OK);
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let (status, v) = admin_call(&r, "GET", "/api/online", &cookie, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let players = v["players"].as_array().unwrap();
    assert_eq!(players.len(), 1);
    assert!(players[0].get("extra").is_none());
    assert_eq!(
        (players[0]["name"].as_str(), players[0]["server"].as_str(), players[0]["city"].as_str()),
        (Some("Viper"), Some("server-a"), Some("Buenos Aires"))
    );
    assert_eq!(admin_call(&r, "GET", "/api/online", "", None).await.0, StatusCode::UNAUTHORIZED);
    // Nothing of it is stored: only the live point.
    let stored: String = sqlx::query_scalar("SELECT point FROM pulses").fetch_one(&t.c.pool).await.unwrap();
    assert!(!stored.contains("Viper"), "{stored}");
}

/// The admin router as a browser at `ip` reaches it (each test its own address: sign-in
/// attempts are limited per address).
fn admin_router_at(t: &Test, ip: [u8; 4]) -> Router {
    let _ = t.c.admin.set(admin::Config::new("https://admin.example", false).unwrap());
    admin::router(Arc::clone(&t.c)).layer(axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from((ip, 1))))
}

/// A request from the admin UI's own pages, as a browser sends it: the status, the JSON, and
/// the session cookie it set (if any).
async fn admin_send(router: &Router, method: &str, path: &str, cookie: &str, body: Option<Value>) -> (StatusCode, Value, Option<String>) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", cookie)
        .header("origin", "https://admin.example")
        .header("x-fes-admin", "1")
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let set = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("__Host-fes-admin=") && !v.contains("Max-Age=0"))
        .map(|v| v.split(';').next().unwrap().to_string());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), set)
}

/// The authenticator app's code for a 30-second step.
fn totp_code(secret: &str, step: i64) -> String {
    format!("{:06}", admin::auth::totp_at(&identity::base32_decode(secret).unwrap(), step))
}

/// The current 30-second step, waiting first if it's about to end: codes from the step
/// before to the one after are taken, so a test that uses each of them once stays inside
/// that window.
async fn totp_step() -> i64 {
    let into = identity::now().rem_euclid(30);
    if into > 20 {
        tokio::time::sleep(std::time::Duration::from_secs((31 - into) as u64)).await;
    }
    identity::now().div_euclid(30)
}

#[tokio::test]
async fn an_admin_sets_up_and_signs_in_with_a_password_and_an_authenticator() {
    let step = totp_step().await;
    let t = start("admin-sign-in").await;
    let r = admin_router_at(&t, [192, 0, 2, 10]);
    let link = t.c.admin_setup_link("kiwi", false).await.unwrap();
    let token = link.split("#setup=").nth(1).unwrap().to_string();
    let password = "correct horse battery staple";

    // The setup link: a weak password is refused and the link still works; then it's used up.
    let (status, ..) = admin_send(&r, "POST", "/api/setup", "", Some(json!({ "token": token, "password": "kiwi" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a weak password");
    let (status, v, enroll) = admin_send(&r, "POST", "/api/setup", "", Some(json!({ "token": token, "password": password }))).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("enroll")), "{v}");
    let enroll = enroll.expect("a session to add a second factor in");
    let (status, ..) = admin_send(&r, "POST", "/api/setup", "", Some(json!({ "token": token, "password": password }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a setup link works once");

    // Before a second factor, nothing else.
    assert_eq!(admin_send(&r, "GET", "/api/overview", &enroll, None).await.0, StatusCode::UNAUTHORIZED);
    let (status, v, _) = admin_send(&r, "POST", "/api/me/totp/begin", &enroll, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let secret = v["secret"].as_str().unwrap().to_string();
    assert!(v["uri"].as_str().unwrap().starts_with("otpauth://totp/"), "{v}");
    let (status, ..) = admin_send(&r, "POST", "/api/me/totp/confirm", &enroll, Some(json!({ "code": "000000" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a wrong code");
    let (status, v, full) = admin_send(&r, "POST", "/api/me/totp/confirm", &enroll, Some(json!({ "code": totp_code(&secret, step - 1) }))).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("full")), "{v}");
    let codes: Vec<String> = v["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
    assert!(codes.len() >= 8, "recovery codes, the first time: {v}");
    let full = full.expect("a full session");
    assert_eq!(
        admin_send(&r, "GET", "/api/overview", &enroll, None).await.0,
        StatusCode::UNAUTHORIZED,
        "the enrolling session is gone"
    );
    let (status, v, _) = admin_send(&r, "GET", "/api/me", &full, None).await;
    assert_eq!((status, v["username"].as_str(), v["totp"].as_bool()), (StatusCode::OK, Some("kiwi"), Some(true)), "{v}");

    // Signing out ends the session.
    admin_send(&r, "POST", "/api/logout", &full, None).await;
    assert_eq!(admin_send(&r, "GET", "/api/me", &full, None).await.0, StatusCode::UNAUTHORIZED);

    // Signing in: the password, then the code. A code used before isn't taken again.
    let (status, ..) = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": "wrong password here" }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, v, half) = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": password }))).await;
    assert_eq!((status, v["stage"].as_str(), v["totp"].as_bool()), (StatusCode::OK, Some("password"), Some(true)), "{v}");
    let half = half.unwrap();
    assert_eq!(
        admin_send(&r, "GET", "/api/me", &half, None).await.0,
        StatusCode::UNAUTHORIZED,
        "a password alone isn't enough"
    );
    let (status, ..) = admin_send(&r, "POST", "/api/login/totp", &half, Some(json!({ "code": totp_code(&secret, step - 1) }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the code used to enrol, again");
    let (status, v, full) = admin_send(&r, "POST", "/api/login/totp", &half, Some(json!({ "code": totp_code(&secret, step) }))).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("full")), "{v}");
    let full = full.unwrap();
    assert_eq!(admin_send(&r, "GET", "/api/me", &full, None).await.0, StatusCode::OK);
    assert_eq!(
        admin_send(&r, "GET", "/api/me", &half, None).await.0,
        StatusCode::UNAUTHORIZED,
        "the half-way session is gone"
    );

    // Changes from other sites are refused, even with the cookie.
    let req = Request::builder()
        .method("POST")
        .uri("/api/logout")
        .header("cookie", &full)
        .header("origin", "https://evil.example")
        .header("x-fes-admin", "1")
        .body(Body::empty())
        .unwrap();
    assert_eq!(r.clone().oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN);
    let req = Request::builder().method("POST").uri("/api/logout").header("cookie", &full).body(Body::empty()).unwrap();
    assert_eq!(r.clone().oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN, "without the admin UI's header");
    assert_eq!(admin_send(&r, "GET", "/api/me", &full, None).await.0, StatusCode::OK, "still signed in");
}

#[tokio::test]
async fn sensitive_changes_want_a_second_factor_lately_and_recovery_codes_work_once() {
    let step = totp_step().await;
    let t = start("admin-recent").await;
    let r = admin_router_at(&t, [192, 0, 2, 11]);
    let token = t.c.admin_setup_link("kiwi", false).await.unwrap().split("#setup=").nth(1).unwrap().to_string();
    let password = "correct horse battery staple";
    let enroll = admin_send(&r, "POST", "/api/setup", "", Some(json!({ "token": token, "password": password })))
        .await
        .2
        .unwrap();
    let secret = admin_send(&r, "POST", "/api/me/totp/begin", &enroll, None).await.1["secret"].as_str().unwrap().to_string();
    let (_, v, full) = admin_send(&r, "POST", "/api/me/totp/confirm", &enroll, Some(json!({ "code": totp_code(&secret, step - 1) }))).await;
    let codes: Vec<String> = v["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
    let full = full.unwrap();

    // Just verified: adding an admin works. Ten minutes on, it wants the code again.
    let (status, v, _) = admin_send(&r, "POST", "/api/admins", &full, Some(json!({ "username": "tank" }))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v["link"].as_str().unwrap().starts_with("https://admin.example/#setup="), "{v}");
    sqlx::query("UPDATE admin_sessions SET verified_at = verified_at - 601").execute(&t.c.pool).await.unwrap();
    for (method, path, body) in [
        ("POST", "/api/admins", Some(json!({ "username": "tank2" }))),
        ("DELETE", "/api/servers/nope", None),
        ("POST", "/api/me/password", Some(json!({ "current": password, "new": "another long passphrase here" }))),
        ("PUT", "/api/restrictions", Some(json!({ "networks": [], "countries": [] }))),
    ] {
        let (status, v, _) = admin_send(&r, method, path, &full, body).await;
        assert_eq!((status, v["reverify"].as_bool()), (StatusCode::FORBIDDEN, Some(true)), "{method} {path}: {v}");
    }
    assert_eq!(admin_send(&r, "GET", "/api/me", &full, None).await.0, StatusCode::OK, "reading is fine");
    let (status, v, _) = admin_send(&r, "POST", "/api/login/totp", &full, Some(json!({ "code": totp_code(&secret, step) }))).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("full")), "confirming it's them: {v}");
    let (status, v, _) = admin_send(&r, "POST", "/api/admins", &full, Some(json!({ "username": "tank2" }))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    // Rules that would lock out the admin making them are refused.
    let (status, v, _) = admin_send(&r, "PUT", "/api/restrictions", &full, Some(json!({ "networks": ["198.51.100.0/24"], "countries": [] }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("lock you out"), "{v}");

    // A recovery code instead of the app: once.
    admin_send(&r, "POST", "/api/logout", &full, None).await;
    let half = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": password })))
        .await
        .2
        .unwrap();
    let (status, v, full) = admin_send(&r, "POST", "/api/login/recovery", &half, Some(json!({ "code": codes[0] }))).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("full")), "{v}");
    let (_, v, _) = admin_send(&r, "GET", "/api/me", full.as_deref().unwrap(), None).await;
    assert_eq!(v["recovery_left"].as_i64(), Some(codes.len() as i64 - 1), "{v}");
    admin_send(&r, "POST", "/api/logout", full.as_deref().unwrap(), None).await;
    let half = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": password })))
        .await
        .2
        .unwrap();
    let (status, ..) = admin_send(&r, "POST", "/api/login/recovery", &half, Some(json!({ "code": codes[0] }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a used recovery code");
}

#[tokio::test]
async fn failed_sign_ins_lock_an_account_and_admins_cant_lock_everyone_out() {
    let t = start("admin-lockout").await;
    let r = admin_router_at(&t, [192, 0, 2, 12]);
    let token = t.c.admin_setup_link("kiwi", false).await.unwrap().split("#setup=").nth(1).unwrap().to_string();
    let password = "correct horse battery staple";
    admin_send(&r, "POST", "/api/setup", "", Some(json!({ "token": token, "password": password }))).await;

    // Five wrong passwords: the account waits, even for the right one.
    for _ in 0..5 {
        let (status, ..) = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": "not the password" }))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, v, cookie) = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": password }))).await;
    assert_eq!((status, cookie), (StatusCode::TOO_MANY_REQUESTS, None), "{v}");
    // A name that doesn't exist answers as a wrong password does.
    let (status, v, _) = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "nobody", "password": password }))).await;
    assert_eq!((status, v["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("wrong name or password")));

    // Nobody disables, removes or resets themselves here (so an admin always remains);
    // another admin's disable ends that admin's sessions.
    let tank = admin_cookie(&t, "tank", 0).await;
    let rata = admin_cookie(&t, "rata", 0).await;
    let ids: Vec<(i64, String)> = sqlx::query_as("SELECT id, username FROM admins").fetch_all(&t.c.pool).await.unwrap();
    let id_of = |name: &str| ids.iter().find(|(_, n)| n == name).unwrap().0;
    for action in ["disable", "remove", "reset"] {
        let (status, v, _) = admin_send(&r, "POST", &format!("/api/admins/{}/{action}", id_of("tank")), &tank, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{action} themselves: {v}");
    }
    let (status, v, _) = admin_send(&r, "POST", &format!("/api/admins/{}/disable", id_of("tank")), &rata, None).await;
    assert_eq!(status, StatusCode::OK, "another admin disables tank: {v}");
    assert_eq!(
        admin_send(&r, "GET", "/api/me", &tank, None).await.0,
        StatusCode::UNAUTHORIZED,
        "a disabled admin's sessions end"
    );
    let (status, ..) = admin_send(&r, "POST", "/api/admins/99999/disable", &rata, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no such admin");
    // Enabled again and reset: their sign-in is cleared, and a new setup link is the way back.
    admin_send(&r, "POST", &format!("/api/admins/{}/enable", id_of("kiwi")), &rata, None).await;
    let (status, v, _) = admin_send(&r, "POST", &format!("/api/admins/{}/reset", id_of("kiwi")), &rata, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let (status, ..) = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": password }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a reset admin's old password");
}

/// The admin UI's own site, as its pages' passkeys are made for it.
const ADMIN_SITE: admin::webauthn::Site<'static> = admin::webauthn::Site {
    rp_id: "admin.example",
    origin: "https://admin.example",
};

/// A passkey's `challenge_id` and challenge from a `begin` answer.
fn passkey_challenge(v: &Value) -> (Value, String) {
    (v["challenge_id"].clone(), v["options"]["challenge"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn an_admin_adds_a_passkey_and_signs_in_with_it_alone_or_after_a_password() {
    use admin::webauthn::b64url;
    use admin::webauthn::SoftAuthenticator;
    let t = start("admin-passkeys").await;
    let r = admin_router_at(&t, [192, 0, 2, 20]);
    let token = t.c.admin_setup_link("kiwi", false).await.unwrap().split("#setup=").nth(1).unwrap().to_string();
    let password = "correct horse battery staple";
    let enroll = admin_send(&r, "POST", "/api/setup", "", Some(json!({ "token": token, "password": password })))
        .await
        .2
        .unwrap();
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM admins WHERE username = 'kiwi'").fetch_one(&t.c.pool).await.unwrap();
    let handle = admin_id.to_string().into_bytes();

    // A passkey as the first second factor: made for this site and this admin, then the session
    // is a full one, with recovery codes.
    let mut key = SoftAuthenticator::new(b"kiwi-laptop-key");
    let (status, v, _) = admin_send(&r, "POST", "/api/me/passkeys/begin", &enroll, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        (v["options"]["rp"]["id"].as_str(), v["options"]["user"]["id"].as_str()),
        (Some("admin.example"), Some(b64url(&handle).as_str()))
    );
    let (id, challenge) = passkey_challenge(&v);
    let finish = json!({ "challenge_id": id, "credential": key.create(&challenge, &ADMIN_SITE), "name": "Laptop\u{7}" });
    let (status, v, full) = admin_send(&r, "POST", "/api/me/passkeys/finish", &enroll, Some(finish.clone())).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("full")), "{v}");
    assert!(!v["recovery_codes"].as_array().unwrap().is_empty(), "{v}");
    let full = full.unwrap();
    let (_, me, _) = admin_send(&r, "GET", "/api/me", &full, None).await;
    assert_eq!(me["passkeys"][0]["name"], "Laptop", "control characters are left out: {me}");
    let key_id = me["passkeys"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(key_id, b64url(&key.id));
    // A challenge is good once; the same passkey isn't added twice.
    let (status, ..) = admin_send(&r, "POST", "/api/me/passkeys/finish", &full, Some(finish)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a used challenge");
    let (_, v, _) = admin_send(&r, "POST", "/api/me/passkeys/begin", &full, None).await;
    assert_eq!(v["options"]["excludeCredentials"][0]["id"], key_id.as_str(), "{v}");
    let (id, challenge) = passkey_challenge(&v);
    let again = json!({ "challenge_id": id, "credential": key.create(&challenge, &ADMIN_SITE) });
    assert_eq!(admin_send(&r, "POST", "/api/me/passkeys/finish", &full, Some(again)).await.0, StatusCode::CONFLICT);

    // Signed out, the passkey alone signs in (it proves the person, not just the device).
    admin_send(&r, "POST", "/api/logout", &full, None).await;
    let begin = || async { passkey_challenge(&admin_send(&r, "POST", "/api/login/passkey/begin", "", None).await.1) };
    let (id, challenge) = begin().await;
    let assertion = key.get(&challenge, &ADMIN_SITE, ADMIN_SITE.rp_id, SoftAuthenticator::VERIFIED, &handle);
    let (status, v, cookie) = admin_send(&r, "POST", "/api/login/passkey/finish", "", Some(json!({ "challenge_id": id, "credential": assertion }))).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("full")), "{v}");
    assert_eq!(admin_send(&r, "GET", "/api/overview", &cookie.unwrap(), None).await.0, StatusCode::OK);

    // What doesn't sign in, each on a challenge of its own.
    let refused = |what: &'static str, credential: Value, id: Value| {
        let r = r.clone();
        async move {
            let (status, v, cookie) = admin_send(&r, "POST", "/api/login/passkey/finish", "", Some(json!({ "challenge_id": id, "credential": credential }))).await;
            assert!(cookie.is_none() && status != StatusCode::OK, "{what}: {status} {v}");
            v["error"].as_str().unwrap_or_default().to_string()
        }
    };
    let (id, challenge) = begin().await;
    let unverified = key.get(&challenge, &ADMIN_SITE, ADMIN_SITE.rp_id, SoftAuthenticator::PRESENT_ONLY, &handle);
    refused("the user wasn't verified", unverified, id).await;
    let (id, challenge) = begin().await;
    key.count -= 2;
    let replayed = key.get(&challenge, &ADMIN_SITE, ADMIN_SITE.rp_id, SoftAuthenticator::VERIFIED, &handle);
    refused("a counter that didn't go up (a cloned key)", replayed, id).await;
    key.count += 2;
    let (id, challenge) = begin().await;
    let someone_else = key.get(&challenge, &ADMIN_SITE, ADMIN_SITE.rp_id, SoftAuthenticator::VERIFIED, b"999");
    assert!(refused("another admin's user handle", someone_else, id).await.contains("someone else"));
    let (id, challenge) = begin().await;
    let mut stranger = SoftAuthenticator::new(b"not-registered");
    let unknown = stranger.get(&challenge, &ADMIN_SITE, ADMIN_SITE.rp_id, SoftAuthenticator::VERIFIED, &handle);
    assert!(refused("a passkey that isn't registered", unknown, id).await.contains("isn't registered"));
    let (id, challenge) = begin().await;
    let elsewhere = key.get(&challenge, &ADMIN_SITE, "evil.example", SoftAuthenticator::VERIFIED, &handle);
    refused("signed for another site", elsewhere, id.clone()).await;
    let fine = key.get(&challenge, &ADMIN_SITE, ADMIN_SITE.rp_id, SoftAuthenticator::VERIFIED, &handle);
    refused("a challenge already tried", fine, id).await;

    // After the password, the passkey is the second factor, and only this admin's are offered.
    let (_, v, half) = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": password }))).await;
    let half = half.unwrap_or_else(|| panic!("{v}"));
    let (_, v, _) = admin_send(&r, "POST", "/api/login/passkey/begin", &half, None).await;
    assert_eq!(v["options"]["allowCredentials"], json!([{ "type": "public-key", "id": key_id }]), "{v}");
    let (id, challenge) = passkey_challenge(&v);
    let assertion = key.get(&challenge, &ADMIN_SITE, ADMIN_SITE.rp_id, SoftAuthenticator::VERIFIED, &handle);
    let (status, v, full) = admin_send(&r, "POST", "/api/login/passkey/finish", &half, Some(json!({ "challenge_id": id, "credential": assertion }))).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("full")), "{v}");
    let full = full.unwrap();

    // An account always keeps a second factor: the only passkey stays until there's an app too.
    let path = format!("/api/me/passkeys/{key_id}");
    let (status, v, _) = admin_send(&r, "DELETE", &path, &full, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
    let step = totp_step().await;
    let secret = admin_send(&r, "POST", "/api/me/totp/begin", &full, None).await.1["secret"].as_str().unwrap().to_string();
    let (status, v, _) = admin_send(&r, "POST", "/api/me/totp/confirm", &full, Some(json!({ "code": totp_code(&secret, step) }))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(admin_send(&r, "DELETE", &path, &full, None).await.0, StatusCode::OK);
    assert_eq!(admin_send(&r, "DELETE", &path, &full, None).await.0, StatusCode::NOT_FOUND, "gone already");
    let (status, v, _) = admin_send(&r, "DELETE", "/api/me/totp", &full, None).await;
    assert!(status == StatusCode::BAD_REQUEST && v["error"].as_str().unwrap().contains("passkey"), "{status} {v}");
    assert_eq!(admin_send(&r, "GET", "/api/me", &full, None).await.1["passkeys"], json!([]));
}

#[tokio::test]
async fn an_admin_changes_their_password_and_recovery_codes_and_ends_their_other_sessions() {
    let step = totp_step().await;
    let t = start("admin-account").await;
    let r = admin_router_at(&t, [192, 0, 2, 21]);
    let token = t.c.admin_setup_link("kiwi", false).await.unwrap().split("#setup=").nth(1).unwrap().to_string();
    let password = "correct horse battery staple";
    let enroll = admin_send(&r, "POST", "/api/setup", "", Some(json!({ "token": token, "password": password })))
        .await
        .2
        .unwrap();
    let secret = admin_send(&r, "POST", "/api/me/totp/begin", &enroll, None).await.1["secret"].as_str().unwrap().to_string();
    let (_, v, here) = admin_send(&r, "POST", "/api/me/totp/confirm", &enroll, Some(json!({ "code": totp_code(&secret, step - 1) }))).await;
    let old_codes: Vec<String> = v["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
    let here = here.unwrap();
    let sign_in = |pass: &'static str, code: String| {
        let r = r.clone();
        async move {
            let (status, v, half) = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": pass }))).await;
            let half = half.ok_or_else(|| format!("{status} {v}"))?;
            let (status, v, full) = admin_send(&r, "POST", "/api/login/totp", &half, Some(json!({ "code": code }))).await;
            full.ok_or_else(|| format!("{status} {v}"))
        }
    };

    // Another session (another browser): listed, and ended from here.
    let there = sign_in(password, totp_code(&secret, step)).await.unwrap();
    let (_, me, _) = admin_send(&r, "GET", "/api/me", &here, None).await;
    let sessions = me["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "{me}");
    let other = sessions.iter().find(|s| s["current"] == false).unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(admin_send(&r, "DELETE", "/api/sessions/short", &here, None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(admin_send(&r, "DELETE", &format!("/api/sessions/{other}"), &here, None).await.0, StatusCode::OK);
    assert_eq!(admin_send(&r, "GET", "/api/me", &there, None).await.0, StatusCode::UNAUTHORIZED, "ended");

    // A new password: the current one first, and a strong one; the other sessions end.
    let there = sign_in(password, totp_code(&secret, step + 1)).await.unwrap();
    let change = |current: &str, new: &str| json!({ "current": current, "new": new });
    let (status, ..) = admin_send(&r, "POST", "/api/me/password", &here, Some(change("not it", "another long passphrase here"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the current password wrong");
    let (status, ..) = admin_send(&r, "POST", "/api/me/password", &here, Some(change(password, "kiwi"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a weak one");
    let (status, v, _) = admin_send(&r, "POST", "/api/me/password", &here, Some(change(password, "another long passphrase here"))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(admin_send(&r, "GET", "/api/me", &there, None).await.0, StatusCode::UNAUTHORIZED, "other sessions end");
    assert_eq!(admin_send(&r, "GET", "/api/me", &here, None).await.0, StatusCode::OK, "this one stays");
    let (status, ..) = admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "kiwi", "password": password }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old password");

    // New recovery codes: the old ones stop working.
    let (status, v, _) = admin_send(&r, "POST", "/api/me/recovery", &here, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let new_codes: Vec<String> = v["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
    assert_eq!(new_codes.len(), old_codes.len());
    let half = |r: Router| async move {
        admin_send(
            &r,
            "POST",
            "/api/login",
            "",
            Some(json!({ "username": "kiwi", "password": "another long passphrase here" })),
        )
        .await
        .2
        .unwrap()
    };
    let h = half(r.clone()).await;
    assert_eq!(
        admin_send(&r, "POST", "/api/login/recovery", &h, Some(json!({ "code": old_codes[0] }))).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, v, _) = admin_send(&r, "POST", "/api/login/recovery", &h, Some(json!({ "code": new_codes[0] }))).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("full")), "{v}");
}

/// Every page of the admin UI answers a signed-in admin, nobody else, and says which ranges
/// it takes.
#[tokio::test]
async fn every_admin_page_answers_a_signed_in_admin_and_no_one_else() {
    let t = start("admin-pages").await;
    let r = admin_router_at(&t, [192, 0, 2, 22]);
    t.join("server-a").await;
    let cookie = admin_cookie(&t, "kiwi", 3600).await;
    for path in [
        "/api/me",
        "/api/admins",
        "/api/restrictions",
        "/api/audit",
        "/api/overview",
        "/api/online",
        "/api/series?range=3600",
        "/api/places?range=0",
        "/api/activity?range=86400",
        "/api/pings?range=86400",
        "/api/bandwidth?range=86400&server=server-a",
        "/api/players-report?range=604800",
        "/api/matches-report?days=7",
        "/api/alerts",
        "/api/updates",
        "/api/sessions",
        "/api/leaderboards",
        "/api/players",
        "/api/reports",
    ] {
        let (status, v, _) = admin_send(&r, "GET", path, &cookie, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {v}");
        assert!(v.is_object(), "{path}: {v}");
        assert_eq!(admin_send(&r, "GET", path, "", None).await.0, StatusCode::UNAUTHORIZED, "{path} signed out");
        let wrong = "__Host-fes-admin=not-a-session";
        assert_eq!(admin_send(&r, "GET", path, wrong, None).await.0, StatusCode::UNAUTHORIZED, "{path} with a made-up cookie");
    }
    let (_, v, _) = admin_send(&r, "GET", "/api/overview", &cookie, None).await;
    assert!(v.to_string().contains("server-a"), "the joined server is on the overview: {v}");
    for path in [
        "/api/series?range=0",
        "/api/series?range=5",
        "/api/places?range=-1",
        "/api/pings?range=0",
        "/api/bandwidth?range=3600",
        "/api/players-report?range=0",
        "/api/matches-report?days=0",
        "/api/matches-report?days=401",
    ] {
        assert_eq!(admin_send(&r, "GET", path, &cookie, None).await.0, StatusCode::BAD_REQUEST, "{path}");
    }
}

/// The admin UI's settings: names for maps and modes, servers' traffic allowances, the alert
/// webhook, the rollout's controls and removing a server; the sensitive ones want a second
/// factor proved lately, and each is in the audit log.
#[tokio::test]
async fn admins_change_the_networks_settings_and_each_change_is_audited() {
    let t = start("admin-settings").await;
    let r = admin_router_at(&t, [192, 0, 2, 23]);
    t.join("server-a").await;
    t.join("server-b").await;
    let cookie = admin_cookie(&t, "kiwi", 3600).await;
    let send = |method: &'static str, path: &'static str, body: Value| {
        let (r, cookie) = (r.clone(), cookie.clone());
        async move { admin_send(&r, method, path, &cookie, Some(body)).await }
    };

    // A map's name: shown wherever the map is.
    let label = |kind: &str, name: &str| json!({ "kind": kind, "id": 615_323_303, "name": name });
    assert_eq!(send("PUT", "/api/labels", label("weapon", "Silo")).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(send("PUT", "/api/labels", label("map", &"x".repeat(49))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        send("PUT", "/api/labels", label("map", "Silo\u{7}")).await.0,
        StatusCode::BAD_REQUEST,
        "a control character"
    );
    assert_eq!(send("PUT", "/api/labels", label("map", "Silo by night")).await.0, StatusCode::OK);
    let (_, v, _) = admin_send(&r, "GET", "/api/activity?range=0", &cookie, None).await;
    assert!(v["labels"].to_string().contains("Silo by night"), "{v}");

    // A traffic allowance: for a server there is, 0 to 10,000 TB; 0 takes it away.
    let allowance = |server: &str, tb: f64| json!({ "server": server, "tb": tb });
    assert_eq!(send("PUT", "/api/allowances", allowance("nowhere", 1.0)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(send("PUT", "/api/allowances", allowance("server-a", -1.0)).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(send("PUT", "/api/allowances", allowance("server-a", 10_001.0)).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(send("PUT", "/api/allowances", allowance("server-a", 2.5)).await.0, StatusCode::OK);
    let allowance_of = || async {
        let (_, v, _) = admin_send(&r, "GET", "/api/bandwidth?range=86400", &cookie, None).await;
        v["allowances"].as_array().unwrap().iter().find(|a| a["server"] == "server-a").unwrap()["allowance"].clone()
    };
    assert_eq!(allowance_of().await, json!(2.5e12));
    assert_eq!(send("PUT", "/api/allowances", allowance("server-a", 0.0)).await.0, StatusCode::OK);
    assert_eq!(allowance_of().await, Value::Null);

    // The alert webhook carries the network's alerts out: a second factor lately, and https.
    let hook = |url: &str| json!({ "url": url });
    let (status, v, _) = send("PUT", "/api/alerts/webhook", hook("https://hooks.example/alerts")).await;
    assert_eq!((status, v["reverify"].as_bool()), (StatusCode::FORBIDDEN, Some(true)), "{v}");
    assert_eq!(send("POST", "/api/alerts/test", json!({})).await.0, StatusCode::BAD_REQUEST, "no webhook to test");
    sqlx::query("UPDATE admin_sessions SET verified_at = ?")
        .bind(identity::now())
        .execute(&t.c.pool)
        .await
        .unwrap();
    for bad in [
        "http://hooks.example/alerts",
        "https://user:pass@hooks.example/",
        "not a url",
        &format!("https://hooks.example/{}", "a".repeat(500)),
    ] {
        assert_eq!(send("PUT", "/api/alerts/webhook", hook(bad)).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(send("PUT", "/api/alerts/webhook", hook("https://hooks.example/alerts")).await.0, StatusCode::OK);
    assert_eq!(admin_send(&r, "GET", "/api/alerts", &cookie, None).await.1["webhook_host"], "hooks.example");
    assert_eq!(send("PUT", "/api/alerts/webhook", hook("")).await.0, StatusCode::OK, "empty takes it away");
    assert_eq!(admin_send(&r, "GET", "/api/alerts", &cookie, None).await.1["webhook_host"], Value::Null);

    // The rollout: paused and resumed, pinned and unpinned; nothing to roll back to yet.
    let rollout = || async { admin_send(&r, "GET", "/api/updates", &cookie, None).await.1["rollout"].clone() };
    for (action, field, value) in [("pause", "paused", true), ("resume", "paused", false), ("pin", "pinned", true), ("unpin", "pinned", false)] {
        let (status, v, _) = admin_send(&r, "POST", &format!("/api/updates/{action}"), &cookie, Some(json!({}))).await;
        assert_eq!(status, StatusCode::OK, "{action}: {v}");
        assert_eq!(rollout().await[field], value, "{action}");
    }
    assert_eq!(send("POST", "/api/updates/explode", json!({})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(send("POST", "/api/updates/rollback", json!({})).await.0, StatusCode::BAD_REQUEST, "nothing before");
    assert_eq!(
        send("POST", "/api/updates/release", json!({ "version": "9.9.9" })).await.0,
        StatusCode::BAD_REQUEST,
        "not a recorded release"
    );

    // Removing a server, and the names it reserved for nobody.
    let (status, v, _) = send("POST", "/api/servers/server-b/purge-names", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(admin_send(&r, "DELETE", "/api/servers/nowhere", &cookie, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(admin_send(&r, "DELETE", "/api/servers/server-b", &cookie, None).await.0, StatusCode::OK);
    let (_, v, _) = admin_send(&r, "GET", "/api/overview", &cookie, None).await;
    assert!(!v.to_string().contains("server-b") && v.to_string().contains("server-a"), "{v}");
    // Ten minutes on, removing one wants the second factor again.
    sqlx::query("UPDATE admin_sessions SET verified_at = verified_at - 601").execute(&t.c.pool).await.unwrap();
    let (status, v, _) = admin_send(&r, "DELETE", "/api/servers/server-a", &cookie, None).await;
    assert_eq!((status, v["reverify"].as_bool()), (StatusCode::FORBIDDEN, Some(true)), "{v}");

    let (_, v, _) = admin_send(&r, "GET", "/api/audit", &cookie, None).await;
    let events: Vec<&str> = v["events"].as_array().unwrap().iter().map(|e| e["event"].as_str().unwrap()).collect();
    for event in [
        "named a map or mode",
        "set a traffic allowance",
        "set the alert webhook",
        "updates: pause",
        "updates: unpin",
        "released a server's unused names",
        "removed a server",
    ] {
        assert!(events.contains(&event), "{event} isn't in the audit log: {events:?}");
    }
    assert!(v["events"].as_array().unwrap().iter().all(|e| e["admin"] == "kiwi"), "{v}");
}

#[tokio::test]
async fn maintenance_is_told_to_launchers_and_servers_and_holds_updates() {
    let t = start("maintenance").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    let b = t.join("server-b").await;
    for (secret, name) in [(&a, "server-a"), (&b, "server-b")] {
        let (status, v) = t
            .call("POST", "/v1/heartbeat", Some(secret), Some(json!({ "name": name, "host": name, "listed": true })))
            .await;
        assert_eq!(status, StatusCode::OK, "{v}");
    }
    let now = identity::now();
    assert_eq!(admin_call(&r, "GET", "/api/maintenance", "", None).await.0, StatusCode::UNAUTHORIZED);
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    // Checked: a target, a member, not in the past.
    for (bad, why) in [
        (json!({ "start": now + 60, "end": now + 120 }), "no target"),
        (json!({ "servers": ["nobody"], "start": now + 60, "end": now + 120 }), "not a member"),
        (json!({ "servers": ["server-a"], "start": now - 7200, "end": now - 3600 }), "in the past"),
        (json!({ "servers": ["server-a"], "start": now + 60, "end": now + 60 + 25 * 3600 }), "too long"),
    ] {
        let (status, v) = admin_call(&r, "POST", "/api/maintenance", &cookie, Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {v}");
    }
    let (status, v) = admin_call(
        &r,
        "POST",
        "/api/maintenance",
        &cookie,
        Some(json!({ "servers": ["server-a", "server-a"], "start": now + 3600, "end": now + 7200, "note": "Moving to a faster machine" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["windows"].as_array().unwrap().len(), 1, "one per server: {v}");
    assert_eq!(v["windows"][0]["server_name"], "server-a");
    let a_window = v["windows"][0]["id"].as_i64().unwrap();
    let (status, v) = admin_call(
        &r,
        "POST",
        "/api/maintenance",
        &cookie,
        Some(json!({ "network": true, "start": now + 86_400, "end": now + 90_000 })),
    )
    .await;
    assert_eq!((status, v["windows"][0]["server_name"].as_str()), (StatusCode::OK, Some("Whole network")), "{v}");

    // Launchers: the server's windows and the network's.
    let (_, dir) = t.call("GET", "/v1/servers", None, None).await;
    let server = |id: &str| dir["servers"].as_array().unwrap().iter().find(|s| s["id"] == id).cloned().unwrap();
    assert_eq!(
        server("server-a")["maintenance"],
        json!([{ "start": now + 3600, "end": now + 7200, "note": "Moving to a faster machine" }])
    );
    assert!(server("server-b").get("maintenance").is_none());
    assert_eq!(dir["network_maintenance"]["start"], json!(now + 86_400));

    // Servers: their own and the network's, marked, for their games' overlay.
    let (_, beat_a) = t
        .call("POST", "/v1/heartbeat", Some(&a), Some(json!({ "name": "server-a", "host": "server-a", "listed": true })))
        .await;
    assert_eq!(beat_a["maintenance"].as_array().unwrap().len(), 2, "{beat_a}");
    assert_eq!(
        (beat_a["maintenance"][0]["network"].as_bool(), beat_a["maintenance"][1]["network"].as_bool()),
        (Some(false), Some(true))
    );
    let (_, beat_b) = t
        .call("POST", "/v1/heartbeat", Some(&b), Some(json!({ "name": "server-b", "host": "server-b", "listed": true })))
        .await;
    assert_eq!(beat_b["maintenance"], json!([{ "start": now + 86_400, "end": now + 90_000, "note": "", "network": true }]));

    // No update starts on a server in its window, or anywhere in the network's.
    assert!(!t.c.in_maintenance("server-a", now).await.unwrap());
    assert!(t.c.in_maintenance("server-a", now + 3600).await.unwrap());
    assert!(!t.c.in_maintenance("server-b", now + 3600).await.unwrap());
    assert!(t.c.in_maintenance("server-b", now + 86_400).await.unwrap());

    // Cancelled: gone from the directory, and audited.
    assert_eq!(admin_call(&r, "DELETE", &format!("/api/maintenance/{a_window}"), &cookie, None).await.0, StatusCode::OK);
    assert_eq!(
        admin_call(&r, "DELETE", &format!("/api/maintenance/{a_window}"), &cookie, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, dir) = t.call("GET", "/v1/servers", None, None).await;
    assert!(dir["servers"].as_array().unwrap().iter().all(|s| s.get("maintenance").is_none()), "{dir}");
    let (_, list) = admin_call(&r, "GET", "/api/maintenance", &cookie, None).await;
    assert_eq!(list["windows"].as_array().unwrap().len(), 2, "cancelled ones stay listed: {list}");
    let events: Vec<String> = sqlx::query_scalar("SELECT event FROM audit ORDER BY id").fetch_all(&t.c.pool).await.unwrap();
    assert_eq!(events, ["maintenance: booked", "maintenance: booked", "maintenance: cancelled"]);
}

#[tokio::test]
async fn players_suggest_from_the_launcher_and_admins_keep_the_roadmap() {
    let t = start("roadmap").await;
    let r = admin_router(&t);
    let me = identity::Identity::generate();
    let suggest = |who: &identity::Identity, time: i64, area: &str, title: &str, text: &str| {
        json!({
            "identity": who.global_id(), "name": "Kiwi", "area": area, "title": title, "text": text, "time": time,
            "signature": who.sign(&identity::suggestion_message(time, area, title, text)), "server": "oceania.example.net", "launcher": "0.4.2",
        })
    };
    let now = identity::now();
    // Only the coordinator started with --roadmap (the community network's) keeps one.
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    assert_eq!(t.call("GET", "/v1/roadmap", None, None).await.0, StatusCode::NOT_FOUND);
    let (status, _) = t
        .call("POST", "/v1/suggestions", None, Some(suggest(&me, now, "Launcher", "Chat to find players", "Please.")))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no suggestions taken without a roadmap");
    assert_eq!(admin_call(&r, "GET", "/api/roadmap", &cookie, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(admin_call(&r, "GET", "/api/me", &cookie, None).await.1["roadmap"], json!(false));
    t.c.enable_roadmap();
    assert_eq!(admin_call(&r, "GET", "/api/me", &cookie, None).await.1["roadmap"], json!(true));
    let a = t.join("server-a").await;
    t.changes(&a, json!([link(&me, "server-a", "Kiwi")])).await;
    // The name isn't signed: someone not linked anywhere is "a player", whatever they say.
    let stranger = identity::Identity::generate();
    let mut posing = suggest(&stranger, now, "Other", "Free admin rights", "From the developer.");
    posing["name"] = json!("JDevWebb");
    assert_eq!(t.call("POST", "/v1/suggestions", None, Some(posing)).await.0, StatusCode::OK);
    let name: String = sqlx::query_scalar("SELECT name FROM suggestions WHERE global_id = ?")
        .bind(stranger.global_id())
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(name, "a player");
    sqlx::query("DELETE FROM suggestions").execute(&t.c.pool).await.unwrap();
    // Signed, fresh and in a known area.
    let mut forged = suggest(&me, now, "Launcher", "Chat to find players", "A chat in the launcher.");
    forged["text"] = json!("Something else.");
    for (bad, why) in [
        (forged, "the text changed after signing"),
        (suggest(&me, now - 3600, "Launcher", "Chat", "Old."), "not fresh"),
        (suggest(&me, now, "Toasters", "Chat", "Unknown area."), "unknown area"),
        (suggest(&me, now, "Launcher", "Hi", "Too short a title."), "short title"),
    ] {
        let (status, v) = t.call("POST", "/v1/suggestions", None, Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {v}");
    }
    let mut ids = Vec::new();
    for title in ["Chat to find players", "Show who's in each match", "Favourite servers"] {
        let (status, v) = t.call("POST", "/v1/suggestions", None, Some(suggest(&me, now, "Launcher", title, "Please."))).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        ids.push(v["id"].as_i64().unwrap());
    }
    // Three a day.
    let (status, v) = t.call("POST", "/v1/suggestions", None, Some(suggest(&me, now, "Other", "A fourth one", "Too many."))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert!(v["error"].as_str().unwrap().contains("try again tomorrow"));

    // The player reads theirs back, signed; nobody else can.
    let mine = |who: &identity::Identity, signer: &identity::Identity| {
        let time = identity::now();
        format!(
            "/v1/suggestions/mine?identity={}&time={time}&signature={}",
            who.global_id(),
            signer.sign(&identity::suggestions_message(time))
        )
    };
    let (status, v) = t.call("GET", &mine(&me, &me), None, None).await;
    assert_eq!((status, v["suggestions"].as_array().map(Vec::len)), (StatusCode::OK, Some(3)), "{v}");
    assert!(v["suggestions"][0].get("identity").is_none(), "players don't get admin fields");
    let other = identity::Identity::generate();
    assert_eq!(t.call("GET", &mine(&me, &other), None, None).await.0, StatusCode::FORBIDDEN);

    // Admins answer, decline or promote them.
    let (status, v) = admin_call(&r, "GET", "/api/suggestions", &cookie, None).await;
    assert_eq!((status, v["suggestions"].as_array().map(Vec::len)), (StatusCode::OK, Some(3)), "{v}");
    assert_eq!(v["suggestions"][0]["identity"], json!(me.global_id()));
    let (status, v) = admin_call(
        &r,
        "PUT",
        &format!("/api/suggestions/{}", ids[2]),
        &cookie,
        Some(json!({ "status": "declined", "reply": "The menu already sorts by ping." })),
    )
    .await;
    assert_eq!((status, v["status"].as_str()), (StatusCode::OK, Some("declined")), "{v}");
    assert_eq!(
        admin_call(&r, "PUT", &format!("/api/suggestions/{}", ids[2]), &cookie, Some(json!({ "status": "maybe" })))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, item) = admin_call(&r, "POST", &format!("/api/suggestions/{}/promote", ids[0]), &cookie, Some(json!({ "lane": "later" }))).await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert_eq!(
        (item["lane"].as_str(), item["public"].as_bool(), item["source"].as_str()),
        (Some("later"), Some(false), Some("Suggested by Kiwi"))
    );
    let (_, v) = t.call("GET", &mine(&me, &me), None, None).await;
    let by_id = |id: i64| v["suggestions"].as_array().unwrap().iter().find(|s| s["id"] == id).cloned().unwrap();
    assert_eq!(by_id(ids[0])["status"], "planned");
    assert_eq!(by_id(ids[2])["reply"], "The menu already sorts by ping.");

    // Only public items reach launchers, without the admins' fields.
    let (_, road) = t.call("GET", "/v1/roadmap", None, None).await;
    assert!(road["lanes"].as_array().unwrap().iter().all(|l| l["items"].as_array().unwrap().is_empty()), "{road}");
    let id = item["id"].as_i64().unwrap();
    let edit = json!({ "lane": "next", "title": "Chat and who's online", "body": "Find people to play with.", "tags": ["Launcher"], "status": "0.4.4", "public": true });
    let (status, v) = admin_call(&r, "PUT", &format!("/api/roadmap/items/{id}"), &cookie, Some(edit)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        admin_call(&r, "PUT", "/api/roadmap/lanes/next", &cookie, Some(json!({ "release": "0.4.3" }))).await.0,
        StatusCode::OK
    );
    let (status, v) = admin_call(&r, "POST", "/api/roadmap/items", &cookie, Some(json!({ "lane": "someday", "title": "X" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
    let (_, road) = t.call("GET", "/v1/roadmap", None, None).await;
    let next = road["lanes"].as_array().unwrap().iter().find(|l| l["id"] == "next").cloned().unwrap();
    assert_eq!(next["release"], "0.4.3");
    assert_eq!(
        next["items"],
        json!([{ "id": id, "title": "Chat and who's online", "body": "Find people to play with.", "tags": ["Launcher"], "status": "0.4.4" }])
    );
    assert_eq!(admin_call(&r, "DELETE", &format!("/api/roadmap/items/{id}"), &cookie, None).await.0, StatusCode::OK);
    let (_, v) = admin_call(&r, "GET", "/api/roadmap", &cookie, None).await;
    assert_eq!((v["items"].as_array().map(Vec::len), v["suggestions"]["new"].as_i64()), (Some(0), Some(1)), "{v}");
}

#[tokio::test]
async fn players_write_to_support_and_read_the_admins_answers() {
    use std::io::Write as _;

    use base64::Engine as _;
    let t = start("support").await;
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    let me = identity::Identity::generate();
    let gz = |text: &str| {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(text.as_bytes()).unwrap();
        e.finish().unwrap()
    };
    let message = |who: &identity::Identity, signer: &identity::Identity, text: &str, files: &[(&str, Vec<u8>, usize)]| {
        let time = identity::now();
        let digests: Vec<(&str, [u8; 32])> = files.iter().map(|(n, g, _)| (*n, identity::digest(g))).collect();
        json!({
            "identity": who.global_id(), "name": "Oni", "server": "server-a", "launcher": "0.4.3", "time": time, "text": text,
            "files": files.iter().map(|(n, g, size)| json!({ "name": n, "size": size, "gzip_base64": base64::engine::general_purpose::STANDARD.encode(g) })).collect::<Vec<_>>(),
            "signature": signer.sign(&identity::support_message("coordinator.test", time, text, &digests)),
        })
    };
    let mine = |who: &identity::Identity, read: bool| {
        let time = identity::now();
        format!(
            "/v1/support/mine?identity={}&time={time}&signature={}{}",
            who.global_id(),
            who.sign(&identity::support_read_message("coordinator.test", time)),
            if read { "&read=1" } else { "" }
        )
    };
    let log = "12:00 joined\n12:01 dropped\n";

    // Only the community network's coordinator has it.
    assert_eq!(t.call("POST", "/v1/support", None, Some(message(&me, &me, "hi", &[]))).await.0, StatusCode::NOT_FOUND);
    t.c.enable_roadmap();
    // A key no member server knows isn't a player: refused.
    let (status, v) = t.call("POST", "/v1/support", None, Some(message(&me, &me, "hi", &[]))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{v}");
    t.changes(&a, json!([link(&me, "server-a", "Oni")])).await;
    // Signed by someone else, or for other text: refused.
    assert_eq!(
        t.call("POST", "/v1/support", None, Some(message(&me, &identity::Identity::generate(), "hi", &[]))).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut moved = message(&me, &me, "hi", &[]);
    moved["text"] = json!("something else");
    assert_eq!(t.call("POST", "/v1/support", None, Some(moved)).await.0, StatusCode::BAD_REQUEST);
    // A file whose size isn't what it says: refused.
    let (status, _) = t.call("POST", "/v1/support", None, Some(message(&me, &me, "hi", &[("launcher.log", gz(log), 3)]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, v) = t
        .call(
            "POST",
            "/v1/support",
            None,
            Some(message(&me, &me, "My game drops every match.\nOn eu1.", &[("launcher.log", gz(log), log.len())])),
        )
        .await;
    assert_eq!((status, v["files_kept"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
    let first = v["id"].as_i64().unwrap();

    // The admins see it, unread, and read it with the player's account beside it.
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let (_, v) = admin_call(&r, "GET", "/api/support", &cookie, None).await;
    assert_eq!(
        (v["threads"][0]["name"].as_str(), v["threads"][0]["unread"].as_i64(), v["threads"][0]["status"].as_str()),
        (Some("Oni"), Some(1), Some("open")),
        "{v}"
    );
    assert_eq!(t.c.support_unread_threads().await.unwrap(), 1);
    let path = format!("/api/support/{}", me.global_id());
    let (status, v) = admin_call(&r, "GET", &path, &cookie, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["messages"][0]["files"][0]["name"].as_str(), Some("launcher.log"), "{v}");
    assert_eq!(t.c.support_unread_threads().await.unwrap(), 0, "read once opened");
    let (status, _) = admin_call(&r, "GET", &format!("{path}/files/{first}/launcher.log"), &cookie, None).await;
    assert_eq!(status, StatusCode::OK);
    // Another conversation's message number doesn't open this file.
    let (status, _) = admin_call(
        &r,
        "GET",
        &format!("/api/support/{}/files/{first}/launcher.log", identity::Identity::generate().global_id()),
        &cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // An admin answers: the player sees it (unread until their Support page shows it).
    let (status, v) = admin_call(
        &r,
        "POST",
        &path,
        &cookie,
        Some(json!({ "text": "Thanks Oni, can you send the game's log after the next drop?" })),
    )
    .await;
    assert_eq!((status, v["status"].as_str()), (StatusCode::OK, Some("waiting")), "{v}");
    let (status, v) = t.call("GET", &mine(&me, false), None, None).await;
    assert_eq!(
        (status, v["unread"].as_i64(), v["messages"][1]["admin"].as_str()),
        (StatusCode::OK, Some(1), Some("admin1")),
        "{v}"
    );
    assert_eq!(v["messages"][1]["from"].as_str(), Some("admin"));

    // Its server's pulse says so while the player's online there (for the overlay).
    let roster = json!({ "full": false, "players": [player(1032, "Oni", Some(&me.global_id()))] });
    assert_eq!(t.call("POST", "/v1/players", Some(&a), Some(roster)).await.0, StatusCode::OK);
    let pulse = json!({ "players": { "online": 1 }, "counters": {}, "online": [{ "id": 1032, "name": "Oni" }] });
    let (_, v) = t.call("POST", "/v1/pulse", Some(&a), Some(pulse.clone())).await;
    assert_eq!(v["support"], json!([{ "player": 1032, "unread": 1 }]), "{v}");

    // Read on the Support page: nothing unread, in the launcher or the pulse.
    let (_, v) = t.call("GET", &mine(&me, true), None, None).await;
    assert_eq!(v["unread"].as_i64(), Some(0));
    let (_, v) = t.call("GET", &mine(&me, false), None, None).await;
    assert_eq!(v["unread"].as_i64(), Some(0));
    let (_, v) = t.call("POST", "/v1/pulse", Some(&a), Some(pulse)).await;
    assert_eq!(v["support"], json!([]), "{v}");

    // Writing again reopens it; resolving is the admins' call.
    assert_eq!(
        t.call("POST", "/v1/support", None, Some(message(&me, &me, "It dropped again.", &[]))).await.0,
        StatusCode::OK
    );
    let (_, v) = admin_call(&r, "GET", "/api/support?status=open", &cookie, None).await;
    assert_eq!(v["threads"].as_array().map(Vec::len), Some(1), "{v}");
    let (status, _) = admin_call(&r, "PUT", &format!("{path}/status"), &cookie, Some(json!({ "status": "resolved" }))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, v) = admin_call(&r, "GET", "/api/support", &cookie, None).await;
    assert_eq!(v["threads"].as_array().map(Vec::len), Some(0), "resolved isn't active: {v}");

    // The player's account deleted on its server: the conversation goes with it.
    let roster = json!({ "full": true, "players": [] });
    assert_eq!(t.call("POST", "/v1/players", Some(&a), Some(roster)).await.0, StatusCode::OK);
    let (_, v) = admin_call(&r, "GET", "/api/support?status=all", &cookie, None).await;
    assert_eq!(v["threads"].as_array().map(Vec::len), Some(0), "{v}");
}

#[tokio::test]
async fn admins_write_first_to_a_player_the_network_knows() {
    let t = start("support-first").await;
    t.c.enable_roadmap();
    let r = admin_router(&t);
    let a = t.join("server-a").await;
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let neutron = identity::Identity::generate();
    let ask = json!({ "text": "Hi Neutron, could you send your game's log from the Support page?" });
    // Nobody with that identity: nobody to write to.
    let (status, _) = admin_call(&r, "POST", &format!("/api/support/{}", neutron.global_id()), &cookie, Some(ask.clone())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let roster = json!({ "full": false, "players": [player(1026, "Neutron", Some(&neutron.global_id()))] });
    assert_eq!(t.call("POST", "/v1/players", Some(&a), Some(roster)).await.0, StatusCode::OK);
    t.changes(&a, json!([link(&neutron, "server-a", "Neutron")])).await;
    let (status, v) = admin_call(&r, "POST", &format!("/api/support/{}", neutron.global_id()), &cookie, Some(ask)).await;
    assert_eq!(
        (status, v["name"].as_str(), v["status"].as_str()),
        (StatusCode::OK, Some("Neutron"), Some("waiting")),
        "{v}"
    );
    // The player's launcher sees it, unread, and so does the overlay (through the pulse).
    let time = identity::now();
    let mine = format!(
        "/v1/support/mine?identity={}&time={time}&signature={}",
        neutron.global_id(),
        neutron.sign(&identity::support_read_message("coordinator.test", time))
    );
    let (_, v) = t.call("GET", &mine, None, None).await;
    assert_eq!((v["unread"].as_i64(), v["messages"][0]["admin"].as_str()), (Some(1), Some("admin1")), "{v}");
    let pulse = json!({ "players": { "online": 1 }, "counters": {}, "online": [{ "id": 1026, "name": "Neutron" }] });
    let (_, v) = t.call("POST", "/v1/pulse", Some(&a), Some(pulse)).await;
    assert_eq!(v["support"], json!([{ "player": 1026, "unread": 1 }]), "{v}");
}

/// With its names set (`--name`), a coordinator takes players' signatures made for one of
/// them only: one made for another coordinator, replayed here with that one's name as the
/// Host, reads nothing.
#[tokio::test]
async fn signed_reads_are_for_this_coordinators_names() {
    let t = start("signed-names").await;
    let _ = t.c.names.set(vec!["coordinator.test".into()]);
    let me = identity::Identity::generate();
    let read = |host: &str, signed_for: &str| {
        let time = identity::now();
        let path = format!(
            "/v1/reports/mine?identity={}&time={time}&signature={}",
            me.global_id(),
            me.sign(&identity::reports_message(signed_for, time))
        );
        let req = Request::builder().method("GET").uri(path).header("host", host).body(Body::empty()).unwrap();
        let router = t.router.clone();
        async move { router.oneshot(req).await.unwrap().status() }
    };
    assert_eq!(read("coordinator.test", "coordinator.test").await, StatusCode::OK);
    assert_eq!(read("Coordinator.test:443", "coordinator.test").await, StatusCode::OK, "the same name, another way");
    assert_eq!(read("evil.example", "evil.example").await, StatusCode::FORBIDDEN);
}

/// Support files: one player's are capped (their message keeps its text), and past the cap in
/// all the oldest go to make room for new ones, not the new ones dropped. An address's files
/// for the day are capped too. Admins delete a conversation (a second factor proved lately).
#[tokio::test]
async fn support_files_are_capped_per_player_and_the_oldest_make_room() {
    let t = start("support-caps").await;
    let r = admin_router(&t);
    t.c.enable_roadmap();
    let sent = |who: &identity::Identity| crate::support::Sent {
        identity: who.global_id(),
        name: "Oni".into(),
        server: String::new(),
        launcher: String::new(),
        time: identity::now(),
        text: "hi".into(),
        files: vec![],
        signature: String::new(),
    };
    let file = |kib: usize| crate::support::File {
        name: "game.log".into(),
        size: 1,
        gzip: vec![7; kib * 1024],
    };
    let caps = (300 * 1024, 200 * 1024);
    let (oni, kiwi) = (identity::Identity::generate(), identity::Identity::generate());
    let now = identity::now();
    // Oni: two of 100 KiB, then a third over their 200 KiB.
    for _ in 0..2 {
        assert!(t.c.add_support_message_within(&sent(&oni), &[file(100)], now, caps).await.unwrap().1);
    }
    let (_, kept) = t.c.add_support_message_within(&sent(&oni), &[file(100)], now, caps).await.unwrap();
    assert!(!kept, "over the player's cap: text only");
    // Kiwi's 150 KiB is past the 300 KiB in all, and Oni's files are recent and open: none
    // go, and Kiwi's message keeps its text only.
    let (_, kept) = t.c.add_support_message_within(&sent(&kiwi), &[file(150)], now, caps).await.unwrap();
    assert!(!kept, "recent files of an open conversation aren't pushed out");
    // Once Oni's conversation is resolved, its oldest file goes to make room.
    assert!(t.c.set_support_status(&oni.global_id(), "resolved").await.unwrap());
    let (_, kept) = t.c.add_support_message_within(&sent(&kiwi), &[file(150)], now, caps).await.unwrap();
    assert!(kept);
    let files: Vec<(String, i64)> = sqlx::query_as("SELECT m.identity, length(f.gzip) FROM support_files f JOIN support_messages m ON m.id = f.message_id ORDER BY f.message_id")
        .fetch_all(&t.c.pool)
        .await
        .unwrap();
    assert_eq!(files, vec![(oni.global_id(), 100 * 1024), (kiwi.global_id(), 150 * 1024)]);

    // An address's files for the day.
    assert!(t.c.support_bytes_allowed("192.0.2.9", crate::support::ADDRESS_FILES_A_DAY - 10, now));
    assert!(!t.c.support_bytes_allowed("192.0.2.9", 11, now));
    assert!(t.c.support_bytes_allowed("192.0.2.9", 11, now + 86_400), "the next day");
    assert!(t.c.support_bytes_allowed("192.0.2.10", 11, now), "another address");

    // Deleting a conversation takes a second factor proved lately, and takes its files.
    let path = format!("/api/support/{}", oni.global_id());
    let stale = admin_cookie(&t, "admin1", 3600).await;
    assert_eq!(admin_call(&r, "DELETE", &path, &stale, None).await.0, StatusCode::FORBIDDEN);
    let fresh = admin_cookie(&t, "admin2", 0).await;
    assert_eq!(admin_call(&r, "DELETE", &path, &fresh, None).await.0, StatusCode::OK);
    assert_eq!(admin_call(&r, "GET", &path, &fresh, None).await.0, StatusCode::NOT_FOUND);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM support_files").fetch_one(&t.c.pool).await.unwrap();
    assert_eq!(left, 1, "Kiwi's file only");
    assert_eq!(admin_call(&r, "DELETE", &path, &fresh, None).await.0, StatusCode::NOT_FOUND);
}

/// A member server can name any identity on an account of its own, but it isn't that player's:
/// it isn't listed beside their conversation, its pulse isn't told of their unread answers,
/// and dropping the account doesn't delete their conversation (before, it was deleted once no
/// account named the identity: here Oni is linked on A, which hasn't sent its roster yet).
#[tokio::test]
async fn a_server_naming_someone_elses_identity_gets_nothing_of_theirs() {
    let t = start("support-claimed").await;
    t.c.enable_roadmap();
    let r = admin_router(&t);
    let (a, b) = (t.join("server-a").await, t.join("server-b").await);
    let cookie = admin_cookie(&t, "admin1", 3600).await;
    let oni = identity::Identity::generate();
    t.changes(&a, json!([link(&oni, "server-a", "Oni")])).await;
    // Server B names Oni's identity on an account of its own.
    let claim = json!({ "full": true, "players": [player(7, "Oni-alt", Some(&oni.global_id()))] });
    assert_eq!(t.call("POST", "/v1/players", Some(&b), Some(claim)).await.0, StatusCode::OK);
    let path = format!("/api/support/{}", oni.global_id());
    assert_eq!(admin_call(&r, "POST", &path, &cookie, Some(json!({ "text": "Hi Oni" }))).await.0, StatusCode::OK);
    let (_, v) = admin_call(&r, "GET", &path, &cookie, None).await;
    assert_eq!(v["accounts"], json!([]), "B's account isn't Oni's: {v}");
    let pulse = json!({ "players": { "online": 1 }, "counters": {}, "online": [{ "id": 7, "name": "Oni-alt" }] });
    let (_, v) = t.call("POST", "/v1/pulse", Some(&b), Some(pulse)).await;
    assert_eq!(v["support"], json!([]), "{v}");
    // B drops its account: Oni's conversation stays.
    let empty = json!({ "full": true, "players": [] });
    assert_eq!(t.call("POST", "/v1/players", Some(&b), Some(empty)).await.0, StatusCode::OK);
    assert_eq!(admin_call(&r, "GET", &path, &cookie, None).await.0, StatusCode::OK);
    // A server that had them linked letting their last account go still deletes it.
    let roster = json!({ "full": true, "players": [player(1, "Oni", Some(&oni.global_id()))] });
    assert_eq!(t.call("POST", "/v1/players", Some(&a), Some(roster)).await.0, StatusCode::OK);
    assert_eq!(
        t.call("POST", "/v1/players", Some(&a), Some(json!({ "full": true, "players": [] }))).await.0,
        StatusCode::OK
    );
    assert_eq!(admin_call(&r, "GET", &path, &cookie, None).await.0, StatusCode::NOT_FOUND);
}

/// Codes while signing in: one sign-in under way per admin (a new one ends the last, so a
/// password can't open many to try codes on), and none taken while the account waits after
/// failures, the right one included.
#[tokio::test]
async fn second_factor_codes_wait_with_the_account() {
    let step = totp_step().await;
    let t = start("admin-code-lock").await;
    let r = admin_router_at(&t, [192, 0, 2, 11]);
    let link = t.c.admin_setup_link("tui", false).await.unwrap();
    let token = link.split("#setup=").nth(1).unwrap().to_string();
    let password = "correct horse battery staple";
    let (_, _, enroll) = admin_send(&r, "POST", "/api/setup", "", Some(json!({ "token": token, "password": password }))).await;
    let enroll = enroll.unwrap();
    let (_, v, _) = admin_send(&r, "POST", "/api/me/totp/begin", &enroll, None).await;
    let secret = v["secret"].as_str().unwrap().to_string();
    let (status, ..) = admin_send(&r, "POST", "/api/me/totp/confirm", &enroll, Some(json!({ "code": totp_code(&secret, step - 1) }))).await;
    assert_eq!(status, StatusCode::OK);

    let sign_in = || async {
        admin_send(&r, "POST", "/api/login", "", Some(json!({ "username": "tui", "password": password })))
            .await
            .2
            .unwrap()
    };
    let first = sign_in().await;
    let second = sign_in().await;
    let (status, ..) = admin_send(&r, "POST", "/api/login/totp", &first, Some(json!({ "code": "000000" }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the first sign-in ended with the second");
    sqlx::query("UPDATE admins SET locked_until = ? WHERE username = 'tui'")
        .bind(identity::now() + 900)
        .execute(&t.c.pool)
        .await
        .unwrap();
    let (status, ..) = admin_send(&r, "POST", "/api/login/totp", &second, Some(json!({ "code": totp_code(&secret, step) }))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "the right code, while the account waits");
    let (status, ..) = admin_send(&r, "POST", "/api/login/recovery", &second, Some(json!({ "code": "aaaa-bbbb-cccc" }))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let locked: i64 = sqlx::query_scalar("SELECT locked_until FROM admins WHERE username = 'tui'")
        .fetch_one(&t.c.pool)
        .await
        .unwrap();
    assert!(locked > identity::now(), "still waiting");
    sqlx::query("UPDATE admins SET locked_until = 0 WHERE username = 'tui'").execute(&t.c.pool).await.unwrap();
    let (status, v, _) = admin_send(&r, "POST", "/api/login/totp", &second, Some(json!({ "code": totp_code(&secret, step) }))).await;
    assert_eq!((status, v["stage"].as_str()), (StatusCode::OK, Some("full")), "{v}");
}

/// A weak password at a setup link leaves the link usable, but no longer than it was.
#[tokio::test]
async fn a_setup_link_keeps_its_expiry_through_weak_passwords() {
    let t = start("setup-expiry").await;
    let r = admin_router_at(&t, [192, 0, 2, 12]);
    let link = t.c.admin_setup_link("weka", false).await.unwrap();
    let token = link.split("#setup=").nth(1).unwrap().to_string();
    let soon = identity::now() + 60;
    sqlx::query("UPDATE setup_tokens SET expires_at = ?").bind(soon).execute(&t.c.pool).await.unwrap();
    let (status, ..) = admin_send(&r, "POST", "/api/setup", "", Some(json!({ "token": token, "password": "weka" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let expires: i64 = sqlx::query_scalar("SELECT expires_at FROM setup_tokens").fetch_one(&t.c.pool).await.unwrap();
    assert_eq!(expires, soon);
}

/// A listing can't top the directory with a made-up count.
#[tokio::test]
async fn listings_keep_to_a_believable_count() {
    let t = start("listing-bounds").await;
    let a = t.join("server-a").await;
    let (status, _) = t
        .call(
            "POST",
            "/v1/heartbeat",
            Some(&a),
            Some(json!({ "name": "Server A", "host": "server-a", "names": ["server-a"], "players_online": u32::MAX, "players_total": u32::MAX })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, v) = t.call("GET", "/v1/servers", None, None).await;
    let a_listed = v["servers"].as_array().unwrap().iter().find(|s| s["name"] == "Server A").cloned().unwrap();
    assert_eq!(a_listed["players_online"], json!(MAX_LISTED_PLAYERS));
}

/// Waits until every server's host check is of `target`; their results by id.
async fn checked(t: &Test, target: &str) -> HashMap<String, Value> {
    let mut checks = HashMap::new();
    for _ in 0..300 {
        let o = t.c.admin_overview().await.unwrap();
        checks = o["servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (s["id"].as_str().unwrap().to_string(), s["host_check"].clone()))
            .collect();
        if checks.values().all(|c| c["target"] == target) {
            return checks;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the checks of {target} didn't finish: {checks:?}");
}

/// A game server's `/api/info` on loopback, saying it's `id`; its port.
async fn info_server(id: &'static str) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        "/api/info",
        get(move || async move { axum::Json(json!({ "name": "5th Echelon", "version": "1.0.0", "id": id })) }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await });
    port
}

#[tokio::test]
async fn a_server_is_listed_only_where_it_answers_as_itself() {
    let t = start("host-check").await;
    t.c.check_hosts();
    // The test's servers are on loopback.
    t.c.check_private_hosts();
    let (honest, liar) = (t.join("honest").await, t.join("liar").await);
    let port = info_server("honest").await;
    let beat = |name: &str, port: u16| json!({ "name": name, "host": "127.0.0.1", "listed": true, "ports": { "api": port, "login": 21126 } });
    let listed = || async {
        let (_, v) = t.call("GET", "/v1/servers", None, None).await;
        let mut ids: Vec<String> = v["servers"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap().to_string()).collect();
        ids.sort();
        ids
    };
    // Both say they're at the honest server's address.
    t.call("POST", "/v1/heartbeat", Some(&honest), Some(beat("Honest", port))).await;
    t.call("POST", "/v1/heartbeat", Some(&liar), Some(beat("Honest (official)", port))).await;
    checked(&t, &format!("127.0.0.1 {port} -")).await;
    assert_eq!(listed().await, ["honest"], "the liar's listing names another server's address");
    // The liar hears that, on its next heartbeat.
    let (_, v) = t.call("POST", "/v1/heartbeat", Some(&liar), Some(beat("Honest (official)", port))).await;
    assert!(v["warnings"].to_string().contains("not in the directory"), "{v}");
    // Moving to an address nobody answers at: out of the directory until checked there.
    let (_, v) = t.call("POST", "/v1/heartbeat", Some(&honest), Some(beat("Honest", 9))).await;
    assert!(v.get("warnings").is_none(), "{v}");
    assert!(listed().await.is_empty(), "not checked at the new address yet");
    // The admin UI says what the check found.
    let mut found = Value::Null;
    for _ in 0..50 {
        found = t.c.admin_overview().await.unwrap()["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "liar")
            .unwrap()["host_check"]
            .clone();
        if !found.is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(found["ok"], false, "{found}");
    assert!(found["why"].as_str().unwrap().contains("\"honest\""), "{found}");
    // The liar hears only that it failed, not what answered there.
    let (_, v) = t.call("POST", "/v1/heartbeat", Some(&liar), Some(beat("Honest (official)", port))).await;
    assert!(!v["warnings"].to_string().contains("honest\\\""), "{v}");
}

#[tokio::test]
async fn a_host_on_this_machine_or_its_network_isnt_asked() {
    let t = start("host-check-private").await;
    t.c.check_hosts();
    let secret = t.join("local").await;
    let port = info_server("local").await;
    t.call(
        "POST",
        "/v1/heartbeat",
        Some(&secret),
        Some(json!({ "name": "Local", "host": "127.0.0.1", "listed": true, "ports": { "api": port, "login": 21126 } })),
    )
    .await;
    let mut found = Value::Null;
    for _ in 0..50 {
        found = t.c.admin_overview().await.unwrap()["servers"][0]["host_check"].clone();
        if found["target"].as_str().is_some_and(|t| t.starts_with("127.0.0.1")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(found["ok"], false, "{found}");
    assert!(found["why"].as_str().unwrap().contains("not a public address"), "{found}");
    let (_, v) = t.call("GET", "/v1/servers", None, None).await;
    assert!(v["servers"].as_array().unwrap().is_empty());
}
