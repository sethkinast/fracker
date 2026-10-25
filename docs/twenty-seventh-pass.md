# Twenty-seventh pass — the OCR cleanup (census resolution, addendum #90; #89 built forms)

Trigger: the parts census (2026-10-25, `~/workspace/fracker-frames/census/`).
Replaying Seth's 16 capture-debug frames through production code held
108 of 269 visible parts with 8 false accepts, and 0 of 61 Neuroptics
cell instances. Root mechanism: the parts layout OCR'd the WHOLE cell —
the count badge landed as a junk leading token, the icon spawned 3–10
junk tokens, and the matcher trimmed only 4 leading tokens, so the gear
word (often line 1 of a THREE-line name) was destroyed, diluted, or
tied among siblings. This pass is the census's fix list, each item
gated on a production-code replay of the same 16 frames.

## Tuning (Neuroptics subset, production matcher)

26 read configurations were swept over the 61 Neuroptics cell
instances (band × PSM × upscale × x-trim × line-split) and scored with
the production matcher itself (a temporary harness driving
`validate_read`/`best_match_margin`, removed before this commit):

- Winner: band y 0.53–1.00, PSM 11, upscale 4 → **24/61** held-to-truth.
- PSM 6 at the same band: 21/61. X-trim: no change. Upscale 6: worse
  (22). Line-split (3 strips, PSM 7): far worse (4) — the strips sever
  the text lines. Bands starting at 0.44–0.50 collapse (11–18).
- Dual-PSM (same band, PSM 6 + 11 union): 25/61 — +1 cell for a second
  engine read of every parts cell. Rejected: not a large gain.
- Oracle ceiling over all 26 configs: 33/61 — the residual losses are
  read-quality-bound, not configuration-bound.
- Failure classes of the winner (37 misses): margin refusal with
  best = truth 20 (reads like "Xmag Prime Neuroptics Blueprint" score
  0.91–0.97 for the truth but a sibling sits inside the 0.10 margin —
  the never-guess guard working as designed against genuinely
  confusable siblings), garbled 9, truth viable but outscored by a
  sibling 7, empty read 1. `MATCH_MARGIN` was NOT retuned: lowering it
  converts the outscored class into sibling false accepts.

## Changes (v0.5.22)

1. **Parts name band** (data only, `data/capture/layouts.toml`):
   `name = [0.0, 0.53, 1.0, 1.0]`, `name_psm = 11`, `name_upscale = 4`.
   The band starts below the badge and the icon body, so neither
   reaches the matcher.
2. **Screen-scoped validation universes** (`capture.rs`):
   `load_universe` now returns a `UniverseSet` — parts frames validate
   against roster part names PLUS built-component forms (#89 rule 1:
   strip the trailing " Blueprint" from component blueprints; the
   gear's own "<Gear> Blueprint" has none), relic frames against relic
   names, arcane frames against arcane names. The combined universe
   remains for unclassified/other frames and `--ocr-ingest`. Bucket
   routing in `StockState` is unchanged. On the tuning set scoping
   changed no held counts; its value is false-accept suppression
   (below).
3. **Matcher** (`ocr.rs`): `best_match_margin` tries ALL leading-token
   suffix trims (was capped at 4 starts — the census measured +51
   items standalone); `strip_tier_label` strips only BALANCED `[...]`
   spans — an unmatched `[` drops just the bracket character and keeps
   the text (census case parts-07 #0: "[ Gauss Prime Blueprint" lost
   the whole read). Both pinned by tests, the census case included.
4. **Count integrity** (`capture.rs`): a session now resolves each
   stack's count by VOTE — the most frequent count observed for it
   across the session's frames, ties toward the larger count —
   instead of max-merging frame stocks (`StockState::merge_from` is
   untouched; finalise still unions session ∩ disk). Documented
   limit: a stack observed exactly once keeps its count, right or
   wrong — on the census frames the Trumna Prime Stock phantom (×1
   read as ×8) is observed exactly once, so it survives; no in-session
   signal distinguishes it. Multi-observation stacks behave: on the
   replay, all 55 re-observed names resolved consistently (e.g. Ember
   Prime Neuroptics Blueprint ×5 in 3 cells → 5; conflicting badge
   reads resolve by mode, ties to max).
5. **#89 groundwork** (`model.rs`, `ocr.rs`): `built_component_form`
   and `StockState::canonical_part_counts` (built + blueprint counts
   summed onto the roster part) land here; the plan consumes them in
   pass 28 (#84).

## Production verification (the gate)

Temporary Rust harness (production `CaptureData` + `CaptureSession` +
`TesseractCliEngine`, the same calls `process_frame` makes), all 16
census frames, before → after:

- Session accounting identical: 16 seen / 15 read / 1 skipped.
- **Distinct parts held: 108 correct + 8 false accepts → 127 correct
  + 0 false accepts.**
- Neuroptics: **0 → 13 of 28 distinct items** (24/61 cell instances on
  the tuning sweep). Misses are the margin-refusal/garbled classes
  above.
- Frame 11 (the mid-scroll composite that produced all 8 census false
  accepts): **zero held cells** — scoping + the margin guard
  neutralise it.
- Relic/arcane layouts untouched; full test suite green.

## Deferred, explicitly

- **Scroll-offset handling** (census recommendation #4): NOT built.
  No cheap, reliable offset signal surfaced during tuning; a
  speculative offset search would multiply per-frame OCR time, and
  scoped universes already neutralise the composite frame's phantoms
  (frame 11: 0 holds). Never-guess beats coverage.
- MATCH_MARGIN / min_score retuning: not touched (see Tuning).
- Dual-PSM second read: rejected on the measured gain (+1/61).
