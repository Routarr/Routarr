-- The interface language has a default the code applied and the table never
-- held: a fresh installation answered "" for it. Seeded like the other
-- defaults, so every reader sees the same value.
INSERT OR IGNORE INTO settings (key, value) VALUES ('ui_language', 'en');
