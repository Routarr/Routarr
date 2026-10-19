-- MyAnimeList is read through Tenrai, under the source id `tenrai`. The MyAnimeList ids are the
-- same, so the cached answers and the resolved titles stay.
UPDATE metadata_cache SET source = 'tenrai' WHERE source = 'jikan';
UPDATE source_identifiers SET source = 'tenrai' WHERE source = 'jikan';
UPDATE settings
   SET value = trim(replace(',' || replace(value, ' ', '') || ',', ',jikan,', ',tenrai,'), ',')
 WHERE key = 'metadata_providers';
-- A pinned test names the sources of its snapshot, which the explanation of a failure reads.
UPDATE rule_tests SET metadata_json = replace(metadata_json, '"jikan"', '"tenrai"')
 WHERE metadata_json LIKE '%"jikan"%';
-- A probe of the address no longer asked says nothing about the new one.
DELETE FROM probe_results WHERE subject = 'source:jikan';
