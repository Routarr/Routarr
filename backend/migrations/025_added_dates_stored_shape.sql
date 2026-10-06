-- A title's added date in the shape every stored timestamp has, UTC, where
-- the Arr wrote it in its own. A value SQLite cannot read stays as it is.
UPDATE media
   SET added_at = COALESCE(strftime('%Y-%m-%d %H:%M:%S', added_at), added_at)
 WHERE added_at IS NOT NULL;
