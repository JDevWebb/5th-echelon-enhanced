-- Players moved to the relay because their joins kept failing (J2), with the address they were
-- at: on that network they start relayed until `until` (unix seconds), not direct again at
-- every sign-in (0.5.5's JC2).
CREATE TABLE relay_moves (
    user_id INTEGER PRIMARY KEY NOT NULL,
    ip TEXT NOT NULL,
    why TEXT NOT NULL,
    until INTEGER NOT NULL
);
