-- Why an API check of a server failed: 'lookup' (the coordinator couldn't look its name up)
-- or 'connect' (no answer). NULL when it answered, and on rows from before this column, where
-- a failure was either.
ALTER TABLE server_pings ADD COLUMN failure TEXT;
