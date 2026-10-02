-- Where each identity has been reported online, by which server. Links made before this was
-- recorded count as seen there (a player links while connected).
CREATE TABLE seen_online (
    global_id TEXT NOT NULL,
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    first_seen INTEGER NOT NULL,
    PRIMARY KEY (global_id, server_id)
);
CREATE INDEX idx_seen_online_server ON seen_online (server_id);
INSERT OR IGNORE INTO seen_online (global_id, server_id, first_seen) SELECT global_id, server_id, linked_at FROM links;

-- The servers that said two identities are friends (a < b, as in friendships), while they
-- are. '*': a friendship from before this was recorded.
CREATE TABLE friendship_servers (
    a TEXT NOT NULL,
    b TEXT NOT NULL,
    server_id TEXT NOT NULL,
    PRIMARY KEY (a, b, server_id)
);
INSERT INTO friendship_servers (a, b, server_id) SELECT a, b, '*' FROM friendships WHERE friends = 1;
