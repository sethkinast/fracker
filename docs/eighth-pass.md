# Worker brief #8 — Fracker eighth pass (Capture Inventory)

Branch: `opencode/eighth-pass` (created from master; check it
out). Read FIRST: `docs/verification-addendum.md` items 20, 24,
30, 32, 34–39; `docs/capture-checklist.md`; and the ground-truth
notes at `fixtures/ocr-samples/seth-relics-2026-10-16/NOTES.md`.

Context: Seth ran v0.2.0 live. The plan generates and is
accurate, EE.log detection works — and the file-based OCR
ingest flopped on real inventory (see §1). He then rejected the
screenshot workflow outright (item 39): inventory capture is a
**tray flow** — he clicks Capture Inventory and scrolls; the
app watches, classifies the screen, and collects. Build that,
on top of a corrected stock model. FAT pass; take the time.

New fixtures on the box (synced into the workspace,
gitignored — never commit them):
- `fixtures/ocr-samples/seth-relics-2026-10-16/` — 9 live
  relic shots (2560x1440, sort=RANK) + NOTES.md + the
  `stock.json` his v0.2.0 ingest produced from them.
- `fixtures/ocr-samples/seth-arcanes-2026-10-16.png` — his
  real Arcanes screen (upload-rescaled to 1957x1100; treat all
  geometry as relative). 11 cols x 7 rows visible; header
  COLLECTED 111/166; names wrap to two lines; count is a plain
  `xN` top-left (blank = 1); **rank pips** (5 diamonds) sit
  under the name; and the same arcane appears as MULTIPLE
  stacks at different ranks (e.g. Arcane Avenger x4 / plain /
  x2) — the §1 disease, second organ.

## §1 Tier/rank-aware stock (the correctness core)

The refinement screen shows one cell per (relic x refinement
tier); arcanes likewise per (arcane x rank). Name-only keys
collided: Axi A1's large stack was overwritten by a x8 stack;
Lith G6 (x36 + x1 stacks) recorded as 36; tier/rank splits —
which crack planning needs — were discarded. And there are
true drops: Lith S14 (Seth's x40 stack) is absent entirely.

- Stock model: relics keyed (relic, tier); arcanes keyed
  (arcane, rank); per-item totals are sums. Planner consumes
  totals AND splits (radiant counts drive crack advice).
- Tier is read from the cell ICON's visual appearance (no
  tier text exists). Rank for arcanes is the pip row (filled
  count) — deterministic pixels, not OCR.
- Establish tier ground truth soundly (the RANK sort groups
  stacks by tier; icon appearance clusters into the four
  tiers) and REPORT measured classification accuracy on a
  labeled subset of Seth's shots. Never-guess applies to
  tiers too: an uncertain tier is recorded as tier-unknown
  (counted in totals, excluded from tier-specific advice),
  never silently assigned.
- Drop forensics: account for EVERY visible stack in Seth's
  9 shots. Seth-confirmed cells must reproduce exactly (Meso
  K4 x38; Lith G6's two stacks; Axi A1's two stacks), and
  Lith S14 must be read — or its failure diagnosed to a named
  cause and fixed if it is a pipeline defect (geometry at
  1440p, name/count read, validation rule). The validation
  set is these shots + stock.json; ship the harness as tests
  that skip gracefully without the fixtures.

## §2 Capture Inventory (the tray flow, item 39)

- Tray menu gains **Capture Inventory** (toggle: capturing
  state is visible in the menu/status line, with live counts
  per bucket). Starting it opens ONE XDG ScreenCast portal
  session (org.freedesktop.portal.ScreenCast over D-Bus;
  request a restore token / persist mode so consent is not
  demanded every session; store the token in app data).
  Frames arrive over PipeWire; the freedesktop 24.08 runtime
  ships libpipewire — link it. New dependencies MUST be
  vendored (regenerate `cargo-sources.json` with
  flatpak-cargo-generator; see docs/flatpak-fix.md) and the
  offline build must stay green.
- While capturing: sample frames (a few fps), skip frames
  whose grid region is unchanged, OCR only stabilized frames.
  Classify each frame's screen — relics (era tabs), parts
  (Sell screen), arcanes (ARCANES header) — and route cells
  to the matching layout + §1 stock keys. Unclassifiable
  frames (menus, gameplay) pause collection silently.
- Scroll dedupe is by stack identity: (relic, tier) /
  (part name) / (arcane, rank) are unique stacks on screen —
  the same stack seen in ten frames counts ONCE. Merging is a
  union, not a sum across frames.
- Stop (menu toggle, or Seth leaving inventory for a while —
  your call, make it forgiving) finalizes: captured buckets
  REPLACE the corresponding buckets in stock.json, the plan
  regenerates, the tray status reports what was collected
  (e.g. "Capture: 512 relic stacks, 38 arcanes").
- Partial captures are safe: a bucket with zero classified
  frames is left untouched, never zeroed.
- Box-side gate: a synthetic frame-sequence test — feed the
  9 relic shots + the arcanes shot as overlapping frame
  sequences through the capture pipeline (no portal) and
  prove the merged stock equals the §1 validation result
  with no double counts.

## §3 Arcanes layout, measured at last (item 34)

Replace the provisional `[arcanes]` layout in
`data/capture/layouts.toml` with geometry measured from
Seth's real shot (grid origin/pitch, count region, two-line
name region, pip region). Reads validate against the arcane
universe like everything else. His Taubound arcanes (Melee */
Secondary *) sit below the fold of this first shot — the
layout must be scroll-robust, which §2 gives it.

## §4 First live-run fixes (item 38)

- **Open plan page does nothing.** The OpenURI portal call
  from the sandbox fails silently and the fallback never
  fires. Fix the open path (portal OpenFile with FD passing
  is the sandbox-correct route for app-owned files; whatever
  you choose must WORK from inside the Flatpak) and make any
  failure VISIBLE (tray status / notification), never silent.
- **Standing labels are raw internals.** The daily-cap bars
  render profile suffixes: the main pool shows as bare
  "Standing" and Simaris shows as "Library". Add a data-owned
  label map (no content in src/): main pool -> "Syndicates",
  Library -> "Simaris", and audit every DailyAffiliation*
  suffix the same way.

## Hard constraints (do not regress)

- SDK extension stays `org.freedesktop.Sdk.Extension.rust-stable`
  (24.08), out-of-tree cmake for leptonica/tesseract, offline
  cargo from committed `cargo-sources.json`.
- Clean sources only; no new network calls beyond what the
  app already makes. ScreenCast frames never leave the app.

## Gates and deliverables

- `cargo build` clean, `cargo test` green (incl. §1 harness
  and §2 sequence test above, fixture-gated).
- Logical commits per section; PUSH `opencode/eighth-pass`.
- Final summary to stdout: tier-classification accuracy as
  measured, S14's diagnosis, what Capture does end-to-end,
  and the list only Seth's machine can verify (portal consent
  flow, live scroll pacing, Plasma behavior).
