-- Players' feedback and problem reports (POST /v1/reports): the launcher asks after a session,
-- the player's server adds its own log lines and forwards it here. Kept 90 days. The files
-- (the player's logs, redacted on their PC) are on disk under reports/<id>/<name>.gz, gzip as
-- received; when they're over the storage cap the oldest reports' files go first, and
-- `dropped` says so (the rest of the report stays).
CREATE TABLE player_reports (
    id TEXT PRIMARY KEY,
    server_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    received_at INTEGER NOT NULL,
    player_id INTEGER NOT NULL,
    player_name TEXT NOT NULL,
    player_identity TEXT,
    rating TEXT,
    -- JSON arrays and objects, as checked.
    problems TEXT NOT NULL,
    triggers TEXT NOT NULL,
    client TEXT NOT NULL,
    summary TEXT NOT NULL,
    comment TEXT NOT NULL,
    server_log TEXT NOT NULL,
    -- open or resolved.
    status TEXT NOT NULL DEFAULT 'open',
    note TEXT NOT NULL DEFAULT '',
    resolved_by TEXT,
    resolved_at INTEGER
);
CREATE INDEX idx_player_reports_received ON player_reports (received_at);
CREATE INDEX idx_player_reports_status ON player_reports (status, created_at);
CREATE INDEX idx_player_reports_player ON player_reports (server_id, player_id);
CREATE INDEX idx_player_reports_identity ON player_reports (player_identity);

CREATE TABLE player_report_files (
    report_id TEXT NOT NULL REFERENCES player_reports(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    -- Uncompressed, and as stored (gzip).
    size INTEGER NOT NULL,
    stored INTEGER NOT NULL,
    dropped INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (report_id, name)
);
