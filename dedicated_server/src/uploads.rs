//! Content the game uploads (UserStorage), received over HTTP on the content server: today
//! only its ShadowNet companion snapshot (type 0x80000003: the player's loadouts, purchases
//! and challenge progress as JSON, ~135 KB, at most hourly), which the game never reads
//! back. Kept per player, latest only, for the admins (admin-only for now).
//!
//! The game asks (`SaveContentAndGetUploadInfo`) and gets a one-time address
//! `/ugc/<pending id>-<secret>` on the content server; it PUTs the bytes there (raw, its
//! Content-Length the size it asked for, a 200 for success), then says how it went
//! (`UploadEnd`). Found in the game's code (2026-10-07): it joins protocol, host and path
//! as they are, sends each header line verbatim, and takes only status 200 as success.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// The ShadowNet companion snapshot.
pub const SHADOWNET: u32 = 0x8000_0003;
/// The most an upload may be (the snapshot is ~135 KB).
pub const MAX_UPLOAD: usize = 512 * 1024;
/// Received bodies waiting for their UploadEnd, at most, in all.
const MAX_HELD: usize = 64 * 1024 * 1024;
/// An upload not finished by then is let go.
const LASTS: Duration = Duration::from_secs(600);
/// Uploads waiting at most (everyone's).
const MAX_PENDING: usize = 1000;
/// A player's next upload at the soonest (the game sends one an hour).
const EVERY: Duration = Duration::from_secs(300);

struct Pending {
    user: u32,
    size: usize,
    secret: String,
    at: Instant,
    body: Option<Vec<u8>>,
}

#[derive(Default)]
struct State {
    pending: HashMap<u64, Pending>,
    last: HashMap<u32, Instant>,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    static STATE: std::sync::LazyLock<Mutex<State>> = std::sync::LazyLock::new(Mutex::default);
    STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Why an upload isn't taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// A type kept nowhere.
    Type,
    Size,
    TooSoon,
    Busy,
}

/// `user`'s game asks to upload `size` bytes of `type_id`: the pending id and the secret of
/// its one-time address.
pub fn begin(user: u32, type_id: u32, size: u32) -> Result<(u64, String), Refused> {
    if type_id != SHADOWNET {
        return Err(Refused::Type);
    }
    let size = size as usize;
    if size == 0 || size > MAX_UPLOAD {
        return Err(Refused::Size);
    }
    let mut st = state();
    st.pending.retain(|_, p| p.at.elapsed() < LASTS);
    st.last.retain(|_, at| at.elapsed() < EVERY);
    if st.last.contains_key(&user) {
        return Err(Refused::TooSoon);
    }
    if st.pending.len() >= MAX_PENDING {
        return Err(Refused::Busy);
    }
    let id = rand::random::<u64>() | 1;
    let secret: String = (0..16).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
    st.pending.insert(
        id,
        Pending {
            user,
            size,
            secret: secret.clone(),
            at: Instant::now(),
            body: None,
        },
    );
    st.last.insert(user, Instant::now());
    Ok((id, secret))
}

/// The address's path for an upload.
pub fn path(id: u64, secret: &str) -> String {
    format!("/ugc/{id}-{secret}")
}

fn parse_path(path: &str) -> Option<(u64, &str)> {
    let (id, secret) = path.strip_prefix("/ugc/")?.split_once('-')?;
    Some((id.parse().ok()?, secret))
}

/// Before the body of a PUT to `path` is read: whether it's an upload waiting for exactly
/// `length` bytes, and there's room to hold them. Anything else is answered at once
/// (`Err(status)`), so a stray PUT holds no buffer and no connection for long.
pub fn expects(path: &str, length: usize) -> Result<(), &'static str> {
    let Some((id, secret)) = parse_path(path) else { return Err("404 Not Found") };
    let mut st = state();
    st.pending.retain(|_, p| p.at.elapsed() < LASTS);
    let held: usize = st.pending.values().filter_map(|p| p.body.as_ref().map(Vec::len)).sum();
    match st.pending.get(&id) {
        Some(p) if p.secret == secret && p.body.is_none() => {
            if length != p.size {
                Err("400 Bad Request")
            } else if held + length > MAX_HELD {
                Err("503 Service Unavailable")
            } else {
                Ok(())
            }
        }
        _ => Err("404 Not Found"),
    }
}

