-- Each member server's players, as it reports them (POST /v1/players): for the admin UI's
-- player list. `identity` is the player's global id, their identity's public key (the same person has the same
-- one on every server), or NULL for an account without one.
CREATE TABLE players (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    id INTEGER NOT NULL,
    name TEXT NOT NULL,
    identity TEXT,
    created_at INTEGER,
    last_seen INTEGER,
    online INTEGER NOT NULL DEFAULT 0,
    play_seconds INTEGER NOT NULL DEFAULT 0,
    sessions INTEGER NOT NULL DEFAULT 0,
    matches INTEGER NOT NULL DEFAULT 0,
    -- A ban: why, until when (NULL: for good), and since when. NULL `banned_at`: not banned.
    banned_reason TEXT,
    banned_until INTEGER,
    banned_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, id)
);
CREATE INDEX idx_players_identity ON players (identity);
CREATE INDEX idx_players_last_seen ON players (last_seen);

-- Play sessions (a game connection from sign-in to sign-out; `ended` NULL while still playing),
-- kept 400 days: what time played is counted from.
CREATE TABLE play_sessions (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    id INTEGER NOT NULL,
    player INTEGER NOT NULL,
    started INTEGER NOT NULL,
    ended INTEGER,
    PRIMARY KEY (server_id, id)
);
CREATE INDEX idx_play_sessions_player ON play_sessions (server_id, player, started);
CREATE INDEX idx_play_sessions_started ON play_sessions (started);

-- Finished matches, from each minute's metrics, kept 400 days. The same match sent twice (a
-- retried report) is one row.
CREATE TABLE matches (
    id INTEGER PRIMARY KEY,
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    mode TEXT NOT NULL,
    map INTEGER NOT NULL,
    game_mode INTEGER NOT NULL,
    started INTEGER NOT NULL,
    ended INTEGER NOT NULL,
    players INTEGER NOT NULL,
    private INTEGER NOT NULL DEFAULT 0,
    UNIQUE (server_id, mode, map, game_mode, started, ended)
);
CREATE INDEX idx_matches_ended ON matches (ended);

-- What admins asked a server to do to a player (ban, unban, kick, reset_password, rename,
-- delete). Sent with the server's pulse until it answers; given up after an hour. A reset
-- password is kept only until the admin who asked reads it once.
CREATE TABLE player_actions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    player INTEGER NOT NULL,
    kind TEXT NOT NULL,
    args TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    -- pending, done, failed or expired.
    status TEXT NOT NULL DEFAULT 'pending',
    message TEXT,
    password TEXT,
    done_at INTEGER
);
CREATE INDEX idx_player_actions_pending ON player_actions (server_id, status, id);
CREATE INDEX idx_player_actions_player ON player_actions (server_id, player, id);

-- The servers' live points (from their pulses) and the live events made from them, kept a
-- day, so a restart of the coordinator doesn't empty the live view.
CREATE TABLE pulses (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    point TEXT NOT NULL,
    PRIMARY KEY (server_id, at)
);
CREATE TABLE live_feed (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    event TEXT NOT NULL
);
CREATE INDEX idx_live_feed_at ON live_feed (at);
