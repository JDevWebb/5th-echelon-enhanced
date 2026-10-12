-- A maintenance window's purpose: 'update' (the rollout installs releases then, and only then)
-- or 'maintenance' (anything else: no update starts during it).
ALTER TABLE maintenance ADD COLUMN kind TEXT NOT NULL DEFAULT 'maintenance';
-- An admin let the rollout go out without an update window (an urgent fix, or a rollback).
ALTER TABLE rollout ADD COLUMN anytime INTEGER NOT NULL DEFAULT 0;
