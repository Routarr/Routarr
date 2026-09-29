-- A proposal whose instance is gone would move a title that no longer exists,
-- and the dashboard would go on counting it. Deleting an instance retires its
-- proposals in the same transaction, and this retires any still pending for
-- an instance that is gone. What was applied is history and stays.
UPDATE decisions
SET superseded = 1
WHERE status = 'pending'
  AND superseded = 0
  AND instance_id NOT IN (SELECT id FROM instances);
