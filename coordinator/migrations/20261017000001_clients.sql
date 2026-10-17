-- The project's own client builds (signed releases): the SHA-256 of each client file of each
-- release, from its SHA256SUMS once the release key's signature checked out.
CREATE TABLE client_builds (
    version TEXT NOT NULL,
    asset TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    seen_at INTEGER NOT NULL,
    PRIMARY KEY (version, asset)
);

-- Identities allowed to sign in with other builds (developers): every member server honours
-- the list.
CREATE TABLE client_allow (
    identity TEXT PRIMARY KEY,
    note TEXT NOT NULL DEFAULT '',
    added_by TEXT NOT NULL,
    added_at INTEGER NOT NULL
);
