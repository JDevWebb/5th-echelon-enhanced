-- Whose each waiting report is, so one player's can't take the whole outbox (reports.rs
-- MAX_OUTBOX_PER_PLAYER). Reports queued before this have none and count for nobody.
ALTER TABLE report_outbox ADD COLUMN user_id INTEGER;
