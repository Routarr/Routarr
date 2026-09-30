-- What a finished task answered, as JSON: the report the same call gives
-- when the caller waits for it. A caller that asked not to wait reads it
-- here.
ALTER TABLE jobs ADD COLUMN result TEXT;
