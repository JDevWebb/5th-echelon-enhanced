-- Maintenance windows: a server (or, with no server, the whole network: the coordinator
-- itself) booked to be down from `starts` to `ends`. A notice for players; nothing is stopped.
CREATE TABLE maintenance (
    id INTEGER PRIMARY KEY,
    server_id TEXT REFERENCES servers(id) ON DELETE CASCADE,
    starts INTEGER NOT NULL,
    ends INTEGER NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    cancelled_at INTEGER
);
CREATE INDEX maintenance_ends ON maintenance (ends);

-- The project's roadmap, as admins keep it: lanes shipping, next, later and requested. Public
-- items are shown in players' launchers.
CREATE TABLE roadmap_items (
    id INTEGER PRIMARY KEY,
    lane TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    -- A JSON array of short tags.
    tags TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT '',
    position INTEGER NOT NULL DEFAULT 0,
    public INTEGER NOT NULL DEFAULT 0,
    -- Who asked for it, for admins ("Discord · Kiwi").
    source TEXT NOT NULL DEFAULT '',
    suggestion_id INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    updated_by TEXT NOT NULL DEFAULT ''
);

-- What players suggested from their launchers, signed with their identity.
CREATE TABLE suggestions (
    id INTEGER PRIMARY KEY,
    global_id TEXT NOT NULL,
    name TEXT NOT NULL,
    server TEXT NOT NULL DEFAULT '',
    launcher TEXT NOT NULL DEFAULT '',
    area TEXT NOT NULL,
    title TEXT NOT NULL,
    text TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    -- new, planned, done or declined; the admins' reply is shown to the player.
    status TEXT NOT NULL DEFAULT 'new',
    reply TEXT NOT NULL DEFAULT '',
    item_id INTEGER,
    updated_at INTEGER NOT NULL
);
CREATE INDEX suggestions_by_identity ON suggestions (global_id, created_at);
