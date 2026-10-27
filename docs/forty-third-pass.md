# Forty-third pass — live-trial fixes: refinement-variant relic rule + reward border offsets

Seth's loop trial (2026-10-26 evening) ran the shipped loop on
real pixels. The Baza Prime Stock crack credited live — stages
4–5 work — and the same session produced two defects, both
now fixed.

First: his relic sync imported **43 phantom names** despite
pass 38's owned-gate. The sync came from the REFINEMENT
screen under its ALL filter. Pass 38's gate was built for the
SELECTION variant — a different screen that badges its
unowned tiles. The refinement variant has no badge and no
count label on x1 stacks, and an owned x1 tile is
pixel-indistinguishable from an unowned one (measured on his
25 native frames: production badge scorer peaks at 0.328 over
1,925 cells; name-text luminance identical across classes).
The gate never fired; the label-less catalogue tiles walked
in under the count contract (no label = x1).

Second: a later crack's pick (Steflos Prime Receiver) was
fully bordered at T+14 but read border continuity 0.31 and
missed. The live border sits at offsets top +1 / bottom +2 /
left −2 / right −2 from the assumed slot rect; pass 39's
offset windows — asymmetric at fixture scale and sparse
after x1.447 scaling + dedup ([−1,0,1,3] / [−3,−1,0,1]) —
sampled only the top side's line.

Third (amendment, Seth's evidence later the same evening):
the single T+14 grab's timing assumption itself fails in
ENDLESS fissure rotations — see §3. The grab is now a burst.

## 1. Variant detection (`capture.rs`, `ocr.rs`)

The two variants share `ScreenClass::Relics`; the pass
separates them per frame.

**Header signal (the live path).** The refinement screen
names itself: its header title is "VOID RELICS /
REFINEMENT", and the header also carries COLLECTED
counters the picker lacks. `ScreenClassifier::relic_variant`
applies the classifier's own squash + fuzzy discipline to
the header text the session already read: REFINEMENT
present (2-edit allowance on the long keyword) ⇒
Refinement, else Selection. Measured on the corpus: the
word read **verbatim in 25/25 headers**. The session
computes the variant beside the brightness reference and
hands it down (`read_frame` → `ReadOptions.relic_variant` →
`read_cells`).

**Reads inference (headerless seams).** Folder ingest has
no header, so there the variant is inferred from the
frame's reads: a held read carrying a refinement tier (a
`[Radiant]`-style second line) proves the refinement
screen, whose stacks are tier-split. Measured: corpus
frames carry 1–24 tier reads on tier-bearing pages; the
selection strip's tiles read Intact on every cell
(real-engine re-measure, pass 43 harness). Failure mode,
accepted and documented in the code: a refinement page
whose visible stacks are all Intact infers Unknown and
keeps the old behaviour. The live path never relies on
inference.

**Filter state — the safe default, taken.** The brief's
OWNED branch (label-less tiles sound when every tile is
owned) needs an ALL-vs-OWNED signal. The corpus headers
offer none: the owned-related control is a sort dropdown
("OWNED ▾", beside SEARCH) under which the full catalogue
renders in all 25 frames, and the header's "ALL" labels
the era-filter row, not an owned filter. There is no
owned-visibility state to calibrate against, so refinement
frames take the label-only rule **unconditionally**. If
Seth ever captures an owned-only refinement state, the
OWNED branch can be calibrated then; until it exists, a
lost x1 sync is recoverable and a poisoned bucket is the
bug being fixed.

## 2. The refinement rule (`ocr.rs`, `resident.rs`)

`relic_refinement_rule` runs in `read_cells` immediately
after the pass-38 gate (and in `ingest_folder`, same as the
gate): on a Refinement frame, every held tile with **no
parsed count label** is refused — `held = false`,
`refine_excluded = true`. A parsed label is the only
positive owned evidence this variant offers; a refinement
second line is not evidence (Meso L1, which Seth does not
own at all, imported as Radiant x1). Labelled tiles are
untouched; like the gate, the rule only ever removes holds.

Refusals are observable in the gate's style: the session
collects the names (first-exclusion order, deduplicated)
and the finalize log gains a line beside the gate's —

```
relic refinement rule: excluded N label-less tile(s) (no count label = no owned evidence on the refinement screen): <names>
```

