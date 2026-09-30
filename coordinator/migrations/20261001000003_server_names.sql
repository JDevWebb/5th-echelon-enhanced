-- The host names (and addresses) players reach each member server by. Players sign the host
-- they connected to; a link or name claim is only accepted from the server that owns that
-- host, so a server can't pass off signatures players made for another. First come, first
-- served: a name another server registered is not taken over.
CREATE TABLE server_names (
    name TEXT PRIMARY KEY,
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE
);

-- Which server claimed a name (for the per-server claim limit).
ALTER TABLE names ADD COLUMN server_id TEXT;
