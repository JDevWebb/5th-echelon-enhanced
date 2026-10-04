-- Players' session events from each member server (POST /v1/events; the server's
-- session_events.rs says what each kind means), kept 30 days: for the admin UI's Sessions
-- page. `id` is the server's own; a repeat it counted comes again with the same id.
CREATE TABLE session_events (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    id INTEGER NOT NULL,
    at INTEGER NOT NULL,
    last_at INTEGER NOT NULL,
    player INTEGER,
    name TEXT,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (server_id, id)
);
CREATE INDEX idx_session_events_at ON session_events (at);
CREATE INDEX idx_session_events_last_at ON session_events (last_at);
