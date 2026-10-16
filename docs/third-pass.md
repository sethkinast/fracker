# Worker brief #3 — Fracker third pass (data out of code, OCR machinery, pets)

Branch: `opencode/third-pass` (created from master; check it out).
Read FIRST: `docs/verification-addendum.md` in full (it is the
steering record — items 20–23 are this pass's charter), then
`BRIEF.md`, `docs/second-pass.md`, and your pass-2 state.

## Scope of THIS pass

1. **Data out of the Rust code (addendum #20).** Nothing game-shaped
   stays hardcoded: the weeklies, rotations, thresholds, mission
   heuristics tables, and any other content move to data files
   (TOML for packs/structured content, JSON/YAML where they fit)
   under `data/`, loaded at runtime. Rust interprets, never embeds.
   Proof obligation: your summary demonstrates one content edit
   (e.g. change a weekly in its data file) that changes the
   generated plan with NO recompilation of content strings — and a
   test that fails if a weekly name appears in `src/`.
2. **Pets dataset (backlog gap).** Find the WFCD companions data at
   its real path/format (the earlier `pets.json` fetch 404'd — list
   the repo's data directory). Wire companions into the item
   universe + backlog regeneration. Kavasa Prime stays EXCLUDED
   regardless (addendum #22 — owned collar, not a weapon).
3. **OCR bootstrap machinery (addendum #21: OCR is the plan).**
   Build the ingestion module end to end except the live capture
   session (that happens later, with Seth at the game):
   - input: a folder of inventory-grid screenshots;
   - grid segmentation + per-cell OCR behind a pluggable engine
     trait (bind tesseract if installable on this box; if not,
     ship the trait + a stub and say so plainly);
   - every read validated against the item universe (fuzzy match
     to a known uniqueName); low-confidence or unmatched reads are
     recorded as NOT held, never guessed;
   - output: a stock state file (loose parts + relic counts, JSON)
     in the shape the planner already consumes.
   Tests: render synthetic grid images with known item names using
   a system font and assert the pipeline extracts them (engine
   permitting); at minimum, fixture-image tests for segmentation
   and validation logic.
4. **Demo export is self-contained (addendum #23).** `--demo`
   embeds icons as ≤128 px data-URI thumbnails (the review surface
   doesn't fetch CDN hotlinks). The product page output keeps
   hotlinks per BRIEF §4.2. Regenerate `docs/demo-plan.html`.

## Explicitly OUT of scope this pass

The live OCR capture session with Seth; tray/ksni runtime; Flatpak;
the EE.log live watch loop; Xuerian write-back; IA/visual redesign
(Seth's sequencing: data correctness first, IA after).

## Gates and deliverables

- `cargo build` clean (warnings explained), `cargo test` green.
- ONE logical commit on `opencode/third-pass`, then PUSH the
  branch to origin.
- Final summary to stdout: per-item status, what was cut and why,
  the data-file inventory (what moved where), OCR engine status on
  this box, demo stats.
