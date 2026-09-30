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

/// The server's id from its `/api/info` (players sign it into identity links).
fn server_id(server: IpAddr) -> Result<String> {
    setup::server_info::fetch(&server.to_string(), Duration::from_secs(5))
        .and_then(|i| i.id)
        .ok_or_else(|| eyre!("the server's /api/info has no id"))
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
    ensure!(b.friend_change("accept", &a.name.to_uppercase()).await? == Relation::Friend, "accepting didn't make friends");
    ensure!(a.poll_friend_event(Duration::from_secs(3)).await? == Some((Kind::Accepted, b.name.clone())), "no accepted notice");
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
    ensure!(b.try_invite(&a.name).await.is_err(), "the pest could still invite");
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
    ensure!(a.link(&identity::Identity::generate(), "another-server", now).await.is_err(), "a link signed for another server was taken");
    a.link(&me, &id, now).await.map_err(|e| eyre!("linking: {e}"))?;
    let name = a.name.clone();
    a.disconnect().await?;
    // A new PC: sign in with the key, set a new password.
    testbot::bot::key_login(ctx.server, &me, &id, &name, now + 1, "a-new-password-1").await.map_err(|e| eyre!("key login: {e}"))?;
    ensure!(
        testbot::bot::key_login(ctx.server, &me, &id, &name, now + 1, "").await.is_err(),
        "the same signature worked twice"
    );
    ensure!(
        testbot::bot::key_login(ctx.server, &identity::Identity::generate(), &id, &name, now + 2, "stolen-password").await.is_err(),
        "another key signed in"
    );
    ensure!(Bot::login(ctx.server, &name, PASSWORD).await.is_err(), "the old password still works");
    Bot::login(ctx.server, &name, "a-new-password-1").await?.disconnect().await
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
    ensure!(!b.friends().await?.iter().any(|(n, _)| *n == a.name), "a stranger is on the game's friend list");
    ensure!(a.try_invite(&b.name).await.is_err(), "a stranger could invite");
    a.friend_change("request", &b.name).await?;
    b.friend_change("accept", &a.name).await?;
    ensure!(b.friends().await?.iter().any(|(n, _)| *n == a.name), "a friend is missing from the game's friend list");
    a.try_invite(&b.name).await.map_err(|e| eyre!("a friend couldn't invite: {e}"))?;
    ensure!(b.poll_invite(Duration::from_secs(3)).await?.as_deref() == Some(a.name.as_str()), "the invite didn't arrive");
    a.disconnect().await?;
    b.disconnect().await
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
    // Names are reserved across the two servers: someone else can't be Kiwi on the second.
    let impostor = identity::Identity::generate();
    ensure!(
        Bot::register_as(other, &kiwi.name.to_lowercase(), PASSWORD, Some((&impostor, &id_b))).await.is_err(),
        "another identity took a reserved name"
    );
    ensure!(Bot::register(other, &kiwi.name, PASSWORD).await.is_err(), "an account without an identity took a reserved name");
    // Kiwi can take their own name there (renaming their account on the second server).
    kiwi_b.rename(&kiwi.name, Some((&kiwi_key, &id_b))).await.map_err(|e| eyre!("Kiwi couldn't take their own name: {e}"))?;
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
    let push = b.wait_notification(Duration::from_secs(3)).await?.ok_or_else(|| eyre!("no 'come in' push for the private match"))?;
    ensure!(push.ui_type == 7003, "push type {} (want 7003)", push.ui_type);
    ensure!(push.ui_param_1 == b.pid, "push names player {} (want {})", push.ui_param_1, b.pid);
    ensure!(push.ui_param_2 == game, "push names session {} (want {game})", push.ui_param_2);
    a.disconnect().await?;
    b.disconnect().await
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
    ensure!(!has_session(&b.search_with_participants(&[host]).await?, lobby), "the lobby of a host who quit is still found");
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
    ensure!(has_session(&a.search_with_participants(&[guest]).await?, lobby), "the guest isn't in the lobby after joining");
    b.leave_session(lobby).await?;
    ensure!(!has_session(&a.search_with_participants(&[guest]).await?, lobby), "the guest is still in the lobby after leaving");
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
    ensure!(!has_session(&b.search_with_participants(&[a.pid]).await?, lobby), "an abandoned, empty lobby is still found");
    ensure!(!b.search_sessions("113 => 1;103 => 0").await?.iter().any(|(s, _)| *s == lobby), "an abandoned, empty lobby is still offered by matchmaking");
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
    let mine: Vec<u32> = b.search_sessions("113 => 1;103 => 0").await?.into_iter().filter(|(_, host)| *host == a.pid).map(|(s, _)| s).collect();
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
    let push = b.wait_notification(Duration::from_secs(5)).await?.ok_or_else(|| eyre!("the lost push was never sent again"))?;
    ensure!(push.ui_type == 7003 && push.ui_param_2 == game, "wrong push: {push:?}");
    a.disconnect().await?;
    b.disconnect().await
}

