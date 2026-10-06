-- Players who joined a session with JoinSession (a public match found by a search, or an
-- invitation), which the game never makes participants: its host doesn't add them. Kept
-- apart from participants so they get no say over the session (participants may change it)
-- and don't keep it alive once its host leaves; only who's in it (presence, the live
-- numbers) reads them.
CREATE TABLE guests (
    game_id INTEGER NOT NULL REFERENCES game_sessions(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (game_id, user_id)
) WITHOUT ROWID;
CREATE INDEX guests_user ON guests (user_id);
