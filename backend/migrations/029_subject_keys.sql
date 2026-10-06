-- Who asked, by the application key that asked: a name can be given again
-- once its key is revoked, and the application that wrote a row is the key,
-- not whoever holds the name now. A person's rows carry none.
ALTER TABLE jobs ADD COLUMN subject_key TEXT;
ALTER TABLE decisions ADD COLUMN subject_key TEXT;
ALTER TABLE execution_logs ADD COLUMN subject_key TEXT;
ALTER TABLE overrides ADD COLUMN subject_key TEXT;

-- The rows written before by a key whose name no other key has had. A name
-- given twice cannot say which key wrote a row, and stays without one.
CREATE TEMP TABLE named_once AS
    SELECT name, MIN(id) AS id FROM api_keys GROUP BY name HAVING COUNT(*) = 1;

UPDATE jobs SET subject_key = (SELECT id FROM named_once WHERE name = jobs.subject)
 WHERE trigger IN ('api', 'auto') AND subject IN (SELECT name FROM named_once);
UPDATE decisions SET subject_key = (SELECT id FROM named_once WHERE name = decisions.subject)
 WHERE actor IN ('api', 'auto') AND subject IN (SELECT name FROM named_once);
UPDATE execution_logs
   SET subject_key = (SELECT id FROM named_once WHERE name = execution_logs.subject)
 WHERE actor IN ('api', 'auto') AND subject IN (SELECT name FROM named_once);
UPDATE overrides SET subject_key = (SELECT id FROM named_once WHERE name = overrides.subject)
 WHERE subject IN (SELECT name FROM named_once);

DROP TABLE named_once;