**Corpus verdict** (production read path, Seth's 25
frames; the pre-rule replay reproduces the live scrape
exactly — 517 stacks held, the poisoned stock's count):

- All **39 label-less phantom names** the live scrape
  imported (38 derived from the stock-2/stock-3 diff plus
  the owned collateral below) are refused — **0 held**.
- **411 labelled stacks still import**, including the live
  scrape's four labelled newcomers at their exact counts:
  Meso S7 x14, Neo N1 x8, Meso X1 x2, Neo C10 x2.
- The poisoned stock's three in-both singleton Radiant
  tiers (Axi V1, Lith A2, Meso A8) are label-less reads and
  are refused too.
- **Collateral, on record:** owned label-less stacks are
  refused with the phantoms — Meso V15 (Seth owns both
  tiers; both read label-less) and every owned x1 stack. A
  refinement sync now captures exactly the labelled
  population (stacks of 2+). x1 stacks keep syncing from
  owned-only screens, and cracks + EE.log keep them
  current; the finalize line names everything refused, so
  the smaller bucket reads as a decision, not a loss.

One honest caveat on pass 38: its gate is also inert on
this corpus in the production path (eye scores peak at
0.328, under the 0.60 floor — on the native tail page the
badges sit off the template's measured phase). On its own
strip fixture, hand-cut at the measured phase, the same
scorer separates cleanly (owned ≤ 0.07, unowned
0.81–0.97). The label rule makes refinement import safety
independent of that question; the selection variant's live
verification is still owed.

## 3. Reward border offsets (`reward.rs`)

The offset sets are replaced by a **contiguous sampling
window**: every integer offset in [−3s, +3s] on all four
sides, s = frame width / 1769 ([−3..3] at fixture scale,
[−4..4] at 2560 wide), generated as a range — scaling can
no longer punch holes in it. Floor (0.55) and margin
(0.30) are unchanged.

One aggregation refinement was forced by measurement, and
it is the smallest one that satisfies the gate. Under the
old per-sample union, widening the window inflates
unbordered slots: on the live frame, slot 1's bottom side
unions **0.394** (the per-card progress bar under every
card sits at the same +2 offset as the winner's bottom
border line) and its right side 0.178, totalling 0.143–
0.224 depending on window width. The per-side
decomposition proves no window containing the winner's
four lines (top +1, bottom +2, left −2, right −2) can push
that slot under 0.10 — the pollution shares the winner's
offsets. So a side now contributes its union samples only
when the side **carries a line**: its best single-offset
continuity must clear `SIDE_LINE_MIN` = **0.60**. The two
real frames separate cleanly — true border sides bottom
at 0.77 (the fixture's bottom line straddles two rows,
0.57/0.77), scene sides peak at 0.42. For a genuinely
bordered card all four sides qualify and the score is the
old union score unchanged: fixture winner **0.962**, live
winner **0.994** with the other slots at **0.00** (they
read 0.14–0.23 under window-union alone, and the bordered
slot read 0.31 under pass 39).

### Burst timing (amendment, 2026-10-26 evening)

Seth supplied the saved frames from four more misses that
night. Three of the four T+14 grabs had landed on
non-reward screens entirely — the next rotation's SELECT
RELIC screen, mid-wave Defense gameplay — and the
classifier had correctly refused them: those grabs were
not detection failures, they were LATE. In endless
rotations the reward screen closes when picks + the
continue/extract vote complete, often before T+14. In
extraction-style missions the screen persists the full
15 s and T+14 is right (two of the night's frames show
the countdown at 2 at grab time).

A first burst draft (anchored at T+8 relative to arming)
died on a second measurement the same night: the EE.log
trigger itself can trail the screen's appearance by
~10 s — tailing capture.log live, Seth saw the "armed"
line with only **5 s remaining** on the reward timer, so
an armed+8 sample can land at screen+18, after the
close. The burst therefore samples **from T+0 at arming,
roughly every 2 s through T+14** (`BURST_OFFSETS_SECS` =
0/2/4/6/8/10/12/14; the stream is already open at T+0 —
sample it, don't wait; the live path snapshots the frame
tap at each offset, the standalone path snapshots its
drained latest the same way, each with a brief grace for
the first frame after arming). Late-armed episodes are
caught by the first samples. Every frame that classifies
as the reward screen is scored under the unchanged
per-frame gates — classifier, border floor + margin,
name discipline — and the credit comes from the
**latest** burst frame that clears them.

Latest-wins is a requirement, not a preference, because
of how the screen opens: the player's **own rolled
reward is pre-selected** — a border is present from
screen-open, before any click. A first-clearing rule
would credit that default selection even when the player
then picks another card; only the latest clearing frame
reflects the player's actual choice (and if the player
never changes it, the default IS the pick, so an early
clear is still correct — which is also why an
early-clearing frame still credits when the screen
closes early: a border cannot un-happen). The accepted
residual, on which Seth confirmed the design: a pick
switched in the final ~second, after the last readable
frame, credits the earlier selection.

If no frame clears, the miss line reports the closest
attempt — the best continuities seen and which frame
they came from (a name-tier near-miss outranks a
border-tier one). Every burst frame is audit-saved
(`reward-<stamp>-t<offset>.png`). One burst per episode;
the arming/coalescing is unchanged.

## 4. Gates

- **Corpus regression** (`refinement_corpus_replay_refuses_the_phantoms`,
  strip-test pattern, skips when the corpus is absent):
  replays all 25 frames through the production read path
  with header-derived variants; asserts the variant
  classification on every frame, zero phantom imports, the
  collateral refusal, the three singleton-Radiant refusals,
  the four labelled newcomers at exact counts, and a broad
  labelled-import floor (>= 290 of the measured 411).
- **Reward fixtures**: the pass-39 fixture still wins
  slot 0 in the 0.9s; the live frame resolves to slot 2
  at >= 0.90 with the other slots <= 0.10. Both skip when
  their image is absent.
- **Unit tests**: the rule (labelled kept / label-less
  refused; inert on Selection and on Unknown-without-a-
  second-line; variant inferred from a second line) and
  the header variant classifier, incl. garbled headers.
- **Burst selection**: latest clearing frame wins (incl.
  a mid-window pick change); early screen-close keeps
  the early pick; a late-armed episode is caught by the
  first samples; no border anywhere misses and reports
  the best frame's continuities; no classifying frame is
  a plain miss — at the selector level, and again at the
  detect level on synthetic screens.
- Full suite: see addendum #108 for the tally. Census
  gates untouched (no capture-path changes beyond the
  relic read seam).

## Live verification owed

Seth's next refinement sync: the finalize log should carry
the refinement-rule line and the stock should gain no
phantom names. His next cracks: bordered picks at native
resolution should credit, including late picks whose
border sits off the assumed rect — and in endless
rotations, where the burst should catch the pick before
the screen closes.
