# Forty-fifth pass — the refinement ×1s come back: the native eye test

Verification addendum: #110.

## The problem

Pass 43 made refinement syncs safe the blunt way: a label-less tile
on the refinement screen could not be told from an unowned
catalogue tile, so every label-less tile was refused. Seth rejected
the cost the same night: the screen shows his ×1 relics label-less,
and he does not want to scan twice. His model of the screen, stated
2026-10-27, is three-state — a number glyph is owned ×N (N > 1), no
glyph is owned ×1, an eye glyph is unowned — and the discriminator
research (`REFINEMENT_X1_DISCRIMINATOR.md` in the frames workspace)
found the statistic that reads it: the eye is present on the native
frames, and bright-pixel mass in the tile's eye window separates the
corpus perfectly (unowned 69–71, owned at most 20, over 527 settled
label-less appearances). Pass 38's template gate failed on these
frames on registration, not signal: its blob sits ~6×8 px off the
native position and the scorer tolerates no translation.

## What shipped

The disposition is a three-state rule evaluated at SESSION level,
because the statistic is only valid per settled appearance and the
rule takes the maximum over a stack's appearances:

- A stack with a parsed count label on any settled appearance
  imports at that label (the vote machinery, unchanged).
- A label-less stack whose eye-window bright mass reaches **40** on
  any settled appearance is unowned: excluded from the import and
  named in the finalize line — the same surface pass 43 used, now
  listing only the truly unowned.
- Any other label-less stack imports at ×1.

40 sits inside the research's zero-error band [21, 68]; the shipped
window statistic is production-shaped (the cell's own eye-window
crop, canonicalised to the window's 40×38 grid, pixels brighter than
70), so the threshold's evidence is the corpus replay below, not the
research grid's exact numbers — on production crops the 11 unowned
Axi read 53–69 and every other measured stack's maximum is 0.

Per-frame code only measures (`relic_refinement_measure` annotates
held label-less refinement results with their mass, after the pass-38
owned-gate, variant-gated exactly as the pass-43 rule was);
`RefinementEvidence` aggregates across the session (label presence
per stack + maximum mass over measured appearances) and both stock
resolution paths consult it: the finalize wholesale replace and the
post-pass-37 restore branch exclude the same convicted stacks, and
folder ingest folds the same evidence. The selection variant is
untouched (owned-only screen; the measure never runs there).

One veto, found by the corpus replay and recorded here as a
deliberate extension of the brief's rule: a tile whose count region
carries label-scale bright mass (≥ 100 resampled bright px; real
labels read 122–170, the eye glyph reads 71, a bare corner 0) is
physically labelled even when its label fails to parse, and is never
eye-measured. The corpus's Lith M4 is the case: its ×15 label sits
at the count region's edge and parses in neither appearance, and its
strokes put 52 bright px inside the eye window — over the conviction
floor. Without the veto the disposition would have excluded a stack
Seth owns 15 of; with it, Lith M4 imports (at the vote-resolved
count — the label itself remains unread, a pre-existing parse gap,
not a disposition error).

## Corpus replay (the 25-frame live corpus, production reads)

- Pre-disposition baseline reproduces pass 43's starting point:
  517 held stacks, 411 of them labelled.
- Import after disposition: **506 stacks / 3,614 items** = 411
  labelled (each at its voted count — the pass-27 vote machinery is
  untouched; where a stack's label reads disagree across appearances
  the mode arbitrates, as in pass 43) + 94 owned ×1s + 1 garble pair
  (below).
- Excluded: exactly the **11 Axi** (A2, A3, A6, A7, A8, A12, A13,
  A15, A19, A23, B7 — all Intact), named in the finalize line. Their
  production eye masses: 53–69. Every other measured stack: 0.
- The ×1s restored include all 65 stock-2 singleton pairs whose
  corpus tiles are label-less, Meso V15 in both tiers, the Axi V1 /
  Lith A2 / Meso A8 Radiant singles, the 26 Meso/Neo ×1s pass 43
  listed as phantoms, and Axi K2 — bare in the corpus, absent from
  stock-2 (a gain that evening or its scroll's coverage gap; owned
  ×1 either way). Three stock-2 singletons (Axi A21, Lith M7, Lith
  T14) read labelled ×2 in this corpus and import with the labelled
  population — the corpus is authoritative for the corpus.
- **Meso C11 [Radiant] verdict**: the production reader names the
  cell "Meso L1" in both settled appearances (frames 22 and 24; raw
  "Mesoll Relic [Radiant]", held, label-less, eye mass 0), so the
  pair imports at ×1 Radiant under the garbled name. Recorded as a
  named known issue per the brief: a wrong name on a real owned ×1
  stack, not a phantom — stock gains "Meso L1" ×1 Radiant and misses
  "Meso C11" ×1 Radiant until a name-OCR fix or a sync from another
  screen corrects it. The corpus gate pins this behaviour so the fix
  lands deliberately. (The tile's Intact sibling reads correctly:
  Meso C11 ×5, frame 13.)

## The pass-38 gate: recommendation (not actioned)

The owned-gate in `relic_owned_gate` still spends a template scorer
that the pass-43 measurements showed inert on native refinement
frames (NCC peak 0.328 over 1,925 cells — it has never fired there;
its strip fixture separates cleanly, 0.81–0.97 vs ≤ 0.07, so the
code is not broken, its registration is). Refinement safety no
longer depends on it: the variant rule, and now the eye disposition,
carry that screen. The gate's live question is the SELECTION variant
(a browse-type capture there is where a catalogue could still
poison stock), and that variant's live verification is still owed.

Recommendation, for Seth's decision — this pass changes nothing
about the gate:

1. **Keep the gate, re-point its eye leg at the bright-mass
   statistic** (defence in depth with a sensor that provably reads
   native frames; the mass test is also cheaper than the template
   correlation). The gate's other legs (label, icon-brightness
   cluster) are unaffected.
2. Or **retire the template leg** once a selection-variant capture
   has been live-verified without it, accepting the label +
   brightness legs as the browse defence.

Silently deleting it is the one outcome to avoid: the gate is
load-bearing in the design record for the browse screen, and the
selection variant has never been exercised live.

## Tests

Full suite green: 378 passed / 0 failed (lib 350; golden 15;
packaging 7; resident 5; tray_quit 1), up from 375 at pass 44. The
corpus replay was rewritten as
`refinement_corpus_replay_three_state_disposition`: it folds the
session's evidence from production `read_frame` results and asserts
the whole disposition — the 11 Axi excluded and named, every
ground-truth label-less pair at ×1, the labelled population at 411
with voted counts, Lith M4 held, the L1 garble pinned, totals
reconciling at 506. Unit tests cover the statistic (threshold
exclusive, grey-at-threshold counts zero, undersized cells read
nothing), the measure (annotation, variant inertness, the
label-region veto), and the evidence (three-state disposition,
label-wins, session-max across a smeared transient, tier-precise
exclusion in `apply_to`). The pass-43 strip test now asserts the
statistic separates the strip's own tiles (its painted badges ≥ 40,
bare tiles 0) instead of the old refusal shape. Census gates
untouched.

## Records

- Brief: `~/workspace/fracker-frames/PASS45_BRIEF.md`; evidence:
  `REFINEMENT_X1_DISCRIMINATOR.md` (same directory).
