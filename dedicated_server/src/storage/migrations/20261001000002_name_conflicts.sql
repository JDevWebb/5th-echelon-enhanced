-- Set when this account's name belongs to another player on the servers sharing friends (the
-- coordinator said so when it linked). The player is asked to rename.
ALTER TABLE users ADD COLUMN name_conflict INTEGER NOT NULL DEFAULT 0;
