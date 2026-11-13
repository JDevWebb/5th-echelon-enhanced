-- 0.5.2: a window can close the server (or the network) to all but testers: sign-ins and the
-- online functions are refused in it. Windows booked before stay notices only.
ALTER TABLE maintenance ADD COLUMN closed INTEGER NOT NULL DEFAULT 0;

-- Accounts that may play while a window keeps everyone else out (admins test with them).
CREATE TABLE maintenance_testers (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    player INTEGER NOT NULL,
    added_by TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, player)
);
