-- An admin's "Update every server now": servers install the rollout's release at once, players
-- on or not (0.5.4).
ALTER TABLE rollout ADD COLUMN forced INTEGER NOT NULL DEFAULT 0;
