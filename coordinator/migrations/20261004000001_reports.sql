-- Reports for the admin UI: who played each day (anonymised: each server sends an HMAC of
-- its players' account ids under a key only it has), when each was first seen, and alerts.
CREATE TABLE daily_players (
    server_id TEXT NOT NULL,
    day INTEGER NOT NULL,
    player TEXT NOT NULL,
    minutes INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (server_id, day, player)
) WITHOUT ROWID;
CREATE INDEX idx_daily_players_day ON daily_players (day);

CREATE TABLE first_seen (
    server_id TEXT NOT NULL,
    player TEXT NOT NULL,
    day INTEGER NOT NULL,
    PRIMARY KEY (server_id, player)
) WITHOUT ROWID;
CREATE INDEX idx_first_seen_day ON first_seen (day);

CREATE TABLE alerts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    server_id TEXT NOT NULL DEFAULT '',
    level TEXT NOT NULL,
    detail TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    resolved_at INTEGER
);
CREATE INDEX idx_alerts_open ON alerts (resolved_at, kind, server_id);