/// Sends `msg` from `sock` to the NAT helper's port `port` and waits for a
/// NAT message back.
fn nat_ask(sock: &UdpSocket, server: IpAddr, port: u16, msg: &nat_proto::Message) -> Result<nat_proto::Message> {
    sock.send_to(&msg.encode(), (server, port))?;
    nat_wait(sock)
}

fn nat_wait(sock: &UdpSocket) -> Result<nat_proto::Message> {
    sock.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut buf = [0u8; 2048];
    let (n, _) = sock.recv_from(&mut buf)?;
    nat_proto::Message::decode(&buf[..n]).ok_or_else(|| eyre!("not a NAT message: {:x?}", &buf[..n]))
}

fn nat_probe(sock: &UdpSocket, server: IpAddr, flags: u8, name: &str) -> Result<(std::net::SocketAddrV4, std::net::SocketAddrV4, bool)> {
    let probe = nat_proto::Message::Probe {
        flags,
        nonce: 7,
        mapping: None,
        name: name.into(),
    };
    match nat_ask(sock, server, testbot::bot::target(server).nat, &probe)? {
        nat_proto::Message::ProbeReply {
            nonce: 7,
            observed,
            advertise,
            flags,
            ..
        } => Ok((observed, advertise, flags & nat_proto::reply_flags::RELAYED != 0)),
        other => Err(eyre!("unexpected answer {other:?}")),
    }
}

/// The NAT helper tells a player the address it sees, on both of its ports.
async fn nat_probe_scenario(ctx: &mut Ctx) -> Result<()> {
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let local = sock.local_addr()?;
    let (observed, advertise, relayed) = nat_probe(&sock, ctx.server, 0, "prober")?;
    ensure!(observed.port() == local.port(), "observed {observed}, but the socket is {local}");
    ensure!(advertise == observed && !relayed, "a local player is advertised as {advertise} (relayed: {relayed})");
    let second = nat_proto::Message::Probe {
        flags: nat_proto::probe_flags::SECOND_PORT,
        nonce: 8,
        mapping: None,
        name: String::new(),
    };
    match nat_ask(&sock, ctx.server, testbot::bot::target(ctx.server).nat + 1, &second)? {
        nat_proto::Message::ProbeReply { observed: o2, .. } => ensure!(o2 == observed, "the second port saw {o2}, the first {observed}"),
        other => return Err(eyre!("unexpected answer {other:?}")),
    }
    Ok(())
}