/// The content server got a PUT to `path`: the HTTP status to answer.
pub fn receive(path: &str, body: Vec<u8>) -> &'static str {
    let Some((id, secret)) = parse_path(path) else { return "404 Not Found" };
    // The snapshot is a JSON object: anything else isn't what was asked for (checked before
    // the lock: up to half a megabyte to parse).
    let json = serde_json::from_slice::<serde_json::Value>(&body).is_ok_and(|v| v.is_object());
    let mut st = state();
    st.pending.retain(|_, p| p.at.elapsed() < LASTS);
    let Some(p) = st.pending.get_mut(&id).filter(|p| p.secret == secret && p.body.is_none()) else {
        return "404 Not Found";
    };
    if body.len() != p.size || !json {
        return "400 Bad Request";
    }
    p.body = Some(body);
    "200 OK"
}

/// `user`'s game says how upload `id` went: what it sent, when it went (and arrived whole).
pub fn finish(user: u32, id: u64, ok: bool) -> Option<Vec<u8>> {
    let mut st = state();
    if st.pending.get(&id).is_none_or(|p| p.user != user) {
        return None;
    }
    let p = st.pending.remove(&id)?;
    p.body.filter(|_| ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upload_goes_once_to_its_own_address() {
        let user = 910_001;
        assert_eq!(begin(user, 0x8000_0002, 10), Err(Refused::Type));
        assert_eq!(begin(user, SHADOWNET, 0), Err(Refused::Size));
        assert_eq!(begin(user, SHADOWNET, (MAX_UPLOAD + 1) as u32), Err(Refused::Size));
        let body = br#"{"Loadout:Items":{}}"#.to_vec();
        let (id, secret) = begin(user, SHADOWNET, body.len() as u32).unwrap();
        assert_eq!(begin(user, SHADOWNET, 10), Err(Refused::TooSoon), "one at a time, not too often");
        assert_eq!(receive(&path(id, "0000"), body.clone()), "404 Not Found", "the secret");
        assert_eq!(receive(&path(id, &secret), b"{short".to_vec()), "400 Bad Request", "the size asked for");
        assert_eq!(receive(&path(id, &secret), vec![0xff; body.len()]), "400 Bad Request", "JSON text only");
        let mut junk = b"{".to_vec();
        junk.resize(body.len(), b'x');
        assert_eq!(receive(&path(id, &secret), junk), "400 Bad Request", "a JSON object, not just a brace");
        assert_eq!(expects(&path(id, "0000"), body.len()), Err("404 Not Found"));
        assert_eq!(expects(&path(id, &secret), body.len() + 1), Err("400 Bad Request"));
        assert_eq!(expects(&path(id, &secret), body.len()), Ok(()));
        assert_eq!(receive(&path(id, &secret), body.clone()), "200 OK");
        assert_eq!(receive(&path(id, &secret), body.clone()), "404 Not Found", "once");
        assert_eq!(finish(user + 1, id, true), None, "someone else's");
        assert_eq!(finish(user, id, true), Some(body));
        assert_eq!(finish(user, id, true), None, "used up");
    }

    #[test]
    fn a_failed_upload_keeps_nothing() {
        let user = 910_002;
        let (id, secret) = begin(user, SHADOWNET, 2).unwrap();
        assert_eq!(receive(&path(id, &secret), b"{}".to_vec()), "200 OK");
        assert_eq!(finish(user, id, false), None);
    }
}
