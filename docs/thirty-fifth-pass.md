# Thirty-fifth pass — Stage 3: analysis-by-synthesis

Passes 33+34 left a characterised residue: slots both readers
could see but neither could hold at zero false accepts —
sibling ties, margin refusals, decoder refusals. This pass
adds a third voice that never reads text at all: it RENDERS
each candidate name in the game's own font and asks which
render the pixels match. Synthesis adjudicates only slots
the shipped arbitration leaves seen-not-held with candidate
evidence; it never overrules a reader hold, and hold-vs-hold
reader disagreements stay refused (0 occurred in either
census). Form is decided by the existing channels
(Blueprint-line evidence, mesh, glyph), never by synthesis
alone. Holds enter stock and coverage marked
`held_via = Synthesis`, and the glyph implication (a
synthesis-held blueprint whose glyph fires implies its built
twin) applies to them exactly as to reader holds.

## The font: Old Roboto, 2013 cut — provenance

Warframe's inventory UI renders in an older Roboto (Warframe
wiki, Fonts page: "Warframe uses an older version of Roboto,
sometimes referred to as 'Old Roboto', from around 2013.
Most notable are the changes on the capital letter 'R' and
the number '1'."). Three public cuts were compared, all from
the Android Open Source Project `platform/frameworks/base`
`data/fonts/Roboto-Regular.ttf`:

| Source tag | Name-table version | Unique ID | R advance (units/2048) |
|---|---|---|---|
| android-4.1.2_r1 | Version 1.00000; 2011 | — | 1298 |
| android-4.3_r2.1 | Version 1.200310; 2013 | Google:Roboto:2013 | 1356 |
| android-5.0.0_r1 | Version 2.001047; 2014 (redesign) | — | 1261 |

The 2013 cut also ships in android-4.4.2_r1; its ASCII glyph
outlines are byte-identical to the 4.3 file's (verified per
glyph). **Vendored**: the android-4.3_r2.1 file, as
`assets/fonts/Roboto-Regular.ttf`, SHA-256
`00ae3fd7698bec219473edf68171a0f14e6cf68d363eea59373156e0b090de11`,
with the Apache-2.0 text (`assets/fonts/LICENSE.txt`) and a
provenance note (`assets/fonts/README.md`); embedded via
`include_bytes!`, so the Flatpak needs no runtime font
lookup.

Identity check against held crops (development probe,
`examples/stage3_probe.rs`: 16 both-readers-held census
slots, per-slot oracle fit and the production fit, mean
line NCC of the true name): 2011 cut 0.368 / 0.273;
**2013 cut 0.367 / 0.272**; 2014 redesign 0.323 / 0.221.
The 2014 redesign is measurably worse — its narrower forms
are not what the game draws. The 2011 and 2013 cuts are
inseparable by this measurement (their differences
concentrate in R and 1, a small share of any line's ink);
the documentary and name-table evidence selects 2013, and
the vendored file is that cut.

## The rasteriser (`src/ttf.rs`)

Deliberately hand-rolled and dependency-free — a new crate
would mean regenerating the offline Flatpak's vendored
source set for a problem this size. Minimal TrueType:
offset table, `head`/`maxp`/`hhea`/`hmtx`, `cmap` formats 4
and 12, `loca` (short and long), `glyf` simple and composite
outlines (scale / xy-scale / 2×2 transforms; point-matching
composites treated as zero-offset and documented as such;
CFF/OTTO rejected). Coverage rasterisation: quadratic
contours flattened with adaptive subdivision, nonzero-
winding scanline fill on a 4× supersampled grid,
box-downsampled, then a separable gaussian blur and a
level/gamma coverage curve — the session-fitted parameters
that absorb the game's AA. No hinting (the fitted curve
absorbs what hinting would change at this size) and no
kerning (Roboto's kern data lives in GPOS, which this UI
text does not apply); both choices are stated here and were
validated by the calibration fits below, not assumed.

## Display model: the wrap is fitted, not assumed

The first design assumed fixed semantic lines ("Gear Prime"
/ part / "Blueprint"). Measurement killed that: the game
word-wraps the full name to the cell's text width —
"Glaive Prime Blade" renders on ONE line; "Ember Prime
Neuroptics Blueprint" wraps to three. Synthesis therefore
models two breakings: the semantic split (kept for form
rules and exemplar keys) and the DISPLAY wrap, a greedy
word wrap using the font's own advances at the fitted size,
with the wrap width fitted per session as a fraction of the
name band's width. Both censuses' fits chose px 16.50 and
wrap fraction 0.865.

## Scoring (`src/synth.rs`)

- **Observation**: the canonical crop's name band, the line
  reader's detected line boxes, top-hat ink per box.
  Detected boxes whose ink touches both band edges are
  dropped (an icon's bottom edge bleeding into the band is
  not a text line; the census's Akstiletto slot detects 3
  boxes for a 2-line name).
