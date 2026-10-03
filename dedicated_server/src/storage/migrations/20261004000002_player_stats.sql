-- The stats the game writes (PlayerStats.WriteStats), one row per player, board,
-- context and stat, already added up as the board says (stat_boards.rs). Leaderboards
-- rank players by one of them.
CREATE TABLE player_stats (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    board_id INTEGER NOT NULL,
    context_id INTEGER NOT NULL,
    stat_id INTEGER NOT NULL,
    value REAL NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, board_id, context_id, stat_id)
) WITHOUT ROWID;

CREATE INDEX player_stats_ranking ON player_stats (board_id, context_id, stat_id, value);
