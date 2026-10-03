//! Implements the `PlayerStatsProtocolServer`: the stats the game writes, kept per
//! player as each board says ([`crate::stat_boards`]), read back by the game and
//! ranked on its leaderboards.

use std::sync::Arc;

use quazal::prudp::ClientRegistry;
use quazal::rmc::types::PropertyVariant;
use quazal::rmc::types::Variant;
use quazal::rmc::Error;
use quazal::rmc::Protocol;
use quazal::ClientInfo;
use quazal::Context;
use sc_bl_protocols::player_stats_service::types::LeaderboardResult;
use sc_bl_protocols::player_stats_service::types::PlayerRank;
use sc_bl_protocols::player_stats_service::types::PlayerStatSet;
use sc_bl_protocols::player_stats_service::types::StatboardResult;
use slog::Logger;

use crate::login_required;
use crate::protocols::player_stats_service::player_stats_protocol::PlayerStatsProtocolServer;
use crate::protocols::player_stats_service::player_stats_protocol::PlayerStatsProtocolServerTrait;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsByPlayers2Request;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsByPlayers2Response;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsByPlayersRequest;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsByPlayersResponse;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsByRank2Request;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsByRank2Response;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsByRankRequest;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsByRankResponse;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsNearPlayer2Request;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsNearPlayer2Response;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsNearPlayerRequest;
use crate::protocols::player_stats_service::player_stats_protocol::ReadLeaderboardsNearPlayerResponse;
use crate::protocols::player_stats_service::player_stats_protocol::ReadStatsByPlayersRequest;
use crate::protocols::player_stats_service::player_stats_protocol::ReadStatsByPlayersResponse;
use crate::protocols::player_stats_service::player_stats_protocol::WriteStatsRequest;
use crate::protocols::player_stats_service::player_stats_protocol::WriteStatsResponse;
use crate::stat_boards;
use crate::stat_boards::Aggregation;
use crate::storage::Ranked;
use crate::storage::StatWrite;
use crate::storage::Storage;
use crate::storage::StoredStats;

/// The most of anything one request may name: players, boards, contexts, stats, places.
const MAX_ITEMS: usize = 100;
/// Larger stat values than this are refused: no stat the game keeps comes near it.
const MAX_VALUE: f64 = 1e12;

struct PlayerStatsProtocolServerImpl {
    storage: Arc<Storage>,
}

/// A stat as the game wants it: a whole number or a fraction ([`stat_boards::is_fraction`]).
fn variant(stat: u32, value: f64) -> Variant {
    if stat_boards::is_fraction(stat) {
        Variant::F64(value)
    } else {
        #[allow(clippy::cast_possible_truncation)]
        Variant::I64(value.round() as i64)
    }
}

/// A written value, if it is a number the server keeps.
fn written_value(value: &Variant) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    let value = match value {
        Variant::I64(v) => *v as f64,
        Variant::U64(v) => *v as f64,
        Variant::F64(v) => *v,
        _ => return None,
    };
    (value.is_finite() && value.abs() <= MAX_VALUE).then_some(value)
}

/// The writes of a WriteStats request the boards allow, and how many they don't.
fn checked_writes(request: &WriteStatsRequest) -> (Vec<StatWrite>, usize) {
    let mut writes = Vec::new();
    let mut refused = 0;
    for update in request.player_stat_updates.iter().take(MAX_ITEMS) {
        let contexts: Vec<u32> = if update.context_ids.is_empty() {
            vec![0]
        } else {
            update.context_ids.iter().copied().take(MAX_ITEMS).collect()
        };
        for context in contexts {
            for stat in update.stats.iter().take(MAX_ITEMS) {
                let allowed = stat_boards::board(update.board_id).filter(|b| b.has_context(context)).and_then(|b| b.aggregation(stat.id));
                match (allowed, written_value(&stat.value)) {
                    (Some(a), Some(value)) if !matches!(a, Aggregation::Ratio(..)) => writes.push(StatWrite {
                        board: update.board_id,
                        context,
                        stat: stat.id,
                        value,
                    }),
                    _ => refused += 1,
                }
            }
        }
    }
    (writes, refused)
}

