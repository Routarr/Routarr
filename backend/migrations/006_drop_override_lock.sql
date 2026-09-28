-- An override pins a title to a category and wins over every rule already.
-- Its lock protected nothing, so the column goes with the field.
ALTER TABLE overrides DROP COLUMN locked;
