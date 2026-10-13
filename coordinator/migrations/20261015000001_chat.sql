-- Chat on the community network (docs/chat.md): every room's messages are kept here, the
-- coordinator's; member servers pass their players' messages on and fetch what's for them.
--
-- room: global, direct or lobby. conv: the conversation, "global", "direct:<a>|<b>" (the two
-- identities, a < b) or "lobby:<server>:<session>". Senders by identity (global id), with the
-- name they had and their server. Kept: global 7 days (the last 500), direct 30 days (the last
-- 200 a conversation), lobby 2 hours; deleted with the account's links.
CREATE TABLE chat_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    room TEXT NOT NULL,
    conv TEXT NOT NULL,
    at INTEGER NOT NULL,
    sender TEXT NOT NULL,
    sender_name TEXT NOT NULL,
    server TEXT NOT NULL,
    -- The other identity, for a direct message.
    recipient TEXT,
    body TEXT NOT NULL,
    -- Words masked in it (counted per player for the admins; the words aren't kept).
    masked INTEGER NOT NULL DEFAULT 0,
    deleted_at INTEGER,
    deleted_by TEXT
);
CREATE INDEX idx_chat_conv ON chat_messages (conv, id);
CREATE INDEX idx_chat_sender ON chat_messages (sender, at);
CREATE INDEX idx_chat_recipient ON chat_messages (recipient, id);

-- How far each player has read each conversation (direct messages' unread counts).
CREATE TABLE chat_reads (
    global_id TEXT NOT NULL,
    conv TEXT NOT NULL,
    up_to INTEGER NOT NULL,
    PRIMARY KEY (global_id, conv)
) WITHOUT ROWID;

-- Mutes by the admins (none are automatic): room "*" is everywhere; until NULL is for good.
CREATE TABLE chat_mutes (
    global_id TEXT NOT NULL,
    room TEXT NOT NULL,
    until INTEGER,
    by_admin TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    at INTEGER NOT NULL,
    PRIMARY KEY (global_id, room)
) WITHOUT ROWID;

-- Players' reports of messages; dealt with on the admin UI's moderation page. Kept 90 days after.
CREATE TABLE chat_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL,
    reporter TEXT NOT NULL,
    reporter_server TEXT NOT NULL,
    why TEXT NOT NULL,
    at INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    resolved_by TEXT,
    resolved_at INTEGER,
    note TEXT NOT NULL DEFAULT '',
    UNIQUE (message_id, reporter)
);
CREATE INDEX idx_chat_reports_status ON chat_reports (status, at);

-- Messages refused for a link, with why, for the admins: kept 7 days.
CREATE TABLE chat_refused (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at INTEGER NOT NULL,
    sender TEXT NOT NULL,
    sender_name TEXT NOT NULL,
    server TEXT NOT NULL,
    room TEXT NOT NULL,
    reason TEXT NOT NULL,
    body TEXT NOT NULL
);

-- Masked words a day, per player (the admins' "most masked this week"); kept 30 days.
CREATE TABLE chat_masked (
    global_id TEXT NOT NULL,
    day INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (global_id, day)
) WITHOUT ROWID;
