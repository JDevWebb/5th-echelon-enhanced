-- What players' games uploaded (servers' `POST /v1/content`): each player's latest of each
-- type, gzip, for the admins only. Today only the ShadowNet companion snapshot (loadouts,
-- purchases, challenge progress).
CREATE TABLE player_content (
    server_id TEXT NOT NULL,
    player_id INTEGER NOT NULL,
    type_id INTEGER NOT NULL,
    player_name TEXT NOT NULL,
    player_identity TEXT,
    size INTEGER NOT NULL,
    gzip BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    received_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, player_id, type_id)
) WITHOUT ROWID;
