# Fortieth pass — crack-queue sort stability (loop stage 2)

Seth's complaint, in his words: the queue's sort "churns as a
fissure ends." Regens over unchanged stock produced different
slates because crack order carried two live inputs — fissure
expiry (within-tier sort) and the <60 min expiry promotion into
tier 1 — and because a chunk existed at all only while a live
fissure for its era happened to be in the feed. This pass makes
the crack slate intrinsic: existence and order are functions of
wants, stock, and relic data alone; mission quality survives
only as a tiebreak between chunks that are, in Seth's words,
"pretty much tied on every metric."

## Premise verification — Conjunction Survival in the feed

Seth's premise: a relic always has at least one mission
available, because Conjunction Survival's fissures are
permanent. Verified against the feed the planner consumes
(DE `worldState.php` → `ActiveMissions`), not assumed:

- The conjunction nodes are Yuvarium (SolNode309) and Circulus
  (SolNode310) on Lua; both decode to **Omnia** tier
  (`VoidT6`), and an Omnia fissure accepts every era — so one
  standing conjunction entry covers Lith, Meso, Neo, Axi, and
  Requiem alike.
- The live feed at 2026-10-26T23:16Z carried Yuvarium (Omnia,
  Survival, Steel Path, ~1.9 h to expiry — it rotates like a
  normal fissure but is re-listed). The committed fixture
  (`fixtures/worldstate.json`, 2026-10-19) carries it too, in
  the normal-Path variant. Two snapshots a week apart, same
  standing entry.
- **Gap, stated plainly:** Circulus appears in NEITHER
  snapshot. Whatever DE's listing rule is, the feed surfaces
  Yuvarium as the standing conjunction entry and not its
  sibling. The premise therefore holds via Yuvarium alone;
  nothing in this pass depends on Circulus being listed (the
  planner's Omnia handling predates the pass — addendum #16 —
  and the pass-40 tests use Yuvarium as the conjunction entry).

Per era, the conclusion is uniform: with the conjunction
entry present, every era has a servable mission; when the
rotating era fissures are also live (the usual case), the
mission-time model picks among them.

## Changes

### 1. Chunk existence is intrinsic — `best_crack_candidate`

`best_crack_for_part` fused two decisions: which relic is a
part's best crack, and which live fissure serves it — returning
None when no fissure matched, which silently deleted chunks
(and flipped cards to Wait/Farm/Parked) on feed rotation. It
is now a composition over a new intrinsic selector,
`best_crack_candidate` (source tier, then score — no fissure
input at all), with the pick attached by `rank_fissures` for
the candidate's era. `best_crack_for_part` keeps its signature
and its callers (the goal-card action text still requires a
real mission to name, and falls back honestly when the feed
has none).

`build_chunks` no longer gates the slate on the card's action
kind — that gate priced the live feed into chunk existence —
and attaches the pick itself; a chunk whose era has no live
fissure in the feed is still emitted, with no invented node or
expiry, its title naming the standing fallback (Conjunction
Survival). With a complete feed that branch is unreachable;
it exists so an incomplete or stale feed degrades to a
labelled chunk, never to a vanished one.

### 2. Intrinsic order; expiry demoted to a label

Crack chunks are all tier 2 now. The tier-1 promotion for
fissures expiring within `tier1_crack_mins` is gone as an
ordering force; the "expiring crack" kind label — the same
mechanism's annotation — stays. The final slate sort keeps
tier order across kinds and earliest-expiry within event
tiers, but crack pairs compare Equal, so the stable sort
preserves the intrinsic build order. (Event chunks keep
their expiry behaviour wholesale: an expiring alert is
intrinsically expiring value, unlike a crack.)

### 3. The tie band

After the strict intrinsic sort (source tier, score, then the
deterministic part/relic name pair), a bubble pass reorders
adjacent chunks within a source tier ONLY when they are tied
on every score component and the later chunk's mission is
faster (batch minutes at its best live fissure — the existing
mission-time model). Bands can chain transitively (A~B and
B~C may reorder A past C); with the slate capped at
`max_crack_chunks` this is a handful of passes.

The band, as shipped tuning (`data/tuning/planner.toml`,
addendum #20 data-pack fields `tie_chance_rel_epsilon` /
`tie_coverage_epsilon`):

- **Chance: relative epsilon 0.10.** |a−b| ≤ 10% of the larger
  chance. Radiant 10% vs 11% is expected-cracks 10.0 vs 9.1 —
  inside run-to-run variance over a batch; mission time
  (routinely a 1.5–2× spread between mission types) is the
  real difference between such cracks.
