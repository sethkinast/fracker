# Fifty-eighth pass — per-cell pack detection (Aya screens)

Verification addendum: #124.

## The ask

Seth's criteria, 2026-11-03 (after the PACK 3 OF 5
screenshot): pass 53's atomic pack rule is replaced
with per-cell detection. The motivating screen (left
Axi A21 Relic, middle Aya, right Meso V13 Relic)
yielded a full `pack_miss` under the old rule, because
the Aya column fails the structural gate and the old
rule credited nothing unless all three captions
proved. Seth's verdict: the non-relic needs no
tracking, but it must not break detection of the good
cells.

## 1. The per-cell rule (`src/packopen.rs`, `src/resident.rs`)

Each of the three columns is now gated and OCR'd on
its own (columns already had separate geometries):

- A cell that proves as a relic under the pass-46
  admission discipline credits +1 Intact, immediately,
  independently of the other cells. Duplicates across
  cells still credit multiset (+2).
- A cell whose column fails the structural gate
  outright — no era icon, no caption glyphs — is
  skipped, not OCR'd and not a miss. This is the Aya
  case, and it uses no dedicated Aya classifier: the
  gate itself is the trivial recognition (no flame
  colour, no "Aya" text read, nothing Aya-specific).
  Skipped cells are logged at instance end
  ("skipped: slot N did not gate as a relic cell").
- A cell that gates as a relic column but proves
  nothing is a per-cell miss, logged at instance end
  ("per-cell miss: slot N unproven"). It never blocks
  the other cells' credits.
- Screen outcome: `pack_open` when >= 1 cell credits,
  `pack_miss` only when 0 cells credit. The
  `pack_miss` reason names only the per-cell miss
  slots (gated but unproven); skipped cells are not
  named, since they were never relic candidates.

The screen classification gate moved with the rule.
`is_pack_screen` was the all-three-columns
conjunction plus dark gaps; it is now dark gaps plus
AT LEAST ONE gated column. The gaps are the
screen-level discriminator that keeps reward and
gameplay frames out (all three reward fixtures still
fail on gaps, and a blank frame fails for having no
column at all), while the column count is no longer
allowed to veto a screen that has relic cells in it.
This also covers a future 1-relic + 2-Aya screen,
which the old gate and a >= 2 gate would both miss.
`PackRead` now carries the per-cell gate
(`gated: [bool; 3]`) alongside the resolutions, and
`detect_pack` runs OCR only on gated cells.

## 2. Exactly-once per cell, chime once per screen

`PackWatch` tracks credit per cell per instance
(`credited: [bool; 3]`, `ever_gated: [bool; 3]`),
replacing the single `proven` flag and the trio
`ReadyToCredit`. A grab that proves a new cell
returns only that cell; a grab that proves a cell
already credited returns `Pending` for it; a cell may
still credit on a later grab while its siblings
already have. An identical screen after an
intervening non-pack frame, or after a conflicting
screen, is a new instance and credits again. The
15 s no-pack-frame / 60 s total disarm backstops and
the <= 6 screens per purchase unit are unchanged.

The chime rule is unchanged in effect and explicit
in code: `pack_open` (whose beacon plays the chime)
is emitted only for the instance's FIRST credit
batch. Later batches of the same instance still
credit stock and regen, but append no second event.
The resident watch tracks this with a `chimed` flag
reset on each new instance (including a `Replaced`
instance), so the sit-until-chime loop behaves
exactly as before: one chime per screen, at >= 1
credit, no chime at 0.

One inherited behaviour is preserved deliberately:
`mark_credited` runs after the credit attempt
whether it succeeded or failed, as `mark_proven`
did — a failed credit still counts as handled, so a
repeat frame cannot double-credit.

## 3. The Aya case, measured

Fixture `fixtures/pack-open/pack-aya.png`, derived
by grayscale conversion from Seth's full-screen dump
`fracker-frames/pack-aya/Screenshot_20261103_230744.png`
(2560x1440 RGBA; the dump itself is NOT committed).
Measured column legs (icon / caption / drop-row
fractions; gates are 0.10 / 0.02 / 0.06–0.45):

- Slot 1 (Axi A21): 0.245 / 0.045 / 0.137 — gates.
- Slot 2 (Aya): 0.000 / 0.000 / 0.171 — fails on
  icon and caption outright (the blue flame sits
  lower and smaller than a relic icon, and the "Aya"
  label sits below it, outside the caption band).
  Its drop-row fraction passes spuriously — the
  column's x-range overlaps neighbouring card
  edges — which is why the conjunction, not any
  single leg, is the gate.
