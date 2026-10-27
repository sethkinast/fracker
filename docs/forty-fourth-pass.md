# Forty-fourth pass — need-based coverage, one definition on both surfaces

Seth's sanity check during the 2026-10-26 loop trial: the
Citrine Prime Hunt card read "Citrine Prime Blueprint · +3
also / Meso C11 · Radiant" — while all three parts behind
the +3 were already in hand, each confirmed owned on the
Hunt tab itself. Lavos Prime Chassis Blueprint was
satisfied via the built Chassis component (#89
canonicalisation), Kompressa Prime Blueprint was held,
Daikyu Prime Upper Limb was held (Daikyu itself is not
owned; its card is legitimately active). Paris Prime
Blueprint correctly did not count: Paris is owned and
mastered, so its want is pruned. Under honest semantics
the line reads "+0 also" and the candidate's score factor
is 1x, not 4x. Seth also observed the crack queue not
counting the Chassis where Hunt did — the surfaces
disagreed, and neither was computing the thing the number
claims to mean.

## 1. The defect

`CrackCandidate.coverage` — rendered as `CrackInfo.covers`
("+N also") and multiplied into the crack score as
(1 + coverage) — counted a relic's other rewards whose
names appear in the active want LIST. A want retires
only when its parent gear is profile-owned (`prune_wants`,
BRIEF §10), so a part that is already satisfied — verified
Have in either stock form, or ⍻-asserted (HaveUnverified)
— keeps counting until the whole gear is done. Worse, the
two consumers did not even build the same list:
`derive_action` (Hunt) cloned the post-prune active set,
while the queue's ranked pass rebuilt a union of the
cards' want lists inside its per-card loop.

## 2. One coverage universe (`plan.rs`)

Coverage now means one thing everywhere: **a relic reward
counts iff its part's PartState is exactly Need.**

- `need_set(rows)` builds the universe from part rows:
  every part at PartState::Need, plan-wide. (A partial
  multi-unit part — some verified units against a x2
  requirement — is Need and counts: a covered drop still
  helps. Have and HaveUnverified never count.)
- `goal_cards` is two-phase: all cards' part rows are
  computed first, the Need universe is built from them,
  and only then are actions derived. `derive_action`
  receives the Need set instead of the want list (its
  `part_rows_for_gear` state computation still uses the
  want list — Need-vs-Unknown row states are a different
  question, untouched).
- The queue's ranked pass builds the same set once, from
  the finished cards, before its loop — replacing the
  per-card rebuild.
- `best_crack_candidate` / `best_crack_for_part` take the
  set as `other_needs`; the (1 + coverage) score factor
  and pass 40's tie-band coverage comparisons consume the
  redefined figure unchanged in form. The chunk alternates
  text now reads "Covers N other needed part(s)".
- `prune_wants`, the profile-owned set, and part-state
  computation are untouched (the Paris/Daikyu behaviour
  there was verified correct during the investigation).

Ordering note: candidates whose coverage was inflated
lose score, so some queue orderings shift — that is the
correction working, not a regression. No tuning values
changed.

## 3. Farm-hint verdict: changed, same fix

`score_farms` built its own want-list set from the cards'
wants and summed co-yield over relics for ALL wanted
parts — satisfied ones included. That is the same defect
in a second surface: a satisfied part's relics have no
hunt value, yet they inflated the nodes that drop them,
ranking farm targets by yield Seth cannot spend. It now
consumes the same Need universe, and its doc states the
sum is over relics for still-needed parts. (Surplus value
— a held part being tradeable — is the parked
surplus-identification question, not a farm-hint input.)

## 4. Gates

- **Surface agreement** (`plan::tests::
  coverage_counts_only_still_needed_parts_on_both_surfaces`):
  the generalised Citrine fixture — Meso C11 drops the
  target plus one satisfied part per evidence class
  (built-form Have via #89 canonicalisation of the
  captured stock, blueprint Have, ⍻ HaveUnverified) plus
  one still-needed part of another card. Queue
  CrackInfo: covers == 1; Hunt: the action picks the same
  relic and its score decomposes as chance x (1 + 1) x
  stock factor — the two surfaces cannot disagree without
  failing this test. The fixture also pins the three
  satisfaction states and the covered part's Need state.
- **Citrine scenario** (`coverage_zero_when_every_other_
  reward_is_satisfied`): no still-needed reward on the
  relic -> covers == 0 and the Hunt score is chance x
  stock factor exactly.
- Pre-existing coverage tests re-checked: the pass-40
  tie-band test ("Meso B1 also drops the other want") calls
  `best_crack_candidate` with a hand-built set — the
  function's membership semantics are unchanged, and it
  passes; the full plan module (35 tests) is green.
- Full suite green: 375 passed / 0 failed (lib 347 =
  345 + 2 net-new; golden 15; packaging 7; resident 5;
  tray_quit 1).
- Census gates untouched (no capture changes).

Live verification owed: Seth's next Hunt/queue read on
the Citrine card — the Meso C11 line should read +0 also.
