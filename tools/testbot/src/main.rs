//! Runs test scenarios against a 5th Echelon (Enhanced) server with headless
//! players. Each run registers fresh accounts.
//!
//!     testbot [--server 127.0.0.1] [scenario ...]
//!
//! With no scenario names, runs them all. Exits non-zero if any fails.

use std::net::IpAddr;
use std::net::UdpSocket;
use std::time::Duration;

use eyre::ensure;
use eyre::eyre;
use eyre::Result;
use quazal::rmc::types::Property;
use sc_bl_protocols::game_session_service::types::GameSessionSearchWithParticipantsResult;
use testbot::bot::Bot;
use testbot::bot::Places;
use testbot::bot::Variant;
use testbot::bot::LOBBY;
use testbot::bot::PRIVATE_MATCH;

const PASSWORD: &str = "testbot-password";

struct Ctx {
    server: IpAddr,
    run: u32,
    n: u32,
}

impl Ctx {
    /// Registers and logs in a fresh player.
    async fn player(&mut self, role: &str) -> Result<Bot> {
        self.n += 1;
        let name = format!("{role}{}_{}", self.run, self.n);
        Bot::register(self.server, &name, PASSWORD).await?;
        Bot::login(self.server, &name, PASSWORD).await
    }
}

use server_api::friends::Relation;
use server_api::misc::friend_event::Kind;

/// The host players sign: the server as they reached it.
fn server_id(server: IpAddr) -> Result<String> {
    Ok(server.to_string())
}

fn names(players: &[server_api::friends::Player]) -> Vec<&str> {
    players.iter().map(|p| p.username.as_str()).collect()
}

/// A friend request, its notice, accepting it, and removing the friend.
async fn friends(ctx: &mut Ctx) -> Result<()> {
    let a = ctx.player("Kiwi").await?;
    let b = ctx.player("Tank").await?;
    ensure!(a.friend_change("request", &b.name).await? == Relation::RequestSent, "the request wasn't sent");
    ensure!(
        b.poll_friend_event(Duration::from_secs(3)).await? == Some((Kind::Request, a.name.clone())),
        "no request notice for the other player"
    );
    ensure!(names(&b.relationships().await?.requests_received) == [a.name.as_str()], "the request isn't waiting");
    // By any case of the name.
    ensure!(
        b.friend_change("accept", &a.name.to_uppercase()).await? == Relation::Friend,
        "accepting didn't make friends"
    );
    ensure!(
        a.poll_friend_event(Duration::from_secs(3)).await? == Some((Kind::Accepted, b.name.clone())),
        "no accepted notice"
    );
    ensure!(names(&a.relationships().await?.friends) == [b.name.as_str()], "not in the friend list");
    let part = &b.name[..b.name.len() - 1];
    ensure!(a.search(part).await?.contains(&(b.name.clone(), Relation::Friend)), "search doesn't show the friend");
    ensure!(a.friend_change("remove", &b.name).await? == Relation::None, "removing failed");
    ensure!(b.relationships().await?.friends.is_empty(), "still friends on the other side");
    a.disconnect().await?;
    b.disconnect().await
}

/// A block hides both players from each other and stops invites.
async fn block(ctx: &mut Ctx) -> Result<()> {
    let a = ctx.player("Blocker").await?;
    let b = ctx.player("Pest").await?;
    ensure!(a.friend_change("block", &b.name).await? == Relation::Blocked, "blocking failed");
    ensure!(!b.friends().await?.iter().any(|(n, _)| *n == a.name), "the blocker is still on the pest's game friend list");
    // An invite looks sent (nobody learns they're blocked) but never arrives.
    b.try_invite(&a.name).await.map_err(|e| eyre!("a blocked invite should look sent: {e}"))?;
    ensure!(a.poll_invite(Duration::from_millis(800)).await?.is_none(), "the pest's invite arrived");
    ensure!(b.search(&a.name).await?.is_empty(), "the pest can find the blocker");
    ensure!(b.friend_change("request", &a.name).await? == Relation::RequestSent, "a blocked request should look sent");
    ensure!(a.relationships().await?.requests_received.is_empty(), "but must not arrive");
    ensure!(a.friend_change("unblock", &b.name).await? == Relation::None, "unblocking failed");
    ensure!(b.friends().await?.iter().any(|(n, _)| *n == a.name), "unblocked, but still hidden");
    a.disconnect().await?;
    b.disconnect().await
}

/// Invitations from two players both arrive (one no longer replaces the other).
async fn invite_queue(ctx: &mut Ctx) -> Result<()> {
    let host1 = ctx.player("Host").await?;
    let host2 = ctx.player("Host").await?;
    let guest = ctx.player("Guest").await?;
    host1.invite(&guest.name).await?;
    host2.invite(&guest.name).await?;
    let mut got = vec![
        guest.poll_invite(Duration::from_secs(3)).await?.unwrap_or_default(),
        guest.poll_invite(Duration::from_secs(3)).await?.unwrap_or_default(),
    ];
    got.sort();
    let mut want = vec![host1.name.clone(), host2.name.clone()];
    want.sort();
    ensure!(got == want, "invitations lost: got {got:?}");
    host1.disconnect().await?;
    host2.disconnect().await?;
    guest.disconnect().await
}

/// An identity links to an account, and signs in to it with a new password.
async fn identity_login(ctx: &mut Ctx) -> Result<()> {
    let id = server_id(ctx.server)?;
    let me = identity::Identity::generate();
    let a = ctx.player("Keyed").await?;
    let now = identity::now();
    ensure!(a.link(&me, "another-server.example", now).await.is_err(), "a link signed for another server was taken");
    a.link(&me, &id, now).await.map_err(|e| eyre!("linking: {e}"))?;
    let name = a.name.clone();
    a.disconnect().await?;
    // A new PC that doesn't know the name: the key finds the account, as the launcher does.
    let found = testbot::bot::key_login(ctx.server, &me, &id, "", now, "")
        .await
        .map_err(|e| eyre!("key login without a name: {e}"))?;
    ensure!(found == name, "the key found {found:?}, not {name:?}");
    let stranger = testbot::bot::key_login(ctx.server, &identity::Identity::generate(), &id, "", now, "").await;
    ensure!(
        matches!(&stranger, Err(s) if s.code() == tonic::Code::NotFound),
        "an identity with no account here: {stranger:?}"
    );
    // A new PC: sign in with the key, set a new password.
    testbot::bot::key_login(ctx.server, &me, &id, &name, now + 1, "a-new-password-1")
        .await
        .map_err(|e| eyre!("key login: {e}"))?;
    ensure!(
        testbot::bot::key_login(ctx.server, &me, &id, &name, now + 1, "").await.is_err(),
        "the same signature worked twice"
    );
    ensure!(
        testbot::bot::key_login(ctx.server, &identity::Identity::generate(), &id, &name, now + 2, "stolen-password")
            .await
            .is_err(),
        "another key signed in"
    );
    // A server that got a key login signed for it can't reuse it here (another host), nor
    // change the password it sets.
    let for_elsewhere = me.sign_login("rogue.example", &name, now + 3, "a-new-password-1");
    let replayed = async {
        let channel = testbot::bot::api_endpoint(ctx.server).map_err(|e| eyre!(e))?.connect().await?;
        server_api::users::users_client::UsersClient::new(channel)
            .key_login(server_api::users::KeyLoginRequest {
                username: name.clone(),
                global_id: me.global_id(),
                time: now + 3,
                signature: for_elsewhere,
                new_password: "attackers-password".into(),
                host: "rogue.example".into(),
                client: testbot::bot::LAUNCHER_CLIENT.into(),
            })
            .await?;
        Ok::<(), eyre::Report>(())
    };
    ensure!(replayed.await.is_err(), "a key login signed for another host was taken");
    ensure!(Bot::login(ctx.server, &name, PASSWORD).await.is_err(), "the old password still works");
    // Unlinked, the key no longer signs in.
    let a = Bot::login(ctx.server, &name, "a-new-password-1").await?;
    a.unlink().await.map_err(|e| eyre!("unlinking: {e}"))?;
    ensure!(
        testbot::bot::key_login(ctx.server, &me, &id, &name, now + 4, "").await.is_err(),
        "the key signed in to an unlinked account"
    );
    a.disconnect().await
}

/// A server with `[limits] require_identity`: no account without an
/// identity, the identity finds its account, and accounts stay linked.
async fn identity_required(ctx: &mut Ctx) -> Result<()> {
    let id = server_id(ctx.server)?;
    let name = format!("Plain{}", ctx.run);
    let plain = Bot::register_as(ctx.server, &name, PASSWORD, None).await;
    ensure!(
        matches!(&plain, Err(s) if s.code() == tonic::Code::FailedPrecondition),
        "an account without an identity: {plain:?}"
    );
    let me = identity::Identity::generate();
    let name = format!("Keyed{}", ctx.run);
    let now = identity::now();
    ensure!(
        matches!(testbot::bot::key_login(ctx.server, &me, &id, "", now, "").await, Err(s) if s.code() == tonic::Code::NotFound),
        "an identity found an account before it had one"
    );
    Bot::register_as(ctx.server, &name, PASSWORD, Some((&me, &id)))
        .await
        .map_err(|e| eyre!("registering with an identity: {e}"))?;
    let found = testbot::bot::key_login(ctx.server, &me, &id, "", now + 1, "a-new-password-1")
        .await
        .map_err(|e| eyre!("key login: {e}"))?;
    ensure!(found == name, "the key found {found:?}, not {name:?}");
    let a = Bot::login(ctx.server, &name, "a-new-password-1").await?;
    ensure!(a.unlink().await.is_err(), "an account was unlinked");
    a.disconnect().await
}

/// The launcher's direct-connection test reaches this machine: behind a
/// reverse proxy too, where the server must use the forwarded address.
async fn direct_test(ctx: &mut Ctx) -> Result<()> {
    let a = ctx.player("Direct").await?;
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:13000").await?;
    let answer = tokio::spawn(async move {
        let mut buf = [0u8; 256];
        let (n, from) = socket.recv_from(&mut buf).await?;
        let challenge = buf[..n].strip_prefix(b"P2P Test - ").ok_or_else(|| eyre!("not a challenge"))?.to_vec();
        socket.send_to(&challenge, from).await?;
        Ok::<_, eyre::Report>(challenge)
    });
    let challenge: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    let back = a
        .test_direct(challenge.clone())
        .await
        .map_err(|e| eyre!("the server couldn't reach this machine: {}", e.message()))?;
    ensure!(back == challenge, "the challenge came back changed");
    ensure!(
        tokio::time::timeout(std::time::Duration::from_secs(2), answer).await??? == challenge,
        "a different challenge arrived"
    );
    a.disconnect().await
}

/// A player's report after a game, as the launcher sends it: the server tells what it saw
/// of the session, and takes the report with a log; a file that isn't one of the
/// launcher's is refused, and a player sends a few a day at most.
/// The game's diagnostics: kept up to the request's and the player's limits, never from
/// someone not signed in.
async fn client_log(ctx: &mut Ctx) -> Result<()> {
    let a = ctx.player("Logger").await?;
    let line = |i: usize| server_api::misc::ClientLogLine {
        at: 0,
        level: "WARN".into(),
        target: "hooks::hooks::nat".into(),
        message: format!("NAT: no answer from the server's NAT helper for {i} s"),
    };
    let kept = a.client_log((0..3).map(line).collect()).await.map_err(|e| eyre!("client log: {}", e.message()))?;
    ensure!(kept == 3, "three lines kept: {kept}");
    let kept = a.client_log((0..100).map(line).collect()).await.map_err(|e| eyre!("client log: {}", e.message()))?;
    ensure!(kept <= 50, "at most 50 a request: {kept}");
    let kept = a.client_log((0..50).map(line).collect()).await.map_err(|e| eyre!("client log: {}", e.message()))?;
    ensure!(kept < 50, "and the player's budget a minute: {kept}");
    a.disconnect().await
}

async fn report(ctx: &mut Ctx) -> Result<()> {
    let a = ctx.player("Reporter").await?;
    let summary = a.session_summary().await.map_err(|e| eyre!("session summary: {}", e.message()))?;
    ensure!(summary.started > 0 && summary.ended == 0, "a connected game has a play session going: {summary:?}");
    let log = setup::feedback::attach("bl-tracing.log", "INFO hooks: attaching\n", &setup::feedback::Private::default())?;
    let request = |files: Vec<server_api::misc::ReportFile>| server_api::misc::ReportRequest {
        rating: "bad".into(),
        problems: vec!["join".into()],
        comment: "a test report".into(),
        triggers: vec!["failed_join".into()],
        client: [("launcher".to_string(), "testbot".to_string())].into_iter().collect(),
        files,
    };
    let file = |name: &str| server_api::misc::ReportFile {
        name: name.into(),
        gzip: log.gzip.clone(),
        size: log.size,
    };
    let id = a.report(request(vec![file("bl-tracing.log")])).await.map_err(|e| eyre!("the report: {}", e.message()))?;
    ensure!(id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()), "a report id: {id:?}");
    let refused = a.report(request(vec![file("../uplay.toml")])).await;
    ensure!(
        matches!(&refused, Err(s) if s.code() == tonic::Code::InvalidArgument),
        "a file named ../uplay.toml: {refused:?}"
    );
    for _ in 0..4 {
        a.report(request(vec![])).await.map_err(|e| eyre!("another report: {}", e.message()))?;
    }
    let sixth = a.report(request(vec![])).await;
    ensure!(matches!(&sixth, Err(s) if s.code() == tonic::Code::ResourceExhausted), "a sixth report today: {sixth:?}");
    a.disconnect().await
}

