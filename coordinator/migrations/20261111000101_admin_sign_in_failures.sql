-- Failed admin sign-ins per account and address (IPv4, or an IPv6 /64): a few lock that
-- address out of that account for a while, so a stranger's failures don't lock out the admin.
-- admins.failures and locked_until stay, as the backstop for all addresses together.
CREATE TABLE admin_sign_in_failures (
    admin_id INTEGER NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    address TEXT NOT NULL,
    failures INTEGER NOT NULL DEFAULT 0,
    locked_until INTEGER NOT NULL DEFAULT 0,
    last_at INTEGER NOT NULL,
    PRIMARY KEY (admin_id, address)
);
