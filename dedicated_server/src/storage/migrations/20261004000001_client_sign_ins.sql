-- What last signed in to each account through the API: when a current client (launcher or
-- game) last did, and when an outdated one last tried with the right password. The game's own
-- sign-in (LoginEx) is let through only after a current one, so a game with an old client DLL
-- can't play.
CREATE TABLE client_sign_ins (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    client TEXT NOT NULL DEFAULT '',
    current_at INTEGER,
    outdated_at INTEGER
);
