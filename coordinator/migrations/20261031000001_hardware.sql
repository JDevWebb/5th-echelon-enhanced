-- Players' PCs (0.5.0, F10): each hardware id a player's game or launcher sent, as its hash
-- keyed again here with the coordinator's own secret (never the value, nor the client's hash).
-- Kept while seen in the last 180 days, for as long as a ban of the player runs, and deleted
-- with the account.
CREATE TABLE player_hardware (
    server_id TEXT NOT NULL,
    player INTEGER NOT NULL,
    kind TEXT NOT NULL,
    hash TEXT NOT NULL,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    PRIMARY KEY (server_id, player, kind, hash)
);
CREATE INDEX idx_player_hardware_hash ON player_hardware (hash);
CREATE INDEX idx_player_hardware_last_seen ON player_hardware (last_seen);

-- A player's PC is a virtual machine: its maker ("VMware").
ALTER TABLE players ADD COLUMN vm TEXT;

-- What a player's PC was found to share with a banned player's: 'ban' (two ids or more: the
-- player was banned for it), 'flag' (one, for the admins), or 'vm' (a new account on a virtual
-- machine, for the admins; banned_server '' and banned_player 0).
CREATE TABLE hardware_matches (
    server_id TEXT NOT NULL,
    player INTEGER NOT NULL,
    banned_server TEXT NOT NULL,
    banned_player INTEGER NOT NULL,
    matched INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (server_id, player, outcome, banned_server, banned_player)
);
