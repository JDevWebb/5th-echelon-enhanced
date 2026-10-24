//! Implements the `GameSessionExProtocolServer` for extended game session management,
//! including advanced session searching capabilities.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;

use quazal::prudp::ClientRegistry;
use quazal::rmc::types::Property;
use quazal::rmc::types::QList;
use quazal::rmc::Error;
use quazal::rmc::Protocol;
use quazal::ClientInfo;
use quazal::Context;
use slog::Logger;

use crate::game_session::PROPERTY_DATA_VERSION;
use crate::login_required;
use crate::protocols::game_session_ex_service::game_session_ex_protocol::GameSessionExProtocolServer;
use crate::protocols::game_session_ex_service::game_session_ex_protocol::GameSessionExProtocolServerTrait;
use crate::protocols::game_session_ex_service::game_session_ex_protocol::SearchSessionsRequest;
use crate::protocols::game_session_ex_service::game_session_ex_protocol::SearchSessionsResponse;
use crate::protocols::game_session_ex_service::types::GameSessionSearchResultEx;
use crate::protocols::game_session_service::types::GameSessionKey;
use crate::protocols::game_session_service::types::GameSessionParticipant;
use crate::protocols::game_session_service::types::GameSessionSearchResult;
use crate::storage::Storage;
use crate::storage::UrlsFor;

/// The most open sessions one matchmaking search compares (newest first).
const MAX_SCANNED: u32 = 1000;

/// Implementation of the `GameSessionExProtocolServerTrait` for extended game session operations.
struct GameSessionExProtocolServerImpl {
    storage: Arc<Storage>,
}

