-- The newest game session when each participant joined: a host's party follows them into a
-- private match only if they were in the party before the match was made (game_session.rs).
-- Session ids only grow, so this orders the two exactly (a clock in seconds can't). Rows from
-- before this were in the party, as far as anyone can tell.
ALTER TABLE participants ADD COLUMN joined_after INTEGER;
