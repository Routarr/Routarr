-- The code a task that failed on an error answers, as the envelope's `error`
-- would have: a caller that asked not to wait reads it here.
ALTER TABLE jobs ADD COLUMN error_code TEXT;