impl<CI> GameSessionExProtocolServerTrait<CI> for GameSessionExProtocolServerImpl {
    /// Handles the `SearchSessions` request, providing extended search capabilities for game sessions.
    ///
    /// This function requires the client to be logged in. It filters sessions based on
    /// various attributes and returns detailed session information.
    fn search_sessions(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: SearchSessionsRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<SearchSessionsResponse, Error> {
        #![allow(clippy::unreadable_literal)]

        let user_id = login_required(&*ci)?;
        info!(logger, "Client searches for session: {:?}", request);
        if !crate::rate_limit::game_requests().check(user_id) {
            return Err(Error::AccessDenied);
        }

        // A client that just accepted an invitation looks for the room with query 8, but it
        // sends the ordinary matchmaking attributes along - and a private room carries
        // property 103 = 0, which no ordinary query matches. So it never found the room it
        // had just been invited into. When an invitation is pending, answer with exactly that
        // room and skip the attribute comparison below.
        let session_type = request.game_session_query.type_id;
        let invited_session = if request.game_session_query.query_id == 8 {
            rmc_err!(
                self.storage.find_pending_invited_session(user_id, session_type, &[]),
                logger,
                "Error resolving invited room"
            )?
        } else {
            None
        };
        let invitation_search = invited_session.is_some();

        let sessions = if let Some(mut session) = invited_session {
            let opened = crate::game_session::advertise_private_slots_as_public(&session.attributes);
            if opened != session.attributes {
                info!(logger, "Opening a seat in room {}: {} -> {}", session.session_id, session.attributes, opened);
                session.attributes = opened;
            }
            info!(
                logger,
                "Answering invitation search of {user_id} with session {} of host {}", session.session_id, session.creator_id
            );
            crate::join_targets::invited(user_id, session.session_id);
            vec![session]
        } else {
            // Without participants: the attributes decide first, and only the answer's few
            // sessions are read in full (below).
            rmc_err!(
                self.storage.search_sessions(session_type, user_id, Some(MAX_SCANNED)),
                logger,
                "Error searching game sessions"
            )?
        };
        // search svm
        // 103 => 2165463540
        // 106 => 3564829
        // 107 => 3909881133
        // 108 => 0
        // 109 => 0
        // 110 => 0
        // 112 => 1
        // search coop
        // 106 => 3564829
        // 107 => 3909881133
        // 108 => 0
        // 109 => 0
        // 110 => 0
        // coop: 113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 2;4 => 0;101 => 3578398534;102 => 3;103 => 0;105 => 2;112 => 2
        // svm:  113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 4;4 => 0;101 => 72621668;102 => 8;103 => 2165463540;105 => 0;112 => 2
        // coop: 113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 0;4 => 2;101 => 3578398534;102 => 3;103 => 0;105 => 2;112 => 2
        // coop: 113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 0;4 => 2;101 => 1328467886;102 => 5;103 => 0;105 => 2;112 => 2
        // coop: 113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 0;4 => 2;101 => 2573003522;102 => 4;103 => 0;105 => 2;112 => 2

        // 101 => might be map id
        // 102 => might be game mode
        // 4 => might be number of players? (svm)
        // 112 => might be number of players? (svm) first is set to 4, but 8 after opening and closening the match settings
        let mut req_attrs = request.game_session_query.parameters.0.into_iter().map(|p| (p.id, p.value)).collect::<HashMap<_, _>>();
        // if not set assume 0 (required for keeping coop and svm apart)
        if let Entry::Vacant(entry) = req_attrs.entry(103) {
            entry.insert(0);
        }
        let scanned: Vec<_> = sessions
            .into_iter()
            .filter_map(|session| {
                let Ok(attrs) = session.attributes.parse::<QList<Property>>() else {
                    warn!(logger, "skipping session {} with unreadable attributes {:?}", session.session_id, session.attributes);
                    return None;
                };
                let attrs = attrs.0.into_iter().map(|p| (p.id, p.value)).collect::<HashMap<_, _>>();
                Some((session, attrs))
            })
            .collect();
        let hidden = if invitation_search {
            None
        } else {
            other_data(&req_attrs, scanned.iter().map(|(_, attrs)| attrs))
        };
        let other = if invitation_search {
            None
        } else {
            other_mode(&req_attrs, scanned.iter().map(|(_, attrs)| attrs))
        };
        let sessions: Vec<_> = scanned
            .into_iter()
            .filter(|(_, attrs)| invitation_search || answers(&req_attrs, attrs, false))
            // A lobby its host left or went quiet in isn't one to join (host migration).
            .filter(|(session, _)| invitation_search || !crate::game_session::lobby_is_dead(_client_registry, session.session_id, session.creator_id))
            .map(|(session, _)| session)
            .take(crate::game_session::MAX_SEARCH_RESULTS as usize)
            .collect();
        // The invited room comes with its participants already.
        let sessions = if invitation_search {
            sessions
        } else {
            rmc_err!(self.storage.with_participants(sessions, UrlsFor::AllBut(user_id)), logger, "Error reading participants")?
        };
        info!(logger, "Found {} sessions", sessions.len());
        let mut detail = serde_json::json!({ "query": request.game_session_query.query_id, "found": sessions.len() });
        // Found nothing, but there are lobbies their game data keeps from them: the game
        // says nothing of it, so the player and the admins hear it from us.
        if let Some(hidden) = hidden.filter(|_| sessions.is_empty()) {
            info!(
                logger,
                "{} open lobbies have other game data ({:#010x}, theirs {:#010x})", hidden.lobbies, hidden.mine, hidden.theirs
            );
            detail["other_data"] = serde_json::json!({
                "lobbies": hidden.lobbies,
                "mine": format!("{:#010x}", hidden.mine),
                "theirs": format!("{:#010x}", hidden.theirs),
                "mine_name": server_api::data_versions::name(hidden.mine),
                "theirs_name": server_api::data_versions::name(hidden.theirs),
            });
            // Once a minute at most (as noted), the overlay says so as it happens.
            if let Some(noted) = crate::reports::search_other_data(user_id, hidden.mine, hidden.theirs, hidden.lobbies) {
                let shown = |v: u32| crate::reports::data_shown(Some(v));
                if let Some((title, text)) = server_api::notices::search_notice(&shown(hidden.mine), &shown(hidden.theirs)) {
                    crate::reports::tell(user_id, title, text, Some(noted));
                }
            }
        }
        // Found nothing, but matches of another Spies vs Mercs playlist are open: the game only
        // ever finds its own, so with few players two lone hosts of different modes never meet.
        if let Some(other) = other.filter(|_| sessions.is_empty() && detail.get("other_data").is_none()) {
            let names = (
                server_api::game_names::playlist_mode(other.mine).and_then(server_api::game_names::mode),
                server_api::game_names::mode(other.mode),
            );
            info!(
                logger,
                "{} open matches of playlist {} (game mode {}), this search's {}", other.rooms, other.theirs, other.mode, other.mine
            );
            detail["other_mode"] = serde_json::json!({ "rooms": other.rooms, "playlist": other.theirs, "game_mode": other.mode });
            if let (Some(mine), Some(theirs)) = names {
                if let Some(noted) = crate::reports::search_other_mode(user_id, other.mine, other.theirs, other.rooms) {
                    let (title, text) = server_api::notices::other_mode_notice(mine, theirs, other.rooms);
                    crate::reports::tell(user_id, title, text, Some(noted));
                }
            }
        }
        crate::session_events::note(crate::session_events::Who::Id(user_id), "search", detail);
        if !invitation_search {
            crate::join_targets::searched(user_id, sessions.iter().map(|s| (s.session_id, s.creator_id)));
        }
        Ok(SearchSessionsResponse {
            search_results: QList(
                sessions
                    .into_iter()
                    .map(|session| GameSessionSearchResultEx {
                        game_session_search_result: GameSessionSearchResult {
                            session_key: GameSessionKey {
                                session_id: session.session_id,
                                type_id: session.session_type,
                            },
                            host_pid: session.creator_id,
                            host_urls: QList::parse_lossy(session.participants.iter().filter(|p| p.user_id == session.creator_id).flat_map(|p| p.station_urls.iter())),
                            attributes: session.attributes.parse().unwrap_or_default(),
                        },
                        participants: QList(
                            session
                                .participants
                                .into_iter()
                                .map(|participant| GameSessionParticipant {
                                    pid: participant.user_id,
                                    name: participant.name,
                                    station_urls: QList::parse_lossy(&participant.station_urls),
                                })
                                .collect(),
                        ),
                    })
                    .collect(),
            ),
        })
    }
}

/// Whether a session's attributes answer a search's: every one the search names but 112
/// (the search says 1 where the room says 2: no idea what it means, and we'd like those rooms
/// found) and, with `any_data`, the game data ([`PROPERTY_DATA_VERSION`]).
fn answers(search: &HashMap<u32, u32>, session: &HashMap<u32, u32>, any_data: bool) -> bool {
    search
        .iter()
        .all(|(id, value)| *id == 112 || (any_data && *id == PROPERTY_DATA_VERSION) || session.get(id) == Some(value))
}

/// Open lobbies a search would have found but for their game data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OtherData {
    lobbies: usize,
    /// The searcher's data.
    mine: u32,
    /// What most of those lobbies have.
    theirs: u32,
}

