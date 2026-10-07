//! Load test: many players signed in at once, doing what players do, while
//! simulated matches send game traffic through the server's relay.
//!
//! ```text
//! testbot [--server HOST] [--info] load [--players 100] [--relayed 20]
//!     [--match-size 4] [--pps 30] [--bytes 200] [--duration 60]
//!     [--connect 32] [--activity 10]
//! ```
//!
//! - Every player signs in the way the game does (an API account, the game
//!   login, the game service), `--connect` at a time.
//! - Each then does something every `--activity` seconds on average: the
//!   friend list (the overlay polls it), a friend search (the game's
//!   "find teammate"), or a lobby search; a quarter host a lobby.
//! - The players form matches of `--match-size`. `--relayed` percent of
//!   them go through the relay, and each sends `--pps` packets of `--bytes`
//!   to every other player in its match. Only traffic that touches the relay
//!   is sent (players who connect directly never reach the server). Its
//!   delivery, loss and latency are measured.
//!
//! Reports every 5 seconds and a summary at the end. The server's own CPU
//! and memory are sampled by `scripts/load-test.sh`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::net::SocketAddrV4;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use eyre::eyre;
use eyre::Result;
use nat_proto::Message;
use tokio::net::UdpSocket;

use crate::bot::Bot;
use crate::bot::LOBBY;

#[derive(Debug, Clone)]
pub struct Options {
    pub players: usize,
    /// Percent of players who go through the relay.
    pub relayed: u32,
    pub match_size: usize,
    /// Packets per second from each player to each other player in its match.
    pub pps: u32,
    pub bytes: usize,
    pub duration: Duration,
    /// Sign-ins at a time.
    pub connect: usize,
    /// Average seconds between a player's actions.
    pub activity: f64,
    /// Names for the players, in order (`--names Kiwi,Fisher`), e.g. for
    /// screenshots; the rest are named Load<run>_<n>.
    pub names: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            players: 100,
            relayed: 20,
            match_size: 4,
            pps: 30,
            bytes: 200,
            duration: Duration::from_secs(60),
            connect: 32,
            activity: 10.0,
            names: Vec::new(),
        }
    }
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut o = Self::default();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let value = it.next().ok_or_else(|| eyre!("{flag} needs a value"))?;
            let n = || value.parse::<u64>().map_err(|_| eyre!("{flag}: {value:?} isn't a number"));
            match flag.as_str() {
                "--players" => o.players = n()? as usize,
                "--relayed" => o.relayed = n()?.min(100) as u32,
                "--match-size" => o.match_size = n()?.max(2) as usize,
                "--pps" => o.pps = n()?.max(1) as u32,
                "--bytes" => o.bytes = (n()? as usize).clamp(16, nat_proto::MAX_PAYLOAD),
                "--duration" => o.duration = Duration::from_secs(n()?),
                "--connect" => o.connect = n()?.max(1) as usize,
                "--activity" => o.activity = value.parse().map_err(|_| eyre!("--activity: {value:?} isn't a number"))?,
                "--names" => o.names = value.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect(),
                other => return Err(eyre!("unknown load option {other}")),
            }
        }
        Ok(o)
    }
}

/// Latencies (microseconds) and failures of one kind of request.
#[derive(Default)]
struct Timings {
    samples: Mutex<Vec<u32>>,
    errors: AtomicU64,
}

