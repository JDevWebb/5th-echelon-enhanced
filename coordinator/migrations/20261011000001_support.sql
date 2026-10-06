-- Support: one conversation per player (their identity) with the network's admins, from the
-- launcher (support.rs). Messages both ways; a player's may carry files (gzip, as sent).
CREATE TABLE support_threads (
    identity TEXT PRIMARY KEY,
    -- The player's name as they last sent it, and where from (server host, launcher version).
    name TEXT NOT NULL,
    server TEXT NOT NULL DEFAULT '',
    launcher TEXT NOT NULL DEFAULT '',
    -- open (the admins' turn), waiting (the player's), resolved.
    status TEXT NOT NULL DEFAULT 'open',
    updated_at INTEGER NOT NULL,
    -- The last message each side has read, by id.
    player_read INTEGER NOT NULL DEFAULT 0,
    admin_read INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_support_threads_updated ON support_threads (status, updated_at);

CREATE TABLE support_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    identity TEXT NOT NULL REFERENCES support_threads(identity) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    -- NULL from the player; the admin's name from an admin.
    admin TEXT,
    body TEXT NOT NULL
);
CREATE INDEX idx_support_messages_thread ON support_messages (identity, id);

CREATE TABLE support_files (
    message_id INTEGER NOT NULL REFERENCES support_messages(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    size INTEGER NOT NULL,
    gzip BLOB NOT NULL,
    PRIMARY KEY (message_id, name)
);
