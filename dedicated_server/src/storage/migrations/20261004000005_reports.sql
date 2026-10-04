-- Players' reports (reports.rs) waiting for the coordinator, as it takes them (JSON), and
-- when each player sent theirs (a few a day at most).
CREATE TABLE report_outbox (
    id TEXT PRIMARY KEY,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE report_log (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    at INTEGER NOT NULL
);
CREATE INDEX report_log_user ON report_log (user_id, at);
