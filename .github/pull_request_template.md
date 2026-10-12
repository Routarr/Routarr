<!-- What this changes and why, in a sentence or two. The title follows
Conventional Commits (CONTRIBUTING.md, "Commits"). -->

## Checklist

- [ ] Each new test was seen to fail with the change it guards removed.
- [ ] Text a person reads is a key in `backend/locales/en.json`.
- [ ] A change to the API other applications call is pinned again with
      `ROUTARR_WRITE_CONTRACT=1 cargo test contract`.
- [ ] The checks of CONTRIBUTING.md ("What CI will check") pass for the areas
      this touches.
