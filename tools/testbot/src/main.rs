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
];

#[tokio::main]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut server: IpAddr = "127.0.0.1".parse()?;
    if let Some(i) = args.iter().position(|a| a == "--server") {
        server = args.get(i + 1).ok_or_else(|| eyre!("--server needs an address"))?.parse()?;
        args.drain(i..=i + 1);
    }
    let names: Vec<&str> = if args.is_empty() { SCENARIOS.to_vec() } else { args.iter().map(String::as_str).collect() };
    let mut ctx = Ctx { server, run: rand::random::<u16>().into(), n: 0 };
    let mut failed = 0;
    for name in names {
        let result = tokio::time::timeout(Duration::from_secs(30), async {
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
