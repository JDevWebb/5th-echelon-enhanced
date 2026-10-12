-- Everyone who has been in a session (a participant or a guest), kept while the session is.
-- The game takes players off a party's participants while they're still in its party, and their
-- games later split the party to carry it on; only someone who was never in a session may not
-- (eu1 2026-10-12: five members of one party refused at once, and the party broke up).
CREATE TABLE past_members (
    game_id INTEGER NOT NULL REFERENCES game_sessions(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (game_id, user_id)
) WITHOUT ROWID;
