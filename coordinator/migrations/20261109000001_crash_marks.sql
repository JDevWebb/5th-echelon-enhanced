-- The Crashes page's marks (crashes.rs): a crash group, by its signature (where it happened,
-- "freeze at ..." for a freeze), marked fixed (in a release), known (nothing to do) or
-- watching. Seeded with what 0.5.0 fixes and what the 2026-11-09 review looked at.
CREATE TABLE crash_marks (
    signature TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    fixed_in TEXT NOT NULL DEFAULT '',
    note TEXT NOT NULL DEFAULT '',
    marked_by TEXT NOT NULL DEFAULT '',
    marked_at INTEGER NOT NULL
);
INSERT INTO crash_marks (signature, status, fixed_in, note, marked_by, marked_at) VALUES
    ('Blacklist_DX11_game.exe+0x504e8', 'fixed', '0.5.0', 'Back to the lobby after a match: a package read the recorded order cannot place (umd-read-fix)', 'setup', 1794182400),
    ('Blacklist_DX11_game.exe+0xa72c73', 'fixed', '0.5.0', 'A player''s navigation disrupted while holding no item (disruption-fix)', 'setup', 1794182400),
    ('Blacklist_DX11_game.exe+0x6a88ce', 'fixed', '0.5.0', 'A destroyed tagging device''s reset (tagging-device-fix)', 'setup', 1794182400),
    ('Blacklist_DX11_game.exe+0x6f205a', 'fixed', '0.5.0', 'Co-op: the AI''s zone test with a player without a character (perception-fix)', 'setup', 1794182400),
    ('Blacklist_DX11_game.exe+0xb116b5', 'fixed', '0.5.0', 'Customize Gear and the hub: a gear item with no object, left out and its name logged (gear-list-fix); which item, not known yet', 'setup', 1794182400),
    ('xaudio2.dll+0x3da72', 'known', '', 'The audio driver', 'setup', 1794182400),
    ('Blacklist_DX11_game.exe+0x1a41f20', 'watching', '', 'Sound engine: an attenuation curve''s stale keys (once, 2026-11-08); no safe guard', 'setup', 1794182400);
