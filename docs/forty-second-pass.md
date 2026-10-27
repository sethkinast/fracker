# Forty-second pass — required quantities (#94) + ready-card surfacing

Seth, with the Afuris card on screen: its 4/4 was a lie of
omission. The planner counted part TYPES and fired `Have` at a
canonical count of 1, but dual-wield builds eat doubled
components — Quassus Prime consumes two Blades and two Handles,
and a 1-of-2 part not only overstated progress, it dropped out
of crack planning entirely (nothing left to farm, said the
planner, wrongly). #94's remaining piece was the surfacing:
when an item is fully ready — every part at its required
count — its Hunt card sorts to the top under a gold border.

Decisions of record (Seth): a multi-unit part keeps its SINGLE
pip slot, segmented per unit (two half-pips for x2); one
unverified (⍻) click asserts the FULL requirement of a
multi-unit part (ruled 2026-10-26); ready cards sort first with
a gold border.

## 1. Data — where the counts come from

The roster feed (Xuerian `wishlist.json`) carries part NAMES
only — no quantities anywhere in it (verified: `RosterPart`
was `{name}`). The app's other pipeline source, the WFCD
warframe-items dataset (already the names/images source;
`fetch.rs` `WFCD_ITEMS_URL`), DOES carry full build recipes —
`components[]` with `itemCount` per parent item (BRIEF.md:
"Requirements are machine-readable… counts included").

Per the brief's fallback, the counts ship as a curated table,
**`data/required.toml`** — content, not code — mechanically
extracted from WFCD warframe-items at commit
`f48e04d49cd98e533c246d36bda02d038ccf7590` (2026-10-25), not
hand-authored. (Chosen over runtime derivation from the weekly
items cache: derivation silently degrades to all-x1 whenever
the cache is absent, and drifts unreviewed on every refresh;
the table is pinned by a test and changes only deliberately.)
Method: for each roster item, join the recipe's components
(uniqueName tails) to the roster's part names, summing
duplicate entries. WFCD's internal codenames needed resolving
(Polearm→Orthos, Bow→Paris, Scythe→Reaper,
CronusLongSword→Dakra, LightningGun→Vadarya, Vento→Venato,
1HShotgun→Euphona, LiDagger→Spira, VenkaClaws→Venka,
HikouHolster→"Hikou Prime Pouch", ZhugeStock→"Zhuge Prime
Grip", Handle→Hilt for Nikana / Cobra & Crane / Silva &
Aegis, CorufellBarrel→"Corufell Prime Stock", the
PrimeArchwing* components→Odonata, sentinel `*SentinelBlueprint`
tails, and Kavasa's parent listed as "Kavasa Prime Kubrow
Collar"). Coverage was total: every part of every part-bearing
roster item (163 items) resolved to exactly one recipe
component; the only unconsumed components were resources
(Orokin Cell, Alertium) and whole-gear ingredients (below).

**The multi-unit population — 49 parts, 28 gears, every
count x2:**

| Gear | Doubled parts (x2) |
|---|---|
| Afuris Prime | Barrel, Receiver |
| Akarius Prime | Barrel, Receiver |
| Akbolto Prime | Barrel, Receiver |
| Akjagara Prime | Barrel, Receiver |
| Aksomati Prime | Barrel, Receiver |
| Akstiletto Prime | Barrel, Receiver |
| Ankyros Prime | Blade, Gauntlet |
| Bo Prime | Ornament |
| Dual Kamas Prime | Blade, Handle |
| Dual Keres Prime | Blade, Handle |
| Dual Zoren Prime | Blade, Handle |
| Fang Prime | Blade, Handle |
| Glaive Prime | Blade |
| Guandao Prime | Blade |
| Gunsen Prime | Blade, Handle |
| Hikou Prime | Pouch, Stars |
| Kestrel Prime | Blade |
| Kogake Prime | Boot, Gauntlet |
| Kronen Prime | Blade, Handle |
| Nami Skyla Prime | Blade, Handle |
| Ninkondi Prime | Handle |
| Okina Prime | Blade, Handle |
| Orthos Prime | Blade |
| Quassus Prime | Blade, Handle |
| Spira Prime | Blade, Pouch |
| Tekko Prime | Blade, Gauntlet |
| Tipedo Prime | Ornament |
| Venka Prime | Blades, Gauntlet |

Verified against the known examples: Quassus Prime = Blueprint
1 + Blade 2 + Handle 2 (5 units) — exact. Akarius Prime =
Blueprint 1 + Barrel 2 + Link 1 + Receiver 2 (6 units).

Verified negatives: every warframe component x1 (all frames
checked through the `*Component` / Helmet→Neuroptics mapping);
all six prime sentinels x1; Odonata Prime and Kavasa Prime x1.
NOT multi-unit despite appearances: Akbronco, Aklex,
Akmagnus and Akvasto Prime consume two BUILT single weapons
(Bronco/Lex/Magnus/Vasto Prime) as ingredients — whole gear,
not roster parts; their roster parts are all x1.

**Disagreement flagged — Zhuge Prime.** Seth's record calls
Zhuge the stretch case at SIX total units. Both machine
sources say five: the WFCD recipe lists Barrel, Blueprint,
Grip (internal name Stock), Receiver and String at x1 each,
and the wiki's manufacturing table independently lists four
ingredients at x1 each. Zhuge therefore ships with NO table
rows (all x1). Akarius, above, totals exactly six units —
the likely source of the transposition. If in-game evidence
overrules both sources, the correction is one line in
`data/required.toml`; the table's provenance header carries
the same flag.

## 2. Planner semantics (`model.rs`, `plan.rs`, `report.rs`)

- `RosterPart.required: u32` (serde default 1, so wishlist
  JSON parses unchanged and tests may set it inline);
  `Roster::apply_required_counts` overlays the table;
  `model::load_required_counts` parses the TOML.
  `report.rs` applies the table at roster load; a missing or
  unparseable table records an endpoint failure and degrades
  to the all-x1 default — never a guessed count.
- `PartRow` gains `held` / `required`. State machine, with
  the #84/#89/#93 precedence (verified > unverified >
  want/unknown) preserved:
  - verified >= required → `Have`, held = verified count;
  - else an unverified entry → `HaveUnverified`, held =
    required (the ruling: the assertion covers the part);
  - else wanted → `Need`, held = verified count — a part at
    0 < held < required is a PARTIAL: a real target with
    `required - held` units to go;
  - else `Unknown`, held = verified count (a partial holding
    on an unwanted part is progress the plan can't spend —
    it satisfies nothing and targets nothing).
