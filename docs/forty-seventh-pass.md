# Forty-seventh pass — item search: one box, the plan's own status

Verification addendum: #112.

## The ask

Seth's "next feature" sketch in QUEUE_REVIEW.md (2026-10-26):
"a little search bar where I can quickly search for an item to
see its status" — one search box, results drawn from the app's
existing plan knowledge. Fixed at brief time: the box lives in
the page header (reachable from all four tabs), answers come
from the already-computed plan (never a second computation),
matching is plain case-insensitive substring (no fuzzy — this
app refuses to paper over garbles), and the surface is
read-only: no ticks, no ⍻ toggles, no tab changes ride along.

## The shape

Pass 41's architecture is not re-opened: the server stays the
only renderer.

- **Snapshot, not request-time rendering.** `GET /search` is
  answered from a `SearchSnapshot` — the generation's `Plan`
  plus the relic tables plus a per-tier distillation of the
  parsed stock file — published by the report pipeline at the
  page's own publication point (next to the generation-hub
  publish, after the report file is renamed into place). A
  search answer is therefore always the generation the page
  on screen shows. The snapshot distils at publish time
  because `RelicStock` carries only totals + a radiant split;
  the per-tier stock a relic result shows lives in the parsed
  `StockState`, which only the pipeline holds. `report.rs`
  keeps the parsed state whole and derives the planner's
  shapes from it exactly as before.
- **Endpoint.** `GET /search?q=<name>` returns a
  server-rendered HTML fragment (`src/search.rs` renders every
  result block). Unwired (no store — `--demo`, tests), the
  route answers 404, mirroring the unverified toggle; wired
  but pre-first-generation, the fragment says "No plan
  generated yet."
- **Box.** One text input in the header fragment (so every tab
  reaches it and fragment swaps carry it), with its results
  container under it. The page script fetches
  `/search?q=…` debounced at 150 ms; answers land as the
  container's innerHTML. Two disciplines from pass 41 are
  kept: bindings are once-per-element (a header swap re-binds
  the fresh nodes), and box state (query, results, focus)
  lives in script closures that `__frackerResync` restores
  onto swapped-in DOM, so a pushed generation never eats a
  query mid-typing. **Staleness: monotonically increasing
  sequence numbers** — a response renders only when its
  sequence is still the newest; an empty query clears the
  results and invalidates in-flight answers; Escape clears.
  The box is hidden on non-http pages (file exports), whose
  loopback endpoints do not exist.

## What a result shows

Matching is case-insensitive substring over gear, part, and
relic names; hits rank exact > prefix > substring, gear before
parts before relics within a rank, then by name; at most 8
render, with a "+N more — keep typing" line beyond that.
Ambiguity is shown, never silently resolved. No match is a
plain line: `No match for "<query>" in the current plan.`

- **Gear** — the card's state: vaulted / resurgence / READY
  badges, "have / total parts", every part row with its state
  (✓ held / ⍻ unverified / needed / unknown), held/required
  units on multi-unit parts (the pass-42 model), the part's
  relic source, and the card's crack line — rendered by the
  Hunt card footer itself, extracted verbatim into
  `card_footer_html` so the search hit and the Hunt card can
  never drift apart.
- **Part** — its state, held/required, which goal gear(s) it
  belongs to (every card that lists it), its best crack where
  the planner computed one (relic, tier, chance, expected
  cracks, owned/radiant counts), and the relics that drop it
  (chance at Intact, vaulted tagged). The ⍻ state renders as
  a **badge only** — deliberately not `data-uv`, so a result
  row can never be mistaken for the Backlog's toggle surface.
- **Relic** — held count, the per-tier breakdown (Intact ·
  Exceptional · Flawless · Radiant), and which of its rewards
  are in genuine Need state (the pass-44 universe). Unknown
  stock says "stock unknown", never a guessed zero.

## Scope decisions (defaults taken at brief time, confirmed in build)

- **Relic results: included.** The per-tier distillation was
  cheap once the pipeline kept the parsed stock state; the
  brief's conditional ("include only if it falls out cheaply,
  else defer") resolves to include.
- **Part universe = the plan cards' parts.** A part appears
  when a goal card lists it, because `PartState` (and held /
  required) exists only there. A part on no card has no state
  to show, and a name-only hit would be a second, weaker kind
  of result; it does not match. Gear matches are card names;
  relic matches are the relic tables.
- **No jump-to-card.** Results are inline status only, per
  the brief's default; tabs are not switched and nothing is
  highlighted elsewhere on the page.

## Tests

Full suite green: 393 passed / 0 failed (lib 365; golden 15;
packaging 7; resident 5; tray_quit 1), up from 380 at pass 46 —
thirteen new tests (eleven in `search.rs`, two HTTP tests in
`server.rs`), plus search-surface assertions folded into two
existing `render.rs` tests.

- `search.rs` unit tests (11): ranking exact > prefix >
  substring (case-insensitive), the 8-result cap with the
  keep-typing line, gear status (progress, per-part states,
  multi-unit 1/2, the Hunt crack line × batch size), part
  status (multi-unit partial; ⍻ badge with no `data-uv`;
  best-crack line; relic sources), relic status (tier
  breakdown, Need-only rewards, unknown-stock variant),
  plain no-match, empty query → empty fragment, lookup
  before first publish.
- `server.rs` HTTP tests (2): the route serves the rendered
  fragment for a known item over real HTTP and the no-match
  fragment for nonsense, with the main page still serving;
  unwired, the route 404s.
- `render.rs`: the header fragment carries the box and its
  container while the five-fragment extraction test still
  passes unchanged, and the page-script test pins the fetch
  and the stale-response guard in the served script.
- Census gates untouched; no plan computation changed.
