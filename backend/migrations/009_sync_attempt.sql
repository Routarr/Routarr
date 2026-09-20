-- Separate "when we last tried" from "when this last worked".
--
-- `update_sync_status` stamped `last_sync_at` on both branches, so a failed
-- attempt refreshed the timestamp exactly as a successful one did. The
-- instance screen then read "synchronised 2 minutes ago" beside an error
-- badge, while the library it was describing had last been refreshed two days
-- earlier — the two screens appeared to disagree when in fact one of them was
-- answering a different question.
--
-- Both are worth keeping. The attempt time is what tells you the scheduler is
-- running at all; the success time is what tells you the data is current.
-- Existing rows carry an attempt time in `last_sync_at`, which the next
-- successful sync corrects on its own.
ALTER TABLE instances ADD COLUMN last_sync_attempt_at TEXT;

UPDATE instances SET last_sync_attempt_at = last_sync_at;

-- A row whose last attempt failed never had a recorded success; saying nothing
-- is more honest than naming the moment it failed.
UPDATE instances SET last_sync_at = NULL
WHERE last_sync_status IS NOT NULL AND last_sync_status != 'success';