- Consumer audit: every targeting consumer — `pick_focus`,
  `derive_action`'s needed list, `build_chunks`, the
  `partially_unobtainable` probe — keys on `state == Need`,
  and partials ARE `Need`, so they stay in focus, in card
  actions and in the crack slate with no call-site changes.
  Coverage remains name-based over wants (a partial is still
  wanted), `best_crack_candidate` is per-part and
  count-agnostic, and `is_held()` keeps its meaning —
  "satisfied" — everywhere it gates exclusion.
- Card progress is counted in UNITS: `parts_total` = Σ
  required, `parts_have` = Σ min(held, required) (surplus
  copies of one part never paper over a missing unit of
  another). The card sort's remaining-parts key is now
  remaining units. `GoalCard.ready` = every part satisfied
  by either evidence class; already-owned gear never reaches
  a card (`prune_wants`), so ready means buildable now.

## 3. Rendering (`render.rs`)

- `seg_html` (shared by Hunt and Backlog, so both segment
  identically): one slot per part type; a required-r part
  renders r sub-segments (`.seg.sub`, 6 px half-pips) filled
  per held unit. Filled segments take the state's class
  (gold / dimmed-gold ⍻); a partial's filled segments wear
  Have gold — verified units in hand — while its unfilled
  segment and its row stay a target's. Unknown stays dashed.
- Hunt rows for required > 1 state the units outright —
  "Blade 1/2", "⍻ Handle 2/2" (held capped at the
  requirement, matching the header's progress framing).
  The header fraction is the unit fraction: 5/6, not 4/4.
- Ready surfacing, Hunt only: `hunt_html` stable-partitions
  ready cards to the top (relative order preserved within
  both groups), and the card's `<article>` gains class
  `ready` → `.g.ready{border-color:var(--gold)}`, the
  palette's existing gold. Backlog order is deliberately
  untouched (planner order stands there).

## 4. Gates

Five net-new tests (lib 324 → 329; suite 357 passed /
0 failed, census gates untouched — no capture changes):

- `model`: the shipped table pins at 49 rows, all x2, known
  examples present, Zhuge absent (the flagged disagreement),
  and applied to the real seed roster the counts land on the
  right parts with defaults elsewhere.
- `plan`: Quassus at 1-of-2 Blades — Need with held 1 /
  required 2, card at 4/5 units, not ready, and the Blade in
  focus AND in the crack slate; at 2 Blades — Have, 5/5,
  ready. ⍻ on the x2 Blade with no verified units —
  HaveUnverified with held = 2, card ready (the ruling).
- `plan`: the six-unit card (Akarius shape) — 5/6 not ready,
  the final Receiver unit flips `ready`.
- `render`: a x2 part at 1 unit fills exactly one of two
  sub-segments; ⍻-filled segments carry `uvhave`; rows read
  "Blade 1/2" / "⍻ Handle 2/2"; the header shows 3/4.
- `render`: ready cards lead Hunt under `class="g ready"`
  (exactly the ready ones), stable within groups; Backlog
  order unchanged.

Decisions made en route: the curated table over runtime
derivation (stated in §1); ready ordering applied in the Hunt
renderer rather than in `goal_cards`, so planner order — and
the Backlog built on it — stays a single source of truth;
an unwanted partial is `Unknown` (its units ride the row but
satisfy nothing); row/header held figures cap at the
requirement (progress framing — true counts remain in the
stock file for the future surplus work).