/// Two players, one relayed: packets reach each other through the relay,
/// each seeing the other at its advertised address; strangers can't use it.
async fn nat_relay(ctx: &mut Ctx) -> Result<()> {
    use nat_proto::Message;
    let a = UdpSocket::bind("0.0.0.0:0")?;
    let b = UdpSocket::bind("0.0.0.0:0")?;
    let stranger = UdpSocket::bind("0.0.0.0:0")?;
    let run = ctx.run;
    let (_, a_adv, a_relayed) = nat_probe(&a, ctx.server, nat_proto::probe_flags::WANT_RELAY, &format!("relayed{run}"))?;
    let (_, b_adv, b_relayed) = nat_probe(&b, ctx.server, 0, &format!("direct{run}"))?;
    ensure!(a_relayed && !b_relayed, "relayed: a {a_relayed}, b {b_relayed}");
    ensure!(a_adv.port() >= 40000, "a relay address was expected, got {a_adv}");

    let to = |sock: &UdpSocket, to, payload: &[u8]| -> Result<()> {
        sock.send_to(&Message::DataTo { to, payload: payload.to_vec() }.encode(), (ctx.server, testbot::bot::target(ctx.server).nat))?;
        Ok(())
    };
    to(&b, a_adv, b"to the relayed player")?;
    ensure!(
        nat_wait(&a)? == Message::DataFrom { from: b_adv, payload: b"to the relayed player".to_vec() },
        "the relayed player got something else"
    );
    to(&a, b_adv, b"and back")?;
    ensure!(
        nat_wait(&b)? == Message::DataFrom { from: a_adv, payload: b"and back".to_vec() },
        "the direct player got something else"
    );
    to(&stranger, a_adv, b"spam")?;
    a.set_read_timeout(Some(Duration::from_millis(300)))?;
    let mut buf = [0u8; 64];
    ensure!(a.recv_from(&mut buf).is_err(), "a stranger's packet was relayed");
    Ok(())
}

/// A game that still registers its local address gets the public one the
/// NAT helper found (with the local one kept for players on its network).
async fn nat_public_address(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.player("Host").await?;
    let mut b = ctx.player("Guest").await?;
    let storm = UdpSocket::bind("0.0.0.0:0")?;
    let (_, advertise, _) = nat_probe(&storm, ctx.server, 0, &a.name)?;
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

const SCENARIOS: &[&str] = &[
    "login",
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
    "nat-public-address",
    "friends",
    "block",
    "invite-queue",
    "identity-login",
    "rename",
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
    if let Some(i) = args.iter().position(|a| a == "--other") {
        let o = args.get(i + 1).ok_or_else(|| eyre!("--other needs an address"))?.clone();
        other = Some(o.parse().ok().or_else(|| setup::net::resolve(&o)).ok_or_else(|| eyre!("can't resolve {o}"))?);
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
        let info = setup::server_info::fetch(&host, Duration::from_secs(5)).ok_or_else(|| eyre!("no /api/info from {host}"))?;
        let ports = info.ports.ok_or_else(|| eyre!("{host}'s /api/info has no ports"))?;
        println!("{host}: API {}, login {}, NAT {:?}", ports.api, ports.login, ports.nat);
        testbot::bot::set_target(testbot::bot::Target {
            host: host.clone(),
            api: ports.api,
            auth: ports.login,
            nat: ports.nat.ok_or_else(|| eyre!("the NAT helper is off"))?,
        });
    }
    if args.first().map(String::as_str) == Some("load") {
        let options = testbot::load::Options::parse(&args[1..])?;
        return testbot::load::run(server, options).await;
    }
    let names: Vec<&str> = if args.is_empty() { SCENARIOS.to_vec() } else { args.iter().map(String::as_str).collect() };
    let mut ctx = Ctx { server, run: rand::random::<u16>().into(), n: 0 };
    let mut failed = 0;
    for name in names {
        let limit = if name == "federation" { 150 } else { 30 };
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
                "nat-public-address" => nat_public_address(&mut ctx).await,
                "friends" => friends(&mut ctx).await,
                "block" => block(&mut ctx).await,
                "invite-queue" => invite_queue(&mut ctx).await,
                "identity-login" => identity_login(&mut ctx).await,
                "rename" => rename(&mut ctx).await,
                // Not in the default list: a server in the "mutual" mode, and two servers.
                "friends-mutual" => friends_mutual(&mut ctx).await,
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
