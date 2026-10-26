# Thirty-fourth pass — Pass B: the line reader, form channels, arbitration

Pass 33's gate failure had two measured causes (see
`docs/thirty-third-pass.md`): an agreeing-wrong band read no
single reader can refuse, and designed predicate refusals plus
strip phase sensitivity costing recall. This pass — Pass B of
the reviewer's programme, built on the uncommitted pass-33
tree — answers with independence: a second, structurally
different reader of the same cells, positive form evidence in
both directions, and an arbitration layer that only holds what
the evidence licenses. The combined stack (passes 33 + 34)
ships as v0.5.28 under this pass's gate.

## The line reader (`src/lineread.rs`)

Per cell, over the name band:

- **Detection**: white top-hat (grey opening, r≈3px disk) kills
  the background gradient and the blueprint mesh's low-frequency
  component; the row-mass profile of the residual is smoothed
  and thresholded at 30% of peak; runs are bridged over ≤6px
  gaps. The text block is the SPAN of all runs carrying ≥12% of
  the strongest run's mass — a deliberate hardening of the
  prototype's max-mass-run block, whose dropped-line behaviour
  was stage 2's measured form-minting mechanism. Line count is
  the block height over the scale-relative pitch (19px at the
  nominal band), splits fall at profile minima near the
  equal-split points, and boxes are x-trimmed by column mass.
- **Reads**: per-line tight crops from the ORIGINAL pixels,
  PSM 7, one batched engine call per segment; the assembled
  read (lines joined in order) runs through the SAME matching
  stack as the band — margin matcher, then radius decode with
  the band as its own witness pool, then strip decode with
  line 1 as the strip — and the SAME shared hold predicate
  (`Universe::hold_permitted`) gates every hold, with pass 32's
  evidence floor intact. The token-subset property battery now
  exercises this chain as its fourth path (0 violations; the
  documented form-collision carve-out unchanged).
- **Form safety is part of the reader**: a per-line artefact
  cannot mint a form, because a hold still needs the assembled
  evidence to satisfy the predicate, and (below) the form
  channels to license a lone reader's form.

Under the stitch's agreement protocol (canonical + every
usable instance, exactly one distinct held name), the line
reader alone holds 232 distinct standard / 221 HC at zero
false accepts — the dominant reader, as the regrid prototype's
union arithmetic (B∪C = 216/217) predicted in shape if not in
magnitude: median canonical crops and multi-draw agreement are
worth more in production than they were per-frame.

## Form channels (brief §2) — measured before trusted

Form is asserted only on positive evidence, in both
directions. Each channel was measured per slot on both
censuses before the arbitration was allowed to use it.

**(a) Blueprint line / certified absence.** The assembled read
witnesses a "Blueprint" line (a token within confusable
distance 2 of *blueprint*) on 120/120 standard and 133/133 HC
blueprint-truth slots, and on 0 built-truth slots on either
census. The detector certifies a band complete when no
significant run is unaccounted: leftover run mass outside the
block ≤ 0.05 of total and run mass within 1.2 pitches below
the block ≤ 0.20 (the benign floor is tile-edge smear at
0.06–0.09; a genuinely dropped line contributes ≥ ~0.25). At
those thresholds certification covers 153/155 standard and
140/141 HC built-truth slots with **zero dangerous
certifications** (certified + no witness on blueprint truth)
across 682 slots.

**(b) Mesh.** Blueprint tiles carry a background grid mesh in
the band. Two statistics were measured: the top-hat residual
in the band's side margins, and the raw gradient energy there.
Both separate weakly on per-instance crops (blueprint median
6.07 vs built 5.78 gradient energy) with long overlapping
tails — and two traps were measured and avoided: median
canonical crops blur the 1px mesh across instances' sub-pixel
phases (separation nearly vanishes), and a backward-difference
statistic's apparent separation on nominal crops is a
phase/border artefact (the same cell scores 6.23 backward,
5.84 forward). The channel therefore asserts only in its
tails (blueprint ≥ 7.2, built ≤ 5.85 on per-instance scores,
slot-median aggregated) and is never the sole carrier of a
form decision: channel (a) carries ordinary assertion, the
mesh breaks only what (a) leaves open. The arbitration result
is flat across the whole threshold plateau (bp-min 6.55–7.6 ×
built-max 5.70–5.95 → 230–231 standard distinct in simulation,
0 FA throughout).

