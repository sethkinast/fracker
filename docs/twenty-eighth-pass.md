# Twenty-eighth pass — draining the queue (addendum #84–88)

Five queued items, built in size order. Full test suite green
(268 tests). Resolutions are also appended to each addendum entry.

## #88 — Varzia badge replaces the vault V (render.rs)

`hunt_card_html` emitted `<em class="bd v">V</em>` on `g.vaulted`
unconditionally, so a resurgence-live goal showed both the V and the
Varzia marker. The V now requires `vaulted && !resurgence_live`.
Test: `varzia_badge_replaces_vault_badge_never_both`.

## #85 — Baro pill while live (plan.rs, render.rs)

`donow_baro` built `ends_in` from `days_until` (day granularity —
"in 0d" on the last day). While a trader window is live it is now
`"Leaves in " + mins_left_label(...)` ("Leaves in 25h00m" in the
pinned test), and the header pill span carries `class="baro-live"`
while `plan.baro` is present, with a `@keyframes baro-pulse`
opacity animation in the extensions CSS block. The not-active
branch (relative countdown to the next arrival) is unchanged, as is
the raw node id in the location text (a pinned decode gap — the app
never invents a name). `BaroView.ends_in` has exactly one consumer
(the header pill), so the string change is contained. Tests:
`donow_views_split_alerts_and_gate_trader` (exact string),
`baro_pill_pulses_only_while_the_trader_is_live` (both states).

## #84 — captured part stock reaches the plan (report.rs, plan.rs, render.rs)

`report.rs` discarded the parts bucket (`to_relic_stock()` only).
It now keeps the parsed `StockState` and passes
`PlanInput.part_stock`: the canonical map from
`StockState::canonical_part_counts` (landed with pass 27; built and
blueprint forms summed onto the roster part per #89 rule 3).
`part_rows_for_gear` constructs `PartState::Have` at canonical
count >= 1 — the roster carries no required-quantity data, so the
threshold is 1 and is noted here rather than silently assumed —
else Need when wanted, else Unknown. Backlog pips light through the
existing `parts_have` derivation (it counts Have rows). Hunt part
rows denote Have in the page's own design language: a `have` row
class, the part name in gold (matching the Have pip colour), and a
"✓ " prefix. Tests at all three layers, including Seth's case
end-to-end: stock {"Dual Zoren Prime Handle": 1} → the Dual Zoren
Prime card's Handle row is Have and `parts_have` is 1. Addendum #77
(per-item part-resolution algorithm) remains DO NOT BUILD and was
not built.

## #86 — tab resets on refresh (tabs.rs, server.rs) — #52 SUPERSEDED

Mechanism, pinned before the fix: `report.rs::next_generation()`
mints a token unique to EVERY regeneration (a test pins that
uniqueness), the resident regenerates on worldstate ticks minutes
apart, and `TabStore::scope_to` cleared the remembered tab on any
generation change — invoked on every serve (`plan_page`) and every
tab record. Any background regen between choosing a tab and the
next serve wiped the choice; the WebSocket push reloads hit the
same path. That is the addendum's fork (i): #52's rule defeats #46
under generation churn.

Per Seth's instruction, #52's reset rule is superseded: `scope_to`
records the new generation but no longer clears `active`. The
serialized store is unchanged (the generation field is retained
and still updated); render still falls back to First Crack when
the stored slug names no current tab (`tab_id_for_slug(...)
.unwrap_or("t1")`). A refresh of any kind — manual, meta, or
push-triggered — now lands back on the viewed tab. Tests updated:
`scope_keeps_the_choice_across_generations`,
`a_pre_item_52_store_loads_as_unscoped` (legacy choice kept),
`a_new_generation_keeps_the_remembered_tab` (HTTP level). Stale
comments in tabs.rs, server.rs, and report.rs were rewritten to
match; the supersession is recorded on addendum #86 itself.

## #87 — crack-queue NEXT accent (render.rs CSS)

The accent was a gradient plus `box-shadow: inset 3px 0 var(--cy)`
on `.ch.next`. The row is a grid `<label class="row ch tick next">`
(padding 9px 0, border-top separator) inside a clip-path panel; an
inset shadow paints relative to the element's padding box and its
registration against the row's visual box is exactly what drifted
in Seth's screenshot. The bar is now deterministic:
`.ch.next{position:relative}` and
`.ch.next::before{content:"";position:absolute;left:0;top:0;
bottom:0;width:3px;background:var(--cy)}` — pinned to the row's own
box edges for the row's full height, independent of shadow paint
order. The gradient is unchanged. Honest limit: no live-DOM
verification was available in the build environment; the pinning
render test asserts the new construction and the absence of the
old shadow, and visual confirmation on Seth's next session is the
remaining check.
