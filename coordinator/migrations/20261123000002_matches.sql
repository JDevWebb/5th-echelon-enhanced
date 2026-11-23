-- 0.5.6's RP4 and RP5: every match the servers' events tell of, kept for good (small: a row a
-- match, a row a player), filled as the events come in. A match is a server's room and when
-- the room was made (`since`), as the admin UI's Match page names it.
CREATE TABLE played_matches (
    server_id TEXT NOT NULL,
    room INTEGER NOT NULL,
    since INTEGER NOT NULL,
    -- First and last event seen of it, and when it ended (its last player left), if it did.
    first_at INTEGER NOT NULL,
    last_at INTEGER NOT NULL,
    ended_at INTEGER,
    mode TEXT NOT NULL DEFAULT '',
    game_mode INTEGER,
    map INTEGER,
    host INTEGER,
    host_name TEXT NOT NULL DEFAULT '',
    private INTEGER NOT NULL DEFAULT 0,
    -- The furthest it got: 1 once a round or mission was under way.
    under_way INTEGER NOT NULL DEFAULT 0,
    -- Its rounds as the host's game read each one's end (`round_result`): a JSON list of
    -- {round, winner_team, winner_player, hacks, score, at}.
    rounds TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (server_id, room, since)
);
CREATE INDEX idx_played_matches_since ON played_matches (since);
CREATE TABLE played_players (
    server_id TEXT NOT NULL,
    room INTEGER NOT NULL,
    since INTEGER NOT NULL,
    player INTEGER NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    first_at INTEGER NOT NULL,
    last_at INTEGER NOT NULL,
    -- How they left: "left", "removed", "timed_out", "abandoned"... (the leave event's), if seen.
    how TEXT,
    PRIMARY KEY (server_id, room, since, player)
);
CREATE INDEX idx_played_players_player ON played_players (server_id, player);
-- Each player's result as their game's stats write gave it (`stats` events with a room): the
-- write's values (kills, deaths, assists, shots, hits, headshots, wins, ties, losses, score,
-- time...). A match a player wrote twice for keeps the sum of their writes, and how many.
CREATE TABLE match_results (
    server_id TEXT NOT NULL,
    room INTEGER NOT NULL,
    since INTEGER NOT NULL,
    player INTEGER NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    writes INTEGER NOT NULL DEFAULT 0,
    result TEXT NOT NULL DEFAULT '{}',
    last_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, room, since, player)
);
