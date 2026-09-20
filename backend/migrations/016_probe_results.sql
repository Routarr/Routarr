-- What the last probe found, so the shell can report it without probing.
--
-- The top bar and the navigation read `/status`, which answers from the
-- database in milliseconds; probing an unreachable host costs a full connect
-- timeout and that endpoint is polled. So the two screens disagreed by
-- construction: the dashboard probed and reported a source that had stopped
-- answering, and the navigation, unable to know, counted zero beside it.
--
-- The subject is namespaced (`source:anilist`, `instance:<id>`) rather than a
-- foreign key: a metadata source has no row of its own anywhere, being a
-- static catalogue entry plus a setting.
CREATE TABLE probe_results (
    subject TEXT PRIMARY KEY,
    reachable INTEGER NOT NULL,
    -- The phrase the probe came back with, since the message names it. Already
    -- what the diagnostics table shows, so it exposes nothing new.
    detail TEXT,
    checked_at TEXT NOT NULL
);
