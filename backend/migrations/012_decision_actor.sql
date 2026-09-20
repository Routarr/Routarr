-- What caused a decision and what caused a write, recorded where the screens
-- already look. `jobs` answers it through its own `trigger` column; these two
-- had no answer at all, so "did the nightly sweep propose this, or did I" was
-- unanswerable. Nullable: rows written before this migration cannot be
-- attributed after the fact, and guessing would be worse than admitting it.
ALTER TABLE decisions ADD COLUMN actor TEXT;
ALTER TABLE execution_logs ADD COLUMN actor TEXT;