/// The lobbies among `sessions` that answer `search` in all but their game data, if any.
fn other_data<'a>(search: &HashMap<u32, u32>, sessions: impl Iterator<Item = &'a HashMap<u32, u32>>) -> Option<OtherData> {
    let mine = *search.get(&PROPERTY_DATA_VERSION)?;
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for data in sessions
        .filter(|s| answers(search, s, true))
        .filter_map(|s| s.get(&PROPERTY_DATA_VERSION))
        .filter(|data| **data != mine)
    {
        *counts.entry(*data).or_default() += 1;
    }
    let lobbies = counts.values().sum();
    let (theirs, _) = counts.into_iter().max_by_key(|(data, n)| (*n, *data == server_api::data_versions::STOCK))?;
    Some(OtherData { lobbies, mine, theirs })
}

/// Open matches a Spies vs Mercs search would have found but for their playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OtherMode {
    rooms: usize,
    /// The search's playlist (attribute 103).
    mine: u32,
    /// The playlist most of those matches are, and its game mode (attribute 102).
    theirs: u32,
    mode: u32,
}

/// The matches among `sessions` that answer `search` (a Spies vs Mercs one: its playlist set)
/// in all but their playlist, if any: of another playlist, with the same game data.
fn other_mode<'a>(search: &HashMap<u32, u32>, sessions: impl Iterator<Item = &'a HashMap<u32, u32>>) -> Option<OtherMode> {
    let mine = *search.get(&103).filter(|p| **p != 0)?;
    let mut without = search.clone();
    without.remove(&103);
    let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
    for s in sessions.filter(|s| answers(&without, s, false)) {
        match (s.get(&103), s.get(&102)) {
            (Some(&theirs), Some(&mode)) if theirs != 0 && theirs != mine => *counts.entry((theirs, mode)).or_default() += 1,
            _ => {}
        }
    }
    let rooms = counts.values().sum();
    let ((theirs, mode), _) = counts.into_iter().max_by_key(|((playlist, _), n)| (*n, std::cmp::Reverse(*playlist)))?;
    Some(OtherMode { rooms, mine, theirs, mode })
}

