-- Play sessions: a game connection, from sign-in to sign-out. `seen_at` moves on every
-- minute while it lasts, so a session the server never saw end (a restart) ends there.
-- A sign-in within a couple of minutes of the last sign-out carries on that session.
CREATE TABLE play_sessions (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    started_at INTEGER NOT NULL,
    seen_at INTEGER NOT NULL,
    ended_at INTEGER,
    -- When it last changed, for sending the changes to the coordinator.
    changed_at INTEGER NOT NULL
);
CREATE INDEX play_sessions_user ON play_sessions (user_id, started_at);
CREATE INDEX play_sessions_changed ON play_sessions (changed_at);

-- Players an admin banned: they can't sign in until `until` (never, when NULL).
CREATE TABLE bans (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    reason TEXT NOT NULL DEFAULT '',
    until INTEGER,
    created_at INTEGER NOT NULL
);

-- Who was in each match, for the players count of a match and each player's matches.
CREATE TABLE match_players (
    game_id INTEGER NOT NULL REFERENCES game_sessions(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (game_id, user_id)
) WITHOUT ROWID;
ALTER TABLE game_sessions ADD COLUMN peak_players INTEGER NOT NULL DEFAULT 0;
-- Set once the finished match went to the coordinator.
ALTER TABLE game_sessions ADD COLUMN reported INTEGER NOT NULL DEFAULT 0;
-- Matches with at least one other player.
ALTER TABLE users ADD COLUMN matches_played INTEGER NOT NULL DEFAULT 0;
