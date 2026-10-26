# Thirty-eighth pass — relic owned-gate, radshare rendering

Two fixes from `PASS38_BRIEF.md`, the first two stages of the
crack-loop work (collect relics → run correct fissure →
radshare if appropriate → detect reward → mark). Shipped as two
commits on `pass38-loop`, released as v0.5.32.

## Fix A — the relic owned-gate (loop stage 1)

### The bug

The relic inventory BROWSE screen shows the full catalogue.
Tiles for relics Seth does not own render faded — the whole
tile is alpha-dimmed — with an eye badge in the top-left
corner, where an owned stack shows its `×N` count label or
nothing. The reader's count contract (`OcrRead.count`:
a `×N` label means N; NO label means exactly 1) cannot tell an
unowned tile from an owned ×1: the unowned tiles' name bands
read as cleanly as owned ones, so every unowned tile a browse
scroll passed entered stock as ×1 Intact. Worse, finalize
REPLACES the relic bucket wholesale whenever a relics screen
was seen — one browse sync overwrote good stock with the
catalogue.

### The evidence (measured before building)

From Seth's browse screenshot
(`fracker-frames/fixtures/browse-strip.png`, 22 tiles measured
at the production cell size ~168×174; the upload is
palette-clipped, luma max ~206, so absolute numbers are
directional):

- the name-band mean does NOT separate (owned 22–29, unowned
  25–27) — faded tiles' names stay nearly full-bright, which
  is why OCR holds them;
- the icon region's 90th-percentile luminance separates
  perfectly (owned 122–124, unowned 49–55);
- the eye badge window separates absolutely (owned max ≤ 37,
  unowned glyph core ~155), and template NCC against the
  badge averaged over the 12 unowned tiles scores 0.81–0.97
  on unowned tiles against −0.10..+0.07 on owned ones.

### The gate

`ocr::relic_owned_gate`, run inside `read_cells` on relic
frames only (and per page in `ocr::ingest_folder`), after
validation, so it sees exactly the tiles that would enter
stock and only ever REMOVES holds:

1. **Catalogue detection (per frame).** Any tile with a
   clear eye badge (`eye_badge_score` ≥ `EYE_NCC_MIN` = 0.60,
   template NCC mirroring `badge_ring_present`) marks the
   frame browse-type. Outside catalogue frames the read path
   is bit-identical to before — owned-only selection and
   refinement screens never trigger the gate.
2. **Exclusion (per tile, all legs required).** A held tile
   is excluded only when: it has NO parsed count label (a
   label is positive owned evidence); it carries a clear eye
   badge itself; and its icon highlight is dim — in the dim
   cluster of a per-frame Otsu split over the frame's
   held-tile highlights that must separate cleanly
   (bright-cluster min ≥ 1.35× dim-cluster max), or, when no
   clean split exists (the unowned tail filling a whole
   viewport), below 70% of the session's owned-brightness
   reference.

The session reference is the running maximum icon highlight
over the session's relic frames: `process_frame` extends it
before each relic read and hands it to `read_frame` →
`ReadOptions::relic_bright_ref` → the gate. At a cold start on
an all-unowned frame the reference is itself a dim value, the
70% cut sits below every tile, and nothing is excluded —
fail-safe toward the known behaviour, never toward false
exclusions.

This is deliberately stricter than the brief's sketch (eye
OR dim alone): the brief's bias rule — exclusion requires
clear evidence, ambiguous tiles stay — is implemented as a
full conjunction. A phantom in stock is the known bug; a
false exclusion of a real relic would be a new, worse one.

### Observability

Excluded tiles keep their `CellResult` with `held = false`
and the new `gate_excluded` flag. `CaptureSession` accumulates
the excluded names (first-exclusion order, deduplicated), and
`finalize_capture` logs them:

```
finalizing: relic owned-gate: excluded N unowned catalogue tile(s): <names>
```

A browse sync's smaller relic bucket now reads as a decision
in the log, not a loss.

### Verification

- Synthetic gate tests (ocr): dimmed + badged variants are
  excluded; dimmed-without-eye, badged-without-dimming, and
  badged-with-a-count tiles are all kept; a frame with no eye
  anywhere is untouched; a uniformly dim frame is excluded
  only with the session reference present.
- Read-path test (capture): `read_cells` over synthetic cells
  unholds exactly the unowned three of six.
- The existing relic fixture test (≥540 stacks across owned
  relic screenshots) is unchanged — it exercises owned-only
  screens, where the gate is inert.
- Browse-strip verdicts (the production read path over
  `browse-strip.png`, tiles cut on the strip's measured grid,
  names fed positionally so the gate is the only variable):

  | Tile | Verdict | | Tile | Verdict |
  |---|---|---|---|---|
  | Neo F3 | kept | | Axi A2 | **excluded** |
  | Neo K9 | kept | | Axi A3 | **excluded** |
  | Neo O2 | kept | | Axi A6 | **excluded** |
  | Neo P4 | kept | | Axi A7 | **excluded** |
  | Neo P7 | kept | | Axi A8 | **excluded** |
  | Neo P9 | kept | | Axi A12 | **excluded** |
  | Neo T11 | kept | | Axi A13 | **excluded** |
  | Neo V7 | kept | | Axi A15 | **excluded** |
  | Neo V12 | kept | | Axi A19 | **excluded** |
  | Neo Y1 | kept | | Axi A23 | **excluded** |
  | | | | Axi B7 | **excluded** |
  | | | | Axi B9 | **excluded** |

  10 owned kept, 12 unowned excluded — exact.

- Live verification is post-ship by construction: Seth's next
  browse sync should write only owned relics and log the
  exclusions. The strip is a chat-clipped upload (luma ≤ ~206
  vs the game's ~250), so the eye threshold and split ratio
  carry margin by design (NCC gap 0.74 wide; split at strip
  scale 2.2× vs the 1.35× floor) — but if the live log ever
  shows a real relic excluded, the legs to revisit are the
  eye NCC floor and the split ratio, in that order.

## Fix B — the radshare verdict reaches the page (loop stage 3)

`radshare_verdict()` ran only inside `derive_action`,
appending its verdict to `GoalAction.text`. No surface
renders that text: `build_chunks` bound it as `_action_text`
and dropped it, and `CrackInfo` had no field for it. The
verdict was dead code with respect to the page.

The computation is now one helper, `radshare_for(input, gear,
cand, pick)`, shared by `derive_action` (which keeps
appending to the action text) and `build_chunks` (which
stores it on the chunk's `CrackInfo::radshare`). Both
surfaces that render from `CrackInfo` show the same compact
marker — the literal `radshare` in a gold `.rad` tag, sibling
to the cyan `.nx` NEXT tag — when, and only when, the verdict
is present:

- the crack-queue row: in the relic column, after the
  refinement;
- the Hunt card: appended to the `▸` crack line.

Tests: a plan-level test drives `plan()` with a recent-gear
rare crack and asserts the verdict lands on the chunk's
CrackInfo, agrees with the goal action text, and stays `None`
for stale gear; render tests assert the marker is present on
both surfaces with a verdict and absent without one; the
pre-existing verdict unit tests are untouched.

## Gates

- Full test suite green (count in the addendum entry).
- `cargo build --release --locked --offline` green pre-tag.
- Settled census unaffected by construction: the gate touches
  relic frames only; parts/arcane paths are untouched, and on
  owned-only relic frames the gate is inert (no eye ⇒ no
  catalogue ⇒ no change).