- **Comparison**: masked NCC per line — Pearson correlation
  over the union support (the observed line's dilated ink
  mask ∪ the guest's ink). The alignment IS the search:
  shifts dx −6..+6, dy −3..+10 around the ink bounding-box
  origins. This range is a measured fix, not a guess: the
  first build anchored at ±2px and scored true lines at
  median 0.107 (wrong candidates 0.173) because the first
  line's detected box merges the item icon's lower rim —
  the observed ink origin sits several px above and beside
  the text's own origin, worst for the wide Neuroptics
  helmets. With the search as alignment, true-score medians
  rose to 0.398 (std) / 0.358 (HC) and exemplar line
  agreement to 0.909 / 0.954 (median).
- **Width pre-filter** (the reviewer's ink-width feature,
  hardened): a line is rejected when its rendered ink width
  disagrees with the observed extent beyond max(5px, 22%).
  On the first line the rule is one-sided — icon overprint
  only ADDS observed ink, so a narrower render is not
  evidence against a candidate there; a wider render is
  impossible on every line. The candidate-level kill fires
  only when EVERY shared line is width-wrong; a single
  rejected line (likelier a wrap-model or detector
  disagreement than a wrong name) costs that line's mass
  share. (The intermediate build killed on any line: over
  half of all calibration truths carried the flag.)
- **Candidate score**: mass-weighted line NCC with
  SET-DISCRIMINATIVE line weights — a line's weight is
  1 − (the modal line-text frequency at that index across
  the slot's candidate field), so the line every sibling
  shares ("Gear Prime") carries almost no weight and the
  discriminating lines decide; plus two-way line-count
  penalties (unexplained observed ink, undetected candidate
  ink). Falls back to raw mass weights when the
  discriminative weights annihilate (fields differing only
  beyond the shared prefix — the count penalties then carry
  the decision).
- **Candidates** per eligible slot: refused/contested
  holds, the matchers' top-4 nominations per canonical
  re-read (band + assembled + per-line reads), radius
  decoder nominations, form siblings, gear siblings, and
  the sort interval between held panorama neighbours under
  the refined key (gear → main-blueprint-last → part →
  blueprint-before-built → name). Sizes: std n=26, min 4 /
  median 11 / max 128; HC n=44, min 1 / 11 / 256 (the maxima
  are sparse-neighbour slots; cost only, reported not
  capped).
- **Exemplars**: real ink crops from both-held slots —
  line 1 keyed by gear, part lines keyed by part word —
  compared with the same masked NCC; leave-one-out in
  calibration.
- **Decision**: form gate on the field (channel verdicts)
  → best candidate ≥ floor AND margin over runner-up ≥
  threshold → under an Unknown verdict, a blueprint winner
  must prove its Blueprint-bearing last wrapped line at
  ≥ 0.92 × floor → margins inside the grey zone require
  exemplar agreement ≥ the exemplar floor. Form never comes
  from the render alone.
- **Calibration** per session (in replay: per census) over
  the both-readers-held slots (std 202, HC 229): staged
  grid — geometry (size × wrap) at a default curve, the
  coverage curve at that geometry, then a refinement —
  maximising mean true-line score less a line-count error
  term; then true vs best-wrong distributions over
  enumerated wrongs (evidence candidates, the form sibling,
  gear siblings), scored IN-SET with the same effective
  transform decide() uses (a full width kill is −1).
  Thresholds are chosen from the measured plateau, never
  tuned per item.

## Calibration evidence

| | std | HC |
|---|---|---|
| Fit | px 16.50, wrap 0.865, blur 0.70, level 0.00, gamma 0.70 | px 16.50, wrap 0.865, blur 1.10, level 0.00, gamma 0.70 |
| True scores q02/25/50/75/98 | −0.003/0.198/0.398/0.591/0.806 | −0.039/0.125/0.358/0.607/0.712 |
| Best-wrong q02/25/50/75/98 | 0.012/0.225/0.472/0.575/0.691 | 0.000/0.191/0.414/0.522/0.614 |
| Margins q05/10/35/50/90 | −0.292/−0.134/−0.013/0.000/0.134 | −0.236/−0.155/−0.013/0.000/0.132 |
| Plateau [wrong p98, true p02] | [0.691, −0.003] — inverted | [0.614, −0.039] — inverted |
| Floor (wrong p99.5 + 0.01) | 0.715 | 0.646 |
| Margin threshold | 0.015 | 0.015 |
| Grey zone | 0.025 | 0.025 |
| Exemplar (n, q05/50/95, floor) | 186, 0.050/0.909/1.000, 0.300 | 216, 0.039/0.954/1.000, 0.300 |

Read the overlap honestly: on both censuses the best-wrong
median exceeds the true median. The best wrong is usually
the form sibling or a same-gear sibling — candidates that
share line 1 exactly and differ in one part word or one
line — so a render score is evidence 
...[truncated 6061 chars]