- **Coverage: epsilon 0 — counts must match exactly.** Each
  coverage step multiplies the score by (c+2)/(c+1) ≥ 1.25 in
  the practical range; a coverage difference is a difference
  in kind. This is the brief's example enforced: high-chance /
  low-coverage and low-chance / high-coverage candidates with
  similar composites are NOT tied, and mission quality never
  reorders them.
- **Stock factor: exact equality, no band.** It is a step
  function of stock knowledge (known 1.5 vs unknown 1.0); the
  two sides of the step never tie.

One interpretation, recorded: the brief's sort key leads with
"focus status." In the current slate every crack chunk
pursues a focus part — focus decides slate MEMBERSHIP, not
order (as it did before this pass) — so the status is uniform
across chunks and the operative tie group is the source tier.
If a future pass admits non-focus chunks, focus status
becomes the band's outer gate; the predicate is already
shaped for it (same-status is checked by construction today).

### 4. NEXT

Unchanged mechanism, new guarantee: the queue renders
`plan.chunks` in order and marks the first crack NEXT, so
NEXT is now the topmost chunk in intrinsic order — per the
premise, its mission is always available.

## Drive-by fix (in scope's blast radius): the tier leg of crack selection was inverted

Splitting `best_crack_for_part` exposed a latent defect the
fissure gate had been masking. The "is this candidate better"
predicate — shared by the old `best_crack_for_part` and by
`derive_action`'s cross-part choice — chained
`tier.cmp().then(score.cmp()).is_gt()`: correct for the score
leg, exactly backwards for the tier leg, so a worse-tier
candidate REPLACED a better-tier incumbent whenever both
reached the comparison. The gate masked it only when the
worse relic's era had no live fissure; with an Omnia fissure
live (the feed's normal state, per the verification above)
nothing was filtered.

Probe-verified on the v0.5.33 tree before fixing: a part
dropped by Lith A1 (Unvaulted, Radiant 20) and Axi B1
(vaulted, Unobtainable, Radiant 25.33) with an Omnia fissure
live — `best_crack_for_part` returned **Axi B1**. The
consequences in shipped behaviour: a card's advised crack
could name an unobtainable relic while an obtainable one
existed, and `build_chunks`' #72b obtainable-representative
rule then discarded the chunk the good relic should have
produced.

Fix: a single explicit predicate, `crack_outranks` (lower
tier wins; within a tier, higher score wins), used by both
selection sites. Regression tests cover both sites. This
does not change chunk scoring, source tiers, or any tuned
value — it makes selection match its documented semantics
("source tier first, then highest score").

Also removed: `CrackCandidate::solo_minutes`, written by the
old fused selector and read nowhere (radshare computes its
own minutes-per-crack from the pick).

## Gates

New tests (all in `src/plan.rs`):

- (a) `stability_rotation_changes_order_only_within_tie_bands`
  — a separated slate (20/11/5) holds its exact order across
  three rotations, including one where only the Omnia
  conjunction entry remains; the old (tier, expiry) sort
  reorders rotation 2.
- (b) `stability_near_tie_pair_flips_with_mission_quality_only`
  — 20 vs 19 (inside the band) flips with mission quality and
  flips back when the feed reverses.
- (c) `stability_separated_pair_never_flips` — 20 vs 10 holds
  under adversarial feeds (best chunk slowest, worst fastest,
  worst expiring).
- (d) `stability_expiring_fissure_annotates_but_never_promotes`
  — the expiring worse chunk stays tier 2 with its label,
  below the better chunk and below a tier-1 event alert.
- (e) `stability_identical_inputs_identical_slate` — two
  regens over identical inputs produce identical slates.
- `stability_chunks_exist_without_any_fissure` — change 1:
  empty feed → chunks with no invented node/expiry; Omnia-only
  feed → every era served.
- `tie_band_is_component_wise` — the band's three legs,
  including the exact chance boundary (20 vs 18 tied, 20 vs
  17 not).
- `cross_tier_selection_keeps_the_lowest_tier` and
  `card_action_picks_the_better_tier_across_parts` — the
  drive-by fix's regressions, one per selection site.

Full suite: 345 passed, 0 failed (lib 317 = the pass-39
baseline's 308 + the 9 tests above; golden 15). Census gates
unaffected — no capture code changed in this pass.
