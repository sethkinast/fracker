# Fiftieth pass — overlay batch (matching, shift, tags, Need rows)

Verification addendum: #116.

## The ask

Five queue items for the search overlay, all with Seth-set
criteria (queue A-3/A-4, B-4/B-5/B-6):

1. **Multi-word matching.** The overlay filter was whole-query
   single-token: any query containing a space matched nothing,
   so "axi a2" and "Dual Zoren" were dead although both words
   are tokens of the intended name. Seth's final rule: no
   artificial rules, the simplest implementation — a query
   matches when EVERY query word prefix-matches at least one
   of the name's tokens.
2. **Query-line shift.** The overlay's query line shifted
   UPWARD once the query held ≥ 1 character (Seth, live).
   His correction of record: it is NOT result mounting (at
   one character no result mounts; the filter engages at 2) —
   it is tied to rendering a glyph in the query line itself.
3. **UNVAULTED in mint.** The overlay's UNVAULTED indicator
   renders in a minty green: the accent light blue's exact
   saturation and lightness, hue swapped to green.
4. **Need-row rarity.** Prime-item results: rows for parts
   in state Need also show the part's rarity.
5. **Need-row best-match relic.** Each Need row also shows
   the best relic to crack for that part by the planner's
   standard source ordering — per-part only, NOT an
   item-level crack line (declined in pass 47, still out).

## 1. Multi-word matching (`src/search.rs`)

`token_fit` now splits the normalised query on whitespace
and, for EACH word, takes the smallest `token.len() -
word.len()` over the name's tokens the word prefixes; the
name's fit is the SUM of the per-word fits, and any word
prefixing no token fails the name. The rank key is
unchanged — (fit, name length, lowercase name) — so a
single-word query reduces to exactly the original measure
and the pinned ranking holds by construction ("Bo" before
"Boar": exact token fit 0 beats the longer prefixed token).
The two-character minimum still applies to the query string
as a whole; there is no per-word minimum. The client
script's `fitOf` mirrors the per-word sum; the mirror test
pins the split and the per-word prefix test.

Pinned as tests: "axi a2" → Axi A2; "meso c11" → Meso C11;
"Dual Zoren" → Dual Zoren Prime; "axi z9" and "Dual Boar"
match nothing (one unprefixable word sinks the query);
multi-word ranking sums per-word fit ("axi a2" prefers
Axi A2, fit 0+0, over Axi A23, 0+1; "meso c1" prefers
Meso C1 over Meso C11). Accepted consequences, approved by
Seth explicitly and pinned: "Silva Aegis" now matches
Silva & Aegis, and reversed word orders match too.

## 2. The query-line shift — mechanism and fix

The overlay header `.so-top` is a flex row with
`align-items:baseline`; its items are the ⌘ prompt (30 px)
and `#search-query` (38 px). A flex item's baseline, when
the item has no text run, is synthesized from its bottom
margin edge (CSS Flexbox baseline rules). So with an EMPTY
query, `#search-query` aligned by its bottom edge (its
46 px min-height box), while the prompt aligned by its real
text baseline — and the row's geometry was computed off
that pairing. The moment a glyph landed, the query item
gained a real alphabetic baseline sitting well above its
bottom edge, the shared baseline jumped UP, and with it
the prompt and the whole row's height: the visible upward
shift, at exactly ≥ 1 character, and stable thereafter
(one line of text has one baseline however many glyphs) —
matching Seth's report in every particular, and having
nothing to do with the result block (which mounts at 2
characters).

Fix (one CSS rule, overlay extension block):
`#search-query:empty::before{content:"\00a0"}` — a
non-breaking space in the query's own 38 px font gives the
empty state the same alphabetic baseline as any glyph, so
the baseline alignment is content-invariant and the line
cannot shift. No markup, script, or layout changes.
Pinned by a render test asserting the rule ships in the
page CSS.

## 3. UNVAULTED mint (B-4)

The accent of record is `--cy: #aee3ef`. In HSL:
**#aee3ef = hsl(191.1°, 67.0%, 81.0%)**. The mint keeps
saturation and lightness exactly and swaps the hue to
green (150°): **hsl(150°, 67.0%, 81.0%) = #aeefce**.
Both overlay UNVAULTED badges — the prime item's and the
relic's, which were the app's only unmodified `.bd`
badges — now carry a `u` modifier, and the extension block
gains `.bd.u{color:#aeefce;border-color:#aeefce}`.
VAULTED (`bd v`) and VARZIA (`bd z`) keep their current
treatment; no other colours change anywhere.

## 4/5. Need-row rarity + best-match relic (B-5/B-6)

`search_part_row_html` gains a detail line
(`<span class="srdeal">`, grid-spanning second row of the
part row) for Need rows only, assembled from the plan's
own per-row computations — the overlay stays a view over
existing truth:

- **Rarity** — `PartRow.rarity`, the band of the
  best-ranked containing relic's Intact chance, rendered
  as the capitalised label in the app's rarity classes
  (`.common` bronze / `.uncommon` silver / `.rare` gold).
  Shown only when the part has a containing relic:
  `rarity_of(0.0)` bands a sourceless part as Rare, which
  is a fallback, not data, and must not print.
- **Best relic** — `PartRow.deal` (`best_deal_for_part`),
  the planner's per-part best-relic pick: lowest source
  tier (Owned > Unvaulted > Varzia > Unobtainable), then
  highest Intact chance within the tier, then name. (The
  crack queue's `best_crack_candidate` is deliberately NOT
  the source: it answers a different question — which held
  relic to crack — and skips relics Seth doesn't hold,
  which would erase the Unvaulted/Varzia display cases.)
  Display: Owned → name + `×N`, N = `deal.owned`, which is
  `RelicStock::count` — the TOTAL across refinement tiers,
  the same total the relic result shows; Unvaulted or
  Varzia (unowned) → name alone; Unobtainable → nothing;
  no deal (no relic source) → nothing.

Worked examples (all pinned in the gate test): a Need
Bo Prime Blade whose best relic is an owned Meso C11 ×5
renders `Rare · Meso C11 ×5`; a Need Handle sourced from
an unowned unvaulted Lith S19 renders `Common · Lith S19`;
a Need Ornament whose best is Varzia's Meso V16 renders
`Uncommon · Meso V16`; a Need Stock whose only source is
the unobtainable Axi Z9 renders `Rare` alone; a sourceless
Need Grip renders no detail line at all; and a Have row
with an identical owned deal renders none either.

## Gates

Suite 409 passed / 0 failed (lib 381; golden 15;
packaging 7; resident 5; tray_quit 1) — new: the three
search tests above (per-word pins, order-free pins,
summed-fit ranking; the whole-query pin replaced), the
Need-row gate test (all four B-6 display cases, the
sourceless guard, the Have-row exclusion, both mint
badges), and the shell test's new assertions (nbsp strut
rule, mint rule). Census and reward gates untouched.
`cargo build --release --locked --offline` green pre-tag.

Live verification owed: Seth types "axi a2" and "Dual
Zoren" in the overlay, watches the query line while typing
the first character, and eyeballs a Need row on a live
item result.
