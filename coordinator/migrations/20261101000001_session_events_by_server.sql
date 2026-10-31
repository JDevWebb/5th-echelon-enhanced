-- The Sessions page for one server reads its events by when they last happened (R14).
CREATE INDEX IF NOT EXISTS idx_session_events_server_last_at ON session_events (server_id, last_at);
