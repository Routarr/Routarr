---
paths:
  - "backend/src/integrations/**"
  - "backend/src/services/{metadata,enrichment,routing,settings}.rs"
  - "backend/src/state.rs"
  - "backend/src/api/{sources,health}.rs"
---
# Metadata sources

- Evaluation reads the configured order (`metadata_order`) whatever the keys, so a cached
  answer keeps counting after its key is removed, a TMDB one until the purge removes it at
  six months (`metadata::cache_days`). Fetching and the "no source can answer" warnings read
  `metadata_providers`, the subset able to answer today.
- A new source needs a `PROVIDERS` entry with its `Addressing` and a `FetchingSource` variant,
  whose matches the compiler finds. These fall through silently when missed:
  `FetchingSource::resolve` for a `Search` source, `FetchingSource::rate` for a paced public
  endpoint, `AppState::metadata_sources`, and for a keyed source `provider_key_from` and
  `provider_keys_from`.
- A source answers in Routarr's vocabulary: an ISO 639-1 language code and an ISO 3166-1
  alpha-2 country code, converted through `backend/src/integrations/language/`, whose tables
  hold every ISO language and country. A name or a three-letter code stored as it came
  matches no rule written against `ja` or `JP`.
- A source that rates sets `certification_scale` in `metadata::fetch`: the country whose system
  the rating belongs to, or `MAL`. `routing::keep_the_regions_rating` puts the first region's
  rating ahead of the source order, so a rating left without its scale loses to every region's.
- A credential setting takes a sealed `Kind` in `KNOWN` (`backend/src/services/settings.rs`, see
  `Kind::sealed`): the settings API, the export, the import and `maintenance::reseal_secrets`
  all read it there. A source's key is `<id>_api_key`, the name `provider_key_from` builds.
