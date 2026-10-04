-- Global stats (stat_boards, the coordinator): each stat the game writes for a player
-- with an identity waits here until the coordinator has it, in order. `id` only grows;
-- with the database's random epoch it tells the coordinator which writes it has already.
CREATE TABLE stats_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    global_id TEXT NOT NULL,
    name TEXT NOT NULL,
    board INTEGER NOT NULL,
    context INTEGER NOT NULL,
    stat INTEGER NOT NULL,
    value REAL NOT NULL
);
CREATE TABLE stats_epoch (epoch TEXT NOT NULL);
INSERT INTO stats_epoch (epoch) VALUES (lower(hex(randomblob(8))));

-- What the coordinator last said, for answering the game without waiting on it: the top
-- of every global leaderboard, the places of players here and their friends, and their
-- stats across the network.
CREATE TABLE global_leaderboards (
    leaderboard INTEGER NOT NULL,
    context INTEGER NOT NULL,
    total INTEGER NOT NULL,
    -- The top places, as the coordinator sent them (JSON).
    top TEXT NOT NULL,
    PRIMARY KEY (leaderboard, context)
);
CREATE TABLE global_ranks (
    global_id TEXT NOT NULL,
    leaderboard INTEGER NOT NULL,
    context INTEGER NOT NULL,
    name TEXT NOT NULL,
    rank INTEGER NOT NULL,
    value REAL NOT NULL,
    stats TEXT NOT NULL,
    PRIMARY KEY (global_id, leaderboard, context)
);
CREATE TABLE global_stats (
    global_id TEXT NOT NULL,
    board INTEGER NOT NULL,
    context INTEGER NOT NULL,
    stat INTEGER NOT NULL,
    value REAL NOT NULL,
    PRIMARY KEY (global_id, board, context, stat)
) WITHOUT ROWID;
