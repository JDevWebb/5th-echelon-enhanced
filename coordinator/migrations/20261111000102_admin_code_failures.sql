-- Wrong second-factor codes (TOTP and recovery) for an account from everywhere together: a
-- few lock the codes for a while. Forgotten a day after the last, or when the admin signs in
-- from the address the last came from; not by a sign-in from elsewhere.
ALTER TABLE admins ADD COLUMN code_failures INTEGER NOT NULL DEFAULT 0;
ALTER TABLE admins ADD COLUMN code_failed_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE admins ADD COLUMN code_failed_from TEXT NOT NULL DEFAULT '';
ALTER TABLE admins ADD COLUMN code_locked_until INTEGER NOT NULL DEFAULT 0;