impl Timings {
    fn record(&self, started: Instant, ok: bool) {
        if ok {
            if let Ok(mut s) = self.samples.lock() {
                s.push(started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32);
            }
        } else {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// (count, p50 ms, p99 ms, max ms, errors), and clears the samples.
    fn take(&self) -> (usize, f64, f64, f64, u64) {
        let mut s = self.samples.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default();
        let errors = self.errors.swap(0, Ordering::Relaxed);
        if s.is_empty() {
            return (0, 0.0, 0.0, 0.0, errors);
        }
        s.sort_unstable();
        let at = |p: f64| f64::from(s[((s.len() - 1) as f64 * p) as usize]) / 1000.0;
        (s.len(), at(0.5), at(0.99), f64::from(*s.last().unwrap()) / 1000.0, errors)
    }
}

#[derive(Default)]
struct Stats {
    sign_in: Timings,
    friends: Timings,
    friend_search: Timings,
    lobby_search: Timings,
    relay_latency: Timings,
    sent: AtomicU64,
    received: AtomicU64,
    bytes_sent: AtomicU64,
}

/// One match player's game socket.
struct Peer {
    socket: Arc<UdpSocket>,
    advertise: SocketAddrV4,
    relayed: bool,
    name: String,
    ticket: nat_proto::Ticket,
    registration: crate::bot::NatRegistration,
}

const MAGIC: u32 = 0x5e10_ad00;

fn packet(size: usize, seq: u32, epoch: Instant) -> Vec<u8> {
    let mut p = vec![0u8; size];
    p[..4].copy_from_slice(&MAGIC.to_be_bytes());
    p[4..8].copy_from_slice(&seq.to_be_bytes());
    p[8..16].copy_from_slice(&(epoch.elapsed().as_micros() as u64).to_be_bytes());
    p
}

pub async fn run(server: IpAddr, o: Options) -> Result<()> {
    let target = crate::bot::target(server);
    let nat = SocketAddrV4::new(
        match server {
            IpAddr::V4(v4) => v4,
            IpAddr::V6(_) => return Err(eyre!("the load test needs an IPv4 server")),
        },
        target.nat,
    );
    let stats = Arc::new(Stats::default());
    let run_id: u16 = rand::random();
    println!(
        "load: {} players, {}% relayed, matches of {}, {} packets/s of {} bytes per link, {} s",
        o.players,
        o.relayed,
        o.match_size,
        o.pps,
        o.bytes,
        o.duration.as_secs()
    );

    // --- Sign everyone in.
    let started = Instant::now();
    let gate = Arc::new(tokio::sync::Semaphore::new(o.connect));
    let mut joins = Vec::new();
    for i in 0..o.players {
        let (gate, stats) = (Arc::clone(&gate), Arc::clone(&stats));
        let name = o.names.get(i).cloned().unwrap_or_else(|| format!("Load{run_id}_{i}"));
        joins.push(tokio::spawn(async move {
            let _permit = gate.acquire().await.ok()?;
            let t = Instant::now();
            let result = async {
                Bot::register(server, &name, "load-test-password").await?;
                Bot::login(server, &name, "load-test-password").await
            }
            .await;
            stats.sign_in.record(t, result.is_ok());
            if let Err(e) = &result {
                if stats.sign_in.errors.load(Ordering::Relaxed) <= 3 {
                    eprintln!("sign-in {name}: {e:#}");
                }
            }
            result.ok()
        }));
    }
    let mut bots = Vec::new();
    for j in joins {
        if let Ok(Some(bot)) = j.await {
            bots.push(bot);
        }
    }
    let (n, p50, p99, max, errors) = stats.sign_in.take();
    println!(
        "signed in {n} of {} in {:.1} s ({:.0}/s): p50 {p50:.0} ms, p99 {p99:.0} ms, max {max:.0} ms, {errors} failed",
        o.players,
        started.elapsed().as_secs_f64(),
        n as f64 / started.elapsed().as_secs_f64()
    );
    if bots.is_empty() {
        return Err(eyre!("nobody could sign in"));
    }
    let pids: Arc<Vec<u32>> = Arc::new(bots.iter().map(|b| b.pid).collect());

    // --- Matches: a room per match, its players in it (the relay carries traffic only
    // between players the server put together), and game sockets, probed like the hook does.
    let mut unjoined = 0usize;
    for group in bots.chunks_mut(o.match_size) {
        let Some((host, guests)) = group.split_first_mut() else { continue };
        let room = match host.create_session(LOBBY).await {
            Ok(room) => room,
            Err(_) => {
                unjoined += 1 + guests.len();
                continue;
            }
        };
        let pid = host.pid;
        if host.add_participants(room, &[pid], &[]).await.is_err() {
            unjoined += 1;
        }
        for guest in guests {
            let pid = guest.pid;
            if guest.add_participants(room, &[pid], &[]).await.is_err() {
                unjoined += 1;
            }
        }
    }
    if unjoined > 0 {
        println!("matches: {unjoined} players couldn't join their match's room (their relayed traffic is dropped)");
    }
    let epoch = Instant::now();
    let mut peers = Vec::new();
    for bot in &bots {
        let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
        let relay = rand::random::<u32>() % 100 < o.relayed;
        let flags = if relay { nat_proto::probe_flags::WANT_RELAY } else { 0 };
        let r = crate::bot::nat_register(&socket, nat, flags, &bot.name, bot.nat_ticket).await?;
        peers.push(Peer {
            socket,
            advertise: r.advertise,
            relayed: r.relayed,
            name: bot.name.clone(),
            ticket: bot.nat_ticket,
            registration: r,
        });
    }
    let relayed_players = peers.iter().filter(|p| p.relayed).count();
    let mut links = 0usize;
    let deadline = Instant::now() + o.duration;
    let mut tasks = Vec::new();

    for group in peers.chunks(o.match_size) {
        let addresses: Vec<(SocketAddrV4, bool)> = group.iter().map(|p| (p.advertise, p.relayed)).collect();
        for (me, peer) in group.iter().enumerate() {
            // Receiver: relayed packets (DataFrom) with our timestamp.
            let (socket, stats2) = (Arc::clone(&peer.socket), Arc::clone(&stats));
            let (name, ticket, flags) = (peer.name.clone(), peer.ticket, if peer.relayed { nat_proto::probe_flags::WANT_RELAY } else { 0 });
            tasks.push(tokio::spawn(async move {
                let mut buf = vec![0u8; 2048];
                while Instant::now() < deadline + Duration::from_secs(2) {
                    let Ok(Ok((n, _))) = tokio::time::timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await else {
                        continue;
                    };
                    // A keepalive whose cookie ran out: come back with the new one, as the hook does.
                    if let Some(Message::ProbeReply { cookie, tag, nonce, .. }) = Message::decode(&buf[..n]) {
                        if tag == [0; 8] && cookie != [0; 16] {
                            let again = Message::Probe {
                                flags,
                                nonce,
                                mapping: None,
                                name: name.clone(),
                                ticket,
                                cookie,
                                rtt_ms: None,
                            };
                            let _ = socket.send_to(&again.encode(), nat).await;
                        }
                        continue;
                    }
                    let Some((_, _, offset)) = nat_proto::data_from(&buf[..n]) else { continue };
                    let p = &buf[offset..n];
                    if p.len() >= 16 && p[..4] == MAGIC.to_be_bytes() {
                        stats2.received.fetch_add(1, Ordering::Relaxed);
                        let sent_us = u64::from_be_bytes(p[8..16].try_into().unwrap_or_default());
                        let lat = (epoch.elapsed().as_micros() as u64).saturating_sub(sent_us);
                        if let Ok(mut s) = stats2.relay_latency.samples.lock() {
                            s.push(lat.min(u64::from(u32::MAX)) as u32);
                        }
                    }
                }
            }));
            // Sender: to every other player, when either side is relayed.
            let targets: Vec<SocketAddrV4> = addresses
                .iter()
                .enumerate()
                .filter(|(other, (_, relayed))| *other != me && (peer.relayed || *relayed))
                .map(|(_, (a, _))| *a)
                .collect();
            if targets.is_empty() {
                continue;
            }
            links += targets.len();
            let (socket, stats2, o2, name, relayed) = (Arc::clone(&peer.socket), Arc::clone(&stats), o.clone(), peer.name.clone(), peer.relayed);
            let (ticket, reg) = (peer.ticket, peer.registration);
            tasks.push(tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(o2.pps)));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                let mut seq = 0u32;
                let mut wrapped = Vec::with_capacity(2048);
                let mut last_probe = Instant::now();
                while Instant::now() < deadline {
                    tick.tick().await;
                    for to in &targets {
                        seq = seq.wrapping_add(1);
                        nat_proto::encode_data_to(&mut wrapped, reg.tag, *to, &packet(o2.bytes, seq, epoch));
                        if socket.send_to(&wrapped, nat).await.is_ok() {
                            stats2.sent.fetch_add(1, Ordering::Relaxed);
                            stats2.bytes_sent.fetch_add(wrapped.len() as u64, Ordering::Relaxed);
                        }
                    }
                    // The hook's keepalive.
                    if last_probe.elapsed() > Duration::from_secs(20) {
                        last_probe = Instant::now();
                        let msg = Message::Probe {
                            flags: if relayed { nat_proto::probe_flags::WANT_RELAY } else { 0 },
                            nonce: seq | 1,
                            mapping: None,
                            name: name.clone(),
                            ticket,
                            cookie: reg.cookie,
                            rtt_ms: None,
                        };
                        let _ = socket.send_to(&msg.encode(), nat).await;
                    }
                }
            }));
        }
    }
    println!(
        "matches: {} games, {relayed_players} players relayed, {links} relayed links, {:.0} packets/s ({:.2} MB/s) into the relay expected",
        peers.len().div_ceil(o.match_size),
        (links as f64) * f64::from(o.pps),
        (links as f64) * f64::from(o.pps) * (o.bytes + nat_proto::DATA_OVERHEAD) as f64 / 1e6
    );

    // --- Everyone keeps doing things.
    for (i, mut bot) in bots.into_iter().enumerate() {
        let (stats, pids, o) = (Arc::clone(&stats), Arc::clone(&pids), o.clone());
        tasks.push(tokio::spawn(async move {
            if i % 4 == 0 {
                let _ = bot.create_session(LOBBY).await;
            }
            while Instant::now() < deadline {
                let wait = rand::random::<f64>() * 2.0 * o.activity;
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                if Instant::now() >= deadline {
                    break;
                }
                let t = Instant::now();
                match rand::random::<u32>() % 3 {
                    0 => {
                        let ok = bot.friends().await.is_ok();
                        stats.friends.record(t, ok);
                    }
                    1 => {
                        let friends: Vec<u32> = (0..3).map(|_| pids[rand::random::<u32>() as usize % pids.len()]).collect();
                        let ok = bot.search_with_participants(&friends).await.is_ok();
                        stats.friend_search.record(t, ok);
                    }
                    _ => {
                        let ok = bot.search_sessions(LOBBY).await.is_ok();
                        stats.lobby_search.record(t, ok);
                    }
                }
            }
            let _ = bot.disconnect().await;
        }));
    }

    // --- Report.
    let mut totals: HashMap<&str, (usize, u64)> = HashMap::new();
    let (mut total_sent, mut total_received) = (0u64, 0u64);
    let mut lat_all: Vec<u32> = Vec::new();
    let report_every = Duration::from_secs(5);
    let mut last = Instant::now();
    loop {
        tokio::time::sleep(report_every.min(deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1500))).await;
        let done = Instant::now() >= deadline + Duration::from_secs(1);
        let secs = last.elapsed().as_secs_f64();
        last = Instant::now();
        let sent = stats.sent.swap(0, Ordering::Relaxed);
        let received = stats.received.swap(0, Ordering::Relaxed);
        let bytes = stats.bytes_sent.swap(0, Ordering::Relaxed);
        total_sent += sent;
        total_received += received;
        let lat_samples = stats.relay_latency.samples.lock().map(|s| s.clone()).unwrap_or_default();
        lat_all.extend(&lat_samples);
        let (_, lp50, lp99, _, _) = stats.relay_latency.take();
        let mut line = format!(
            "t+{:>3}s relay {:>6.0} pkt/s in ({:.2} MB/s), {:>6.0} out, latency p50 {lp50:.1} ms p99 {lp99:.1} ms",
            epoch.elapsed().as_secs(),
            sent as f64 / secs,
            bytes as f64 / secs / 1e6,
            received as f64 / secs
        );
        for (label, t) in [("friends", &stats.friends), ("friend search", &stats.friend_search), ("lobby search", &stats.lobby_search)] {
            let (n, p50, p99, _, errors) = t.take();
            let e = totals.entry(label).or_default();
            e.0 += n;
            e.1 += errors;
            if n > 0 || errors > 0 {
                line.push_str(&format!(
                    " | {label} {n} p50 {p50:.0} p99 {p99:.0} ms{}",
                    if errors > 0 { format!(" {errors} err") } else { String::new() }
                ));
            }
        }
        println!("{line}");
        if done {
            break;
        }
    }
    for t in tasks {
        let _ = t.await;
    }
    total_received += stats.received.swap(0, Ordering::Relaxed);

    lat_all.sort_unstable();
    let at = |p: f64| lat_all.get(((lat_all.len().max(1) - 1) as f64 * p) as usize).map_or(0.0, |v| f64::from(*v) / 1000.0);
    let loss = if total_sent == 0 {
        0.0
    } else {
        100.0 * (1.0 - total_received as f64 / total_sent as f64)
    };
    println!("--- summary");
    println!(
        "relay: {total_sent} packets sent, {total_received} delivered ({loss:.2}% lost), latency p50 {:.1} ms, p99 {:.1} ms, max {:.1} ms",
        at(0.5),
        at(0.99),
        at(1.0)
    );
    for label in ["friends", "friend search", "lobby search"] {
        let (n, errors) = totals.get(label).copied().unwrap_or_default();
        println!("{label}: {n} done, {errors} failed");
    }
    Ok(())
}
