-- Game tickets issued before this time (Unix seconds) no longer connect: set when the
-- password changes, on a ban, and when the account is made (an id used again after a
-- deletion mustn't take the old account's tickets).
ALTER TABLE users ADD COLUMN tickets_after INTEGER NOT NULL DEFAULT 0;
