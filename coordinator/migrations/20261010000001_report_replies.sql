-- An admin's reply to a report, which the player reads in their launcher (unlike the note,
-- which only admins see).
ALTER TABLE player_reports ADD COLUMN reply TEXT NOT NULL DEFAULT '';
ALTER TABLE player_reports ADD COLUMN replied_by TEXT;
ALTER TABLE player_reports ADD COLUMN replied_at INTEGER;
