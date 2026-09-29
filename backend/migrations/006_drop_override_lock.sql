-- An override pins a title to a category and wins over every rule on its own.
-- A lock on it protects nothing, so the column goes with the field.
ALTER TABLE overrides DROP COLUMN locked;
