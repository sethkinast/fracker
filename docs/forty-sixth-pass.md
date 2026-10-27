# Forty-sixth pass — relic name admission: refuse the name the read did not prove

Verification addendum: #111.

## The problem

Pass 45 pinned a known issue: the production reader names the owned
Meso C11 [Radiant] cell "Meso L1" in both settled appearances (raw
"Mesoll Relic [Radiant]"), so that stack imported at ×1 Radiant under
a false name. Seth's ruling reversed the priority pass 45 had
accepted: importing a relic under a name the read does not prove is
actively false information — strictly worse than missing the relic.
"Meso L1" is a real, different relic in the 800-name universe, so
universe-membership validation cannot catch it; only the read's own
evidence can.

## The resolver, traced

One resolver validates every name that reaches relic stock:
`Universe::best_match_margin` (confusable-Levenshtein similarity
over the whole read and every leading-token suffix trim) consumed
by `ocr::validate_read`. Both relic-stock write paths funnel
through it — live capture (`capture::read_cells` → `read_frame`)
and folder ingest (`ocr::ingest_folder`, also `--ocr-ingest`).
Three bars decided admission, all string-global:

- absolute: `score ≥ layout.min_score` (0.8 for the relic layout);
- margin: winner − runner-up ≥ `MATCH_MARGIN` (0.10);
- `hold_permitted`: the pass-33 head-witness predicate.

The hole is in the third bar's merged-witness clause: a single
fused token may testify for the head AND the code at once. The
garble "Mesoll" is zero edits from the merged target "mesol1"
(the era token ends in 'o', the code token starts with 'l'), so
one blurred token witnessed a two-token name. The first two bars
do not save it: the garble wins at 0.8571 with a margin of 0.1071
over "Meso A11" — seven thousandths over the margin bar.

## Calibration: why the bar is a token witness, not a bigger margin

Measured over the refinement corpus replay (production
`read_frame`): 1,680 non-empty relic reads, 1,235 held.

- Winner scores of held reads: minimum 0.8333, p1 = 1.0.
- Margins of held reads: minimum 0.10, median 0.1429; 1,003 of
  1,235 sit below 0.15. The garble's margin (0.1071) is inside
  the legitimate population — a clean "Meso C11" read itself
  wins with margin only 0.125 (over "Meso A11"). **No
  score-margin threshold separates the garble from legitimate
  reads.** Raising the absolute bar fails too: the only
  legitimate held read under 0.875 is "Ayi V2" (Axi V2, 0.8333),
  and the garble at 0.8571 would clear any bar it clears.
- The token-witness axis separates perfectly: exactly **2 of
  1,235** held reads cannot assign the winner's tokens to
  distinct read tokens in order — the two garble appearances.
  Every legitimate held read assigns with per-token
  confusable-Levenshtein distance ≤ 1 (worst total assignment
  cost 1 over the whole corpus), including split-glyph reads
  ("| ith S7", "Lith MI 0", "Neo A711") and reads behind a junk
  token (the suffix-trim interpretations).

Universe structure forces the per-token form: relic names are two
tokens (era + code; the only exceptions are the four "Axi
Vanguard X1" names and the Requiem names), and codes repeat
across eras — 205 of 265 codes appear in more than one era, and
"L1" and "C11" each exist in four eras. The era token and the
code token are each discriminative against a different sibling
set, so each needs its own read token. A fused token spanning
the boundary spends the era's last letter buying the code's
first, and can testify for neither.

## What shipped

A third admission bar for relic names, in the parts discipline's
shape (absolute evidence + clear margin): a held relic result
imports only when

1. the resolver's existing bars cleared (score ≥ min_score,
   margin ≥ 0.10, hold permitted), **and**
2. **every token of the resolved name is witnessed by a distinct
   token of the cleaned read, in order, at per-token
   confusable-Levenshtein distance ≤ 1**
   (`RELIC_TOKEN_WITNESS_MAX`).

Below the bar, the read is REFUSED, never imported:
`relic_name_admission` un-holds the result and records a
`NameRefusal` (raw read, refused winner + score, runner-up +
score) on it before the owned-gate or the refinement measure can
see it, so a refused name feeds no other evidence. The session
collects refusals and the finalize log states them —

`relic name admission: refused 1 read(s) whose resolved name the
read did not prove: "Mesoll Relic [Radiant]" -> Meso L1 (0.86)
over Meso A11 (0.75), margin 0.11`

— and folder ingest prints the same record per refusal. A fused
read refuses even when its string is an unambiguous name: the
fused shape is the defect class, not the ambiguity (a true
"Meso L1" tile read cleanly imports as L1; the gate refuses the
garble, not the name). One honest limit, recorded in the code:
a boundary-preserving garble — a C11 tile read cleanly as
"Meso L1" — is indistinguishable from a true L1 read by any
string evidence and remains the name-OCR problem, not an
admission problem.

## Corpus replay (the 25-frame live corpus, production reads)

- **505 stacks / 3,613 items** imported = 411 labelled + 94 owned
  ×1s — pass 45's 506/3,614 minus exactly the garble pair.
- Refusals: exactly **2 reads**, both appearances of the C11
  [Radiant] cell, both recorded as `"Mesoll Relic [Radiant]" ->
  Meso L1 (0.86) over Meso A11 (0.75), margin 0.11`. No other
  corpus read is refused. "Meso L1" does not appear in stock;
  the C11 Radiant stack is missed, not misnamed (its Intact
  sibling still imports at ×5, frame 13).
- The 11 unowned Axi exclusions and the rest of the pass-45
  disposition are unchanged.

## Tests

Full suite green: 380 passed / 0 failed (lib 352; golden 15;
packaging 7; resident 5; tray_quit 1), up from 378 at pass 45 —
the two new tests are the gate's unit tests below; the corpus
replay's assertions were rewritten in place.

Unit tests pin the gate both ways: the fused garble holds under
the resolver's own bars (asserted first — that admission is the
defect) and is then refused with its record; clean reads of both
siblings import; witnessed noise imports ("Ayi V2", split
"Lith MI 0", junk-prefixed "x8 Meso C11 Relic"); a fused clean
read refuses as a shape. Census gates untouched.
