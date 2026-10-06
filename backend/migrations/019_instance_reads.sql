-- The version the Arr reported at its last sync, which the warnings hold
-- against the oldest release Routarr supports.
ALTER TABLE instances ADD COLUMN arr_version TEXT;

-- When the Arr's root folders were last listed: listing them makes the Arr
-- walk every folder inside each, so a scheduled sync lists them once a day
-- and reads the free space of the mounts in between.
ALTER TABLE instances ADD COLUMN root_folders_read_at TEXT;
