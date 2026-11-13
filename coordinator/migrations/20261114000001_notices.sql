-- 0.5.2's U: urgent notices to every player ("a hotfix is coming", "we're looking into connection
-- problems"), posted from the admin UI and shown by every launcher while live.
CREATE TABLE notices (
    id INTEGER PRIMARY KEY,
    text TEXT NOT NULL,
    -- info, issue (something's wrong and we know) or fixed.
    kind TEXT NOT NULL,
    posted_by TEXT NOT NULL,
    posted_at INTEGER NOT NULL,
    edited_at INTEGER,
    -- When it stops showing by itself (none: until ended).
    ends_at INTEGER,
    ended_at INTEGER
);