pub fn new_protocol<T: 'static>(storage: Arc<Storage>) -> Box<dyn Protocol<T>> {
    Box::new(GameSessionExProtocolServer::new(GameSessionExProtocolServerImpl { storage }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK: u32 = server_api::data_versions::STOCK;
    const MODDED: u32 = 0xc37e_d697;

    fn attrs(list: &[(u32, u32)]) -> HashMap<u32, u32> {
        list.iter().copied().collect()
    }

    /// ijsman's two PCs on na1 (2026-10-10): one modded, one not, each searching and finding
    /// nothing while the other's lobby was open.
    #[test]
    fn lobbies_hidden_by_game_data_are_counted() {
        let search = attrs(&[(106, 3_564_829), (107, MODDED), (103, 0), (112, 1)]);
        let stock = attrs(&[(106, 3_564_829), (107, STOCK), (103, 0), (112, 2)]);
        let other_mode = attrs(&[(106, 3_564_829), (107, STOCK), (103, 2_165_463_540), (112, 2)]);
        let same = attrs(&[(106, 3_564_829), (107, MODDED), (103, 0), (112, 2)]);
        assert!(answers(&search, &same, false), "112 never decides");
        assert!(!answers(&search, &stock, false) && answers(&search, &stock, true));
        assert_eq!(
            other_data(&search, [&stock, &stock, &other_mode].into_iter()),
            Some(OtherData {
                lobbies: 2,
                mine: MODDED,
                theirs: STOCK
            }),
            "only lobbies the search wants otherwise"
        );
        assert_eq!(other_data(&search, [&other_mode, &same].into_iter()), None);
        let no_data = attrs(&[(106, 3_564_829), (103, 0)]);
        assert_eq!(other_data(&no_data, [&stock].into_iter()), None, "a search without game data");
        // Several others: most of them, the unmodified game on a tie.
        let a = attrs(&[(106, 3_564_829), (107, 1), (103, 0)]);
        assert_eq!(other_data(&search, [&a, &stock].into_iter()).map(|o| (o.lobbies, o.theirs)), Some((2, STOCK)));
        assert_eq!(other_data(&search, [&a, &a, &stock].into_iter()).map(|o| o.theirs), Some(1));
    }

    /// Pa3cio's Classic searches on eu1 (2026-10-20) found nothing while Tea_bag hosted
    /// Blacklist.
    #[test]
    fn matches_of_another_playlist_are_counted() {
        const CLASSIC: u32 = 2_165_463_540;
        const BLACKLIST: u32 = 4_008_726_265;
        let search = attrs(&[(106, 3_564_829), (107, STOCK), (103, CLASSIC), (112, 1)]);
        let blacklist = attrs(&[(106, 3_564_829), (107, STOCK), (103, BLACKLIST), (102, 7), (112, 2)]);
        let modded = attrs(&[(106, 3_564_829), (107, MODDED), (103, BLACKLIST), (102, 7)]);
        let private = attrs(&[(106, 3_564_829), (107, STOCK), (103, 0), (102, 7)]);
        let classic = attrs(&[(106, 3_564_829), (107, STOCK), (103, CLASSIC), (102, 8)]);
        assert_eq!(
            other_mode(&search, [&blacklist, &blacklist, &modded, &private, &classic].into_iter()),
            Some(OtherMode {
                rooms: 2,
                mine: CLASSIC,
                theirs: BLACKLIST,
                mode: 7
            }),
            "not other game data, a private match, nor its own playlist's"
        );
        let coop_search = attrs(&[(106, 3_564_829), (107, STOCK), (103, 0)]);
        assert_eq!(other_mode(&coop_search, [&blacklist].into_iter()), None, "only a Spies vs Mercs search");
        assert_eq!(other_mode(&search, [&classic].into_iter()), None);
    }
}
