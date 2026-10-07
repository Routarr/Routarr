-- Every country's rating TMDb and TheTVDB give, as a JSON object from country
-- code to rating. The certification regions pick one each time it is read,
-- so a change of regions holds from the next simulation.
ALTER TABLE metadata_cache ADD COLUMN certifications TEXT NOT NULL DEFAULT '{}';

-- The one rating each of their answers kept moves into the object, and the
-- answer is asked again for every country's.
UPDATE metadata_cache
   SET certifications = CASE
           WHEN certification_scale IS NOT NULL AND TRIM(certification) != ''
           THEN json_object(certification_scale, TRIM(certification))
           ELSE '{}'
       END,
       certification = NULL,
       certification_scale = NULL,
       expires_at = datetime('now')
 WHERE source IN ('tmdb', 'tvdb') AND certification IS NOT NULL;
