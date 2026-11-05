//! Match traffic captures (the 0.5.0 plan's replays, way 3: research towards replaying a
//! whole match in the game itself). While a match is recorded (recording.rs), every game
//! packet on Storm's socket goes to `<recording>.fen` beside it: what this game sent to each
//! other player and what it got from them, as the game itself sent and read them (the
//! relay's wrapping taken off). Each game owns its player's objects (the pawn, its weapons;
//! Storm replicates them from their owner), so the packets a player's game sent are the
//! truth about that player, and every player's together are the whole match.
//!
//! The last [`BEFORE`] before a recording begins are kept too (the match's start: the
//! level's objects and the players' pawns being created). Other players are numbered per
//! capture (the header has no addresses); the packets themselves are kept as they were, so a
//! capture stays on this PC. On for every player (all are its testers), `NetCapture = false`
//! turns it off; the newest [`KEPT`] are kept.
//!
//! The file: gzip around `FENC`, u16 version 1, u64 Unix milliseconds of the first packet,
//! the recorder's name (u8 length, bytes), then each packet: u32 milliseconds since the
//! first, u8 flags (1 sent by this game, 2 relayed), u8 the other player's number, u16
//! length, the bytes.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::Write as _;
use std::net::SocketAddrV4;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use tracing::info;
use tracing::warn;

/// Packets kept from before a recording begins.
const BEFORE: Duration = Duration::from_secs(30);
/// The most kept in memory before one (a busy match is ~200 KB a second).
const BEFORE_BYTES: usize = 16 * 1024 * 1024;
/// Captures kept in the folder.
const KEPT: usize = 10;
/// Packets queued for the writer at most; past that they're dropped (and counted).
const QUEUE: usize = 20_000;

static ON: AtomicBool = AtomicBool::new(false);

struct Packet {
    at: Instant,
    when: SystemTime,
    sent: bool,
    relayed: bool,
    peer: SocketAddrV4,
    data: Vec<u8>,
}

#[derive(Default)]
struct State {
    /// The last [`BEFORE`] of packets, while no capture is open.
    before: VecDeque<Packet>,
    before_bytes: usize,
    writer: Option<mpsc::SyncSender<Packet>>,
    dropped: u64,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    static STATE: std::sync::LazyLock<Mutex<State>> = std::sync::LazyLock::new(Mutex::default);
    STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Turned on at start when `NetCapture` is (the default).
pub fn enable() {
    ON.store(true, Ordering::Relaxed);
}

/// A game packet on Storm's socket: sent to `peer`, or received from it (`relayed`: through
/// the relay, unwrapped). Called from the socket hooks: quick, never waits on the disk.
pub fn packet(sent: bool, relayed: bool, peer: SocketAddrV4, data: &[u8]) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let p = Packet {
        at: Instant::now(),
        when: SystemTime::now(),
        sent,
        relayed,
        peer,
        data: data.to_vec(),
    };
    let mut st = state();
    if let Some(w) = &st.writer {
        if w.try_send(p).is_err() {
            st.dropped += 1;
        }
        return;
    }
    st.before_bytes += p.data.len();
    st.before.push_back(p);
    while st.before.front().is_some_and(|f| f.at.elapsed() > BEFORE) || st.before_bytes > BEFORE_BYTES {
        let Some(f) = st.before.pop_front() else { break };
        st.before_bytes -= f.data.len();
    }
}

/// A recording began at `recording` (its .fer): the capture goes beside it, with the last
/// [`BEFORE`] first.
pub fn begin(recording: &Path, recorder: &str) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let path = recording.with_extension("fen");
    prune(recording.parent().unwrap_or(Path::new(".")));
    let file = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            warn!("Capture: couldn't make {} ({e})", path.display());
            return;
        }
    };
    let (tx, rx) = mpsc::sync_channel::<Packet>(QUEUE);
    let earlier: Vec<Packet> = {
        let mut st = state();
        st.before_bytes = 0;
        st.dropped = 0;
        st.writer = Some(tx);
        st.before.drain(..).collect()
    };
    let recorder = recorder.to_string();
    let spawned = std::thread::Builder::new()
        .name("fe-net-capture".into())
        .spawn(move || write(file, &path, &recorder, earlier, &rx));
    if let Err(e) = spawned {
        warn!("Capture: couldn't start ({e})");
        state().writer = None;
    }
}

/// The recording ended: the capture is finished (its writer drains what's queued).
pub fn end() {
    let dropped = {
        let mut st = state();
        st.writer = None;
        st.dropped
    };
    if dropped > 0 {
        warn!("Capture: {dropped} packets dropped (the disk didn't keep up)");
    }
}

fn write(file: std::fs::File, path: &Path, recorder: &str, earlier: Vec<Packet>, rx: &mpsc::Receiver<Packet>) {
    let mut z = flate2::write::GzEncoder::new(std::io::BufWriter::new(file), flate2::Compression::fast());
    let mut peers: HashMap<SocketAddrV4, u8> = HashMap::new();
    let first = earlier.first().map(|p| (p.at, p.when));
    let mut start: Option<(Instant, SystemTime)> = first;
    let (mut count, mut bytes, mut flushed) = (0u64, 0u64, Instant::now());
    let mut header_written = false;
    let mut put = |z: &mut flate2::write::GzEncoder<_>, p: &Packet| -> std::io::Result<()> {
        let (t0, w0) = *start.get_or_insert((p.at, p.when));
        if !header_written {
            header_written = true;
            z.write_all(b"FENC")?;
            z.write_all(&1u16.to_le_bytes())?;
            let ms = w0.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            z.write_all(&ms.to_le_bytes())?;
            let name = &recorder.as_bytes()[..recorder.len().min(255)];
            z.write_all(&[name.len() as u8])?;
            z.write_all(name)?;
        }
        let next = u8::try_from(peers.len()).unwrap_or(u8::MAX);
        let peer = *peers.entry(p.peer).or_insert(next);
        let t = u32::try_from(p.at.saturating_duration_since(t0).as_millis()).unwrap_or(u32::MAX);
        let len = u16::try_from(p.data.len()).unwrap_or(u16::MAX);
        z.write_all(&t.to_le_bytes())?;
        z.write_all(&[u8::from(p.sent) | (u8::from(p.relayed) << 1), peer])?;
        z.write_all(&len.to_le_bytes())?;
        z.write_all(&p.data[..usize::from(len)])?;
        count += 1;
        bytes += u64::from(len);
        Ok(())
    };
    let result = (|| -> std::io::Result<()> {
        for p in &earlier {
            put(&mut z, p)?;
        }
        while let Ok(p) = rx.recv() {
            put(&mut z, &p)?;
            if flushed.elapsed() >= Duration::from_secs(5) {
                flushed = Instant::now();
                z.flush()?;
            }
        }
        z.finish()?.flush()
    })();
    match result {
        Ok(()) => info!("Capture: {count} packets ({} KB) with {} other players -> {}", bytes / 1024, peers.len(), path.display()),
        Err(e) => warn!("Capture: writing {} failed ({e})", path.display()),
    }
}

/// The oldest captures beyond the newest [`KEPT`] - 1 deleted.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "fen"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort();
    let over = files.len().saturating_sub(KEPT - 1);
    for (_, p) in files.into_iter().take(over) {
        let _ = std::fs::remove_file(p);
    }
}
