-- Set when the network's admins find this account's name breaks the rules (slurs, hate, a web
-- address): the player is asked to choose another, and renaming clears it.
ALTER TABLE users ADD COLUMN name_not_allowed INTEGER NOT NULL DEFAULT 0;
