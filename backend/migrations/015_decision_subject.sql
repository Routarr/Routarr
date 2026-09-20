-- Who asked, beside what caused it.
--
-- `actor` holds the trigger — `manual`, `schedule`, `webhook` — which answers
-- "did the nightly sweep propose this, or did I" and stops there. With a
-- session mode in front, several people can be the "I" in that question, and a
-- key can be a script rather than any of them. This column is the name the mode
-- vouched for, when a mode vouched for one.
--
-- Nullable, and null in three honest cases: the scheduler, which nobody asked;
-- `none` and `external`, where everyone shares one anonymous subject that names
-- nobody; and every row written before this migration.
ALTER TABLE decisions ADD COLUMN subject TEXT;
ALTER TABLE execution_logs ADD COLUMN subject TEXT;
