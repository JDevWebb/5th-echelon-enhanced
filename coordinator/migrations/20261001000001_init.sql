-- Member servers. Each joined with the join token and got a secret; only its hash is kept.
CREATE TABLE servers (
    id TEXT PRIMARY KEY,
    secret_hash TEXT NOT NULL UNIQUE,
    -- The directory entry the server last sent (JSON), and when.
    listing TEXT,
    last_seen INTEGER,
    joined_at INTEGER NOT NULL
);

-- Accounts linked to a player's identity (their public key), one per server. Each link was
-- signed by the player, and the signature checked here.
CREATE TABLE links (
    global_id TEXT NOT NULL,
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    username TEXT NOT NULL,
    linked_at INTEGER NOT NULL,
    PRIMARY KEY (global_id, server_id)
);
CREATE INDEX idx_links_server ON links (server_id, username);

-- Friendships between identities, a < b. `friends` 0 is kept, so a server learns about the end
-- of a friendship too. The latest change wins.
CREATE TABLE friendships (
    a TEXT NOT NULL,
    b TEXT NOT NULL,
    friends INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    PRIMARY KEY (a, b)
);
CREATE INDEX idx_friendships_b ON friendships (b);

-- Blocks, one direction each: from_id blocked to_id (blocked 0: unblocked).
CREATE TABLE blocks (
    from_id TEXT NOT NULL,
    to_id TEXT NOT NULL,
    blocked INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    PRIMARY KEY (from_id, to_id)
);
CREATE INDEX idx_blocks_to ON blocks (to_id);
