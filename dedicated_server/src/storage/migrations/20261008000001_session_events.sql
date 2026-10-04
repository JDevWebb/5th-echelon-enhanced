-- What happened in players' time on the server (session_events.rs), until the coordinator
-- has it. A repeat of the same thing soon after is the same row, counted; `sent` is cleared
-- when it changes, so the coordinator gets the new count too.
CREATE TABLE session_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at INTEGER NOT NULL,
    last_at INTEGER NOT NULL,
    user_id INTEGER,
    name TEXT,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 1,
    sent INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX session_events_unsent ON session_events (sent, id);
CREATE INDEX session_events_repeat ON session_events (kind, user_id, last_at);
CREATE INDEX session_events_at ON session_events (at);
-- Finished matches the coordinator hasn't taken yet stay until it has.
CREATE INDEX game_sessions_unreported ON game_sessions (reported, destroyed_at);