- Slot 3 (Meso V13): 0.156 / 0.051 / 0.110 — gates.

Gaps are dark (max 4.7). Detection proves Axi A21
and Meso V13, skips slot 2 with no OCR (its raw read
is empty), and the screen yields `pack_open` + chime.
The existing 3-relic fixtures still yield 3 credits
each — no regression from atomic to per-cell.

## 4. The fixture gap: why the Aya screen saved no frame

Diagnosis, verified against the call site
(`pack_watch` in `src/resident.rs`): the screen
never classified as a pack frame at all. The audit
save is NOT on a success-only path — `save_pack_frame`
runs for any classified screen's first frame,
proven or unproven alike, exactly as documented —
but it sits INSIDE the `is_pack_screen` branch, and
the "pack screen seen" log line with it. Under the
old all-three-columns gate the Aya screen's middle
column failed, so the whole frame took the non-pack
branch: `note_frame(false)` ended no instance (none
existed), nothing was saved, and nothing was even
logged as a seen screen. The save path was never
reached; the classification gate was the blocker.

No separate behaviour fix was added for this: the
per-cell gate change in this pass is itself what
lets mixed screens classify, and therefore save.
A mixed screen's first frame now saves under
`capture-debug/packs/` like any other pack screen's.
Residual note: a screen that still fails to classify
at all (no gated column, or lit gaps) will still
save nothing — that is the classification gate
doing its job against non-pack frames, and only a
live mixed-screen session can confirm the save
lands in production.

## 5. Superseded wording

The pass-53 atomic rule — "only when ALL THREE
captions prove, credit +1 Intact each; anything less
is a miss, never a partial credit" — is superseded
by this pass and replaced everywhere it appeared in
`packopen.rs` / `resident.rs` (module docs, the
`PackWatch` contract, the `pack_credit` /
`pack_screen_ended` comments). The never-fabricate
rule is NOT superseded: an unproven cell still
credits nothing, it just no longer vetoes its
siblings.

## Explicitly NOT built, per the brief

- No dedicated Aya/non-relic classifier beyond the
  structural gate's trivial recognition.
- No change to backstops, unit cap, or chime
  semantics beyond the >= 1-credit rule above.
- No failure backoff for transient outages, no
  profile long-TTL direction.

## Tests

New/rewritten `packopen` tests (11 packopen tests
total, all passing under the repo's tesseract shim):

- `aya_fixture_credits_both_relics_and_skips_the_middle_cell`:
  the mixed fixture classifies, proves Axi A21 +
  Meso V13, gates `[true, false, true]`, skips slot 2
  with no OCR, credits exactly those two cells once,
  ends proven with no per-cell miss.
- The 3-relic fixtures still resolve their ground
  truth (all columns gating).
- `one_unprovable_caption_credits_the_other_two_and_names_its_slot`:
  the mirrored-caption screen credits slots 1 and 3,
  logs slot 2 as its per-cell miss, owes no
  `pack_miss`.
- `zero_creditable_cells_yield_a_pack_miss_with_no_chime_owed`:
  all-gated, all-unproven ends unproven naming all
  three slots — the resident side emits `pack_miss`
  and no `pack_open` for it.
- `cells_credit_independently_across_grabs_and_instances`:
  slot 1 credits on grab 1, slot 2 alone on grab 2,
  repeats are no-ops, and the same cells credit again
  after an intervening non-pack frame.
- Exactly-once pins (same screen, identical screen
  after absence, conflicting-screen replacement with
  the old instance's credited cells and miss slot
  reported) and the 15 s / 60 s disarm pins, all
  updated to per-cell steps.

Full suite green (see §6 of the coordinator's
report for the count). The resident-side
`pack_screen_ended` / chime-once wiring has no
async test harness in this repo (as before with
`pack_miss`); it is covered by the watch-level pins
plus the fixture tests, and by the live behaviour
Seth will see: one chime per mixed screen.

## 6. Ship

Version 0.5.53: `Cargo.toml` bump plus regenerated
`Cargo.lock` in the same commit (lock diff = the
version line only). `cargo build --release --locked
--offline` green before report. Work commit only —
no merge, tag, or push from this pass (the
coordinator ships).
