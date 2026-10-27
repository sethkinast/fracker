# Forty-seventh pass — item search overlay (Seth's spec)

Verification addendum: #112.

## The ask

Seth's spec, set by him directly (2026-10-27) after he halted a
first draft whose acceptance criteria he had not written: an
Alfred / cmd+space-style overlay for checking an item's crack
status DURING the 15-second reward window — speed and glance
legibility are the design drivers. Not a page search box.

- Trigger: an icon in the header, AND a global key listener —
  the page has no text inputs, so any printable keypress
  (Ctrl/Cmd/Alt combos ignored) opens the overlay seeded with
  that character. Esc closes; window/tab blur closes. Those
  are the only exits.
- Look: ~90% dark opaque, much bigger type than the page.
- Interaction: no submit, no selection, no jumping or
  navigation. Typing filters; exactly ONE result shows, fully
  expanded.
- Index: relics and prime items (gear) only. Parts are not
  indexed standalone.
- Matching: prefix match by word token, never substring. A
  name tokenises into words; the token "Prime" is excluded
  (it discriminates nothing). The ENTIRE query is compared as
  one string — never tokenised — and a name matches iff the
  query is a prefix of one single token. Minimum query length
  2. Ranking: closest token fit first (a token EQUAL to the
  query beats a longer token it prefixes), then shorter name,
  then alphabetical.
- Relic result: vault status; his stock count, TOTAL only
  (no refinement-tier breakdown); its rewards common-first in
  the app's rarity colouring, each marked by his state (Need
  vs Have).
- Prime item result: its parts with acquisition status
  (incl. held/required), its vaulted status, and ⍻
  (unconfirmed) toggling via the existing POST /unverified
  path — the one permitted mutation. NO crack line (Seth,
  explicitly excluded).
- All status derives from the already-computed plan and
  stock; the overlay is a view over existing truth, never a
  second computation of it.

## What shipped

**Matching core** (`src/search.rs`). `name_tokens` (split on
non-alphanumerics, lowercase, drop "prime"), `token_fit`
(smallest token-minus-query length over prefixed tokens),
`best_match` (min by fit, name length, name). The pinned cases
are unit tests: "Bo" beats "Boar"; "Silva Aegis" matches
nothing while "Silva" and "Aegis" each match "Silva & Aegis
Prime"; "prime" matches nothing; a 1-character query engages
nothing; case-insensitive; relic code tokens match ("C11"
finds Meso C11); substrings never match.

**Index fragment** (`render::search_index_html`). Every prime
item (the plan's goal cards) and every relic (the full
800-name table) as a fully rendered result block, items then
relics, each alphabetical. A block carries its matching
metadata in data attributes (`data-kind`, `data-name`,
`data-tokens`); the client filters over those and clones the
single winning block into the overlay — it never templates
plan data. The render context gained a `SearchData` borrow
(the report pipeline's own locals: relic tables, relic stock,
canonical part stock, ⍻ map — the same values PlanInput
took), so the overlay cannot drift from the planner's truth.

- Item block: icon, name, the Hunt card's vault badges
  (VAULTED / VARZIA / UNVAULTED, same rule), the card's
  segmented progress, and one row per part reusing the Hunt
  row's exact toggle contract (classes, `data-uv`, titles) —
  verified-Have rows never toggle; every other row does. The
  `.gf` crack footer is absent by construction (pinned by
  test against a card whose action text is a crack line).
