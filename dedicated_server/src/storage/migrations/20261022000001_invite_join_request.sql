-- An invitation a join request queued (Friends.JoinNow): the joiner asked for it, the host
-- didn't send it. Delivered as a join the player asked for, and it holds only while its room
-- is a party or a public match, whoever it's from.
ALTER TABLE invites ADD COLUMN join_request INTEGER NOT NULL DEFAULT 0;
