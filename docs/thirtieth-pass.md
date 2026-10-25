# Thirtieth pass — unverified stock (addendum #93)

Seth's spec (2026-10-25): "Clicking any row that is not a VERIFIED
stock should add it to a new section in stock.json for UNVERIFIED
stock. We'll use U+237B, the not-checkmark symbol, to denote
unverified stock. Clicking again should toggle back off. All
other surfaces in the app should respect unverified stock, but
only the Hunt tab allows mutating the unverified stock."

The motivation is the capture's own honesty: even after the
pass-29 decoder, the never-guess rule refuses reads it cannot
prove, and some of those parts are genuinely in Seth's
inventory. Rather than weaken verification, the assertion
becomes Seth's — recorded as his, rendered as his, and spent
the moment capture verifies the part itself.

## Interpretation (pinned with the brief)

- "Rows" are the per-part rows inside Hunt goal cards. A row
  whose part is verified-Have is NOT clickable; every other
  part row (Need, Unknown, already-unverified) toggles.
- Membership is keyed by the ROSTER part name — the canonical
  key `canonical_part_counts` already produces — so form (built
  vs blueprint) is irrelevant at this layer; canonicalisation
  sums forms for verified stock, and the assertion names the
  part itself.
- Toggling asserts "I hold this part": progress semantics
  identical to a verified hold (count 1), visually and
  structurally distinct everywhere it appears.

## Data layer (src/ocr.rs)

`StockState` gains one field, serialised as a top-level
`"unverified"` map alongside `relics` / `parts` / `arcanes`:

```rust
#[serde(default)]
pub unverified: BTreeMap<String, u32>,
```

Flat roster-part-name → count, count stored as 1 on toggle-on;
toggle-off removes the entry outright (no zero counts). The
`#[serde(default)]` is the backward-compatibility story: every
stock.json written before this pass loads with the section
empty (pinned by a round-trip + legacy-format test).
`StockState::toggle_unverified` is the pure transition;
`unverified_part_counts()` hands the planner its map in the
same shape as `held_parts()`.

## Finalize: preserve + prune (src/capture.rs)

`CaptureSession::finalize_into` still replaces only the buckets
that saw classified frames, and the unverified section is not a
capture bucket — it rides through untouched on the loaded
state. One exception, inside the `saw_parts` branch: entries
the fresh capture verifies are pruned, because the assertion
is spent once capture confirms the part. Verification is
checked against the captured parts bucket by
`part_verified_in_bucket`: the bucket holds raw screen names,
so a roster part is verified when its own name OR its
built-component form (name minus a trailing " Blueprint") was
captured at count ≥ 1 — exactly the two forms
`canonical_part_counts` sums under #89. The gear's own
"<Gear> Blueprint" strips to "<Gear>", never a captured part
name, so it cannot false-prune. A capture that never saw the
Parts screen prunes nothing (pinned by tests both ways).

## Planner semantics (src/plan.rs, src/report.rs)

`PartState` gains `HaveUnverified` between the two existing
held/want states. Resolution in `part_rows_for_gear`, per part:

1. canonical verified count ≥ 1 → `Have` (unchanged),
2. else unverified membership → `HaveUnverified`,
3. else today's `Need` / `Unknown`.

`PartState::is_held()` (Have | HaveUnverified) is the single
predicate for held-semantics consumers: `parts_have` (goal
progress) counts both. Farming-target selection everywhere
filters on `Need`, so an unverified-held part drops out of
crack planning and goal actions with no further changes — it
is respected, not targeted. report.rs threads the third stock
shape through beside `to_relic_stock()` and
`canonical_part_counts()`: `StockState::unverified_part_counts()`
into the new `PlanInput::unverified_parts`. A test pins the
precedence chain, including an asserted *want* leaving Need,
and verified capture outranking the same part's assertion.

## Rendering (src/render.rs)

- Pip class `uvhave`: lights like `have` (gold) at 45% opacity
  — lit, but visibly not the solid verified pip.
