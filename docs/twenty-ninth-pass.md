# Twenty-ninth pass — the gated radius decoder (addendum #92, resolving #91)

The margin ceiling of #91 stood as the documented limiter until
an external review of the Neuroptics brief proposed the remedy
shape #91's decision entry had named as its reopening condition:
bounded-distance decoding on the gear word — a rule that provably
cannot flip the destroyed-gear-word counter-class, because a
false accept requires more corruption than the truth's own
decoding radius. Seth commissioned the build behind a gate
(stress test + full-census A/B, ship only on zero false accepts).
The first gate FAILED and nothing shipped; the structural fix
below passed a fresh gate unchanged in protocol. This doc covers
both attempts.

## The decoder (src/ocr.rs, src/capture.rs)

Runs only on parts-screen reads the margin matcher refused, after
validation, and can only add holds. Relic and arcane paths are
untouched; counts and the session vote are untouched.

1. **Nomination.** The refused read's best margin candidate names
   a suffix class: every universe name sharing the candidate's
   longest trailing token run that at least one other name also
   ends with (heads are the leading token runs before it). The
   nomination is trusted only when the candidate scored at or
   above `RADIUS_MIN_CANDIDATE_SCORE` (0.50, the parts layout's
   `min_score`): the decoder builds only on a candidate the
   matcher itself considers plausible.
2. **Tail verification.** The read's tail must align to the class
   suffix within the pre-registered total budget
   `RADIUS_TAIL_BUDGET` (4): every suffix token witnessed by a
   distinct read token, in order, at metric-closure distance
   ≤ `RADIUS_TOKEN_WITNESS` (2) — a truncated token ("pri" for
   "prime", "bluepri" for "blueprint") witnesses at exactly 2 —
   plus `RADIUS_TOKEN_EDIT` (3) per skipped junk read token. The
   tokens before the split, squashed to alphanumerics, are the
   fragment.
3. **Decode.** Each class head A has radius
   t_A = floor((D_A − 1)/2), D_A its distance to the nearest
   sibling head. The fragment is accepted only by the unique head
   within its own radius; the disjoint-balls geometry guarantees
   uniqueness, and `decode` refuses if two heads ever hit.
4. **Strip second read.** If the band fragment does not decode,
   the cell's line-1 strip (name band y 0.00–0.40, PSM 7,
   upscale 4) is read and its fragment decoded — but the class and
   its tail verification still come from the band read; a strip
   fragment alone never picks a class.

All decoder distances use the **metric closure** of the confusable
relation (substitution free iff the chars share a connected
component), not the margin matcher's pairwise distance. The
stress test caught why: the pairwise relation is non-transitive
(`1`~`i`, `1`~`l`, `i`≁`l`), so its distance violates the triangle
inequality and radius balls under it overlap — the fragment
"1aros" is one pairwise edit from both `inaros` and `lavos`
(pairwise distance 3; metric distance 2, so both radii are 0 and
"1aros" refuses). The closure is the smallest enlargement that
restores the proof. The margin matcher keeps the pairwise
relation bit-identically, and a regression test pins the witness.

## Gate attempt 1 — FAILED, not shipped

- Stress test passed: 26 real suffix classes, 403,142 decodes,
  227,907 in-radius corruptions, 100% decoded to the truth,
  ball-disjointness asserted per member. (Plus 284 out-of-radius
  sibling accepts: corruptions that landed inside a sibling's
  ball, indistinguishable from that sibling read cleanly —
  outside the guarantee, reported for the record.)
- Census A/B at the pre-registered budget 4: union 149 correct
  but **2 false accepts**, both on frame-parts-11 (the mid-scroll
  composite), both via the band path:
  - truth `Nezha Prime Systems Blueprint`, band raw `"Nezh rime"`
    → held as `Nezha Prime Chassis` (best candidate 0.47);
  - truth `Nova Prime Systems Blueprint`, band raw `"Nova Prime"`
    → held as `Nova Prime Chassis` (best 0.56).
- Mechanism: the composite cells' bands contain only another
  item's line 1, so the tail witnesses just "prime". The token
  alignment charged a flat 3 for a wholly deleted suffix token,
  so `[rime]` aligned to `[prime, chassis]` at cost 4 and
  `[prime]` at cost 3 — the tail "verified" a class whose
  distinguishing token was never witnessed, and the fragment
  (within radius of the head) did the rest. The decoder also
  applied no floor to the nominating candidate's score. On
  aligned frames this shape never occurs, which is why the
  39-instance verification subset was clean.
