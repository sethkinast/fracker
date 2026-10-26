# Thirty-seventh pass — settle+time-gated intake, observability wiring, settled-census gate swap

v0.5.30's first full live run closed the pass-36 defect: 264
parts written, the best live result to date, with Seth's
page-at-a-time pacing (~3–4 s rests, animated background on).
His verdict: good enough; the residual ~29-item delta against
his 293 total is handled by manual reconciliation (some missed
items are already built; the delta self-heals as items cycle
and EE.log remains the source of truth for new acquisitions).
This pass is therefore a cleanup pass, commissioned by
`PASS37_BRIEF.md`: make the intake admit only settled
viewports at a human scroll pace, wire the observability whose
absence made the v0.5.30 run's evidence undecidable from its
own dump, and swap the canonical gate to the settled census.
Recognition is out of scope — no reader, margin, confusable,
or synthesis changes.

Seth's capture model, which the gate encodes: start capture,
get feedback when a viewport is captured, scroll, wait,
repeat. No fast-scroll handling. Acceptance criterion: at
most ~1 capture per couple of seconds; a page rested on is
captured exactly once per position (the pass-36 pool of
K = 4 instances per position is unchanged — "captured" here
is the position's admission, its pool instances arriving at
the gate's cadence).

## Settle gate (scope 1a) — `stitch::lattice_phase`

The snap study measured that settled viewports share one
global lattice phase (0.00–0.02 px modulo the layout pitch)
and transients sit at +14/+48/−65 px: phase, not sharpness,
separates them. The stitch already owns the machinery (the
row profile and phase fit its scroll estimator uses), so the
gate reuses it rather than introducing a second estimator:

`stitch::lattice_phase(frame, layout) -> Option<f32>` folds
the grid's gradient row profile at the LAYOUT pitch and
refits the phase, accepting the fit only under the session
planner's own "real fit" test (fold mass > 1.0, peak > 4×
the mean bin, ≥ 8 active rows). `None` means the frame
carries no measurable lattice (blank, synthetic, too little
content) — the gate passes such frames and the content
policy judges them; a frame whose lattice cannot be read is
not evidence of motion.

Measured on the real corpora before the threshold was set
(probe over the census frames, the live dump, and the
fixture suite):

| corpus | settled / landing | transient |
|---|---|---|
| census (15 frames) | −0.11 (all 12 settled) | +13.53, +47.50, −65.16 |
| live dump (60 frames) | −0.11 settled; +0.53/+2.78/+1.23 landings | −19.73, +13.54, +50.37, +77.95, +19.34 |

The gap is (2.78, 13.53). The band is
`SETTLED_PHASE_MAX_PX = 8.0` px — ≥ 5.2 px of margin on both
sides of the measured classes. (One estimator note: the
fold reads parts-52 at +19.34 where the snap study's
neighbour cross-correlation read +30; different estimators,
same classification — off-lattice by a wide margin either
way.)

## Time gate and latch (scope 1b) — `PartsIntake::offer_at`

Arrival time is a caller input, not a clock read buried in
the policy: `offer_at(frame, layout, now)` keeps the intake
pure and unit-testable, and `CaptureSession` supplies the
time through an optional feed clock (`set_feed_clock`) —
unset in production (the real clock), modelled by the
replay harnesses (below). The pre-37 `offer` remains as the
real-clock wrapper.

Gate order, before the pass-36 content policy:

1. **Session cap** (unchanged accounting; first, so a
   tripped session stays cheap).
2. **Phase gate** — |phase| > 8.0 px → `DropOffLattice`,
   counted. Charged even inside the time interval: a scroll
   performed during the 2 s window still has its transients
   refused on arrival, and the phase is the cheapest strong
   signal the intake computes.
3. **Time gate** — an offer arriving < `MIN_ACCEPT_INTERVAL`
   = 2 s after the last ACCEPT → `DropInterval`, counted.
   Drops never move the clock (a stream of drops cannot
   postpone the next admission indefinitely), and the clock
   is session-scoped: it survives screen changes, so leaving
   and returning to the parts screen does not reset the
   pace.
4. **Content policy** (pass 36, unchanged) — with one
   addition at its head, the **latch**: the print a new
   position's acceptance displaced is held as
   `latched`; a candidate matching it is `DropLatched`,
   counted, and earns no progress credit. This refuses
   exactly the A→B→A immediate return inside one bundle.
   The latch is single-deep by design (an A→B→C→A scroll
   re-admits A — it is a genuine new look, and the stitch's
   dy continuity, not the intake, owns ordering) and is
   cleared wherever pass 36 already invalidates position
   continuity: bundle flush and screen reset.

Pass-36 failsafes are untouched: 512 accepted-frame session
cap, 96-frame bundle queue with counted drops, 900 s session
duration cap, K = 4 per position, bundle geometry and the
reader instance cap as shipped.

The counters the new verdicts increment are session-visible
(`parts_off_lattice_dropped`, `parts_interval_dropped`,
`parts_latched_dropped`) alongside the existing accepted /
duplicates / cap-refused accessors.

## Observability (scope 2)

LIVE_DIAG2's three findings, closed:

- **Intake counters in the finalize log.** `finalize_capture`
  now records an `intake:` line (accepted / duplicates /
  off-lattice / interval / latched / session-cap refused)
  and a `stitch path:` line naming the path the session
  actually took — "background stitch thread (dedicated
  second engine)" vs "synchronous fallback (frame engine,
  capture thread)" (`CaptureSession::stitch_path_label`).
  The v0.5.30 run's central ambiguity — dispatcher or
  fallback? — is now a log line, not a code-reading
  exercise.
