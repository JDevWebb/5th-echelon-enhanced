-- What each stat write added (0.4.8, fair play F1), kept 30 days: the stats that add up, with
-- how far into the player's session it came (NULL: no session known), so the admins see gains
-- far above what the network's players normally make in a minute of play.
CREATE TABLE stat_gains (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at INTEGER NOT NULL,
    server_id TEXT NOT NULL,
    global_id TEXT NOT NULL,
    board INTEGER NOT NULL,
    context INTEGER NOT NULL,
    stat INTEGER NOT NULL,
    gain REAL NOT NULL,
    value REAL NOT NULL,
    played INTEGER
);
CREATE INDEX idx_stat_gains_stat ON stat_gains (board, stat, at);
CREATE INDEX idx_stat_gains_at ON stat_gains (at);