- Hunt rows: verified → "✓ name" as today (gold, no handler);
  unverified-held → "⍻ name" (dimmed gold), class `uvhave`;
  the have/total count in `seg_html` appends " ⍻" whenever any
  part of the card is unverified-held, on Hunt cards and
  Backlog cells alike — a goal held only via assertions is
  distinguishable from a fully verified goal on its card.
- Mutation affordance: every Hunt part row that is not
  verified-Have gets `class="... uv"`, `data-uv="<full roster
  part name>"`, `role="button"`, and a `title` spelling out the
  toggle ("Click to assert unverified stock (⍻): I hold this —
  capture has not verified it" / "…click to withdraw the
  assertion"). CSS gives `.r.uv` a pointer cursor and a dotted
  underline on hover. Verified rows emit none of it.
- The page script binds `.r.uv[data-uv]` clicks to
  `fetch('/unverified', POST {"name": ...})` in the same
  fire-and-forget style as `/tick`; the state lands through
  the regeneration push / meta-refresh, not optimistic paint.
- Backlog renders the ⍻ state (dimmed pips, ⍻ count) and NO
  handlers — `hunt_card_html` is the only emitter of
  `data-uv`, pinned by a render test scanning every page.

One pre-existing assertion was widened by this change:
`unobtainable_relic_rows_strike` pinned the exact string
`<div class="r dead">`; a Need-state Hunt row now legitimately
continues its class list with the toggle class/attributes, so
the test pins the `<div class="r dead` prefix instead. The
strike behaviour it exists to pin is unchanged.

## The endpoint (src/server.rs, src/resident.rs)

`POST /unverified` joins `/tick` in the resident's loopback
server, following its established shape: JSON
(`{"name": "..."}`) or form body, visible 400 on an
unparseable body, JSON response `{"ok":true,"unverified":bool}`.
The wiring is an `UnverifiedToggle` context the resident hands
`server::serve` (stock path, roster seed path, regen hook);
a server without it answers the route 404.

`UnverifiedToggle::toggle`:

1. Takes the context's mutex (the TickStore pattern — the file
   is rewritten whole, so read-modify-write cycles serialise).
2. Re-reads the roster seed (`<data_dir>/cache/wishlist.json`)
   and validates the name against the roster parts universe —
   the same key set the planner's canonical map uses. Unknown
   name → 404, no state change, no regeneration. (Re-read per
   click: toggles are rare, and the seed can be refreshed by a
   fetch between clicks.)
3. Loads stock.json (missing → default; unparseable →
   `Unavailable`, never clobbered — the same caution as
   capture finalize), toggles, and persists through
   `StockState::write_to` (temp + rename).
4. Fires the regen hook: the resident passes a closure sending
   `Trigger::Manual` into its trigger channel — the tray
   Regenerate verb's own path, which schedules a report and
   never auto-opens the page. The new generation publishes
   through the GenerationHub, so open pages reload with the ⍻
   state immediately.

The endpoint is necessarily view-agnostic — it cannot know
which tab a POST came from. The specified control is the UI
gating (only Hunt emits callers); the server-side defence is
the roster validation. Recorded here so the boundary is a
decision, not an oversight.

## Tests

285 green (277 baseline + 8 new):

- ocr.rs: unverified section round-trips under its own key;
  a legacy file without the section loads it empty;
  `toggle_unverified` flips on at 1 and off by removal.
- capture.rs: finalize preserves unverified entries for parts
  the capture never saw, prunes assertions verified exactly and
  by built form, and prunes nothing on a parts-less capture.
- plan.rs: precedence verified > unverified > Need/Unknown;
  unverified holds count in `parts_have`; an asserted want is
  held, not a target.
- render.rs: ⍻ row + handler on unverified and Need rows, ✓
  and no handler on verified rows, dimmed pip + ⍻ count on
  Backlog, `data-uv="` absent from Backlog/First Crack/Routine.
- server.rs: body parsing (JSON + form); a full HTTP
  round-trip — toggle on persists `{name: 1}` and fires regen
  once, toggle off removes it, unknown name is 404 with no
  state change and no regen, garbage body is 400.
- tests/golden.rs gained the new (empty) PlanInput field;
  golden outputs are unchanged.
