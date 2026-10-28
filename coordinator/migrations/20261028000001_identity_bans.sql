-- Bans by identity, across the network (0.4.8): every account of a banned identity, on every
-- member server, is banned, including accounts it makes or links later. Until when (NULL: for
-- good), why (the player sees it), since when and by which admin.
CREATE TABLE identity_bans (
    identity TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    until INTEGER,
    at INTEGER NOT NULL,
    by TEXT NOT NULL
);
