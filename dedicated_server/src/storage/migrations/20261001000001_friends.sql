-- Real friend lists, blocking, and one name per person regardless of case.
--
-- Until now every account on a server was everyone's friend. `relationships` holds what each
-- player chose instead, one row per direction:
--
--   request  user_id asked other_id to be friends (pending)
--   friend   they are friends (always both directions)
--   block    user_id blocked other_id: no invites, requests or presence either way
--
-- A player blocking someone keeps only their own block row; everything else between the two
-- goes.
CREATE TABLE relationships (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    other_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('request', 'friend', 'block')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, other_id)
);
CREATE INDEX idx_relationships_other ON relationships (other_id, kind);

-- Things to tell a player about (a friend request, a request accepted), handed out one at a
-- time through the API's event poll, like invites.
CREATE TABLE friend_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    other_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('request', 'accepted')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_friend_events_user ON friend_events (user_id, id);

-- Upstream's third sample account still had its sample password (20260928000000 disabled only
-- 1000 and 1002), so anyone who knew it could sign in as AAAABBBB.
UPDATE users SET password = NULL, password_hash = NULL WHERE id = 1001 AND username = 'AAAABBBB';

-- One account per name, whatever the case: "Kiwi" and "kiwi" were two accounts before.
-- Later duplicates get "-<id>" appended (the oldest keeps the name). name_key is the lower-cased
-- name, recomputed at start with full Unicode rules (SQLite's lower() only knows ASCII).
UPDATE users SET username = username || '-' || id
WHERE id NOT IN (SELECT MIN(id) FROM users GROUP BY lower(username));
ALTER TABLE users ADD COLUMN name_key TEXT;
UPDATE users SET name_key = lower(username);
CREATE UNIQUE INDEX idx_users_name_key ON users (name_key);

-- The player's identity across servers: the public key their launcher holds (see
-- docs/friends.md). Proved with a signature when linked, so no one can claim another's.
ALTER TABLE users ADD COLUMN global_id TEXT;
CREATE UNIQUE INDEX idx_users_global_id ON users (global_id);
-- The time of the last sign-in with that key; each must be newer, so none can be replayed.
ALTER TABLE users ADD COLUMN last_key_login INTEGER;

-- Invites now cascade when an account is deleted (they referenced users without it, so
-- deleting someone with a pending invite failed). Invites are transient: nothing to copy.
DROP INDEX IF EXISTS idx_invites_pending_receiver;
DROP TABLE invites;
CREATE TABLE invites (
    id INTEGER PRIMARY KEY,
    sender INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    receiver INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created DATETIME DEFAULT CURRENT_TIMESTAMP,
    session_type INTEGER,
    session_id INTEGER,
    delivered_at DATETIME,
    consumed_at DATETIME,
    expires_at DATETIME
);
CREATE INDEX idx_invites_pending_receiver ON invites (receiver, session_type, consumed_at, expires_at);

-- Friend changes waiting to reach the coordinator (federation.rs), oldest first.
CREATE TABLE federation_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