**(c) Built-copy glyph.** The figure marker at the tile's
top-left (Seth-confirmed semantics: the cell is a blueprint
AND a built copy is owned). The detector is an NCC template
(the average of three sighted windows) searched over the
badge window; floor 0.55 against a measured built-truth
maximum of 0.31 on both censuses. It fires on 34/120 standard
blueprint slots, 0/155 built (HC likewise 0 built) — mostly
main blueprints, whose built forms are not parts-universe
items; of the seven component blueprints whose built twin is
also census-visible, five fired. The implication (glyph ⇒
the built sibling is held stock, addendum #89 makes both
forms first-class) was enabled on this measurement: detector
false-positive rate on built cells is zero on 682 slots.

**Verdicts.** Blueprint evidence: a witnessed Blueprint line,
a blueprint-tail mesh, or the glyph. Built evidence is a
conjunction: certified no-third-line AND no witnessed
Blueprint line AND built-tail mesh AND no glyph. (The
no-witness clause is a calibration find: *Wukong Prime
Neuroptics Blueprint* has a built-tail mesh score, 5.82, on a
slot whose Blueprint line IS witnessed — without the clause
the channels read Contested and the slot is refused.) Any
positive channel with none contradicting asserts the form;
channels pointing both ways refuse it.

## Arbitration (brief §3)

Per slot, each reader's agreement verdict and the channels
combine: both readers holding the same name holds it, unless a
form channel contradicts the held form. Exactly one reader
holding requires the channels to SATISFY the held name's form
(a refusal to hold is not a contradiction; a different held
name or a contradicting channel is). Different held names
refuse the slot. Names without a form sibling in the universe
are exempt from form rules — there is no form to prove.

Production attribution (held slots): standard — Both 202,
Line-only 39, Band-only 7; HC — Both 229, Line-only 56,
Band-only 11. The pass-33 false accept dies by the designed
mechanism, not by special-casing: the band still holds
*Panthera Prime Blueprint* on the Acceltra slot, the line
reader does not agree, and arbitration refuses the slot on
name disagreement. The guards' measured price is two correct
holds across the censuses (*Xaku Prime Chassis*, both
censuses: its mesh scores in the blueprint tail, 7.36, against
a built truth — arbitration believes the channel; on the
standard census the item returns via its blueprint sibling's
glyph implication).

## Strip phase policy (brief §4)

The band reader's strip phase now reads at three vertical
offsets {−8, 0, +8}px around the fitted placement under a
uniform policy — pass 33 measured the strip on fitted cells to
be clean at −8px and garbage at 0 on the same cell — and a
cell holds only when the offsets that decode agree on one
name. No per-cell tuning anywhere: the offset set is a
constant of the reader.

## Boundary-snap ablation (brief §4)

The alternative cell cutting — origins snapped per instance
to the strongest local tile edges (±6px search) instead of
the fitted grid — was run under the full stack on both
censuses, and it loses on both. Standard: 238 distinct
(fitted: 239), Neuroptics 27 (28), counts 447/450 (453/453),
still 0 FA. HC: 233 distinct (234), counts 778/900 = 86.4%
(93.2%), and **one false accept the fitted grid does not
make** — a snapped crop let the line reader hold *Titania
Prime Blueprint* on a *Titania Prime Neuroptics Blueprint*
slot (via Line, form verdict Blueprint), exactly the
part-word-drop mint the form channels exist to prevent, here
caused by the crop shift rather than the channels. The
adoption rule (improve §5 metrics at 0 FA) is not met on
either census; the fitted grid stands and `snap_cells`
remains an off-by-default diagnostic toggle. This also
closes Seth's checkpoint question — the sub-200 problem was
never the cell cutting; it was single-reader evidence.

## Gate (brief §5) — production harness, session mode

| bar | standard | HC |
|---|---|---|
| false accepts = 0 | **0** | **0** |
| distinct held | **239** / 264 (bar 200) | **234** / 264 in-grid (bar 195) |
| Neuroptics distinct | **28** (bar 27) | **28** (bar 26) |
| counts | 248/248 slots, 453/453 cells | 275/296 slots, 846/908 cells = 93.2% (bar ≥ 92.4%) |
| frames alive | 15/15 | 31/31 |
| coverage | 248 held, 27 seen-not-held, 22 never-fully-visible, 4 glyph-implied | 296 / 45 / 44, 6 glyph-implied |

Glyph-implied distinct gain: *Xaku Prime Chassis* (standard;
its blueprint sibling's glyph fired on a correctly held slot).
The remaining glyph implications corroborated twins already
held directly. No unverifiable implications occurred.

A/B control: with the line reader and strip offsets switched
off, the same binary reproduces pass 33's gate exactly (157
distinct, 1 FA, Neuroptics 24, counts 303/303) — the delta is
the new machinery, and the old path is intact beneath it.

Full suite: 303 tests green (294 at pass 33 + 9 lineread),
including the property battery extended to the line reader's
hold paths. The combined stack ships as v0.5.28.
