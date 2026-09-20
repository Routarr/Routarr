-- When a root folder last answered, as opposed to when we last heard of it.
--
-- `last_synced_at` is stamped on every row a sync writes, including one the Arr
-- has just reported unreachable, so it says "seen in a pass" and not "was
-- reachable". A NAS asleep for three days would read "just now".
--
-- Null for a folder that has never answered since this column existed, which
-- reads as "unknown" rather than as "never" — the diagnostics screen states the
-- distinction instead of inventing a date.
ALTER TABLE root_folders ADD COLUMN last_accessible_at TEXT;

-- Nothing has ever been recorded, so seed from what is known now: a folder the
-- last sync found reachable was reachable at that moment.
UPDATE root_folders SET last_accessible_at = last_synced_at WHERE accessible = 1;
