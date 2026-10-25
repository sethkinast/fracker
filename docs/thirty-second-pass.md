# Thirty-second pass — #95: minimum-evidence floor for the radius decoder

The high-contrast evaluation (see `~/workspace/fracker-frames/hc/
HC_REPORT.md`) produced the shipped radius decoder's first
false accepts — 6 cells on the HC frame set against the
standard-theme gate's zero. Five of the six are one mechanism,
recorded as addendum #95: the decoder's strip phase, fed a
2-character icon-edge artefact fragment, minted holds for the
3-char head "lex". Seth commissioned the hardening; this pass
is the fix, gated dually on both censuses.

## The defect, precisely

Bounded-distance decoding bounds how FAR a fragment may sit
from its head. It says nothing about how little of the head
was observed. On Receiver-class cells whose band gear words
were destroyed (`a Prime Receiver`, `x Prime Receiver`), the
margin matcher refused correctly and the band decode refused —
then the line-1 strip read returned `Le`, `le` or `ie`: not
fragments of any gear word, but the class icon's edge rendered
deterministically into the strip crop (identical fragments
across the Knell, Hystrix and Velox cells). A 2-char fragment
is one deletion from the head "lex" — formally inside its
radius (d=1 <= t=1) — so the decoder held Lex Prime Receiver.
The worst cell (Hystrix, parts-08 #16): the margin matcher's
best was the TRUTH at 0.955, refused at margin 0.098, and the
strip phase converted that correct refusal into a wrong hold.
The mechanism is theme-independent in principle; HC's bright
icons made it systematic, and the standard-theme census
simply never produced the shape.

The sixth false accept (Volt Prime Systems Blueprint held as
Volt Prime Blueprint) is a margin-path sibling win — the
production matcher behaving as designed on a garbled read.
It is NOT this pass's defect and the margin matcher is
untouched.

## The fix (src/ocr.rs)

One acceptance rule, applied in `RadiusClass::decode` — the
single point both fragment sources (band and strip) are
judged, so the floor is uniform by construction:

    a fragment F decodes to head H only if, in addition to
    d(F, H) <= t_H under the metric-closure distance,

        |F| >= max(3, |H| - t_H)

(`RADIUS_MIN_FRAGMENT_CHARS = 3`.) The length-relative term
is already implied by decode's existing length-difference
check — a fragment shorter than `|H| - t_H` is more than
`t_H` deletions away — and is stated in the rule so the floor
reads as one test. The binding new constraint is the absolute
term, and it binds exactly for short heads: "le"/"ie" now
refuse against "lex" (2 < 3), while the genuine decodes the
decoder exists for are untouched — "xmag" -> Mag (4 >= 3,
d=1), "xnova" -> Nova, "ember" -> Ember, clean "lex" -> Lex
(3 >= 3, d=0).

The cost is stated, not hidden: a genuine sub-3-char fragment
(a "ma" that really was Mag's) no longer decodes. Two observed
characters cannot identify a head among dozens of siblings;
the refusal is the honest outcome, paid deliberately like the
radius ceiling itself. The optional second layer considered
in the defect record — session-level artefact repetition —
was NOT needed (the floor alone cleared the HC gate) and was
not built.

## Regression tests (pinned)

- `radius_decoder_hc_artefact_fragments_now_refuse`: all five
  HC shapes end-to-end through the public strip path (band +
  strip reads exactly as recorded in HC_REPORT §3), plus the
  genuine counter-case — a clean "Lex Prime" strip fragment
  against a destroyed band read still decodes to Lex Prime
  Receiver. The floor prices evidence; it does not ban short
  heads.
- `radius_decoder_evidence_floor_boundary`: the boundary at
  class level — "lex" decodes, "le"/"lx"/"ie" refuse;
  "mag"/"xmag" decode, "ma" refuses.
- The five-row pins, the frame-11 witnessed-token tests and
  the score-floor pins are unchanged and green.

## Stress-test reconciliation (reported, not hidden)

The pass-29 stress battery is byte-identical (403,142
decodes, 227,907 in-radius corruptions) and its assertions now
reflect the shipped rule: an in-radius variant below the
floor must refuse, and such cases are counted and printed
rather than dropped from the battery.

| stress outcome | pass 29 | pass 32 |
|---|---|---|
| decodes | 403,142 | 403,142 |
| in-radius corruptions | 227,907 | 227,907 |
| in-radius -> truth | 227,907 (asserted) | 227,410 |
| in-radius below-floor refusals | — (decoded) | **497** |
| out-of-radius sibling accepts | 284 | 225 |

The 497 are sub-3-char in-radius fragments of short heads —
1-edit deletions of 3-char heads ("ma" of "mag") and 2–3-edit
random corruptions landing under 3 chars. The floor also
refused 59 corruptions that previously landed inside a
sibling's ball on sub-3-char fragments (284 -> 225 sibling
accepts): the same artefact shape, removed from the
out-of-radius population too.

## Dual re-gate (pre-registered bars; both passed in full)

Gate 1 — standard census (`examples/census_replay.rs`,
decoder on; bar: union >= 144 and zero false accepts):

| run | union correct | false | Neuroptics instances | Neuroptics distinct |
|---|---|---|---|---|
| pass-29 re-gate (baseline) | 146 | 0 | 40/61 | 20/28 |
| pass 32 (floor on) | **146** | **0** | 40/61 | 20/28 |

No census cell changed hands: the floor cost the standard
set nothing measurable.

Gate 2 — HC set (the evaluation's per-cell harness and
truth, 1,023 cells; bars: zero DECODER false accepts,
distinct-held >= 138, the only remaining false accept the
pre-existing Volt margin cell):

| run | distinct held | held correct (cells) | false accepts |
|---|---|---|---|
| HC evaluation (v0.5.25) | 142 | 389 | 6 (5 decoder + 1 margin) |
| pass 32 (floor on) | **142** | **389** | **1 (margin only)** |

All five Receiver-class cells now refuse (Knell ×3, Velox,
Hystrix — the Hystrix cell's matcher best is the truth and it
is simply not held, which is the designed outcome). The one
remaining false accept is the Volt margin-path cell,
unchanged in kind, reported as out of scope per the brief.
Distinct-held is identical to the pre-fix run: the floor
removed five wrong holds and cost no correct ones on either
set.

Full suite: 287 tests green (259 lib + 28 integration;
285 baseline + the 2 new pins).

## Honest limits

- The floor is a minimum-evidence rule, not a proof: a 3-char
  artefact at distance <= t from a 3-char head would still
  decode. No such fragment has been observed (the icon-edge
  artefacts are 2 chars), and the dual census now stands as
  the regression gate for the shape.
- The strip phase's underlying weakness remains what it was:
  it reads icon edges as text. The icon experiment's salvage
  variants and scroll-phase handling (#90) are the larger
  levers; this pass only stops the strip from minting holds
  out of artefacts.
- Chroma/Khora stays refused by design, as before.
