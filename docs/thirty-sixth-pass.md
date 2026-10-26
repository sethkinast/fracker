# Thirty-sixth pass — live-capture hotfix: intake dedupe, progress bundles, background stitch, failsafes

v0.5.28's first live runs were defective. Finalize did all
stitch+reader work at stop time (~140 s for 48 frames; the tray
counter read 0 throughout, because `captured.parts` is written
only by `finish_parts`). A confirmed full-list scroll at
~3 s/step yielded 92 parts from 99 slots. Seth's diagnosis from
the run's capture-debug frames: frames 1–31 are page 1
identical, 32 a scroll transition, 33–48 page 2 identical. The
live intake accepted near-duplicate frames of a static viewport
(the game's animated background defeats the resident's
byte-identical pre-dedupe), so the 48-frame flush threshold
tripped after ~2 viewports; bundle processing then blocked the
capacity-1 PipeWire channel for ~140 s and every later page was
dropped; wholesale finalize wrote the 2-viewport result as the
whole stock. Forensic quantification: ~2,200–2,800 serial
tesseract images in one `finish_parts` call, because live
frames give ~16 sightings/slot vs the census's ~1.9.

The animated background stays ON (Seth's explicit choice); the
dedupe below is built to be robust to it. Finalize's wholesale
replace semantics are unchanged (the partial-scroll guard is
deferred by Seth's ruling, out of scope here).

**Gate outcome: the base dual census gate passes in full; the
duplicate-inflation simulation passes on the standard census
and FAILS on the HC census (231 vs 234 distinct, 1 false
accept). Per the brief's stop rule the failure is reported,
not tuned away; §Gate has the full evidence and §Open items
the options. This branch is therefore NOT merge-ready as the
brief's gate stands.**

## Intake dedupe (scope 1) — `PartsIntake`, `stitch::ViewportPrint`

Each offered parts frame is fingerprinted on the grid region
in the stitch's own gradient form, so no second image
representation is introduced: per-cell 10×5 block-mean
gradient thumbnails, soft-normalised by the cell's mean
gradient (+1.0) so global gain (the animation's shimmer, any
brightness drift) cancels; plus the gradient row profile the
scroll estimator uses. Two prints are the same viewport when
≥90% of cells match (per-cell mean distance ≤ 0.30, in units
of the soft normaliser) and at most 3 cells differ. A position
is accepted up to `VIEWPORT_ACCEPT_CAP` **K = 4** frames — the
canonical median needs 3 pooled instances; the fourth leaves
headroom for the gain check to exclude a transient and still
pool 3. Seth's stuck run accepted 31 of one page; the
duplicate-inflation sim below accepts 4 per position and drops
the rest, counted (`parts_dupes_dropped`).

Per-frame intake cost is the fingerprint: one gradient pass
over the grid rect plus 33 thumbnail reductions — measured
inside the sim feed rates (§Gate), ~0.5–1.0 s/frame at
2560×1440 including PNG decode and header classification,
against a ~3 s/step live pace; the fingerprint itself is a
small fraction of that.

## Bundle boundaries (scope 2) — progress, positions atomic

Bundles close on accumulated panorama progress (the row
profile's scroll estimate between consecutive accepted
prints), never on raw frame count:

- A flush is requested once the pooled positions cover one
  viewport (rows × fitted pitch) of progress.
- **Boundaries fall between positions, never inside one.** The
  flush verdict rides the first frame of the NEXT position and
  closes the pending bundle before that frame is pushed. A
  position split across bundles is stitched twice under
  independent fits and its counts sum at merge — measured on
  the HC sim before this rule (7 names doubled; §Gate).
- `BUNDLE_MIN_FRAMES` **= 8**: a progress flush is honoured
  only once the pending bundle holds ≥8 frames. The stitch's
  pitch/segment fit needs context: a 4-frame head bundle on
  the HC census fit its region independently of the main
  bundle and produced 29 instance-disagreement flags and +15
  marginal slots against the single-bundle control (0 flags),
  costing 2 distinct at merge; the control reproduces the
  shipped numbers exactly, which exonerated the merge path
  and isolated the split. At the floor, both censuses
  reproduce their shipped distinct with 0 disagreement flags
  on HC.
- `BUNDLE_MAX_FRAMES` **= 48** stays as the hard bundle cap
  (the pass-33 memory envelope, 48 × ~3.7 MB ≈ 180 MB); it is
  a failsafe, not a boundary rule, and can split a position
  when it trips (only reachable in pathological streams; the
  queue failsafe below bounds the backlog regardless).

## Background bundle processing (scope 3) — `StitchDispatcher`

A drained bundle is stitched on a dedicated thread with a
**dedicated second `OcrEngine` instance**. The split is
forced by the engine, not by taste: `TesseractCliEngine`
holds its cache lock across whole batched reads
(`src/ocr.rs`), so the frame engine cannot serve reads while
a bundle is mid-batch; a second instance (probed at capture
start in `run_capture`) gives the stitch thread its own cache
and lock domain. Jobs are FIFO over an mpsc channel; reports
return over a second channel and are merged by the capture
worker in `process_frame`'s poll point (before the parts
branch, so a merge lands before the next frame is handled),
in dispatch order. Merges run the old `finish_parts` body
(per-report votes incl. glyph-implied holds, then
`resolve_voted_stock` over the union), so `captured.parts`
updates land continuously and the tray counter climbs during
the scroll instead of at stop. Stop/quit is a bounded join by
construction: `finish_parts` drains the final partial bundle,
collects every outstanding report through the same channel,
then shuts the dispatcher down (sender dropped, thread
joined) inside `run_capture` — no detached work outlives the
session. Sessions without a stitch engine (unit tests, stub
engines) fall back to stitching the drained bundle
synchronously on the worker; behaviour is otherwise
identical.

## Reader instance cap (scope 4) — N = 3

`stitch_parts_session` previously read canonical + EVERY
usable instance of every slot (≈600 crops/bundle before the
line reader's ~1,200–1,800). It now reads canonical + at most
`READER_INSTANCE_CAP` **N = 3** best-ranked instances per
slot, ranked by `stitch::rank_instances`: gain-check inliers
first (via `measure_link` against the sibling pool at the
canonical crop's geometry), then sighting margin descending,
then |gain − pool median gain| ascending, then link residual
ascending. Counts read the same capped set; `SlotReport` still
reports all usable instances. Evidence: N ∈ {3, 6} on the HC
census produced byte-identical gate numbers (232 at the time
of the sweep, both) while N = 6 cost +46% finish time
(455.5 s vs 311.3 s) — the cap does not bind on either census
at 3, and 3 is the smallest value with that evidence. (The
232 in the sweep predates the bundle floor; see §Gate for the
interaction.)

## Failsafes (addendum) — trip loud, never silent

All three caps record their trip in session state and surface
it in the finalize output (`CaptureSession::failsafe_summary`,
logged by `resident::finalize_capture` beside the coverage
line; the harness prints it too). Each has a trip test in
`src/capture.rs::tests`.

1. **Session frame cap** `PARTS_SESSION_FRAME_CAP` **= 512**
   accepted parts frames. Arithmetic: a full scroll is
   ~9 viewports × (4 accepts at the cap + a few transition
   frames) ≈ 60 frames typical; a hesitant, micro-stopped
   scroll might triple that (~180). 512 is ~3× the padded
   estimate and ~8× the typical need — past it, the session
   is not scrolling, it is leaking; intake refuses further
   frames (counted, `parts_frames_capped`) and the session
   still finalizes normally.
2. **Bounded bundle queue** `BUNDLE_QUEUE_MAX_FRAMES`
   **= 96** frames (two bundles). When the stitch thread
   falls behind, drained bundles coalesce onto the waiting
   queue (consecutive ranges of one scroll — coalescing is
   appending); overflow frames are dropped, counted
   (`BundleQueue::dropped_frames`), and the trip is flagged
   for the finalize summary. Worst-case buffering stays
   bounded: ≤96 queued + 48 in flight + 48 accumulating
   ≈ 192 frames.
3. **Session duration cap** `SESSION_MAX_DURATION`
   **= 900 s**. Seth's full three-screen scroll takes
   ~2–4 minutes; 15 minutes is ~4× that and bounds how long a
   forgotten capture can hold the portal stream — the idle
   auto-stop never fires on a static Parts screen, which
   classifies Parts forever. `run_capture_limited` checks
   elapsed time each loop iteration; on trip it records the
   flag and breaks into the normal finalize path
   (auto-finalize, not abort).

## Gate (brief §5 + addendum) — production harness, session mode

Configuration for every run below: final branch build
(K = 4, floor 8, atomic boundaries, N = 3, caps as above).
The full test suite is green: **309 passed, 0 failed**
(303 at `63dbd6f` + 6 new pass-36 tests: intake collapse,
progress flush, session-cap trip, queue coalesce/drop,
duration-cap auto-finalize, rank_instances cap). Note the
property battery makes the debug-profile suite very slow on
this VM (~57 s for the lib target in release; the gate was
run in release throughout).

### Base censuses vs bars

| Metric | Std bar | Std | HC bar | HC |
|---|---|---|---|---|
| distinct correct | ≥239 | **239** | ≥234 | **234** |
| false accepts | 0 | **0** | 0 | **0** |
| Neuroptics distinct | 28 | **28** | 28 | **28** |
| counts (cell) | 453/453 | 452/452 | ≥93.2% | 846/908 = 93.2% |
| counts (slot) | — | 278/278 | — | 301/325 |

Bundles: std 8+7 frames, HC 8+22. Intake: all frames
accepted, 0 duplicates dropped (the censuses are distinct
frames), no failsafe tripped. One deviation inside the pass
bar: the standard counts denominator is 452, not the shipped
453 — one census cell no longer maps into a held slot under
the split-bundle segmentation. No count value is wrong
(452/452 correct, distinct and FA unchanged); the shipped
453 was measured on the whole-session fit.

Investigation trail (all numbers above are the final build):
the first split implementation (flush on progress, no floor,
boundary after the crossing frame) scored HC 232. The reader
cap sweep (N = 6) reproduced 232 exactly, exonerating the
cap. A single-bundle control through the new async path
reproduced the shipped 234/93.2% exactly, exonerating the
merge path and isolating per-bundle fit context as the cause;
the floor (8) and position-atomic boundaries restored 234
with the disagreement signature gone (29 flags → 0).

### Duplicate-inflation simulation (×8, whole-frame gain/gamma jitter)

| Metric | Std base | Std sim | HC base | HC sim |
|---|---|---|---|---|
| distinct correct | 239 | **241** | 234 | **231** |
| false accepts | 0 | **0** | 0 | **1** |
| Neuroptics distinct | 28 | 28 | 28 | 28 |
| intake accepted / dropped | 15 / 0 | 48 / 70 | 30 / 0 | 72 / 175 |
| wall: feed / finish / total (s) | 10.9 / 158.9 / 169.8 | 53.1 / 218.0 / 271.1 | 13.9 / 244.8 / 258.7 | 123.2 / 267.9 / 391.1 |

(Wall times were measured on a 2-core VM sharing CPU with
another agent's census work and, for one HC sim attempt, a VM
restart that killed the run; treat them as magnitudes, not
benchmarks. The intake feed rate is the operative figure:
120 inflated frames fed in 53.1 s std / 248 in 123.2 s HC —
~0.45–0.5 s/frame end-to-end at intake, against a ~3 s/step
live pace; processing no longer blocks intake.)

Standard sim passes: the stock diff (join-free) is
+3 gained (Corvas Prime Blueprint, Redeemer Prime Handle,
Wisp Prime Blueprint), −1 lost (Gara Prime Chassis
Blueprint), no count changes on shared names — marginal-slot
churn from denser pooling, both directions, 0 FA.

HC sim fails the brief's bar (same distinct, 0 FA) on the
resolved stock itself, verified join-free by diffing the
harness's stock dump against the base run's:

- 6 names lost: Afuris Prime Blueprint, Akbolto Prime
  Blueprint, Akbronco Prime Blueprint, Aksomati Prime
  Receiver, Ankyros Prime Blade, Venka Prime Gauntlet.
- 3 names gained: Mesa Prime Systems Blueprint, Nova Prime
  Blueprint, Strun Prime Blueprint.
- 1 false accept, at slot and stock level: the stock holds
  "Akbronco Prime Link" where the truth is Akbronco Prime
  Blueprint (band read the pooled cell as Link; glyph 0.75,
  verdict Blueprint, held via Band). The same slot produces
  the same FA in the background-only diagnostic below with
  pixel-identical cell content, so the mechanism is the dense
  pool itself (12 sightings incl. 4-copy pools change the
  canonical election and pooled read on this marginal form
  cell), not jittered pixels.
- Count inflation on boundary-shared panorama rows persists
  in part (e.g. Afuris Prime Link 1→2): consecutive census
  frames overlap by a row, so a bundle boundary leaves that
  row stitched in both bundles and the merge sums the two
  resolutions. Position-atomic boundaries removed
  intra-position splits (the pre-fix sim doubled 7 names);
  inter-frame overlap at bundle edges is intrinsic to
  splitting an overlapping stream and needs cross-bundle
  slot reconciliation to remove (§Open items).
- Losses concentrate in the head bundle's territory
  (A-region names): in the sim the head bundle holds 12
  frames but only 3 distinct positions, vs the base's 8
  frames / 8 positions — the frame-counted floor passes
  bundles whose position coverage is thin, and their
  marginal slots refuse where the base's fuller bundles
  held.

### Diagnostic: background-only jitter (not the gate)

`--simulate-live-bg` jitters only pixels outside the grid —
the production-faithful duplicate model (production
duplicates are pixel-identical inside the grid; only the
background animates). HC result: **216 distinct, 1 FA** (the
same Akbronco slot), bundles 12+24+36. Worse, not better:
with cell content identical, the outcome is dominated by
bundle placement (three bundles, thin head) and dense-pool
arbitration, and the placement variance across sim variants
(230 / 231 / 216) is itself the finding — under dense
duplicate input the bundle pipeline's HC result is
placement-sensitive in a way the sparse base (234 across
unsplit, 8+22, and floor variants) is not. This diagnostic is
reported so the gate failure is correctly attributed; it
does not replace the gate.

### Harness note

The sim exposed a join defect in this pass's harness splice:
cells were attributed through one representative ordinal per
census frame, which starves slots in later bundles when a
bundle boundary splits a position's copies (99 unjoined
cells, 20 unmapped slots on the std sim). The join now tries
every accepted ordinal per frame (first hit wins); base-mode
results are bit-identical before/after (239 / 0 / 452/452,
0 unmapped). The HC sim's distinct loss is NOT a join
artifact — it is confirmed on the resolved stock, above.
Cell-level counts in sim mode (std 367/367, HC 508/538) are
still measured through the representative join and are
reported for completeness only.

## Open items (for Seth's decision — not tuned here, per the stop rule)

1. **Cross-bundle slot reconciliation**: align bundle fits
   into one session lattice at merge (the stitch's relink
   machinery, lifted across reports) so boundary-shared rows
   resolve once. Removes the count doubling and most
   placement sensitivity; the largest of the options.
2. **Position-counted bundle floor**: flush on ≥8 distinct
   positions (and ≥1 viewport progress) instead of ≥8
   frames, so dense streams form bundles with base-like
   position coverage. Small change; targets the head-bundle
   losses specifically.
3. **Dense-pool form arbitration**: the Akbronco FA arises
   when many sightings of one cell pool into a form read the
   sparse stream refuses. Reader/arbitration territory
   (pass 34 machinery), not intake.
4. Accept the sim delta as the cost of incremental finalize
   and re-baseline the sim bar — Seth's call, with the
   production caveat that real captures ARE dense streams:
   the base census's sparse profile is the unrealistic one,
   which is why this report does not recommend (4) without
   (1) or (2).
