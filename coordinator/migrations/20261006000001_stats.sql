-- Each person's stats across the network (POST /v1/stats): the servers forward every stat
-- the game writes, and they're added up here the way the board says (stat_boards). One row
-- per person (their global id), board, context and stat; ratio stats are worked out when
-- read, never stored. Kept for good: it's the players' record. `updated_at` is when the
-- value last changed (between equal values, whoever got there first ranks higher).
CREATE TABLE global_stats (
    global_id TEXT NOT NULL,
    board INTEGER NOT NULL,
    context INTEGER NOT NULL,
    stat INTEGER NOT NULL,
    value REAL NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (global_id, board, context, stat)
) WITHOUT ROWID;
CREATE INDEX idx_global_stats_rank ON global_stats (board, context, stat, value);

-- The name each person last played under, for the leaderboards (the latest wins).
CREATE TABLE global_names (
    global_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

-- How far each server's stat writes are applied: its last write's id, per epoch (a server
-- whose database was reset starts a new epoch, and its ids again from 1).
CREATE TABLE stat_sequences (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    epoch TEXT NOT NULL,
    last_id INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, epoch)
);
