-- "Added within the last N days" now counts N times 24 hours and refuses 0,
-- which used to mean the last 24 hours. Every rule is written through serde,
-- so the condition has this one spelling.
UPDATE rules
   SET conditions = replace(conditions, '{"type":"added_within_days","value":0}',
                                        '{"type":"added_within_days","value":1}'),
       exclusions = replace(exclusions, '{"type":"added_within_days","value":0}',
                                        '{"type":"added_within_days","value":1}')
 WHERE instr(conditions, '{"type":"added_within_days","value":0}') > 0
    OR instr(exclusions, '{"type":"added_within_days","value":0}') > 0;
