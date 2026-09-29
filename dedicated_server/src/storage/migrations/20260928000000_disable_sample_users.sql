-- 5th Echelon Enhanced: upstream's sample accounts ship in every database.
-- Keep the rows (ids are referenced) but remove their credentials, so nobody
-- can log in as them. 'Tracking' (105) stays: it is the game's own telemetry
-- login.
UPDATE users SET password = NULL, password_hash = NULL WHERE id IN (1000, 1002);
