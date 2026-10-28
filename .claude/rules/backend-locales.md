---
paths:
  - "backend/locales/**"
  - "backend/src/localization.rs"
  - "scripts/{check-locales.py,add-locale.py,translation-context.py,glossary.json}"
---
# Backend dictionaries

- A new key needs `en.json` alone, since every language falls back to English key by key.
  `scripts/check-locales.py` fails on a key the code uses and `en.json` lacks, a key another
  language has and English lacks, a changed `{placeholder}`, an empty value, a key no source
  file quotes, a key written twice, and a language below 50%.
- A translation says each core term (apply, move, exception, root folder...) in the one word
  `scripts/glossary.json` gives its language. The check picks the keys from their English, so
  an English sentence using the word in another sense goes in that concept's `except`.
- `python3 scripts/add-locale.py <code> < batch.json` merges translations into
  `backend/locales/<code>.json` and refuses a lost or invented `{placeholder}`.
- A translator works from `python3 scripts/translation-context.py batch.json <codes>`: per
  language, the batch, the words the file already uses for its terms, the glossary concepts and
  the punctuation habits, never from whole dictionaries. A term a batch settles in every
  language goes into `scripts/glossary.json`.
- The dictionaries are `include_str!`'d. When a locale edit is missing from a fresh build,
  `touch backend/src/localization.rs` and build again.