- Relic block: name, vault badge, stock as
  `RelicStock::count` — the per-relic TOTAL across tiers;
  "stock unknown" when holdings are Unknown. Rewards sorted
  common → uncommon → rare (rarity banded off the Intact
  chance by the planner's tuning, stable within a band) in
  the app's rarity colours. Each reward's state mark comes
  from the plan's own part rows when the plan tracks the
  part; otherwise from the raw stock shapes (verified count,
  then ⍻ assertion); a reward the plan does not track at all
  (Forma, a part of gear he already owns) carries NO mark —
  it is not a Need, and the overlay never invents a Have.
- Relic icon: the brief asks for the relic's icon in the
  result header. The app's icon dataset (data/icons.json) is
  gear-only (67 entries) and the relic tables carry no image
  field — no relic image source exists in the app's data.
  The header therefore takes the same letter-monogram chip
  an unresolved gear icon gets, rather than an invented URL.
  Recorded here as a data-forced substitution; if a relic
  image source is adopted later, the chip is the one-line
  swap point (`search_relic_html`).

**Delivery** (pass-41 discipline, latency-adapted). The index
is a per-generation SIDECAR: `generate()` renders it from the
same plan + context and publishes it (tmp+rename) beside the
report as `plan-search.html` BEFORE the page itself, so a
client that sees the new generation always finds the
matching index. A sidecar write failure degrades the overlay
only and is swallowed — never the report. The loopback
server serves it at `GET /search-index` (a missing sidecar is
an empty 200). A sidecar rather than a sixth page fragment:
the ~800 relic blocks (~690 KB against real data) stay out of
the plan page's payload and out of the fragment-diff path.

**Client** (`search::client_script`, page script tail). The
overlay shell (query line, result mount, hint) ships in the
page OUTSIDE every fragment, so fragment swaps never disturb
it; the trigger icon rides inside the header fragment (its
click is document-delegated, surviving header swaps). The
script PREFETCHES the index at page load and refetches when
the generation changes (chained onto `__frackerResync`):
prefetch, not lazy-first-open, because the overlay exists
for the reward window — the first open must never wait on
the network, and loopback makes the prefetch free. Keystroke
handling is local: seed on open, append printable chars,
Backspace deletes, Esc closes, blur closes; while the
overlay is closed, modifier combos and non-printables are
ignored and typing in a form field (none exist today) is
respected. The shown block's ⍻ rows POST through the same
`/unverified` path under the same once-per-element guard
flag the page's binder uses, so the two binders can never
double-bind a row; the toggle's regen flows back through the
normal push/poll sync, which invalidates and refetches the
index, so statuses stay truthful. Opening/closing touches no
tab, no tick, no page state.

**Look.** Fixed overlay at rgba(4,6,9,.9); query at 38px,
result title 30px, rows 20px against the page's 14px body —
one-glance sizing. Styles live only in the renderer's
extension CSS block; the reference stylesheet is untouched.

## Tests

Full suite green: 396 passed / 0 failed (lib 368; golden 15;
packaging 7; resident 5; tray_quit 1), up from 380 at pass
46 — sixteen new tests:

- `search`: the pinned matching cases above, the ranking
  tie-breaks, the tokeniser, the sidecar path, and a mirror
  test pinning the client script to the matcher's constants
  and semantics (minimum length substituted from the Rust
  constant, token-prefix test, routes, Esc/blur, resync
  chaining).
- `render`: the relic block (vault status; ×5 total with a
  3-Radiant split present in the fixture and no tier text;
  rewards reordered common-first in rarity classes; exactly
  one Need mark and one Have mark, the untracked Forma
  unmarked); the item block (progress, ✓/⍻/Need rows,
  toggle offered on unverified and Need rows, never on
  verified Have, and no crack line); the page gate (overlay
  shell ships, script prefetches, five fragments still
  extract in order, trigger icon inside the header fragment);
  empty index without search data.
- `server`: `/search-index` serves the published sidecar and
  answers an empty 200 before one exists.

Census gates untouched. End-to-end smoke (offline demo
fixtures): the sidecar lands beside the page with 861 blocks
(800 relics + 61 goal items); the Meso C11 block shows its
rewards common-first with the Citrine-family parts marked
Need and Paris Prime Blueprint correctly unmarked (owned,
mastered).

Live verification owed: Seth opens the overlay during a real
reward window — trigger feel, one-glance legibility at his
resolution, and a ⍻ toggle from the overlay landing in the
plan.
