-- A random number per account that every API token names. Changing it (a new password, a
-- sign-in with the identity key) ends every token issued before; a new account that reuses a
-- deleted one's id gets its own, so the old tokens don't pass for it. Filled in at start.
ALTER TABLE users ADD COLUMN token_epoch INTEGER NOT NULL DEFAULT 0;
