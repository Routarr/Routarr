-- The default category had two sources of truth, and they drifted apart.
--
-- The routing engine falls back to the `default_category` **setting**. The
-- interface's "default" badge and the guard that refuses to delete a category
-- in use read the `is_default` **flag** on the row. Nothing kept them in step:
-- changing the setting from the Settings page never touched the flag.
--
-- The visible half was an interface badging one category while the engine used
-- another. The dangerous half was the guard protecting the flagged category
-- rather than the one actually being fallen back on — so the real fallback
-- could be deleted, after which every unmatched item routed to a category that
-- no longer existed, had no root folder, and was silently skipped.
--
-- The setting wins because it is what the engine reads. The flag goes.

-- Repair on the way out, using the flag one last time: on an installation where
-- the setting was pointed at a category that has since been deleted, the
-- flagged row is the best evidence left of what the fallback used to be.
UPDATE settings
SET value = (SELECT name FROM categories WHERE is_default = 1 LIMIT 1),
    updated_at = datetime('now')
WHERE key = 'default_category'
  AND value NOT IN (SELECT name FROM categories)
  AND EXISTS (SELECT 1 FROM categories WHERE is_default = 1);

-- And if there was no flagged row either, any category beats naming one that
-- does not exist: an unmatched item has to land somewhere.
UPDATE settings
SET value = (SELECT name FROM categories ORDER BY display_order, name LIMIT 1),
    updated_at = datetime('now')
WHERE key = 'default_category'
  AND value NOT IN (SELECT name FROM categories)
  AND EXISTS (SELECT 1 FROM categories);

ALTER TABLE categories DROP COLUMN is_default;
