-- What a player's game uploaded (UserStorage, uploads.rs): the latest of each type, gzip,
-- until the coordinator has it (`sent`). Today only the ShadowNet companion snapshot.
CREATE TABLE player_content (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    type_id INTEGER NOT NULL,
    size INTEGER NOT NULL,
    gzip BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    sent INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, type_id)
) WITHOUT ROWID;