/// A game far from the server (its first resend comes before the answer) sends
/// SYN and CONNECT twice and switches to the last SYN answer's signature: it
/// must still sign in and play.
async fn slow_handshake(ctx: &mut Ctx) -> Result<()> {
    testbot::conn::SLOW_HANDSHAKE.store(true, std::sync::atomic::Ordering::Relaxed);
    let result = async {
        let mut a = ctx.player("Far").await?;
        a.search_sessions("113 => 1;103 => 0")
            .await
            .map_err(|e| eyre!("the game service didn't answer after a repeated handshake: {e}"))?;
        a.disconnect().await
    }
    .await;
    testbot::conn::SLOW_HANDSHAKE.store(false, std::sync::atomic::Ordering::Relaxed);
    result
}

/// Online means signed in to the game service: a ticket alone (the launcher's
/// connection test) isn't, and leaving ends it.
async fn presence(ctx: &mut Ctx) -> Result<()> {
    let seer = ctx.player("Seer").await?;
    let seen = ctx.player("Seen").await?;
    let online = |list: Vec<(String, bool)>, name: &str| list.into_iter().any(|(n, on)| n == name && on);
    ensure!(online(seer.search_online(&seen.name).await?, &seen.name), "a signed-in player isn't online");
    ctx.n += 1;
    let tested = format!("Tested{}_{}", ctx.run, ctx.n);
    Bot::register(ctx.server, &tested, PASSWORD).await?;
    Bot::ticket_only(ctx.server, &tested, PASSWORD).await?;
    ensure!(!online(seer.search_online(&tested).await?, &tested), "a ticket without a game connection counts as online");
    let name = seen.name.clone();
    seen.disconnect().await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while online(seer.search_online(&name).await?, &name) {
        ensure!(tokio::time::Instant::now() < deadline, "still online after leaving");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    seer.disconnect().await
}

/// A match from creation to deletion: found by its attributes, changed by its host (and
/// found by the new ones only), its guest removed, deleted. Only its members change it,
/// only its host removes others, and only its host deletes it.
async fn session_lifecycle(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut guest = ctx.player("Guest").await?;
    let mut stranger = ctx.player("Stranger").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    // Maps no other scenario's sessions have.
    let (first, second) = (900_000 + ctx.run * 2, 900_001 + ctx.run * 2);
    let attrs = |map: u32| format!("113 => 0;103 => 0;102 => 6;101 => {map}");
    let game = host.create_session(&attrs(first)).await?;
    host.add_participants(game, &[host.pid], &[]).await?;
    let has = |found: Vec<(u32, u32)>| found.iter().any(|(s, _)| *s == game);
    ensure!(has(guest.search_sessions(&attrs(first)).await?), "the match isn't found by its attributes");
    ensure!(
        guest.search_sessions_plain().await?.iter().any(|&(s, h)| s == game && h == host.pid),
        "the plain search doesn't list the match with its host"
    );

    // The host picks another map: found by it, not by the old one.
    host.update_session(game, &attrs(second)).await?;
    ensure!(has(guest.search_sessions(&attrs(second)).await?), "not found by its new map");
    ensure!(!has(guest.search_sessions(&attrs(first)).await?), "still found by its old map");
    ensure!(stranger.update_session(game, &attrs(first)).await.is_err(), "someone not in the match changed it");
    ensure!(has(guest.search_sessions(&attrs(second)).await?), "a refused change changed it");

    // A guest comes in. Only the host removes others; then the guest isn't in it.
    guest.add_participants(game, &[guest.pid], &[]).await?;
    guest.join_session(game).await?;
    let guest_in = |found: Vec<sc_bl_protocols::game_session_service::types::GameSessionSearchWithParticipantsResult>| {
        found.iter().any(|r| r.game_session_search_result.session_key.session_id == game)
    };
    ensure!(guest_in(stranger.search_with_participants(&[guest.pid]).await?), "the guest isn't in the match");
    ensure!(
        stranger.remove_participants(game, &[guest.pid]).await.is_err(),
        "someone not in the match removed the guest"
    );
    ensure!(guest.remove_participants(game, &[host.pid]).await.is_err(), "the guest removed the host");
    host.remove_participants(game, &[guest.pid]).await?;
    ensure!(!guest_in(stranger.search_with_participants(&[guest.pid]).await?), "a removed guest is still in the match");

    // Only the host's delete deletes it.
    stranger.delete_session(game).await?;
    ensure!(has(guest.search_sessions(&attrs(second)).await?), "someone else deleted the host's match");
    host.delete_session(game).await?;
    ensure!(!has(guest.search_sessions(&attrs(second)).await?), "a deleted match is still found");
    for bot in [host, guest, stranger] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// The game's own invitations (GameSession's): sent, counted and listed on both sides,
/// declined, cancelled, accepted (the guest is then in the room). Only someone in the room
/// invites into it, and only an invitation that exists is accepted.
async fn game_invitations(ctx: &mut Ctx) -> Result<()> {
    use sc_bl_protocols::game_session_service::types::GameSessionInvitationReceived;
    use sc_bl_protocols::game_session_service::types::GameSessionKey;
    let mut host = ctx.player("Host").await?;
    let mut guest = ctx.player("Guest").await?;
    let mut other = ctx.player("Other").await?;
    let mut stranger = ctx.player("Stranger").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let room = host.create_session(LOBBY).await?;
    host.add_participants(room, &[host.pid], &[]).await?;

    host.send_invitation(room, &[guest.pid, other.pid], "come play").await?;
    ensure!(host.invitation_counts().await? == (0, 2), "the host's counts");
    ensure!(guest.invitation_counts().await? == (1, 0), "the guest's counts");
    let mut got = guest.invitations_received().await?;
    ensure!(got.len() == 1, "the guest has {} invitations", got.len());
    ensure!(
        (got[0].session_key.session_id, got[0].sender_pid, got[0].message.as_str()) == (room, host.pid, "come play"),
        "the invitation: {:?}",
        got[0]
    );
    // Declined: gone for the guest; the other one is still out.
    guest.decline_invitation(got.remove(0)).await?;
    ensure!(guest.invitation_counts().await? == (0, 0), "declined, still there");
    ensure!(host.invitation_counts().await? == (0, 1), "the host's counts after a decline");
    // Cancelled by the host: gone for the other player.
    let sent = host.invitations_sent().await?;
    let to_other = sent
        .into_iter()
        .find(|i| i.recipient_pid == other.pid)
        .ok_or_else(|| eyre!("the host's sent list lacks the other player"))?;
    host.cancel_invitation(to_other).await?;
    ensure!(other.invitation_counts().await? == (0, 0), "cancelled, still there");

    // Accepted: the guest is in the room.
    host.send_invitation(room, &[guest.pid], "again").await?;
    let invitation = guest.invitations_received().await?.pop().ok_or_else(|| eyre!("the second invitation didn't arrive"))?;
    guest.accept_invitation(invitation).await?;
    let found = stranger.search_with_participants(&[guest.pid]).await?;
    ensure!(
        found.iter().any(|r| r.game_session_search_result.session_key.session_id == room),
        "accepting didn't put the guest in the room"
    );
    ensure!(guest.invitation_counts().await? == (0, 0), "an accepted invitation is still pending");

    // Refused: inviting into a room you aren't in, accepting one nobody sent.
    ensure!(
        stranger.send_invitation(room, &[other.pid], "sneaky").await.is_err(),
        "someone not in the room invited into it"
    );
    let made_up = GameSessionInvitationReceived {
        session_key: GameSessionKey { type_id: 1, session_id: room },
        sender_pid: host.pid,
        message: String::new(),
        creation_time: quazal::rmc::types::DateTime(0),
    };
    ensure!(other.accept_invitation(made_up).await.is_err(), "an invitation nobody sent was accepted");
    ensure!(other.invitation_counts().await? == (0, 0), "a refused invitation was stored");
    for bot in [host, guest, other, stranger] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// Joins the game reports failing are counted in the player's session summary (what a
/// feedback report carries), a different game version apart.
async fn failed_joins(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Joiner").await?;
    // CONNECTION_FAILED, then DATA_VERSION_MISMATCH.
    a.report_failed_joins(&[(123_456, 0xb08a_1a05), (123_457, 0xeea4_40ee)]).await?;
    let summary = a.session_summary().await.map_err(|e| eyre!("session summary: {}", e.message()))?;
    ensure!(
        (summary.failed_joins, summary.version_mismatches) == (2, 1),
        "the summary counts {} failed joins, {} version mismatches",
        summary.failed_joins,
        summary.version_mismatches
    );
    a.disconnect().await
}

/// Stats after matches add up per board and context, the ratios are worked out, stats no
/// board has are left out, and the leaderboard ranks players by their score.
async fn stats(ctx: &mut Ctx) -> Result<()> {
    // Spies vs Mercs, in a game mode (context) no other scenario writes: kills (100),
    // deaths (101), their ratio (102) and the score (127, leaderboard 5).
    const BOARD: u32 = 10;
    const MODE: u32 = 228;
    let mut scorer = ctx.player("Scorer").await?;
    let mut rival = ctx.player("Rival").await?;
    scorer
        .write_stats(&[(BOARD, MODE, &[(100, Variant::I64(9)), (101, Variant::I64(4)), (127, Variant::I64(500))])])
        .await?;
    rival
        .write_stats(&[(BOARD, MODE, &[(100, Variant::I64(3)), (101, Variant::I64(1)), (127, Variant::I64(800))])])
        .await?;
    // Another match: added to what's there. A mode the board doesn't have, and a board that
    // doesn't exist, are left out.
    scorer
        .write_stats(&[
            (BOARD, MODE, &[(100, Variant::I64(1)), (127, Variant::I64(100))]),
            (BOARD, 999, &[(100, Variant::I64(50))]),
            (99, 0, &[(100, Variant::I64(1))]),
        ])
        .await?;
    let read = rival.read_stats(&[scorer.pid, rival.pid], BOARD, MODE, &[100, 101, 102, 127]).await?;
    let of = |pid: u32| {
        read.iter()
            .find(|(p, _)| *p == pid)
            .map(|(_, s)| s.iter().map(|(id, v)| format!("{id}={v}")).collect::<Vec<_>>().join(" "))
    };
    ensure!(
        of(scorer.pid).as_deref() == Some("100=I64(10) 101=I64(4) 102=F64(2.5) 127=I64(600)"),
        "the scorer's stats: {:?}",
        of(scorer.pid)
    );
    ensure!(
        of(rival.pid).as_deref() == Some("100=I64(3) 101=I64(1) 102=F64(3.0) 127=I64(800)"),
        "the rival's stats: {:?}",
        of(rival.pid)
    );
    let other_mode = rival.read_stats(&[scorer.pid], BOARD, 227, &[100]).await?;
    ensure!(other_mode.is_empty(), "stats appeared in another mode: {other_mode:?}");

    // The leaderboard: the rival first, on score.
    let (total, top) = scorer.leaderboard(5, MODE, Places::From(1, 10)).await?;
    let place = |list: &[(u32, u32, String)], pid: u32| list.iter().find(|(p, ..)| *p == pid).map(|(_, rank, score)| (*rank, score.clone()));
    let (rival_place, scorer_place) = (place(&top, rival.pid), place(&top, scorer.pid));
    ensure!(total >= 2, "{total} players on the leaderboard");
    ensure!(
        matches!((&rival_place, &scorer_place), (Some((r, rs)), Some((s, ss))) if r < s && rs == "I64(800)" && ss == "I64(600)"),
        "the leaderboard: rival {rival_place:?}, scorer {scorer_place:?}"
    );
    let (_, around) = rival.leaderboard(5, MODE, Places::Around(scorer.pid, 3)).await?;
    ensure!(place(&around, scorer.pid) == scorer_place, "around the scorer: {around:?}");
    let (_, theirs) = rival.leaderboard(5, MODE, Places::Of(&[scorer.pid])).await?;
    ensure!(place(&theirs, scorer.pid) == scorer_place, "the scorer's own place: {theirs:?}");
    scorer.disconnect().await?;
    rival.disconnect().await
}

/// A name check answers free, taken or not allowed, and makes nothing.
async fn name_check(ctx: &mut Ctx) -> Result<()> {
    use server_api::users::name_available_response::Answer;
    let server = ctx.server;
    let check = |name: String| async move {
        let channel = testbot::bot::api_endpoint(server).map_err(|e| eyre!(e))?.connect().await?;
        let answer = server_api::users::users_client::UsersClient::new(channel)
            .name_available(server_api::users::NameRequest { name })
            .await?
            .into_inner();
        Ok::<_, eyre::Report>((answer.answer(), answer.reason))
    };
    let a = ctx.player("Held").await?;
    let (taken, why) = check(a.name.to_lowercase()).await?;
    ensure!(taken == Answer::Taken && !why.is_empty(), "another player's name, in other case, answered {taken:?}");
    let free = format!("Free{}_{}", ctx.run, ctx.n + 1);
    ensure!(check(free.clone()).await?.0 == Answer::Free, "an unused name isn't free");
    // Checking made nothing: the name is still free to register.
    ensure!(check(free.clone()).await?.0 == Answer::Free, "a checked name was taken by the check");
    Bot::register(ctx.server, &free, PASSWORD).await?;
    ensure!(check(free).await?.0 == Answer::Taken, "a registered name is still free");
    for bad in ["", "Admin", "has space", "x".repeat(33).as_str(), "Kiwі"] {
        let (answer, why) = check(bad.to_string()).await?;
        ensure!(answer == Answer::NotAllowed && !why.is_empty(), "{bad:?} answered {answer:?}");
    }
    a.disconnect().await
}

/// Renaming keeps the account (friends, id) and frees the old name.
async fn rename(ctx: &mut Ctx) -> Result<()> {
    let a = ctx.player("Named").await?;
    let b = ctx.player("Other").await?;
    a.friend_change("request", &b.name).await?;
    b.friend_change("accept", &a.name).await?;
    ensure!(a.rename(&b.name.to_uppercase(), None).await.is_err(), "took another player's name");
    let new = format!("{}x", a.name);
    ensure!(a.rename(&new, None).await? == new, "renaming failed");
    ensure!(names(&b.relationships().await?.friends) == [new.as_str()], "the friend sees the old name");
    let old = a.name.clone();
    a.disconnect().await?;
    Bot::login(ctx.server, &new, PASSWORD).await?.disconnect().await?;
    // The old name is free again.
    Bot::register(ctx.server, &old, PASSWORD).await?;
    Bot::login(ctx.server, &old, PASSWORD).await?.disconnect().await?;
    b.disconnect().await
}

/// In the "mutual" mode: only friends are listed, and only friends invite.
async fn friends_mutual(ctx: &mut Ctx) -> Result<()> {
    let a = ctx.player("Host").await?;
    let b = ctx.player("Guest").await?;
    ensure!(a.relationships().await?.mode == "mutual", "the server isn't in the mutual mode");
    // Who's online is for friends: an empty search lists none of the strangers who are.
    ensure!(!b.search_online("").await?.iter().any(|(n, _)| *n == a.name), "an empty search listed an online stranger");
    ensure!(b.search_online(&a.name).await?.iter().all(|(_, online)| !online), "a search showed a stranger online");
    ensure!(!b.friends().await?.iter().any(|(n, _)| *n == a.name), "a stranger is on the game's friend list");
    ensure!(a.try_invite(&b.name).await.is_err(), "a stranger could invite");
    a.friend_change("request", &b.name).await?;
    b.friend_change("accept", &a.name).await?;
    ensure!(b.friends().await?.iter().any(|(n, _)| *n == a.name), "a friend is missing from the game's friend list");
    ensure!(
        b.search_online("").await?.contains(&(a.name.clone(), true)),
        "an empty search doesn't list an online friend"
    );
    a.try_invite(&b.name).await.map_err(|e| eyre!("a friend couldn't invite: {e}"))?;
    ensure!(b.poll_invite(Duration::from_secs(3)).await?.as_deref() == Some(a.name.as_str()), "the invite didn't arrive");
    a.disconnect().await?;
    b.disconnect().await
}

/// Wrong passwords for an account from one address stop that address only:
/// its owner, elsewhere, still signs in. (Through the loopback "proxy", so
/// against a server on this machine.)
async fn login_lockout(ctx: &mut Ctx) -> Result<()> {
    use testbot::bot::login_as_client;
    let owner = ctx.player("Owner").await?;
    let (home, stranger, travel) = ("198.51.100.10", "203.0.113.66", "192.0.2.77");
    login_as_client(ctx.server, &owner.name, PASSWORD, home)
        .await
        .map_err(|e| eyre!("the owner's first sign-in: {e}"))?;
    let mut refused = None;
    for i in 0..15 {
        match login_as_client(ctx.server, &owner.name, "not-the-password", stranger).await {
            Err(e) if e.code() == tonic::Code::ResourceExhausted => {
                refused = Some(i);
                break;
            }
            Err(e) if e.code() == tonic::Code::Unauthenticated => {}
            other => return Err(eyre!("a wrong password answered {other:?}")),
        }
    }
    ensure!(refused == Some(10), "the guessing address was stopped after {refused:?} tries, not 10");
    ensure!(
        login_as_client(ctx.server, &owner.name, PASSWORD, stranger).await.is_err(),
        "the guessing address still got in"
    );
    login_as_client(ctx.server, &owner.name, PASSWORD, home)
        .await
        .map_err(|e| eyre!("the owner was locked out at home: {e}"))?;
    login_as_client(ctx.server, &owner.name, PASSWORD, travel)
        .await
        .map_err(|e| eyre!("the owner was locked out elsewhere: {e}"))?;
    owner.disconnect().await
}

/// Two servers sharing a coordinator (`--other` is the second): friends made
/// on one show up on the other once both players link there too.
async fn federation(ctx: &mut Ctx, other: IpAddr) -> Result<()> {
    let (id_a, id_b) = (server_id(ctx.server)?, server_id(other)?);
    let (kiwi_key, tank_key) = (identity::Identity::generate(), identity::Identity::generate());
    let kiwi = ctx.player("Kiwi").await?;
    let tank = ctx.player("Tank").await?;
    kiwi.link(&kiwi_key, &id_a, identity::now()).await?;
    tank.link(&tank_key, &id_a, identity::now()).await?;
    kiwi.friend_change("request", &tank.name).await?;
    tank.friend_change("accept", &kiwi.name).await?;

    // The same people on the second server, with accounts of their own.
    let on_b = |name: &str| format!("{name}B");
    for (bot, key) in [(&kiwi, &kiwi_key), (&tank, &tank_key)] {
        Bot::register(other, &on_b(&bot.name), PASSWORD).await?;
        let there = Bot::login(other, &on_b(&bot.name), PASSWORD).await?;
        there.link(key, &id_b, identity::now()).await?;
        there.disconnect().await?;
    }
    let kiwi_b = Bot::login(other, &on_b(&kiwi.name), PASSWORD).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if names(&kiwi_b.relationships().await?.friends) == [on_b(&tank.name).as_str()] {
            break;
        }
        ensure!(tokio::time::Instant::now() < deadline, "the friendship didn't reach the second server");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // Tank is online on the first server only: the second tells Kiwi where to find them.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(100);
    loop {
        let r = kiwi_b.relationships().await?;
        if r.elsewhere.iter().any(|e| e.username == tank.name && !e.host.is_empty()) {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "a friend online on the first server wasn't shown on the second: {:?}",
            r.elsewhere
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // Names are reserved across the two servers: someone else can't be Kiwi on the second.
    let impostor = identity::Identity::generate();
    ensure!(
        Bot::register_as(other, &kiwi.name.to_lowercase(), PASSWORD, Some((&impostor, &id_b))).await.is_err(),
        "another identity took a reserved name"
    );
    ensure!(
        Bot::register(other, &kiwi.name, PASSWORD).await.is_err(),
        "an account without an identity took a reserved name"
    );
    // Kiwi can take their own name there (renaming their account on the second server).
    kiwi_b
        .rename(&kiwi.name, Some((&kiwi_key, &id_b)))
        .await
        .map_err(|e| eyre!("Kiwi couldn't take their own name: {e}"))?;
    // An account made on the second server before someone reserved its name elsewhere is
    // flagged when it links, and renaming clears it.
    let clash = format!("Clash{}", ctx.run);
    Bot::register(other, &clash, PASSWORD).await?;
    Bot::register_as(ctx.server, &clash, PASSWORD, Some((&identity::Identity::generate(), &id_a))).await?;
    let late_key = identity::Identity::generate();
    let late = Bot::login(other, &clash, PASSWORD).await?;
    late.link(&late_key, &id_b, identity::now()).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while late.relationships().await?.my_name() != server_api::friends::NameStatus::Conflict {
        ensure!(tokio::time::Instant::now() < deadline, "the name clash wasn't flagged");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    late.rename(&format!("{clash}b"), Some((&late_key, &id_b))).await.map_err(|e| eyre!("renaming: {e}"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while late.relationships().await?.my_name() != server_api::friends::NameStatus::Reserved {
        ensure!(tokio::time::Instant::now() < deadline, "renaming didn't clear the clash");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    late.disconnect().await?;

    // A block on the second server reaches the first.
    kiwi_b.friend_change("block", &on_b(&tank.name)).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let r = kiwi.relationships().await?;
        if r.friends.is_empty() && names(&r.blocked) == [tank.name.as_str()] {
            break;
        }
        ensure!(tokio::time::Instant::now() < deadline, "the block didn't reach the first server");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    kiwi_b.disconnect().await?;
    kiwi.disconnect().await?;
    tank.disconnect().await
}

fn attr(attrs: &[Property], id: u32) -> Option<u32> {
    attrs.iter().find(|p| p.id == id).map(|p| p.value)
}

fn has_session(found: &[GameSessionSearchWithParticipantsResult], session: u32) -> bool {
    found.iter().any(|r| r.game_session_search_result.session_key.session_id == session)
}

/// Login chain end to end, and presence in the friends list.
async fn login(ctx: &mut Ctx) -> Result<()> {
    let a = ctx.player("Host").await?;
    let b = ctx.player("Guest").await?;
    let online = |list: &[(String, bool)], name: &str| list.iter().find(|(n, _)| n == name).map(|(_, o)| *o);
    ensure!(online(&b.friends().await?, &a.name) == Some(true), "the host isn't shown online to the guest");
    let a_name = a.name.clone();
    a.disconnect().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    ensure!(online(&b.friends().await?, &a_name) == Some(false), "a player who quit still shows online");
    b.disconnect().await
}

/// Invite into a lobby: delivered, found by the friend search, joinable.
async fn lobby_invite(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    a.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;

    a.invite(&b.name).await?;
    let from = b.poll_invite(Duration::from_secs(3)).await?;
    ensure!(from.as_deref() == Some(a.name.as_str()), "invite not delivered (got {from:?})");

    let found = b.search_with_participants(&[a.pid]).await?;
    let room = found
        .iter()
        .find(|r| r.game_session_search_result.session_key.session_id == lobby)
        .ok_or_else(|| eyre!("the friend search didn't return the host's lobby: {found:?}"))?;
    ensure!(room.game_session_search_result.host_pid == a.pid, "wrong host");
    ensure!(!room.game_session_search_result.host_urls.0.is_empty(), "no host address to connect to");

    b.add_participants(lobby, &[b.pid], &[]).await?;
    b.join_session(lobby).await?;
    ensure!(b.wait_notification(Duration::from_millis(500)).await?.is_none(), "a public lobby join got a push");
    a.disconnect().await?;
    b.disconnect().await
}

/// Invite into a private match: the search returns the match room and the
/// host's lobby, and the guest that adds itself gets the "come in" push.
async fn private_match_invite(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    a.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    b.register_urls(&["prudp:/address=127.0.0.1;port=3075;sid=15;type=3"]).await?;
    let lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    let game = a.create_session(PRIVATE_MATCH).await?;
    a.add_participants(game, &[], &[a.pid]).await?;
    // As the game does: the host's lobby is announced invite-only, and the guest isn't in it
    // (e.g. invited back after leaving the match). The answer must still carry it.
    a.set_session(lobby, true).await?;
    let own = b.create_session(LOBBY).await?; // every player opens a lobby on entering multiplayer
    b.add_participants(own, &[b.pid], &[]).await?;

    a.invite(&b.name).await?;
    ensure!(b.poll_invite(Duration::from_secs(3)).await?.is_some(), "invite not delivered");
    let found = b.search_with_participants(&[a.pid]).await?;
    let kinds: Vec<(u32, Option<u32>)> = found
        .iter()
        .map(|r| (r.game_session_search_result.session_key.session_id, attr(&r.game_session_search_result.attributes.0, 113)))
        .collect();
    ensure!(kinds.contains(&(game, Some(0))), "the match room is missing from the answer: {kinds:?}");
    ensure!(kinds.iter().any(|(_, k)| *k == Some(1)), "the host's lobby is missing from the answer: {kinds:?}");

    // The game's invite route: abandon and split out of its own lobby, then add itself.
    b.abandon_session(own).await?;
    b.split_session(own).await?;
    b.add_participants(game, &[], &[b.pid]).await?;
    let push = b
        .wait_notification(Duration::from_secs(3))
        .await?
        .ok_or_else(|| eyre!("no 'come in' push for the private match"))?;
    ensure!(push.ui_type == 7003, "push type {} (want 7003)", push.ui_type);
    ensure!(push.ui_param_1 == b.pid, "push names player {} (want {})", push.ui_param_1, b.pid);
    ensure!(push.ui_param_2 == game, "push names session {} (want {game})", push.ui_param_2);
    a.disconnect().await?;
    b.disconnect().await
}

/// A room full at its current size (attributes 3 + 4, as the host last sent them) isn't
/// offered by matchmaking, and an invitee isn't put into it; the host raising the size (co-op
/// lobbies go from 2 to 4-6) opens it again.
async fn full_room(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut guest = ctx.player("Guest").await?;
    let mut seeker = ctx.player("Seeker").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    // A public co-op room on a map no other scenario has, at the default 2 seats.
    let map = 910_000 + ctx.run;
    let coop = |public: u32| format!("113 => 0;103 => 0;102 => 3;101 => {map};3 => {public};4 => 0");
    let query = format!("113 => 0;103 => 0;102 => 3;101 => {map}");
    let room = host.create_session(&coop(2)).await?;
    host.add_participants(room, &[], &[host.pid]).await?;
    let offered = |found: Vec<(u32, u32)>| found.iter().any(|(s, _)| *s == room);
    ensure!(offered(seeker.search_sessions(&query).await?), "a co-op room with a free seat isn't offered");
    guest.join_session(room).await?;
    ensure!(!offered(seeker.search_sessions(&query).await?), "a full co-op room (2 of 2) is offered");
    ensure!(!seeker.search_sessions_plain().await?.iter().any(|&(s, _)| s == room), "the plain search lists a full room");
    host.update_session(room, &coop(4)).await?;
    ensure!(offered(seeker.search_sessions(&query).await?), "raised to 4 seats, 2 in it: not offered");

    // A private 2-seat co-op match, two invited: the first takes the seat, the second isn't
    // put into it until the host raises the size.
    let mut second = ctx.player("Second").await?;
    let game = host.create_session("113 => 0;103 => 0;102 => 3;3 => 0;4 => 2").await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    host.set_session(game, true).await?;
    host.invite(&seeker.name).await?;
    host.invite(&second.name).await?;
    ensure!(seeker.poll_invite(Duration::from_secs(3)).await?.is_some(), "invite not delivered");
    ensure!(second.poll_invite(Duration::from_secs(3)).await?.is_some(), "second invite not delivered");
    ensure!(
        has_session(&seeker.search_with_participants(&[host.pid]).await?, game),
        "the invited player doesn't find the match"
    );
    let members = |found: &[GameSessionSearchWithParticipantsResult]| {
        found
            .iter()
            .find(|r| r.game_session_search_result.session_key.session_id == game)
            .map(|r| r.participant_ids.0.clone())
            .unwrap_or_default()
    };
    ensure!(
        members(&host.search_with_participants(&[host.pid]).await?).contains(&seeker.pid),
        "the first invitee wasn't added"
    );
    let found = second.search_with_participants(&[host.pid]).await?;
    ensure!(has_session(&found, game), "the second invitee isn't answered with the match");
    ensure!(!members(&found).contains(&second.pid), "the second invitee was put into a full match");
    host.update_session(game, "113 => 0;103 => 0;102 => 3;3 => 0;4 => 4").await?;
    ensure!(
        members(&second.search_with_participants(&[host.pid]).await?).contains(&second.pid),
        "raised to 4 seats, the second invitee still isn't added"
    );
    for bot in [host, guest, seeker, second] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// The host leaving a public match its guests (who joined from a search) are still in ends
/// it for nobody: a guest's game may split it to carry on. A stranger still may not.
async fn host_leaves_match(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut guest = ctx.player("Guest").await?;
    let mut stranger = ctx.player("Stranger").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let game = host.create_session("113 => 0;103 => 0;102 => 3;3 => 4;4 => 0").await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    guest.join_session(game).await?;
    ensure!(stranger.split_session(game).await.is_err(), "a stranger split the match");
    host.abandon_session(game).await?;
    let split = guest
        .split_session(game)
        .await
        .map_err(|e| eyre!("a guest's split of the match its host left was refused: {e}"))?;
    ensure!(split != game, "the split made no new session");
    ensure!(stranger.split_session(game).await.is_err(), "a stranger split the match once its host left");
    for bot in [host, guest, stranger] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// Nobody joins a private match uninvited: not by naming themselves twice (private and
/// public), nor for sharing some other room with the host. The host's own party follows it
/// in, by adding itself or being added; a stranger can't be pulled in.
async fn private_room_join(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut party = ctx.player("Party").await?;
    let mut carried = ctx.player("Party").await?;
    let mut stranger = ctx.player("Stranger").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let lobby = host.create_session(LOBBY).await?;
    host.add_participants(lobby, &[host.pid], &[]).await?;
    for p in [&mut party, &mut carried] {
        p.add_participants(lobby, &[p.pid], &[]).await?;
        p.join_session(lobby).await?;
    }
    // The host also sits in the stranger's public lobby: a room the two share.
    let public = stranger.create_session(LOBBY).await?;
    stranger.add_participants(public, &[stranger.pid], &[]).await?;
    host.add_participants(public, &[host.pid], &[]).await?;
    let game = host.create_session(PRIVATE_MATCH).await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    host.set_session(game, true).await?;

    ensure!(
        stranger.add_participants(game, &[stranger.pid], &[stranger.pid]).await.is_err(),
        "[me, me] joined a private match uninvited"
    );
    ensure!(
        stranger.add_participants(game, &[], &[stranger.pid]).await.is_err(),
        "a stranger joined a private match uninvited"
    );
    ensure!(
        host.add_participants(game, &[], &[stranger.pid]).await.is_err(),
        "the host pulled a stranger into its private match"
    );
    ensure!(
        stranger.wait_notification(Duration::from_millis(500)).await?.is_none(),
        "the stranger was nudged into the match"
    );

    party
        .add_participants(game, &[], &[party.pid])
        .await
        .map_err(|e| eyre!("the host's party couldn't follow it: {e}"))?;
    host.add_participants(game, &[], &[carried.pid])
        .await
        .map_err(|e| eyre!("the host couldn't take its party along: {e}"))?;
    let push = carried
        .wait_notification(Duration::from_secs(3))
        .await?
        .ok_or_else(|| eyre!("no 'come in' push for the party"))?;
    ensure!(push.ui_type == 7003 && push.ui_param_2 == game, "wrong push: {push:?}");
    // A stranger who adds themselves to the host's (public) party after the match was made
    // isn't the party that follows it: they can't walk in that way either.
    stranger.add_participants(lobby, &[stranger.pid], &[]).await?;
    ensure!(
        stranger.add_participants(game, &[], &[stranger.pid]).await.is_err(),
        "a stranger joined a private match through the host's party, after the match was made"
    );
    for p in [host, party, carried, stranger] {
        p.disconnect().await?;
    }
    Ok(())
}

/// Splitting makes sessions: only of a session the player is in (or just left), and no
/// more than creating them would.
async fn split_limits(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut stranger = ctx.player("Stranger").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let lobby = host.create_session(LOBBY).await?;
    host.add_participants(lobby, &[host.pid], &[]).await?;
    ensure!(stranger.split_session(lobby).await.is_err(), "a stranger split someone else's session");
    let mut made = 0;
    while host.split_session(lobby).await.is_ok() {
        made += 1;
        ensure!(made < 40, "splitting is unlimited");
    }
    ensure!(made > 0, "the host couldn't split its own session");
    host.disconnect().await?;
    stranger.disconnect().await
}

/// A private match isn't handed to strangers: not in matchmaking, and not by searching for
/// its host. Its own players, and the invited, still find it.
async fn private_room_hidden(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut guest = ctx.player("Guest").await?;
    let mut stranger = ctx.player("Stranger").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let lobby = host.create_session(LOBBY).await?;
    host.add_participants(lobby, &[host.pid], &[]).await?;
    // A match matchmaking would find (103 => 0), if it weren't invite-only.
    let game = host.create_session("113 => 0;103 => 0;3 => 0;4 => 8;102 => 7").await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    host.set_session(game, true).await?;
    ensure!(
        !stranger.search_sessions("113 => 0;102 => 7").await?.iter().any(|(s, _)| *s == game),
        "matchmaking offered a private match"
    );
    let found = stranger.search_with_participants(&[host.pid]).await?;
    ensure!(!has_session(&found, game), "a stranger found the private match by its host");
    ensure!(has_session(&found, lobby), "the host's public lobby went missing too");
    host.invite(&guest.name).await?;
    ensure!(guest.poll_invite(Duration::from_secs(3)).await?.is_some(), "invite not delivered");
    ensure!(
        has_session(&guest.search_with_participants(&[host.pid]).await?, game),
        "the invited guest doesn't find the match"
    );
    for p in [host, guest, stranger] {
        p.disconnect().await?;
    }
    Ok(())
}

/// A host who quits takes their lobby with them.
async fn cleanup(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    a.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    let host = a.pid;
    ensure!(has_session(&b.search_with_participants(&[host]).await?, lobby), "lobby not found while the host is there");
    a.disconnect().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    ensure!(
        !has_session(&b.search_with_participants(&[host]).await?, lobby),
        "the lobby of a host who quit is still found"
    );
    b.disconnect().await
}

/// With trusted_subnet covering the players, an address from the wrong
/// adapter is replaced by the one the server saw (run with trusted_subnet =
/// 127.0.0.0/8 for a local server).
async fn trusted_subnet(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    a.register_urls(&["prudp:/address=192.168.1.50;port=3074;sid=15;type=3"]).await?; // the home network adapter
    let lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    let found = b.search_with_participants(&[a.pid]).await?;
    let room = found
        .iter()
        .find(|r| r.game_session_search_result.session_key.session_id == lobby)
        .ok_or_else(|| eyre!("lobby not found"))?;
    let addrs: Vec<String> = room.game_session_search_result.host_urls.0.iter().map(|u| u.address.clone()).collect();
    ensure!(!addrs.iter().any(|a| a == "192.168.1.50"), "the wrong-adapter address reached the friend: {addrs:?}");
    ensure!(addrs.iter().any(|a| a.starts_with("127.")), "not corrected to the observed address: {addrs:?}");
    a.disconnect().await?;
    b.disconnect().await
}

/// A station URL's address is checked whatever its scheme: a `udp:` URL naming someone
/// else's address is corrected, and a host name never reaches other players.
async fn station_url_schemes(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    a.register_urls(&["udp:/address=198.51.100.1;port=3074;type=2", "prudps:/address=victim.example;port=3074;type=3"])
        .await?;
    let lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    let found = b.search_with_participants(&[a.pid]).await?;
    let room = found
        .iter()
        .find(|r| r.game_session_search_result.session_key.session_id == lobby)
        .ok_or_else(|| eyre!("lobby not found"))?;
    let addrs: Vec<String> = room.game_session_search_result.host_urls.0.iter().map(|u| u.address.clone()).collect();
    ensure!(
        !addrs.is_empty() && addrs.iter().all(|a| a.starts_with("127.")),
        "another address reached the friend: {addrs:?}"
    );
    a.disconnect().await?;
    b.disconnect().await
}

/// The newest sign-in wins: the same account signing in from another PC
/// closes the first game's connection, and what it left (its lobby) goes with
/// it; the new one works as usual.
async fn second_sign_in(ctx: &mut Ctx) -> Result<()> {
    let mut first = ctx.player("Twin").await?;
    let mut friend = ctx.player("Friend").await?;
    first.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let lobby = first.create_session(LOBBY).await?;
    first.add_participants(lobby, &[first.pid], &[]).await?;
    ensure!(
        has_session(&friend.search_with_participants(&[first.pid]).await?, lobby),
        "the first game's lobby isn't found"
    );

    // Another socket: another PC, as the server sees it.
    let mut second = Bot::login(ctx.server, &first.name, PASSWORD).await?;
    ensure!(first.signed_out(Duration::from_secs(3)).await?, "the first game wasn't disconnected");
    ensure!(
        !has_session(&friend.search_with_participants(&[first.pid]).await?, lobby),
        "the first game's lobby outlived its connection"
    );
    second.register_urls(&["prudp:/address=127.0.0.1;port=3075;sid=15;type=3"]).await?;
    let again = second.create_session(LOBBY).await?;
    second.add_participants(again, &[second.pid], &[]).await?;
    ensure!(
        has_session(&friend.search_with_participants(&[second.pid]).await?, again),
        "the second game's lobby isn't found"
    );
    second.disconnect().await?;
    friend.disconnect().await
}

/// How a test player reaches other players: no word with the NAT helper (a LAN, or a hook
/// that never registered), a direct address, or through the server's relay.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reach {
    Unregistered,
    Direct,
    Relayed,
}

/// A game that loses the server mid-session signs in again from the same address and ports
/// while its old connection is still open there (the server only notices it's gone after a
/// minute): at once or after a few quiet seconds, with a new PRUDP session number or, by
/// chance, the old one; for a player reached directly, through the relay, or not registered
/// with the NAT helper at all. Each time it's signed in and served, registers with the NAT
/// helper again, can be reached, and friends find the lobby it makes; once the old
/// connections time out, the lobbies the dropped games made are gone, the player is out of
/// the friend's lobby they joined, and nothing of the new game's is touched (its lobby, a
/// lobby it joined again). Every problem is listed, not just the first.
async fn reconnect(ctx: &mut Ctx) -> Result<()> {
    let mut cases = vec![];
    for reach in [Reach::Unregistered, Reach::Direct, Reach::Relayed] {
        cases.push((reach, ctx.player("Dropped").await?, ctx.player("Friend").await?));
    }
    let server = ctx.server;
    let mut runs = cases.into_iter().map(|(reach, a, friend)| reconnect_case(server, reach, a, friend));
    let (Some(x), Some(y), Some(z)) = (runs.next(), runs.next(), runs.next()) else {
        unreachable!()
    };
    let (x, y, z) = tokio::join!(x, y, z);
    let problems: Vec<String> = [x, y, z].into_iter().flat_map(|r| r.unwrap_or_else(|e| vec![format!("{e}")])).collect();
    for p in &problems {
        println!("  {p}");
    }
    ensure!(problems.is_empty(), "{} problem(s) reconnecting", problems.len());
    Ok(())
}

/// The NAT helper registration of a player reached as `reach`, from `socket` (the game's
/// Storm socket, the same port across restarts).
async fn reconnect_register(server: IpAddr, reach: Reach, socket: &tokio::net::UdpSocket, bot: &Bot) -> Result<Option<testbot::bot::NatRegistration>> {
    let flags = match reach {
        Reach::Unregistered => return Ok(None),
        Reach::Direct => 0,
        Reach::Relayed => nat_proto::probe_flags::WANT_RELAY,
    };
    let r = testbot::bot::nat_register(socket, nat_addr(server, false)?, flags, &bot.name, bot.nat_ticket).await?;
    ensure!(r.relayed == (reach == Reach::Relayed), "registered as relayed: {}", r.relayed);
    Ok(Some(r))
}

/// Whether a packet `friend` sends through the relay to `to` reaches `socket`.
async fn reconnect_relays(
    server: IpAddr,
    from: &tokio::net::UdpSocket,
    from_reg: &testbot::bot::NatRegistration,
    to: &tokio::net::UdpSocket,
    to_reg: &testbot::bot::NatRegistration,
) -> Result<bool> {
    use nat_proto::Message;
    let payload = rand::random::<u64>().to_be_bytes().to_vec();
    let msg = Message::DataTo {
        tag: from_reg.tag,
        to: to_reg.advertise,
        payload: payload.clone(),
    };
    from.send_to(&msg.encode(), nat_addr(server, false)?).await?;
    Ok(matches!(nat_wait_data(to, Duration::from_secs(2)).await, Some(Message::DataFrom { payload: p, .. }) if p == payload))
}

async fn reconnect_case(server: IpAddr, reach: Reach, mut a: Bot, mut friend: Bot) -> Result<Vec<String>> {
    let mut problems = vec![];
    let a_nat = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let f_nat = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let mut a_reg = reconnect_register(server, reach, &a_nat, &a).await?;
    // The friend is reached the same way (a relayed player's friends often are too).
    let mut f_reg = reconnect_register(server, reach, &f_nat, &friend).await?;
    a.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let mut lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    // Two lobbies of the friend's: each dropped game joins the first; the second, the first
    // game only, and the last game again.
    friend.register_urls(&["prudp:/address=127.0.0.1;port=3075;sid=15;type=3"]).await?;
    let (theirs, rejoined) = (friend.create_session(LOBBY).await?, friend.create_session(LOBBY).await?);
    friend.add_participants(theirs, &[friend.pid], &[]).await?;
    friend.add_participants(rejoined, &[friend.pid], &[]).await?;
    a.add_participants(rejoined, &[a.pid], &[]).await?;
    let mut lost = vec![];
    for (quiet, same_session) in [(0, false), (0, true), (6, false), (6, true)] {
        a.add_participants(theirs, &[a.pid], &[]).await?;
        let how = format!(
            "{reach:?}, reconnecting after {quiet} s with {} session number",
            if same_session { "the same" } else { "a new" }
        );
        let name = a.name.clone();
        a = match a.reconnect(server, PASSWORD, Duration::from_secs(quiet), same_session).await {
            Ok(a) => a,
            Err(e) => {
                problems.push(format!("{how}: {e}"));
                // Started again: another port, so the case goes on.
                Bot::login(server, &name, PASSWORD).await?
            }
        };
        if let Err(e) = a.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await {
            problems.push(format!("{how}: the game service didn't serve the new connection: {e}"));
            continue;
        }
        if reach != Reach::Unregistered {
            match reconnect_register(server, reach, &a_nat, &a).await {
                Ok(r) => a_reg = r,
                Err(e) => problems.push(format!("{how}: registering with the NAT helper again: {e}")),
            }
        }
        if let (Reach::Relayed, Some(ar), Some(fr)) = (reach, &a_reg, &f_reg) {
            if !reconnect_relays(server, &f_nat, fr, &a_nat, ar).await? {
                problems.push(format!("{how}: the relay doesn't reach the reconnected player"));
            }
            if !reconnect_relays(server, &a_nat, ar, &f_nat, fr).await? {
                problems.push(format!("{how}: the reconnected player's packets aren't relayed"));
            }
        }
        lost.push(lobby);
        lobby = a.create_session(LOBBY).await?;
        a.add_participants(lobby, &[a.pid], &[]).await?;
        if !has_session(&friend.search_with_participants(&[a.pid]).await?, lobby) {
            problems.push(format!("{how}: friends don't find the new lobby"));
        }
    }
    a.add_participants(rejoined, &[a.pid], &[]).await?;
    // The old connections time out (a minute without a packet) and the old NAT helper
    // registrations (90 s): the new game keeps talking, as a game's own traffic does.
    let until = std::time::Instant::now() + Duration::from_secs(100);
    while std::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_secs(10)).await;
        a.search_sessions("113 => 1;103 => 0").await?;
        friend.search_sessions("113 => 1;103 => 0").await?;
        if reach != Reach::Unregistered {
            a_reg = reconnect_register(server, reach, &a_nat, &a).await.map_err(|e| eyre!("{reach:?}: re-probing: {e}"))?;
            f_reg = reconnect_register(server, reach, &f_nat, &friend).await.map_err(|e| eyre!("{reach:?}: re-probing: {e}"))?;
        }
    }
    let found = friend.search_with_participants(&[a.pid]).await?;
    if !has_session(&found, lobby) {
        problems.push(format!("{reach:?}: the old connections timing out took the new game's lobby with them"));
    }
    let stale: Vec<u32> = lost.into_iter().filter(|l| has_session(&found, *l)).collect();
    if !stale.is_empty() {
        problems.push(format!(
            "{reach:?}: the old connections timed out, but friends still find the lobbies the dropped games made ({stale:?})"
        ));
    }
    if has_session(&found, theirs) {
        problems.push(format!(
            "{reach:?}: the old connections timed out, but the player is still in the friend's lobby the dropped games joined"
        ));
    }
    if !has_session(&found, rejoined) {
        problems.push(format!(
            "{reach:?}: the old connections timing out took the player out of a lobby the new game joined again"
        ));
    }
    if !has_session(&friend.search_with_participants(&[friend.pid]).await?, theirs) {
        problems.push(format!("{reach:?}: the friend's own lobby ended when the dropped player's old connections did"));
    }
    if let (Reach::Relayed, Some(ar), Some(fr)) = (reach, &a_reg, &f_reg) {
        if !reconnect_relays(server, &f_nat, fr, &a_nat, ar).await? || !reconnect_relays(server, &a_nat, ar, &f_nat, fr).await? {
            problems.push(format!("{reach:?}: once the old registrations expired, the relay no longer carries the reconnected player"));
        }
    }
    a.disconnect().await?;
    friend.disconnect().await?;
    Ok(problems)
}

/// A ticket works only from the address that asked for it, or one next to it (a VPN's other
/// exit, the same /24): someone who saw it (and the CONNECT) on the way can't sign in with it
/// from elsewhere. Needs a server on loopback, where 127.0.0.2 is a neighbouring address and
/// 127.0.2.1 a farther one.
async fn ticket_elsewhere(ctx: &mut Ctx) -> Result<()> {
    if !ctx.server.is_loopback() {
        println!("  (skipped: needs a server on 127.0.0.1)");
        return Ok(());
    }
    ctx.n += 1;
    let name = format!("Ticket{}_{}", ctx.run, ctx.n);
    Bot::register(ctx.server, &name, PASSWORD).await?;
    let stolen = Bot::login_from(ctx.server, &name, PASSWORD, "127.0.2.1".parse()?).await;
    ensure!(stolen.is_err(), "a ticket was used from another address");
    let neighbour = Bot::login_from(ctx.server, &name, PASSWORD, "127.0.0.2".parse()?).await;
    ensure!(neighbour.is_ok(), "a ticket from the next address (a VPN's other exit) was refused");
    neighbour?.disconnect().await?;
    Bot::login(ctx.server, &name, PASSWORD).await?.disconnect().await
}

/// A flood of SYNs that never go on to CONNECT (what forged sources look like) leaves
/// nothing behind, so it doesn't keep anyone from signing in, not even from the same address.
async fn syn_flood(ctx: &mut Ctx) -> Result<()> {
    use quazal::prudp::packet::PacketFlag;
    use quazal::prudp::packet::PacketType;
    use quazal::prudp::packet::QPacket;
    use quazal::prudp::packet::StreamType;
    use quazal::prudp::packet::VPort;
    let qctx = quazal::Context::splinter_cell_blacklist();
    let auth = (ctx.server, testbot::bot::target(ctx.server).auth);
    for _ in 0..40 {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        for session in 0..50u8 {
            let syn = QPacket {
                source: VPort {
                    port: 15,
                    stream_type: StreamType::RVSec,
                },
                destination: VPort {
                    port: 1,
                    stream_type: StreamType::RVSec,
                },
                packet_type: PacketType::Syn,
                flags: PacketFlag::NeedAck.into(),
                conn_signature: Some(0),
                session_id: session,
                ..Default::default()
            };
            sock.send_to(&syn.to_bytes(&qctx), auth)?;
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let a = ctx.player("Flooded").await?;
    a.disconnect().await
}

/// Junk on the server's ports doesn't take it down.
async fn bad_packets(ctx: &mut Ctx) -> Result<()> {
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let junk: [&[u8]; 5] = [
        b"",
        b"\x31\x3f\x04\x00\x00\x00\x00\x00\x00\x00",
        b"\xff\xff\xff\xff\xff\xff",
        &[0x3f; 900],
        b"\x31\x3f\x02\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff",
    ];
    for port in [21126u16, 21127] {
        for j in junk {
            sock.send_to(j, (ctx.server, port))?;
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let a = ctx.player("After").await?;
    a.disconnect().await
}

/// A guest who leaves a lobby is no longer in it (friends' searches for them
/// don't return it).
async fn leave_session(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    a.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    b.add_participants(lobby, &[b.pid], &[]).await?;
    b.join_session(lobby).await?;
    let guest = b.pid;
    ensure!(
        has_session(&a.search_with_participants(&[guest]).await?, lobby),
        "the guest isn't in the lobby after joining"
    );
    b.leave_session(lobby).await?;
    ensure!(
        !has_session(&a.search_with_participants(&[guest]).await?, lobby),
        "the guest is still in the lobby after leaving"
    );
    ensure!(has_session(&b.search_with_participants(&[a.pid]).await?, lobby), "the host's lobby went with the guest");
    a.disconnect().await?;
    b.disconnect().await
}

/// A host who abandons their lobby (nobody left in it) ends it.
async fn abandon_empty(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    a.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    ensure!(has_session(&b.search_with_participants(&[a.pid]).await?, lobby), "lobby not found");
    a.abandon_session(lobby).await?;
    ensure!(
        !has_session(&b.search_with_participants(&[a.pid]).await?, lobby),
        "an abandoned, empty lobby is still found"
    );
    ensure!(
        !b.search_sessions("113 => 1;103 => 0").await?.iter().any(|(s, _)| *s == lobby),
        "an abandoned, empty lobby is still offered by matchmaking"
    );
    a.disconnect().await?;
    b.disconnect().await
}

/// A retransmitted CreateSession (the same packet twice) creates one session.
async fn duplicate_request(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    a.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let lobby = a.create_session_twice(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mine: Vec<u32> = b
        .search_sessions("113 => 1;103 => 0")
        .await?
        .into_iter()
        .filter(|(_, host)| *host == a.pid)
        .map(|(s, _)| s)
        .collect();
    ensure!(mine == [lobby], "a retransmitted request created {} sessions: {mine:?}", mine.len());
    a.disconnect().await?;
    b.disconnect().await
}

/// A lost "come in" push is sent again, so the guest still gets into the match.
async fn lost_push(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    a.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    b.register_urls(&["prudp:/address=127.0.0.1;port=3075;sid=15;type=3"]).await?;
    let lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    let game = a.create_session(PRIVATE_MATCH).await?;
    a.add_participants(game, &[], &[a.pid]).await?;
    let own = b.create_session(LOBBY).await?;
    b.add_participants(own, &[b.pid], &[]).await?;
    a.invite(&b.name).await?;
    ensure!(b.poll_invite(Duration::from_secs(3)).await?.is_some(), "invite not delivered");
    b.search_with_participants(&[a.pid]).await?;
    b.abandon_session(own).await?;
    b.split_session(own).await?;
    b.drop_next_push();
    b.add_participants(game, &[], &[b.pid]).await?;
    let push = b
        .wait_notification(Duration::from_secs(5))
        .await?
        .ok_or_else(|| eyre!("the lost push was never sent again"))?;
    ensure!(push.ui_type == 7003 && push.ui_param_2 == game, "wrong push: {push:?}");
    a.disconnect().await?;
    b.disconnect().await
}

/// The NAT helper's address.
fn nat_addr(server: IpAddr, second: bool) -> Result<std::net::SocketAddrV4> {
    let IpAddr::V4(ip) = server else { return Err(eyre!("the NAT helper needs IPv4")) };
    let port = testbot::bot::target(server).nat;
    Ok(std::net::SocketAddrV4::new(ip, if second { port + 1 } else { port }))
}

async fn nat_wait(sock: &tokio::net::UdpSocket, wait: Duration) -> Option<nat_proto::Message> {
    let mut buf = [0u8; 2048];
    let (n, _) = tokio::time::timeout(wait, sock.recv_from(&mut buf)).await.ok()?.ok()?;
    nat_proto::Message::decode(&buf[..n])
}

/// The next relayed packet, skipping late answers to earlier probes.
async fn nat_wait_data(sock: &tokio::net::UdpSocket, wait: Duration) -> Option<nat_proto::Message> {
    let until = tokio::time::Instant::now() + wait;
    loop {
        let left = until.checked_duration_since(tokio::time::Instant::now())?;
        match nat_wait(sock, left).await? {
            nat_proto::Message::ProbeReply { .. } => continue,
            other => return Some(other),
        }
    }
}

/// The NAT helper tells anyone the address it sees, on both of its ports, but
/// registers only a player with their ticket who proved the address.
async fn nat_probe_scenario(ctx: &mut Ctx) -> Result<()> {
    use nat_proto::Message;
    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let local = sock.local_addr()?;
    let probe = |flags, nonce| {
        Message::Probe {
            flags,
            nonce,
            mapping: None,
            name: String::new(),
            ticket: [0; 16],
            cookie: [0; 16],
            rtt_ms: None,
        }
        .encode()
    };
    sock.send_to(&probe(0, 7), nat_addr(ctx.server, false)?).await?;
    let Some(Message::ProbeReply { observed, tag, .. }) = nat_wait(&sock, Duration::from_secs(2)).await else {
        return Err(eyre!("no answer to a plain probe"));
    };
    ensure!(observed.port() == local.port(), "observed {observed}, but the socket is {local}");
    ensure!(tag == [0; 8], "a probe without a ticket was registered");
    sock.send_to(&probe(nat_proto::probe_flags::SECOND_PORT, 8), nat_addr(ctx.server, true)?).await?;
    match nat_wait(&sock, Duration::from_secs(2)).await {
        Some(Message::ProbeReply { observed: o2, .. }) => ensure!(o2 == observed, "the second port saw {o2}, the first {observed}"),
        other => return Err(eyre!("unexpected answer {other:?}")),
    }

    let player = ctx.player("Prober").await?;
    let r = testbot::bot::nat_register(&sock, nat_addr(ctx.server, false)?, 0, &player.name, player.nat_ticket).await?;
    ensure!(
        r.advertise == r.observed && !r.relayed,
        "a local player is advertised as {} (relayed: {})",
        r.advertise,
        r.relayed
    );
    // Someone else's name, without its ticket, gets nowhere.
    let other = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    ensure!(
        testbot::bot::nat_register(&other, nat_addr(ctx.server, false)?, 0, &player.name, [9; 16]).await.is_err(),
        "a probe with a forged ticket registered"
    );
    player.disconnect().await
}

/// Two players, one relayed: packets reach each other through the relay,
/// each seeing the other at its advertised address; strangers, and packets
/// without the right tag, get nowhere.
async fn nat_relay(ctx: &mut Ctx) -> Result<()> {
    use nat_proto::Message;
    let nat = nat_addr(ctx.server, false)?;
    let (mut pa, mut pb) = (ctx.player("Relayed").await?, ctx.player("Direct").await?);
    let a = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let b = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    // Someone at another address (on loopback, 127.0.0.3): the same address with the tag is the
    // player's own other port, which the relay takes (see below).
    let stranger = tokio::net::UdpSocket::bind(if ctx.server.is_loopback() { "127.0.0.3:0" } else { "0.0.0.0:0" }).await?;
    let ra = testbot::bot::nat_register(&a, nat, nat_proto::probe_flags::WANT_RELAY, &pa.name, pa.nat_ticket).await?;
    let rb = testbot::bot::nat_register(&b, nat, 0, &pb.name, pb.nat_ticket).await?;
    ensure!(ra.relayed && !rb.relayed, "relayed: a {}, b {}", ra.relayed, rb.relayed);
    ensure!(ra.advertise.port() >= 40000, "a relay address was expected, got {}", ra.advertise);

    let send = |tag, to, payload: &[u8]| {
        Message::DataTo {
            tag,
            to,
            payload: payload.to_vec(),
        }
        .encode()
    };
    // Strangers: the relay carries game traffic only between players the server put together.
    b.send_to(&send(rb.tag, ra.advertise, b"from a stranger"), nat).await?;
    ensure!(
        nat_wait_data(&a, Duration::from_millis(600)).await.is_none(),
        "the relay carried a packet between two players in no room together"
    );
    let room = pa.create_session(LOBBY).await?;
    pa.add_participants(room, &[pa.pid], &[]).await?;
    pb.add_participants(room, &[pb.pid], &[]).await?;
    // The refusal stands a second (the game retries; its join may come a moment later).
    tokio::time::sleep(Duration::from_millis(1100)).await;
    b.send_to(&send(rb.tag, ra.advertise, b"to the relayed player"), nat).await?;
    ensure!(
        nat_wait_data(&a, Duration::from_secs(2)).await
            == Some(Message::DataFrom {
                tag: ra.tag,
                from: rb.advertise,
                payload: b"to the relayed player".to_vec()
            }),
        "the relayed player got something else"
    );
    a.send_to(&send(ra.tag, rb.advertise, b"and back"), nat).await?;
    let got = nat_wait_data(&b, Duration::from_secs(2)).await;
    ensure!(
        got == Some(Message::DataFrom {
            tag: rb.tag,
            from: ra.advertise,
            payload: b"and back".to_vec()
        }),
        "the direct player got something else: {got:?} (a: {ra:?}, b: {rb:?})"
    );
    // The largest game packet, as Storm sends them while a co-op mission loads.
    let full = vec![0x33; nat_proto::MAX_PAYLOAD];
    b.send_to(&send(rb.tag, ra.advertise, &full), nat).await?;
    ensure!(
        nat_wait_data(&a, Duration::from_secs(2)).await
            == Some(Message::DataFrom {
                tag: ra.tag,
                from: rb.advertise,
                payload: full
            }),
        "a full-size game packet wasn't relayed"
    );
    if ctx.server.is_loopback() {
        stranger.send_to(&send(rb.tag, ra.advertise, b"spam"), nat).await?;
    }
    b.send_to(&send([1; 8], ra.advertise, b"wrong tag"), nat).await?;
    ensure!(
        nat_wait_data(&a, Duration::from_millis(400)).await.is_none(),
        "a stranger's or an untagged packet was relayed"
    );
    // The player's game sending from another port of its address (its second Storm socket,
    // or a NAT giving each its own), with its tag: theirs, and the answer comes back there.
    let b2 = tokio::net::UdpSocket::bind(b.local_addr()?.ip().to_string() + ":0").await?;
    b2.send_to(&send(rb.tag, ra.advertise, b"from my other port"), nat).await?;
    ensure!(
        nat_wait_data(&a, Duration::from_secs(2)).await
            == Some(Message::DataFrom {
                tag: ra.tag,
                from: rb.advertise,
                payload: b"from my other port".to_vec()
            }),
        "the player's traffic from another port of its address wasn't relayed"
    );
    a.send_to(&send(ra.tag, rb.advertise, b"back to the other port"), nat).await?;
    ensure!(
        nat_wait_data(&b2, Duration::from_secs(2)).await
            == Some(Message::DataFrom {
                tag: rb.tag,
                from: ra.advertise,
                payload: b"back to the other port".to_vec()
            }),
        "the answer didn't go back to the port that sent"
    );
    pa.disconnect().await?;
    pb.disconnect().await
}

/// A relayed player joins a direct player's match, both games having told the NAT helper
/// their round trip to it: the join is noted with how the two reach each other (the
/// federation test checks it reaches the coordinator).
async fn relay_ping(ctx: &mut Ctx) -> Result<()> {
    let nat = nat_addr(ctx.server, false)?;
    let (mut host, mut guest) = (ctx.player("PingHost").await?, ctx.player("PingGuest").await?);
    let (h, g) = (tokio::net::UdpSocket::bind("0.0.0.0:0").await?, tokio::net::UdpSocket::bind("0.0.0.0:0").await?);
    let rh = testbot::bot::nat_register(&h, nat, 0, &host.name, host.nat_ticket).await?;
    let rg = testbot::bot::nat_register(&g, nat, nat_proto::probe_flags::WANT_RELAY, &guest.name, guest.nat_ticket).await?;
    ensure!(rg.relayed && !rh.relayed, "relayed: guest {}, host {}", rg.relayed, rh.relayed);
    let room = host.create_session("113 => 0;3 => 8;4 => 0;102 => 7").await?;
    guest.join_session(room).await?;
    // Saved every couple of seconds; the players stay until then.
    tokio::time::sleep(Duration::from_secs(3)).await;
    guest.disconnect().await?;
    host.disconnect().await
}

/// A game that still registers its local address gets the public one the
/// NAT helper found (with the local one kept for players on its network).
async fn nat_public_address(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    let storm = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let advertise = testbot::bot::nat_register(&storm, nat_addr(ctx.server, false)?, 0, &a.name, a.nat_ticket).await?.advertise;
    // 127.0.0.1 is what the trusted_subnet rule would give too, so only the
    // NAT helper changes the port.
    a.register_urls(&["prudp:/address=127.0.0.1;port=13000;RVCID=5;hdrType=0;type=2"]).await?;
    let lobby = a.create_session(LOBBY).await?;
    a.add_participants(lobby, &[a.pid], &[]).await?;
    let found = b.search_with_participants(&[a.pid]).await?;
    let room = found
        .iter()
        .find(|r| r.game_session_search_result.session_key.session_id == lobby)
        .ok_or_else(|| eyre!("lobby not found"))?;
    let urls: Vec<(String, u16, Option<String>)> = room
        .game_session_search_result
        .host_urls
        .0
        .iter()
        .map(|u| (u.address.clone(), u.port, u.params.get("type").cloned()))
        .collect();
    ensure!(
        urls.contains(&(advertise.ip().to_string(), advertise.port(), Some("2".into()))),
        "the public address {advertise} isn't advertised: {urls:?}"
    );
    ensure!(urls.contains(&("127.0.0.1".into(), 13000, None)), "the local address wasn't kept: {urls:?}");
    a.disconnect().await?;
    b.disconnect().await
}

/// Clients older than the server allows can't sign in, and neither can the game after one
/// tried; a current client lets it in.
async fn outdated_client(ctx: &mut Ctx) -> Result<()> {
    use testbot::bot::api_sign_in;
    ctx.n += 1;
    let name = format!("Outdated{}_{}", ctx.run, ctx.n);
    Bot::register(ctx.server, &name, PASSWORD).await?;
    ensure!(
        Bot::game_sign_in_only(ctx.server, &name, PASSWORD).await.is_err(),
        "the game signed in with no client signing in first"
    );
    for client in ["game/0.0.1", "", "launcher/0.1.0-dev"] {
        match api_sign_in(ctx.server, &name, PASSWORD, client).await {
            Err(e) if e.downcast_ref::<tonic::Status>().is_some_and(|s| s.code() == tonic::Code::FailedPrecondition) => {}
            other => return Err(eyre!("the client {client:?} wasn't refused: {:?}", other.map(|_| ()))),
        }
        ensure!(
            Bot::game_sign_in_only(ctx.server, &name, PASSWORD).await.is_err(),
            "the game signed in after the client {client:?}"
        );
    }
    // A current client lets the game in; an outdated one trying after it shuts it out again.
    Bot::login(ctx.server, &name, PASSWORD).await.map_err(|e| eyre!("a current client couldn't play: {e}"))?;
    ensure!(api_sign_in(ctx.server, &name, PASSWORD, "game/0.0.1").await.is_err(), "an outdated client was let in");
    ensure!(
        Bot::game_sign_in_only(ctx.server, &name, PASSWORD).await.is_err(),
        "the game signed in after an outdated client tried"
    );
    api_sign_in(ctx.server, &name, PASSWORD, testbot::bot::GAME_CLIENT).await?;
    Bot::game_sign_in_only(ctx.server, &name, PASSWORD)
        .await
        .map_err(|e| eyre!("the game couldn't sign in after a current client: {e}"))?;
    Ok(())
}

/// A probe between players of a public match goes ahead only with its host, or when both
/// games asked to reach each other (as two players really in the match do): anyone may be
/// counted in a public match without playing in it, and a probe hands each side the other's
/// address. Three players and a stranger, as in a real match.
async fn public_match_probes(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut first = ctx.player("Guest").await?;
    let mut second = ctx.player("Guest").await?;
    let mut stranger = ctx.player("Stranger").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let game = host.create_session("113 => 0;103 => 1;3 => 8;4 => 0;102 => 8").await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    first.join_session(game).await?;
    second.join_session(game).await?;
    // The stranger adds themselves (public rooms let anyone) and plays no part.
    stranger.add_participants(game, &[], &[stranger.pid]).await?;
    let (h, f, s) = (host.connection_id().await?, first.connection_id().await?, second.connection_id().await?);
    let station = |port: u16| format!("udp:/address=127.0.0.1;port={port}");
    let quiet = Duration::from_millis(700);

    // Guest to guest, the other not having asked: refused; the relay carries them instead.
    first.request_probes(&station(13001), &[s]).await?;
    ensure!(second.wait_probe(quiet).await?.is_none(), "a guest was probed for another guest who hadn't asked");
    // The other asks back: both games want it, so it goes ahead.
    second.request_probes(&station(13002), &[f]).await?;
    ensure!(
        first.wait_probe(Duration::from_secs(3)).await?.is_some_and(|url| url.contains("13002")),
        "two guests who both asked weren't probed to each other"
    );
    // The host, either way.
    first.request_probes(&station(13001), &[h]).await?;
    ensure!(host.wait_probe(Duration::from_secs(3)).await?.is_some(), "a guest couldn't reach the host");
    host.request_probes(&station(13000), &[s]).await?;
    ensure!(second.wait_probe(Duration::from_secs(3)).await?.is_some(), "the host couldn't reach a guest");
    // The stranger: never, for anyone but the host.
    stranger.request_probes(&station(13009), &[f, s]).await?;
    ensure!(first.wait_probe(quiet).await?.is_none(), "a stranger in the match was probed to a guest");
    ensure!(second.wait_probe(quiet).await?.is_none(), "a stranger in the match was probed to a guest");
    for bot in [host, first, second, stranger] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// The same in a party: anyone may join one by its id, so a probe between its members goes
/// ahead only with its host, or when both games asked.
async fn party_probes(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut friend = ctx.player("Friend").await?;
    let mut stranger = ctx.player("Stranger").await?;
    let party = host.create_session(LOBBY).await?;
    host.add_participants(party, &[host.pid], &[]).await?;
    friend.add_participants(party, &[friend.pid], &[]).await?;
    // The stranger joins by the party's id alone, as anyone could.
    stranger.join_session(party).await?;
    let (h, f) = (host.connection_id().await?, friend.connection_id().await?);
    let quiet = Duration::from_millis(700);
    stranger.request_probes("udp:/address=127.0.0.1;port=13019", &[f]).await?;
    ensure!(friend.wait_probe(quiet).await?.is_none(), "a stranger in a party was probed to a member");
    stranger.request_probes("udp:/address=127.0.0.1;port=13019", &[h]).await?;
    ensure!(host.wait_probe(Duration::from_secs(3)).await?.is_some(), "the party's host, as anyone's");
    friend.request_probes("udp:/address=127.0.0.1;port=13018", &[h]).await?;
    ensure!(host.wait_probe(Duration::from_secs(3)).await?.is_some(), "a member couldn't reach the host");
    for bot in [host, friend, stranger] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// A party member the game took off the party's participants may still split it to carry it
/// on (eu1 2026-10-12: five members of one party refused at once); a stranger still may not.
async fn party_member_split(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut member = ctx.player("Member").await?;
    let mut stranger = ctx.player("Stranger").await?;
    let party = host.create_session(LOBBY).await?;
    host.add_participants(party, &[host.pid], &[]).await?;
    member.add_participants(party, &[member.pid], &[]).await?;
    member.remove_participants(party, &[member.pid]).await?;
    ensure!(stranger.split_session(party).await.is_err(), "a stranger split the party");
    let split = member
        .split_session(party)
        .await
        .map_err(|e| eyre!("a member taken off the party's participants couldn't split it: {e}"))?;
    ensure!(split != party, "the split made no new session");
    for bot in [host, member, stranger] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// A party member the host is friends with invites to the party: the newcomer gets into it
/// though it's invite-only. Someone who walked in by the party's id may not, and a member's
/// invitation never leads into a private match, even one the party was turned into.
async fn member_invite(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut member = ctx.player("Member").await?;
    let mut newcomer = ctx.player("Newcomer").await?;
    let mut walkin = ctx.player("Walkin").await?;
    let mut other = ctx.player("Other").await?;
    let mut late = ctx.player("Late").await?;
    ensure!(host.friend_change("request", &member.name).await? == Relation::RequestSent, "no friend request");
    ensure!(member.friend_change("accept", &host.name).await? == Relation::Friend, "not friends");
    let party = host.create_session(LOBBY).await?;
    host.add_participants(party, &[host.pid], &[]).await?;
    member.add_participants(party, &[member.pid], &[]).await?;
    // Walks in by the party's id while it's public; then the host makes it invite-only.
    walkin.add_participants(party, &[walkin.pid], &[]).await?;
    host.set_session(party, true).await?;
    let in_party = |found: &[GameSessionSearchWithParticipantsResult], who: u32| {
        found
            .iter()
            .find(|r| r.game_session_search_result.session_key.session_id == party)
            .is_some_and(|r| r.participant_ids.0.contains(&who))
    };

    member.invite(&newcomer.name).await?;
    ensure!(newcomer.poll_invite(Duration::from_secs(3)).await?.is_some(), "the member's invitation wasn't delivered");
    ensure!(
        has_session(&newcomer.search_with_participants(&[member.pid]).await?, party),
        "the newcomer doesn't find the member's party"
    );
    newcomer.join_session(party).await?;
    ensure!(
        in_party(&host.search_with_participants(&[host.pid]).await?, newcomer.pid),
        "invited by a member, the newcomer isn't in the invite-only party"
    );

    // Walked in by the party's id: not a member the host trusts.
    walkin.invite(&other.name).await?;
    ensure!(other.poll_invite(Duration::from_secs(3)).await?.is_some(), "the walk-in's invitation wasn't delivered");
    other.join_session(party).await?;
    ensure!(
        !in_party(&host.search_with_participants(&[host.pid]).await?, other.pid),
        "invited by someone who walked in, a stranger got into the invite-only party"
    );

    // Turned into a private match, the member's invitation leads nowhere.
    member.invite(&late.name).await?;
    ensure!(late.poll_invite(Duration::from_secs(3)).await?.is_some(), "the second invitation wasn't delivered");
    member.update_session(party, PRIVATE_MATCH).await?;
    ensure!(
        late.add_participants(party, &[], &[late.pid]).await.is_err(),
        "a member's invitation got someone into a private match"
    );
    for bot in [host, member, newcomer, walkin, other, late] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// A failed join is explained in the game's overlay the moment it's reported (a game that
/// shows notices takes them from the event poll), and what was explained there isn't counted
/// for the launcher's "how did it go"; a failure with no known reason still is. A game that
/// doesn't show notices (before 0.4.4) never takes them, so nothing counts as explained for
/// it; and the game's own room isn't a failure to tell anyone about.
async fn join_notice(ctx: &mut Ctx) -> Result<()> {
    const LOCKED: u32 = 0x9718_2b1b;
    const TIMED_OUT: u32 = 0xac2d_801a;
    let mut host = ctx.player("Host").await?;
    let mut joiner = ctx.player("Joiner").await?;
    let mut old = ctx.player("OldGame").await?;
    let lobby = host.create_session(LOBBY).await?;
    host.add_participants(lobby, &[host.pid], &[]).await?;
    let wait = Duration::from_secs(3);

    joiner.report_failed_joins(&[(lobby, LOCKED)]).await?;
    let (title, text) = joiner.poll_notice(wait).await?.ok_or_else(|| eyre!("no notice for a locked session"))?;
    ensure!(title == "Couldn't join: the session is locked", "{title}");
    ensure!(text.contains("match is already under way"), "{text}");
    let summary = joiner.session_summary().await?;
    ensure!(
        summary.failed_joins == 1 && summary.joins_explained == 1,
        "explained in the game, so not for the launcher to ask about: {summary:?}"
    );

    joiner.report_failed_joins(&[(lobby, TIMED_OUT)]).await?;
    let (title, _) = joiner.poll_notice(wait).await?.ok_or_else(|| eyre!("no notice for a join that timed out"))?;
    ensure!(title == "Couldn't join", "{title}");
    let summary = joiner.session_summary().await?;
    ensure!(
        summary.failed_joins == 2 && summary.joins_explained == 1,
        "a failure with no known reason is still the launcher's to ask about: {summary:?}"
    );

    // The joiner's own lobby: the game's own churn, nothing to tell.
    let own = joiner.create_session(LOBBY).await?;
    joiner.add_participants(own, &[joiner.pid], &[]).await?;
    joiner.report_failed_joins(&[(own, LOCKED)]).await?;
    ensure!(joiner.poll_notice(Duration::from_secs(1)).await?.is_none(), "a notice for the game's own room");

    // A game that doesn't show notices polls without asking for them.
    old.report_failed_joins(&[(lobby, LOCKED)]).await?;
    ensure!(old.poll_invite(Duration::from_secs(1)).await?.is_none());
    let summary = old.session_summary().await?;
    ensure!(
        summary.failed_joins == 1 && summary.joins_explained == 0,
        "a game that can't show notices had one counted as explained: {summary:?}"
    );
    for bot in [host, joiner, old] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// Someone joining a public match from a search reaches its players through the relay
/// before their join reaches the server (the host's game has them connect first), once
/// they've probed its host: not before, and not by probes.
async fn relay_to_joiner(ctx: &mut Ctx) -> Result<()> {
    use nat_proto::Message;
    let nat = nat_addr(ctx.server, false)?;
    let mut host = ctx.player("Host").await?;
    let mut member = ctx.player("Member").await?;
    let mut joiner = ctx.player("Joiner").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let game = host.create_session("113 => 0;103 => 1;3 => 8;4 => 0;102 => 7").await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    member.join_session(game).await?;
    let (m, j) = (tokio::net::UdpSocket::bind("0.0.0.0:0").await?, tokio::net::UdpSocket::bind("0.0.0.0:0").await?);
    let rm = testbot::bot::nat_register(&m, nat, nat_proto::probe_flags::WANT_RELAY, &member.name, member.nat_ticket).await?;
    let rj = testbot::bot::nat_register(&j, nat, 0, &joiner.name, joiner.nat_ticket).await?;
    ensure!(rm.relayed && !rj.relayed, "relayed: member {}, joiner {}", rm.relayed, rj.relayed);
    let send = |payload: &[u8]| {
        Message::DataTo {
            tag: rm.tag,
            to: rj.advertise,
            payload: payload.to_vec(),
        }
        .encode()
    };
    m.send_to(&send(b"too soon"), nat).await?;
    ensure!(nat_wait_data(&j, Duration::from_millis(600)).await.is_none(), "relayed to someone in no match yet");
    // The joiner's game probes the host it found.
    let h = host.connection_id().await?;
    joiner.request_probes("udp:/address=127.0.0.1;port=13005", &[h]).await?;
    ensure!(host.wait_probe(Duration::from_secs(3)).await?.is_some(), "the joiner couldn't probe the public host");
    // The refusal above stands a second.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    m.send_to(&send(b"welcome"), nat).await?;
    ensure!(
        nat_wait_data(&j, Duration::from_secs(2)).await
            == Some(Message::DataFrom {
                tag: rj.tag,
                from: rm.advertise,
                payload: b"welcome".to_vec()
            }),
        "a player of the match didn't reach the joiner through the relay"
    );
    for bot in [host, member, joiner] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// A search that finds nothing while a lobby with other game data is open (one PC modded, the
/// other not) is told: the player's summary counts it and names both sides' data.
async fn search_other_data(ctx: &mut Ctx) -> Result<()> {
    const STOCK: u32 = 0xe90c_0d2d;
    const BALACLAVA: u32 = 0xdd55_acd3;
    let mut host = ctx.player("Host").await?;
    let mut modded = ctx.player("Modded").await?;
    let mut plain = ctx.player("Plain").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let map = 920_000 + ctx.run;
    let game = host
        .create_session(&format!("113 => 0;103 => 1;3 => 8;4 => 0;102 => 8;101 => {map};107 => {STOCK}"))
        .await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    let query = |data: u32| format!("113 => 0;103 => 1;102 => 8;101 => {map};107 => {data}");
    ensure!(
        plain.search_sessions(&query(STOCK)).await?.iter().any(|(s, _)| *s == game),
        "the same data doesn't find the lobby"
    );
    ensure!(modded.search_sessions(&query(BALACLAVA)).await?.is_empty(), "other data found the lobby");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let summary = modded.session_summary().await.map_err(|e| eyre!("session summary: {}", e.message()))?;
        if summary.searches_other_data > 0 {
            ensure!(
                !summary.other_data_mine.is_empty() && !summary.other_data_theirs.is_empty() && summary.other_data_mine != summary.other_data_theirs,
                "both sides' data aren't named: mine {:?}, theirs {:?}",
                summary.other_data_mine,
                summary.other_data_theirs
            );
            break;
        }
        ensure!(tokio::time::Instant::now() < deadline, "the summary doesn't count the hidden lobby");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // The game's overlay says so as it happens, and then the launcher needn't ask.
    let (title, text) = modded
        .poll_notice(Duration::from_secs(3))
        .await?
        .ok_or_else(|| eyre!("no notice for lobbies hidden by game data"))?;
    ensure!(title == "No lobbies with your game data" && text.contains("unmodified"), "{title}: {text}");
    let summary = modded.session_summary().await.map_err(|e| eyre!("session summary: {}", e.message()))?;
    ensure!(
        summary.searches_explained == summary.searches_other_data,
        "told in the game, still counted for the launcher: {summary:?}"
    );
    for bot in [host, modded, plain] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// A GET to the server's community API (its config port: `CONFIG_PORT`, else 80), as the
/// launcher and the overlay ask it; the body of a 200.
fn community_get(server: IpAddr, path: &str) -> Result<String> {
    use std::io::Read;
    use std::io::Write;
    let port: u16 = std::env::var("CONFIG_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(80);
    let mut stream = std::net::TcpStream::connect_timeout(&std::net::SocketAddr::new(server, port), Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {server}\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (head, body) = response.split_once("\r\n\r\n").ok_or_else(|| eyre!("no HTTP answer"))?;
    ensure!(
        head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200"),
        "{path}: {}",
        head.lines().next().unwrap_or_default()
    );
    Ok(body.to_string())
}

/// What's being played: a friend's line has the match's mode and map, and the server's
/// `/api/playing` lists the public match (mode, map, players and seats) and only counts the
/// private one.
async fn whats_being_played(ctx: &mut Ctx) -> Result<()> {
    const VIRUS_VAULT: u32 = 0x05F6_9010;
    let mut host = ctx.player("Host").await?;
    let mut friend = ctx.player("Friend").await?;
    let mut coop = ctx.player("Coop").await?;
    ensure!(host.friend_change("request", &friend.name).await? == Relation::RequestSent, "the request wasn't sent");
    ensure!(friend.friend_change("accept", &host.name).await? == Relation::Friend, "not friends");
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let game = host.create_session(&format!("113 => 0;103 => 1;3 => 8;4 => 0;102 => 8;101 => {VIRUS_VAULT}")).await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    friend.join_session(game).await?;
    let private = coop.create_session("113 => 0;103 => 0;3 => 0;4 => 2;102 => 3").await?;
    coop.add_participants(private, &[], &[coop.pid]).await?;

    // The friend's line: SvM Classic on Virus Vault.
    let lists = friend.relationships().await?;
    let line = lists.friends.iter().find(|p| p.username == host.name).and_then(|p| p.activity.clone());
    ensure!(
        line.as_ref().is_some_and(|a| a.game_mode == 8 && a.map == VIRUS_VAULT && a.room == "match"),
        "the friend's activity: {line:?}"
    );
    ensure!(
        line.as_ref().and_then(server_api::playing::activity_what).as_deref() == Some("SvM Classic · Virus Vault"),
        "reads as {:?}",
        line.as_ref().and_then(server_api::playing::activity_what)
    );

    // The server's list (worked out at most every 5 seconds).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let server = ctx.server;
        let body = tokio::task::spawn_blocking(move || community_get(server, "/api/playing")).await??;
        let playing: server_api::playing::Playing = serde_json::from_str(&body)?;
        let listed = playing.matches.iter().find(|m| m.map == Some(VIRUS_VAULT) && m.game_mode == Some(8));
        if let Some(m) = listed {
            ensure!((m.players, m.seats) == (2, 8), "the public match reads {}", m.line());
            ensure!(m.line() == "SvM Classic · Virus Vault · 2/8", "{}", m.line());
            ensure!(playing.private_matches >= 1, "the private match isn't counted: {body}");
            ensure!(!body.contains(&host.name) && !body.contains(&coop.name), "names in what's being played: {body}");
            break;
        }
        ensure!(tokio::time::Instant::now() < deadline, "the public match isn't listed: {body}");
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
    for bot in [host, friend, coop] {
        bot.disconnect().await?;
    }
    Ok(())
}

const SCENARIOS: &[&str] = &[
    "login",
    "public-match-probes",
    "party-probes",
    "party-member-split",
    "member-invite",
    "join-notice",
    "relay-to-joiner",
    "search-other-data",
    "whats-being-played",
    "lobby-invite",
    "private-match-invite",
    "cleanup",
    "trusted-subnet",
    "bad-packets",
    "leave-session",
    "abandon-empty",
    "duplicate-request",
    "lost-push",
    "nat-probe",
    "nat-relay",
    "relay-ping",
    "nat-public-address",
    "friends",
    "block",
    "invite-queue",
    "identity-login",
    "rename",
    "direct-test",
    "presence",
    "slow-handshake",
    "second-sign-in",
    "reconnect",
    "ticket-elsewhere",
    "private-room-join",
    "full-room",
    "host-leaves-match",
    "split-limits",
    "private-room-hidden",
    "station-url-schemes",
    "syn-flood",
    "login-lockout",
    "outdated-client",
    "report",
    "client-log",
    "name-check",
    "session-lifecycle",
    "game-invitations",
    "failed-joins",
    "stats",
];

#[tokio::main]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut host = String::from("127.0.0.1");
    if let Some(i) = args.iter().position(|a| a == "--server") {
        host = args.get(i + 1).ok_or_else(|| eyre!("--server needs an address"))?.clone();
        args.drain(i..=i + 1);
    }
    let mut other: Option<IpAddr> = None;
    let mut other_host = None;
    if let Some(i) = args.iter().position(|a| a == "--other") {
        let o = args.get(i + 1).ok_or_else(|| eyre!("--other needs an address"))?.clone();
        other = Some(o.parse().ok().or_else(|| setup::net::resolve(&o)).ok_or_else(|| eyre!("can't resolve {o}"))?);
        other_host = Some(o);
        args.drain(i..=i + 1);
    }
    let server: IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(_) => setup::net::resolve(&host).ok_or_else(|| eyre!("can't resolve {host}"))?,
    };
    // --info: take the ports from the server's /api/info, as the launcher
    // does (a server behind a reverse proxy or with remapped ports).
    if let Some(i) = args.iter().position(|a| a == "--info") {
        args.remove(i);
        // Each server's own ports (the second server's too, for federation).
        let both = [Some((server, host.clone())), other.zip(other_host.clone())];
        for (ip, name) in both.into_iter().flatten() {
            let info = setup::server_info::fetch(&name, Duration::from_secs(5)).ok_or_else(|| eyre!("no /api/info from {name}"))?;
            let ports = info.ports.ok_or_else(|| eyre!("{name}'s /api/info has no ports"))?;
            println!("{name}: API {}, login {}, NAT {:?}", ports.api, ports.login, ports.nat);
            testbot::bot::set_target(
                ip,
                testbot::bot::Target {
                    host: name.clone(),
                    api: ports.api,
                    auth: ports.login,
                    nat: ports.nat.ok_or_else(|| eyre!("the NAT helper is off"))?,
                    // HTTPS by name, as the launcher uses it.
                    api_tls: ports.api_tls.filter(|_| name.parse::<IpAddr>().is_err()),
                    identity_required: info.features.iter().any(|f| f == "identity-required"),
                },
            );
        }
    }
    if args.first().map(String::as_str) == Some("load") {
        let options = testbot::load::Options::parse(&args[1..])?;
        return testbot::load::run(server, options).await;
    }
    let names: Vec<&str> = if args.is_empty() {
        SCENARIOS.to_vec()
    } else {
        args.iter().map(String::as_str).collect()
    };
    let mut ctx = Ctx {
        server,
        run: rand::random::<u16>().into(),
        n: 0,
    };
    let mut failed = 0;
    for name in names {
        let limit = match name {
            "federation" => 300,
            "reconnect" => 180,
            _ => 30,
        };
        let result = tokio::time::timeout(Duration::from_secs(limit), async {
            match name {
                "login" => login(&mut ctx).await,
                "lobby-invite" => lobby_invite(&mut ctx).await,
                "private-match-invite" => private_match_invite(&mut ctx).await,
                "cleanup" => cleanup(&mut ctx).await,
                "trusted-subnet" => trusted_subnet(&mut ctx).await,
                "bad-packets" => bad_packets(&mut ctx).await,
                "leave-session" => leave_session(&mut ctx).await,
                "abandon-empty" => abandon_empty(&mut ctx).await,
                "duplicate-request" => duplicate_request(&mut ctx).await,
                "lost-push" => lost_push(&mut ctx).await,
                "nat-probe" => nat_probe_scenario(&mut ctx).await,
                "nat-relay" => nat_relay(&mut ctx).await,
                "relay-ping" => relay_ping(&mut ctx).await,
                "nat-public-address" => nat_public_address(&mut ctx).await,
                "friends" => friends(&mut ctx).await,
                "block" => block(&mut ctx).await,
                "invite-queue" => invite_queue(&mut ctx).await,
                "identity-login" => identity_login(&mut ctx).await,
                "rename" => rename(&mut ctx).await,
                "direct-test" => direct_test(&mut ctx).await,
                "presence" => presence(&mut ctx).await,
                "slow-handshake" => slow_handshake(&mut ctx).await,
                "second-sign-in" => second_sign_in(&mut ctx).await,
                "reconnect" => reconnect(&mut ctx).await,
                "ticket-elsewhere" => ticket_elsewhere(&mut ctx).await,
                "private-room-join" => private_room_join(&mut ctx).await,
                "full-room" => full_room(&mut ctx).await,
                "host-leaves-match" => host_leaves_match(&mut ctx).await,
                "public-match-probes" => public_match_probes(&mut ctx).await,
                "party-probes" => party_probes(&mut ctx).await,
                "party-member-split" => party_member_split(&mut ctx).await,
                "member-invite" => member_invite(&mut ctx).await,
                "join-notice" => join_notice(&mut ctx).await,
                "relay-to-joiner" => relay_to_joiner(&mut ctx).await,
                "search-other-data" => search_other_data(&mut ctx).await,
                "whats-being-played" => whats_being_played(&mut ctx).await,
                "split-limits" => split_limits(&mut ctx).await,
                "private-room-hidden" => private_room_hidden(&mut ctx).await,
                "station-url-schemes" => station_url_schemes(&mut ctx).await,
                "syn-flood" => syn_flood(&mut ctx).await,
                "login-lockout" => login_lockout(&mut ctx).await,
                "outdated-client" => outdated_client(&mut ctx).await,
                "report" => report(&mut ctx).await,
                "client-log" => client_log(&mut ctx).await,
                "name-check" => name_check(&mut ctx).await,
                "session-lifecycle" => session_lifecycle(&mut ctx).await,
                "game-invitations" => game_invitations(&mut ctx).await,
                "failed-joins" => failed_joins(&mut ctx).await,
                "stats" => stats(&mut ctx).await,
                // Not in the default list: a server in the "mutual" mode, one requiring
                // identities, and two servers.
                "friends-mutual" => friends_mutual(&mut ctx).await,
                "identity-required" => identity_required(&mut ctx).await,
                "federation" => match other {
                    Some(other) => federation(&mut ctx, other).await,
                    None => Err(eyre!("federation needs --other <second server>")),
                },
                other => Err(eyre!("unknown scenario {other:?} (known: {})", SCENARIOS.join(", "))),
            }
        })
        .await
        .unwrap_or_else(|_| Err(eyre!("timed out")));
        match result {
            Ok(()) => println!("PASS {name}"),
            Err(e) => {
                failed += 1;
                println!("FAIL {name}: {e:#}");
            }
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
