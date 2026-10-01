-- Releases seen on GitHub whose SHA256SUMS carries the release key's signature (unsigned ones
-- are never recorded, so never rolled out).
CREATE TABLE releases (
    version TEXT PRIMARY KEY,
    page TEXT NOT NULL,
    published_at TEXT NOT NULL,
    seen_at INTEGER NOT NULL
);

-- The rollout of `target` to every member server (one row). Stages: idle, canary (one server
-- first), rolling (the rest), done, halted (the canary failed, or an admin stopped it).
CREATE TABLE rollout (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    target TEXT,
    previous TEXT,
    stage TEXT NOT NULL DEFAULT 'idle',
    canary TEXT,
    stage_since INTEGER NOT NULL DEFAULT 0,
    paused INTEGER NOT NULL DEFAULT 0,
    -- Pinned: new releases aren't rolled out on their own.
    pinned INTEGER NOT NULL DEFAULT 0,
    note TEXT NOT NULL DEFAULT ''
);
INSERT INTO rollout (id) VALUES (1);

-- What each server last reported about updates (its updater's status).
ALTER TABLE servers ADD COLUMN update_status TEXT;

-- Each server's metrics, as sent every minute (kept a week), and per hour (kept 400 days).
CREATE TABLE samples (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (server_id, at)
);
CREATE INDEX idx_samples_at ON samples (at);
CREATE TABLE hourly (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    hour INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (server_id, hour)
);
CREATE INDEX idx_hourly_hour ON hourly (hour);

-- Round trips from the coordinator to each server (NULL: no answer).
CREATE TABLE server_pings (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    ms REAL,
    PRIMARY KEY (server_id, at)
);
-- Players' pings to each server, as launchers measured them, with where the player was (city,
-- from DB-IP; the address isn't kept).
CREATE TABLE player_pings (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    country TEXT NOT NULL,
    city TEXT NOT NULL,
    ms INTEGER NOT NULL
);
CREATE INDEX idx_player_pings ON player_pings (server_id, at);

-- Names for the game's map and mode ids, as admins identify them.
CREATE TABLE labels (
    kind TEXT NOT NULL,
    id INTEGER NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (kind, id)
);

-- Admins of the coordinator's web UI.
CREATE TABLE admins (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT,
    -- TOTP secret (base32), once confirmed; the last time step used, so a code works once.
    totp_secret TEXT,
    totp_pending TEXT,
    totp_last_step INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    last_login INTEGER,
    disabled INTEGER NOT NULL DEFAULT 0,
    -- Failed sign-ins in a row, and until when sign-in is refused.
    failures INTEGER NOT NULL DEFAULT 0,
    locked_until INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE passkeys (
    id TEXT PRIMARY KEY,
    admin_id INTEGER NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    alg INTEGER NOT NULL,
    public_key BLOB NOT NULL,
    sign_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    last_used INTEGER
);
CREATE TABLE recovery_codes (
    admin_id INTEGER NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    used_at INTEGER,
    PRIMARY KEY (admin_id, code_hash)
);
-- Sessions: only the token's hash is kept. Stage: password (a second factor to go), enroll
-- (signed in by a setup link: may only add a second factor), full.
CREATE TABLE admin_sessions (
    token_hash TEXT PRIMARY KEY,
    admin_id INTEGER NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    stage TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    -- When a second factor was last proved: sensitive changes want it recent.
    verified_at INTEGER NOT NULL DEFAULT 0,
    ip TEXT NOT NULL,
    country TEXT NOT NULL,
    user_agent TEXT NOT NULL,
    totp_tries INTEGER NOT NULL DEFAULT 0
);
-- One-time links that let a new admin (or one locked out) set a password and a second factor.
CREATE TABLE setup_tokens (
    token_hash TEXT PRIMARY KEY,
    admin_id INTEGER NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
);
-- WebAuthn challenges, each used once.
CREATE TABLE webauthn_challenges (
    id TEXT PRIMARY KEY,
    admin_id INTEGER,
    kind TEXT NOT NULL,
    challenge TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE TABLE audit (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    admin TEXT NOT NULL,
    ip TEXT NOT NULL,
    country TEXT NOT NULL,
    event TEXT NOT NULL,
    detail TEXT NOT NULL
);
CREATE INDEX idx_audit_at ON audit (at);
-- Admin UI settings, e.g. where admins may sign in from.
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