/// `wanted` stats of a board (all of the board's when none are named), ratios worked
/// out and stats never written at their defaults.
fn stat_values(board: u32, stored: &[(u32, f64)], wanted: &[u32]) -> Vec<PropertyVariant> {
    let Some(board) = stat_boards::board(board) else { return vec![] };
    let all: Vec<u32> = board.entries.iter().map(|(id, _)| *id).collect();
    let wanted = if wanted.is_empty() { &all[..] } else { wanted };
    let get = |stat: u32| stored.iter().find(|(id, _)| *id == stat).map_or_else(|| stat_boards::default_value(stat), |(_, v)| *v);
    wanted
        .iter()
        .take(MAX_ITEMS)
        .filter_map(|&stat| {
            let value = match board.aggregation(stat)? {
                Aggregation::Ratio(left, right) => stat_boards::ratio(get(left), get(right)),
                _ => get(stat),
            };
            Some(PropertyVariant {
                id: stat,
                value: variant(stat, value),
            })
        })
        .collect()
}

fn stat_set(board: u32, stored: StoredStats, wanted: &[u32]) -> PlayerStatSet {
    PlayerStatSet {
        player_pid: stored.user_id,
        stats: stat_values(board, &stored.stats, wanted).into(),
        player_name: stored.name,
        submitted_time: quazal::rmc::types::DateTime(stored.submitted),
    }
}

/// At most [`MAX_ITEMS`] of a count the game asked for.
fn capped(count: u32) -> u32 {
    count.min(MAX_ITEMS as u32)
}

/// A leaderboard query's fields: board, context, reset frequency and the stats wanted.
type Query<'a> = (u32, u32, u32, &'a [u32]);

impl PlayerStatsProtocolServerImpl {
    /// A leaderboard's places as the game wants them, each with the player's stats
    /// (those the query names, all when none) and the stat they are ranked by as the score.
    fn leaderboard_result(
        &self,
        logger: &Logger,
        (board_id, context_id, reset_frequency, stat_ids): Query,
        places: impl FnOnce(&stat_boards::Leaderboard) -> eyre::Result<Vec<Ranked>>,
    ) -> Result<LeaderboardResult, Error> {
        let mut result = LeaderboardResult {
            board_id,
            context_id,
            reset_frequency,
            leaderboard_total_player_count: 0,
            player_ranks: vec![].into(),
        };
        let Some(leaderboard) = stat_boards::leaderboard(board_id) else {
            info!(logger, "No leaderboard {board_id}");
            return Ok(result);
        };
        result.leaderboard_total_player_count = rmc_err!(self.storage.leaderboard_size(leaderboard, context_id), logger, "error counting a leaderboard")?;
        let places = rmc_err!(places(leaderboard), logger, "error reading a leaderboard")?;
        let players: Vec<u32> = places.iter().map(|p| p.user_id).collect();
        let mut stats = rmc_err!(self.storage.stats_of(&players, leaderboard.board, context_id), logger, "error reading stats")?;
        let ranks: Vec<PlayerRank> = places
            .into_iter()
            .map(|place| {
                let stored = match stats.iter().position(|s| s.user_id == place.user_id) {
                    Some(i) => stats.swap_remove(i),
                    None => StoredStats {
                        user_id: place.user_id,
                        name: place.name.clone(),
                        submitted: 0,
                        stats: vec![],
                    },
                };
                PlayerRank {
                    player_stat_set: stat_set(leaderboard.board, stored, stat_ids),
                    rank_status: 0,
                    rank: place.rank,
                    score: variant(leaderboard.stat, place.value),
                }
            })
            .collect();
        result.player_ranks = ranks.into();
        Ok(result)
    }

    /// Each query's leaderboard, its places from `places`.
    fn leaderboards<'q>(
        &self,
        logger: &Logger,
        queries: impl Iterator<Item = Query<'q>>,
        places: impl Fn(&stat_boards::Leaderboard, u32) -> eyre::Result<Vec<Ranked>>,
    ) -> Result<Vec<LeaderboardResult>, Error> {
        queries
            .take(MAX_ITEMS)
            .map(|query| self.leaderboard_result(logger, query, |lb| places(lb, query.1)))
            .collect()
    }
}

