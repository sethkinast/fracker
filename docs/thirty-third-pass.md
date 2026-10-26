# Thirty-third pass — session stitch, shared hold predicate, coverage accounting

Capture used to read parts frames as they arrived: each frame
was OCR'd on its own lattice and the results merged by name.
That design threw away the structure of the capture itself —
the frames are successive viewports over ONE sorted list, so a
part that scrolls through five frames is one physical slot with
five sightings, and reading it five independent times wastes
four of them and lets any single bad read mint stock.

This pass rebuilt parts finalization around the session
(reviewer proposal Q1–Q6, commissioned as the pass-33 brief):

- **Per-session pitch fit** (`src/stitch.rs`): fold-sharpness
  grid search over the session's frames fits row and column
  pitch separately (the layout's 180.65px row constant measured
  185.0px true). A deviation beyond tolerance starts a new
  segment (resize/scale event) rather than forcing one lattice
  over both.
- **Greedy linking with loop closure**: consecutive frames link
  by vertical overlap correlation; accepted links must close
  the position loop to ≤1px over a segment.
- **Canonical crops**: a slot's usable instances (fully
  visible, adequate viewport margin) are composited — pixel-wise
  median at ≥3 sightings, best single below — and cut on integer
  boundaries (bilinear resampling was measured to degrade OCR).
- **Slot agreement protocol**: the canonical crop AND every
  usable instance are read; a slot holds a name only when it is
  the ONE name any of its reads holds. Two different held names
  make the slot contested — refused and flagged.
- **One shared hold predicate** (`Universe::hold_permitted`,
  addendum #97): the margin matcher and both radius-decoder
  paths gate on the same head-witness + no-clean-strict-subset
  rule, replacing pass 29's margin-path-only head guard and
  pass 30's decoder-local subset guard. The token-subset
  property battery exercises every hold path over every
  (item, token-subset) pair of the parts universe.
- **Ring-gated counts**: a count badge is read only when the
  badge ring template matches (NCC ≥ 0.50); the built-copy
  glyph's small circles calibrate at ≤ 0.34 and never mint a
  count.
- **Coverage accounting**: every slot reports Held /
  SeenNotHeld / NeverFullyVisible; finalize surfaces the
  counts, so a capture's blind spots are stated, not silent.
- Count votes are consumed only for held slots.

## The gate failed, and this pass did not ship

The brief's ship gate (production harness, session mode, both
censuses) required zero false accepts and ≥159 distinct held
(standard) / ≥157 (HC). The build measured:

| | distinct | false accepts | Neuroptics |
|---|---|---|---|
| standard | 157 (bar 159) | **1** | 24 (bar 25) |
| high-contrast | 154 (bar 157) | 0 | 25 |

Counts passed on both censuses (standard 303/303 held cells;
HC 92.4% of held cells, no worse than pass 32's 92.3%), and all
46 session frames stayed alive. The failure is two measured
effects, neither fixable by tuning without breaking the other:

1. **The false accept is an agreeing-wrong read.** *Panthera
   Prime Blueprint* was held for an *Acceltra Prime Blueprint*
   slot, and two independent sightings' reads both reduced to
   "Pantera…" — the stitch's agreement rule, designed to refuse
   disagreement, cannot refuse agreement, right or wrong.
   (An earlier build of this pass shipped the stitch with a
   looser protocol and measured std 157 + the same FA; the
   agreement protocol is what it is — the cell is the known
   residue class the reviewer assigned to stage-3
   analysis-by-synthesis.)
2. **The predicate's designed refusals cost real recall.**
   The shared predicate refuses fragments whose witnesses sit
   inside a cleaner sibling (e.g. a "mus" fragment of *Magnus*),
   and strip OCR on fitted cells proved phase-sensitive — the
   same cell reads clean at −8px and garbage at 0 — stranding
   slots whose single fitted phase is the bad one. The gate
   bars themselves had been calibrated against prototype
   numbers achieved WITH false accepts in them (the regrid
   ablations), so part of the shortfall is the bars pricing in
   holds an honest build cannot make.

Seth's direction on the failure (2026-10-25): do not ship a
gate-failed build, do not lower the bars, and do not tune to
the failing cells. The stitch stack stands uncommitted as the
foundation for Pass B (the second-reader pass,
`docs/thirty-fourth-pass.md`), whose independent line reader
is the designed answer to both effects: an agreeing-wrong band
read must now also convince a structurally different reader,
and the strip's phase sensitivity is addressed by a uniform
multi-offset read policy. The combined stack ships as one
release only if Pass B's gate passes in full.