- **Debug-dump cap is recorded.** The dump's per-name
  60-frame cap is counted as it engages
  (`debug_capped_names`), and the finalize log carries one
  line per capped name: "debug dump cap reached for
  <name>; further frames not dumped". The capped folder
  can no longer masquerade as the capture's end.
- **Bundle-queue drops logged at drop time.** The capture
  worker's progress callback (which runs after every frame)
  watches `bundle_queue_dropped_frames()` and records each
  increment with the session total, instead of the drop
  surfacing only in the finalize failsafe summary.

## Gate swap (scope 3)

The canonical gate is the settled census
(`census-settled/`, 12 settled frames, 396 cells, 209
distinct; built and baselined against v0.5.29 before this
pass). The full 15-frame census and the HC census remain on
disk as history; HC stays retired as a gate (Seth does not
use the HC theme). The dense duplicate-inflation simulation
is rebuilt on the settled set (×8 jittered copies per
frame). A new gate driver, `examples/live_replay.rs`,
replays Seth's real v0.5.30 dump through the production
session intake.

The harnesses model arrival times through the feed clock —
an input to the gate, never a policy knob: base gate one
frame per 3000 ms (Seth's page pace), sim and live replay
one per 500 ms (the measured worker delivery rate,
LIVE_DIAG2 §4).

## Gate

| gate | bar | result |
|---|---|---|
| settled base | baseline reproduced exactly: 191 distinct / 0 FA / Neuroptics 23/23 / counts 367/367 cells, 191/191 slots | **191 / 0 / 23/23 / 367/367, 191/191 — exact** |
| settled dense sim | distinct ≥ 191, 0 FA (SETTLED_CENSUS §5) | **193 / 0 FA / 23/23**; counts 365/395 cells, 190/193 slots (note 3) |
| live replay | ≤ 21 accepts, no settled page lost, phase classification = INTAKE_REPLAY §3 | **13 accepts; pages hold 3/4/4/2 accepts; 5 transients off-lattice, 55 frames on-lattice** |
| suite | green, new tests for gate/interval/latch/counters | **298 green (293 lib + 5 bin), incl. 6 new pass-37 tests** |
| locked release build | `cargo build --release --locked --offline` | at ship time, below |

Intake lines, as printed by the harnesses:

- base: 12 accepted, 0 duplicates, 0 off-lattice,
  0 interval, 0 latched, 0 cap-refused; queue dropped 0.
  The new gates are inert on settled, well-paced input —
  the pass changes nothing for the canonical capture.
- sim: 44 accepted, 0 duplicates, 0 off-lattice, 40
  interval, 4 latched, 0 cap-refused; queue dropped 0.
  (96 copies fed; 88 reached the intake — the remaining 8
  jittered copies fail header classification upstream and
  are never offered, the same accounting the production
  path gives them.)
- live replay: 13 accepted, 5 duplicates, 5 off-lattice,
  37 interval, 0 latched, 0 cap-refused — 60 offers, every
  one accounted. The trace is the design working on Seth's
  own frames: each rested page pools its instances at the
  2 s cadence (accepts at frames 0/4/8, 13/17/21/25,
  35/39/43/47, 53/57 in 500 ms-modelled time), page 2's
  pool fills to K = 4 and its remaining rest frames drop
  as duplicates, and all five transients are refused by
  phase before the content policy ever sees them.

## Deviations and notes

1. **`offer` → `offer_at` migration.** Making arrival time
   explicit is the one signature change; `offer` survives
   as the real-clock wrapper so no other caller moved.
2. **Fixture fidelity fixes (test data only).** The gate
   reads fixtures honestly, and two fixtures were not
   honest. (a) The mosaic fixture's cells folded to an
   arbitrary phase — synthetic block content has no
   dominant name-band edge, and the fold keys on exactly
   that edge in real frames. `mosaic_frame` now paints a
   name-band edge per cell at the calibrated fraction
   (production pitch 185.41 × name-band 0.8054, the snap
   study's numbers), with seeded per-cell level and span —
   real names differ per cell, and a uniform line pulled
   every cell's thumbnail toward identity, measurably
   collapsing distinct fixtures into "same viewport". With
   the edge, fixtures measure −1.40 px like real settled
   frames, and a +10 px synthetic shift measures +10.71 —
   the gate separates them. (b) The bundle-flush test's
   panorama painted rows from y 0 while its windows' grids
   start at y 60, putting every window ~11 px off the
   layout lattice; the panorama now carries the same 60 px
   header the layout expects. (c) The session count-vote
   test feeds its three frames through a modelled feed
   clock — they are three scroll observations, and the
   time gate correctly refused them when they arrived
   microseconds apart.
3. **Sim counts denominators.** The sim's 365/395 cells
   (base: 367/367) is the dense-feed join effect seen in
   every prior sim — jittered copies attribute more cells
   across split bundles; §5 pre-registers the sim bar as
   distinct + false accepts, and both hold with margin
   (193 ≥ 191, 0 FA). Reported here in full rather than
   silently.
4. **The latch fired 4× in the sim, 0× in base and live.**
   The sim's jittered copies occasionally match the print
   displaced by the latest acceptance; pre-37 those copies
   re-entered as stale-position instances. The refusal is
   the latch's designed case.
5. **No gate was dropped.** The brief's fallback (gate the
   sim on the live-replay trace if the sim harness could
   not be adapted) was not needed: the sim adapted
   mechanically via the feed clock, and the live-replay
   gate runs as well.

## Open items (unchanged by this pass)

- Partial-capture guard: still deferred by Seth's ruling;
  finalize remains wholesale-replacement per seen bucket.
- Required quantities (#94) and the surplus-parts goal:
  queued / deferred, not built here.
- The 18 settled-census residue items are the name-
  separation limit documented in pass 35; recognition work
  remains out of scope until Seth commissions it.