- Budget 3 gave 147 + 1 false; budget 2 gave 142 + 0. Neither
  shipped: tuning the budget down on the same data that judged
  it is post-hoc, not a fix.

## The structural fix (re-gate)

Two additive changes; the budget stays at the pre-registered 4.

1. **Witnessed suffix tokens** in `token_align_cost`: suffix
   tokens can no longer be deleted in the alignment — each must
   be witnessed as in step 2 above, and a substitution beyond the
   witness cap is forbidden. An unwitnessed suffix token fails
   the alignment at ANY budget, which kills the failure class
   rather than pricing it. Per-token distances moved to the
   metric closure, matching the rest of the decoder.
2. **Best-candidate score floor** (`RADIUS_MIN_CANDIDATE_SCORE`),
   applied in both decode entry points. It is the second line of
   defence only: it would have stopped the "Nezh rime" case
   (0.47) but not "Nova Prime" (0.56); the witnessed rule stops
   both.

Regression tests pin both gate-failing reads through the public
decode path — at budget 8 as well, so the refusal is structural —
plus a missing-final-suffix-token refusal with a perfect
fragment, a missing-middle-token refusal, and a truncated final
token ("… bluepri") that still decodes.

**Strip-read batching.** Attempt 1 measured +7.2 s/frame (VM,
debug build), nearly all strip reads spawning one tesseract
process per refused cell. The strip reads now go through a new
`OcrEngine::read_text_batch`: the tesseract engine groups by
(psm, upscale) and serves the group through the same crop-hash
cache + single-process batch reader as the cell path (addendum
#81), and `read_frame` collects the strip work into phase 2
after the band decodes. Acceptance logic is unchanged.

## Re-gate — PASSED

Identical protocol, fresh run:

- Full suite: **277 tests green** (249 lib, incl. the stress
  test with identical coverage numbers, + 28 integration).
- Census A/B (release build, `examples/census_replay.rs`):
  decoder off reproduces the pass-27 baseline exactly; decoder
  on:

| run | union correct | false | cells correct | Neuroptics inst. | Neuroptics distinct |
|---|---|---|---|---|---|
| decoder off (baseline) | 127 | 0 | 212 | 24/61 | 13/28 |
| decoder on, budget 4 + fix | **146** | **0** | 245 | **40/61** | **20/28** |
| (attempt 1, budget 4, no fix) | 149 | **2** | — | 44/61 | 22/28 |

Per-frame held = correct (off → on): first-00 18→24,
parts-01 14→16, 02 14→18, 03 18→24, 04 20→23, 05 21→21,
06 19→19, 07 15→17, 08 13→14, 09 15→15, 10 16→16,
**11 0→0**, 12 13→16, 13 16→22, 14 0→0. Frame 11 holds nothing,
as in the baseline — both former false accepts now refuse.
- Timing (release build, this VM): mean `read_frame`
  3,687 ms/frame off → 4,805 ms/frame on (**+1.1 s/frame**) with
  the batched strip path — against attempt 1's +7.2 s/frame
  unbatched in a debug build. The remaining cost is the strip
  batch plus the refused-cell matcher work.

## Honest limits

- Heads at distance 3 have radius 1, so the decoder recovers the
  ≤1-edit (metric) population only. Chroma stays refused: Khora
  sits at D=3, so the census's 2-edit "Bhioma" corruption does
  not decode. Pinned in tests, by design.
- The witnessed rule costs a few of attempt 1's correct decodes:
  reads whose tails genuinely dropped a whole suffix token
  (union 146 here vs attempt 1's 147 correct + 2 false). Those
  reads are now refused rather than decoded — the rule cannot
  tell a dropped token from a composite cell, and it chooses
  refusal. That is the never-guess trade, taken knowingly.
- Class derivation when the suffix itself is garbled beyond the
  witness caps remains out of reach (the strip path inherits the
  band read's class for the same reason).
- The gate ran on one tesseract build (distro 5.3.4); the Flatpak
  sidecar build is not part of it — the standing pass-27 caveat.
  Frame 14 still holds 0 (its grid is shifted; pre-existing).
