//! A tally of the RMC calls the server could not answer.
//!
//! The game has client code for more protocols than this server implements,
//! and much of it may never run. Counting what the game really asks for shows
//! which services are worth building. A call counts as unhandled when its
//! protocol isn't registered, its method isn't known, or its handler is still
//! a generated stub.
//!
//! The first call of each kind is logged as a warning; repeats are logged at
//! 2, 4, 8, ... calls so a busy stub doesn't flood the log.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Serialize;
use slog::Logger;

/// Why a call went unanswered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// No handler is registered for the protocol.
    UnknownProtocol,
    /// The protocol is registered but doesn't know the method.
    UnknownMethod,
    /// The method exists but its handler is a stub.
    Unimplemented,
}

/// One protocol/method pair the server couldn't answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnhandledCall {
    pub protocol_id: u16,
    pub method_id: u32,
    /// The protocol's name, when this server knows it.
    pub protocol: Option<String>,
    /// The method's name, when the protocol is registered and knows it.
    pub method: Option<String>,
    pub kind: Kind,
    pub count: u64,
    /// Unix time in seconds.
    pub first_seen: u64,
    pub last_seen: u64,
}

static CALLS: Mutex<BTreeMap<(u16, u32), UnhandledCall>> = Mutex::new(BTreeMap::new());

/// Names of protocols the game uses, including ones this server doesn't
/// register, so an unknown protocol can still be named in the log.
#[must_use]
pub fn protocol_name(protocol_id: u16) -> Option<&'static str> {
    Some(match protocol_id {
        3 => "NatTraversalProtocol",
        10 => "TicketGrantingProtocol",
        11 => "SecureConnectionProtocol",
        14 => "NotificationProtocol",
        16 => "SimpleAuthenticationProtocol",
        18 => "HealthProtocol",
        19 => "MonitoringProtocol",
        20 => "FriendsProtocol",
        25 => "AccountManagementProtocol",
        29 => "UbiAccountManagementProtocol",
        35 => "PrivilegesProtocol",
        36 => "TrackingProtocol3",
        39 => "LocalizationProtocol",
        42 => "GameSessionProtocol",
        49 => "UplayWinProtocol",
        53 => "UserStorageProtocol",
        55 => "PlayerStatsProtocol",
        71 => "OfflineGameNotificationsProtocol",
        105 => "ChallengeHelperProtocol",
        106 => "ClanHelperProtocol",
        107 => "LadderHelperProtocol",
        123 => "GameSessionExProtocol",
        1001 => "TrackingExtensionProtocol",
        5002 => "OverlordNewsProtocol",
        5003 => "OverlordCoreProtocol",
        5007 => "OverlordChallengeProtocol",
        _ => return None,
    })
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Counts an unanswered call and logs it (see the module docs for when).
/// The most distinct calls remembered.
const MAX_KINDS: usize = 512;

pub fn record(logger: &Logger, protocol_id: u16, method_id: u32, protocol: Option<String>, method: Option<String>, kind: Kind) {
    let protocol = protocol.or_else(|| protocol_name(protocol_id).map(String::from));
    let count = {
        let mut calls = CALLS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let t = now();
        // Bounded: ids come from anyone who connects, and there are 2^48 of them.
        if calls.len() >= MAX_KINDS && !calls.contains_key(&(protocol_id, method_id)) {
            return;
        }
        let call = calls.entry((protocol_id, method_id)).or_insert_with(|| UnhandledCall {
            protocol_id,
            method_id,
            protocol: protocol.clone(),
            method: method.clone(),
            kind,
            count: 0,
            first_seen: t,
            last_seen: t,
        });
        call.count += 1;
        call.last_seen = t;
        call.count
    };
    if count.is_power_of_two() {
        let name = format!("{}.{}", protocol.as_deref().unwrap_or("?"), method.map_or_else(|| method_id.to_string(), |m| m));
        warn!(
            logger,
            "Unhandled RMC call {name} ({protocol_id}.{method_id}): {kind:?}, {count} so far";
            "unhandled_rmc" => true
        );
    }
}

/// Every unanswered call so far, most frequent first.
#[must_use]
pub fn snapshot() -> Vec<UnhandledCall> {
    let calls = CALLS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut list: Vec<_> = calls.values().cloned().collect();
    list.sort_by(|a, b| b.count.cmp(&a.count).then(a.protocol_id.cmp(&b.protocol_id)).then(a.method_id.cmp(&b.method_id)));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_names_unhandled_calls() {
        let logger = Logger::root(slog::Discard, slog::o!());
        // Protocol ids this test owns, so other tests' calls don't interfere.
        record(&logger, 20, 11, None, None, Kind::UnknownProtocol);
        record(&logger, 20, 11, None, None, Kind::UnknownProtocol);
        record(&logger, 64999, 3, None, None, Kind::UnknownProtocol);
        let calls = snapshot();
        let friends = calls.iter().find(|c| c.protocol_id == 20 && c.method_id == 11).unwrap();
        assert_eq!(friends.count, 2);
        assert_eq!(friends.protocol.as_deref(), Some("FriendsProtocol"));
        assert_eq!(friends.kind, Kind::UnknownProtocol);
        let unknown = calls.iter().find(|c| c.protocol_id == 64999).unwrap();
        assert_eq!(unknown.protocol, None, "unknown ids stay unnamed");
        let pos = |p: u16| calls.iter().position(|c| c.protocol_id == p).unwrap();
        assert!(pos(20) < pos(64999), "most frequent first");
    }
}
