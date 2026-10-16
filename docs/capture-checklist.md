# OCR capture checklist (sixth pass §5, addendum #34)

Fracker bootstraps local stock from inventory **screenshots**. This is the
one-time capture session; afterwards the pipeline validates every read
against the item universe and **never guesses** (an unreadable cell is
recorded as not-held, never as stock).

This session runs **with Seth** and *is* the acceptance test. Nothing here
can be produced from the dev box: these are real inventory screens.

Raw screenshots are **gitignored** (`fixtures/ocr-samples/`) — never
committed. Ground-truth notes for the existing fixtures live in
`fixtures/ocr-samples/NOTES.md`; match that style for new ones.

## Before you start

- **Resolution/UI beyond the game:** capture at the game's normal window
  size; the layouts scale reference geometry to the actual screenshot
  size, so a re-shot at another size still aligns.
- **No hover tooltips.** The relic fixtures show a tooltip occluding ~2
  cells; a clean shot is the ground truth. Park the cursor off the grid.
- Keep the **stack-count labels** in frame (relic screen: `xN` top-left,
  blank = 1; parts/arcanes: circled badge top-left, blank = 1).
- One screen per file. Name files `NN-<screen>.png` so ingest order is
  deterministic.

## 1. Relics — one shot per era tab

Screen: **Void Relics / Refinement**.

- One shot per **era tab**: Lith, Meso, Neo, Axi (and Requiem if owned).
- Sort **by NAME** (the reference filter — stable, matches the fixtures).
- Same refinement view as the fixtures (`relics-axi-02-clean.png`).
- The header (`COLLECTED …/… RADIANT …/…`) is global, not per-tab; the
  pipeline ignores it.
- If a tab has more rows than fit on screen, take **scrolled shots**
  covering every row; a partial cell at the viewport edge is rejected, so
  overlap rows between scroll positions.

Command:

```
fracker --ocr-ingest <folder> --ocr-layout relic --stock-out data/stock.json
```

## 2. Parts — scrolled shots per category

Screen: **Inventory / Sell**.

- One set of **scrolled shots per parts category** (at minimum
  **PRIME PARTS**; add other owned part categories as needed).
- Scroll in **overlapping** steps: the bottom row is clipped at the
  viewport edge, so the next shot must show that row fully.
- Sort order does not matter (all cells are read).
- Re-run with `--ocr-layout parts` for each folder.

```
fracker --ocr-ingest <parts-folder> --ocr-layout parts --stock-out data/stock.json
```

## 3. Arcanes — the steady-state arcane source

Screen: **Arcanes** inventory.

- Neither the profile endpoint nor EE.log carries arcane state
  (addendum #34): **this screen is the source of truth** once captured.
- Scroll in overlapping steps like the parts screen.
- Use `--ocr-layout arcanes`.

```
fracker --ocr-ingest <arcanes-folder> --ocr-layout arcanes --stock-out data/stock.json
```

> **The `[arcanes]` layout in `data/capture/layouts.toml` is PROVISIONAL** —
> a best-effort grid derived from the parts screen, never measured against
> a real Arcanes capture. After the first capture, correct the geometry
> (`x0`/`cell_w`/`y0`/`cell_h`, and the `name`/`count` regions) against the
> shot. Until then, an off grid yields **empty** arcane stock, not bad
> stock — never-guess validation is unchanged.

## 4. Replace the debrief stopgap

`data/arcanes.toml` currently carries Seth's **spoken debrief** as a
**labeled stopgap** (11 Taubound arcanes). Once the Arcanes capture
succeeds, the captured `data/stock.json` arcane bucket replaces the
stopgap list; update the `[stopgap]` label or drop the file.

## After the session

- `data/stock.json` holds `relics`, `parts`, and `arcanes` buckets; all
  reads were universe-validated.
- Spot-check against the screen: a couple of counts per tab/category, and
  that the header totals are *not* in the stock (they are global).
- Record ground-truth notes next to the shots (the NOTES.md pattern) so
  the pipeline can be pinned later.
