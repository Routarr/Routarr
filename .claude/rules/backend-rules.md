---
paths:
  - "backend/src/models/rule.rs"
  - "backend/src/services/rule_engine/**"
  - "backend/src/api/conditions.rs"
  - "frontend/src/api/conditions.ts"
---
# Rules and conditions

- A new `Condition` variant needs `kind`, `is_empty` and `metadata_field` in
  `backend/src/models/rule.rs` and an arm in `evaluate_single_condition` (the compiler finds
  all four), a `CONDITIONS` entry in `backend/src/api/conditions.rs` (its tests find a missing
  one), and a `ConditionLabel<PascalKind>` key in `backend/locales/en.json`, which nothing
  checks. The frontend builder reads the catalogue and changes only for a new `value_type`.
- The values of a plain condition are alternatives. The `_all` variants
  (`genre_contains_all`, `keyword_contains_all`, `origin_country_all`, `tag_in_all`) require
  every value, and nothing reads an AND out of a separator. Compare through `contains_any` and
  `contains_all`, which fold case, accents and punctuation (`normalise_value`) and invent no
  synonym. A coded value keeps what folding drops: a rating through `certification_key`
  (`R+` is not `R`), a status through `status_key`. The validator's `key_of` and the editor's
  `keyOfKind` (`frontend/src/api/conditions.ts`) name the key of each kind.
