-- The name the network's admins checked and allowed (a false positive of the name rules): the
-- server's check on start leaves the account alone while it still has this name.
ALTER TABLE users ADD COLUMN name_approved TEXT;
