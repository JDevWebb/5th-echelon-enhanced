-- 0.5.6's RL: launchers' pings to every relay of a network, one start's as a batch (a random
-- id), the relay it picked marked. Older rows (pings to directory servers) have neither.
ALTER TABLE player_pings ADD COLUMN picked INTEGER NOT NULL DEFAULT 0;
ALTER TABLE player_pings ADD COLUMN batch TEXT NOT NULL DEFAULT '';
