-- A task's detail as a dictionary key and its values, rendered when the Tasks
-- screen reads it: the interface language is the one in force then, not the
-- one in force when the task ran. `detail` keeps the English text for a row
-- written before these columns, and for anything reading the table directly.
ALTER TABLE jobs ADD COLUMN detail_key TEXT;
ALTER TABLE jobs ADD COLUMN detail_params TEXT;
