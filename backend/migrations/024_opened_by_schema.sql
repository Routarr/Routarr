-- The last migration each release had applied when it opened the database.
-- An older release refuses the database only when a newer one migrated it
-- further than the older knows: a release with the same schema reads it as
-- its own.
ALTER TABLE _opened_by ADD COLUMN schema TEXT;