impl<T> PlayerStatsProtocolServerTrait<T> for PlayerStatsProtocolServerImpl {
    /// The game's stats after a match or mission: added to the player's, each as its
    /// board says. Stats no board has are left out.
    fn write_stats(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<T>,
        request: WriteStatsRequest,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<WriteStatsResponse, Error> {
        let user_id = login_required(&*ci)?;
        let (writes, refused) = checked_writes(&request);
        if refused > 0 {
            let boards: Vec<u32> = request.player_stat_updates.iter().map(|u| u.board_id).collect();
            info!(logger, "Stats of {user_id}: {} kept, {refused} not on their boards (boards {boards:?})", writes.len());
        }
        rmc_err!(self.storage.write_stats(user_id, &writes), logger, "error writing stats")?;
        Ok(WriteStatsResponse)
    }

    /// Players' stats, per board and context the game asks for. Players without any
    /// there are left out; the game takes the defaults for them.
    fn read_stats_by_players(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<T>,
        request: ReadStatsByPlayersRequest,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<ReadStatsByPlayersResponse, Error> {
        login_required(&*ci)?;
        info!(logger, "ReadStatsByPlayers: pids={:?}", request.player_pids);
        let players: Vec<u32> = request.player_pids.iter().copied().take(MAX_ITEMS).collect();
        let mut results = Vec::new();
        for query in request.queries.iter().take(MAX_ITEMS) {
            let wanted: Vec<u32> = query.stat_ids.iter().copied().collect();
            let contexts: Vec<u32> = if query.context_ids.is_empty() {
                vec![0]
            } else {
                query.context_ids.iter().copied().take(MAX_ITEMS).collect()
            };
            for context_id in contexts {
                let stored = rmc_err!(self.storage.stats_of(&players, query.board_id, context_id), logger, "error reading stats")?;
                let sets: Vec<PlayerStatSet> = stored.into_iter().map(|s| stat_set(query.board_id, s, &wanted)).collect();
                results.push(StatboardResult {
                    board_id: query.board_id,
                    context_id,
                    reset_frequency: query.reset_frequency,
                    player_stat_sets: sets.into(),
                    default_stat_values: stat_values(query.board_id, &[], &wanted).into(),
                });
            }
        }
        Ok(ReadStatsByPlayersResponse { results: results.into() })
    }

    fn read_leaderboards_near_player(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<T>,
        request: ReadLeaderboardsNearPlayerRequest,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<ReadLeaderboardsNearPlayerResponse, Error> {
        login_required(&*ci)?;
        info!(logger, "Leaderboards near {}: {:?}", request.player_pid, request.queries);
        let queries = request.queries.iter().map(|q| (q.board_id, q.context_id, q.reset_frequency, &q.stat_ids[..]));
        let results = self.leaderboards(logger, queries, |lb, context| {
            self.storage.leaderboard_around(lb, context, request.player_pid, capped(request.count))
        })?;
        Ok(ReadLeaderboardsNearPlayerResponse { results: results.into() })
    }

    fn read_leaderboards_by_rank(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<T>,
        request: ReadLeaderboardsByRankRequest,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<ReadLeaderboardsByRankResponse, Error> {
        login_required(&*ci)?;
        info!(logger, "Leaderboards from rank {}: {:?}", request.starting_rank, request.queries);
        let queries = request.queries.iter().map(|q| (q.board_id, q.context_id, q.reset_frequency, &q.stat_ids[..]));
        let results = self.leaderboards(logger, queries, |lb, context| {
            self.storage.leaderboard_from(lb, context, request.starting_rank, capped(request.count))
        })?;
        Ok(ReadLeaderboardsByRankResponse { results: results.into() })
    }

    /// Where the players asked about (friends) stand.
    fn read_leaderboards_by_players(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<T>,
        request: ReadLeaderboardsByPlayersRequest,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<ReadLeaderboardsByPlayersResponse, Error> {
        login_required(&*ci)?;
        info!(logger, "Leaderboards of {:?}: {:?}", request.player_pids, request.queries);
        let players: Vec<u32> = request.player_pids.iter().copied().take(MAX_ITEMS).collect();
        let queries = request.queries.iter().map(|q| (q.board_id, q.context_id, q.reset_frequency, &q.stat_ids[..]));
        let results = self.leaderboards(logger, queries, |lb, context| self.storage.leaderboard_of(lb, context, &players))?;
        Ok(ReadLeaderboardsByPlayersResponse { results: results.into() })
    }

    fn read_leaderboards_near_player_2(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<T>,
        request: ReadLeaderboardsNearPlayer2Request,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<ReadLeaderboardsNearPlayer2Response, Error> {
        login_required(&*ci)?;
        info!(logger, "Leaderboards near {} (2): {:?}", request.player_pid, request.queries);
        let queries = request.queries.iter().map(|q| (q.board_id, q.context_id, q.reset_frequency, &q.stat_ids[..]));
        let results = self.leaderboards(logger, queries, |lb, context| {
            self.storage.leaderboard_around(lb, context, request.player_pid, capped(request.count))
        })?;
        Ok(ReadLeaderboardsNearPlayer2Response { results: results.into() })
    }

    fn read_leaderboards_by_rank_2(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<T>,
        request: ReadLeaderboardsByRank2Request,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<ReadLeaderboardsByRank2Response, Error> {
        login_required(&*ci)?;
        info!(logger, "Leaderboards from rank {} (2): {:?}", request.starting_rank, request.queries);
        let queries = request.queries.iter().map(|q| (q.board_id, q.context_id, q.reset_frequency, &q.stat_ids[..]));
        let results = self.leaderboards(logger, queries, |lb, context| {
            self.storage.leaderboard_from(lb, context, request.starting_rank, capped(request.count))
        })?;
        Ok(ReadLeaderboardsByRank2Response { results: results.into() })
    }

    /// As [`Self::read_leaderboards_by_players`], with the players named in each query.
    fn read_leaderboards_by_players_2(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<T>,
        request: ReadLeaderboardsByPlayers2Request,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<ReadLeaderboardsByPlayers2Response, Error> {
        login_required(&*ci)?;
        info!(logger, "Leaderboards of players (2): {:?}", request.queries);
        let mut results = Vec::new();
        for q in request.queries.iter().take(MAX_ITEMS) {
            let players: Vec<u32> = q.estimated_pids.iter().copied().take(MAX_ITEMS).collect();
            let query = (q.board_id, q.context_id, q.reset_frequency, &q.stat_ids[..]);
            results.push(self.leaderboard_result(logger, query, |lb| self.storage.leaderboard_of(lb, q.context_id, &players))?);
        }
        Ok(ReadLeaderboardsByPlayers2Response { results: results.into() })
    }
}

/// Creates a new boxed `PlayerStatsProtocolServer` instance.
///
/// This function is typically used to register the player stats protocol
/// with the server's protocol dispatcher.
pub fn new_protocol<T: 'static>(storage: Arc<Storage>) -> Box<dyn Protocol<T>> {
    Box::new(PlayerStatsProtocolServer::new(PlayerStatsProtocolServerImpl { storage }))
}

#[cfg(test)]
mod tests {
    use quazal::rmc::types::QList;
    use sc_bl_protocols::player_stats_service::types::PlayerStatUpdate;

    use super::*;

    fn stat(id: u32, value: Variant) -> PropertyVariant {
        PropertyVariant { id, value }
    }

    #[test]
    fn only_what_the_boards_keep_is_written() {
        let request = WriteStatsRequest {
            player_stat_updates: QList(vec![
                // Ladder 2: kills kept; a stat the board lacks and a string are not.
                PlayerStatUpdate {
                    board_id: 17,
                    context_ids: QList(vec![2]),
                    stats: QList(vec![stat(100, Variant::I64(5)), stat(101, Variant::I64(1)), stat(122, Variant::String("x".into()))]),
                },
                // A board without contexts is context 0; an absurd value is refused.
                PlayerStatUpdate {
                    board_id: 2,
                    context_ids: QList(vec![]),
                    stats: QList(vec![stat(140, Variant::I64(1200)), stat(145, Variant::F64(f64::INFINITY))]),
                },
                // A ratio, a context the board doesn't have, a board there isn't.
                PlayerStatUpdate {
                    board_id: 10,
                    context_ids: QList(vec![228]),
                    stats: QList(vec![stat(102, Variant::F64(2.0))]),
                },
                PlayerStatUpdate {
                    board_id: 10,
                    context_ids: QList(vec![1]),
                    stats: QList(vec![stat(100, Variant::I64(1))]),
                },
                PlayerStatUpdate {
                    board_id: 99,
                    context_ids: QList(vec![]),
                    stats: QList(vec![stat(100, Variant::I64(1))]),
                },
            ]),
        };
        let (writes, refused) = checked_writes(&request);
        assert_eq!(
            writes,
            [
                StatWrite {
                    board: 17,
                    context: 2,
                    stat: 100,
                    value: 5.0
                },
                StatWrite {
                    board: 2,
                    context: 0,
                    stat: 140,
                    value: 1200.0
                },
            ]
        );
        assert_eq!(refused, 6);
    }

    #[test]
    fn stats_go_out_as_the_game_reads_them() {
        // Spies vs Mercs: kills and deaths stored, the ratio worked out as a fraction.
        let values = stat_values(10, &[(100, 9.0), (101, 4.0)], &[100, 102, 122, 213, 5000]);
        let shown: Vec<(u32, String)> = values.iter().map(|p| (p.id, format!("{:?}", p.value))).collect();
        assert_eq!(
            shown,
            [
                (100, "I64(9)".into()),
                (102, "F64(2.25)".into()),
                (122, "I64(0)".into()),
                (213, format!("I64({})", i32::MAX))
            ],
            "a stat the board lacks is left out"
        );
        assert_eq!(
            stat_values(1, &[], &[]).len(),
            stat_boards::board(1).unwrap().entries.len(),
            "all of a board's when none are named"
        );
    }
}
