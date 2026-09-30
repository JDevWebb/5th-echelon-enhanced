-- Names reserved across the group: each belongs to one identity, so "Kiwi" is the same player
-- on every member server. Claimed when an account is made or linked (first come, first
-- served), released when the identity no longer uses it anywhere.
CREATE TABLE names (
    name_key TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    global_id TEXT NOT NULL,
    claimed_at INTEGER NOT NULL
);
CREATE INDEX idx_names_global_id ON names (global_id);
