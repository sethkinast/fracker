# Sixty-second pass — small-fixes bundle (v0.5.57)

Verification addendum: #128.

Seven small fixes, released by Seth 2026-11-18 00:10 EST
("that's enough for a build for tonight"). The queue was
built up over the evening of Nov 17–18: Gift of the Lotus
losing its strike overnight, hours-only durations,
Ctrl+A dead in the search overlay, the tab title, a
favicon, Nightwave ticks not surviving the day, backlog
gaps (Mewan twice, no pets, no amps), and the MR31 stamp
for a Legendary 1. Nothing else is in scope. Explicitly
NOT in this pass: the p75 acquisition-budget value model
(needs Seth's confirm on the p75 target,
Radiant-means-radshare accounting, and traces priced in
relic terms), Maroo mission-completion detection
(mechanism choice pending: weekly_ledger vs TickStore),
the profile long-TTL direction (held), the
invalid-payload latch follow-up (decision pending), the
chime volume tweak (parked), and the pass-38 template
gate (decision owed).

Four implementation slices ran in parallel worktrees off
origin/master `58e931d` and merged into one integration
branch (retention, format, overlay, backlog). One merge
conflict, in `src/render.rs`: the format slice's new
`<title>Fracker</title>` against the overlay slice's
favicon link in the same head block — resolved to keep
both. Integrated suite: **535 passed, 0 failed** (lib
505, up from 481 at v0.5.56; ab_pass61 1, golden 15,
packaging 7, resident 6, tray_quit 1; 24 new tests).

## 1a. Gift of the Lotus keeps its strike across the day boundary

The §4 mission ledger was today-scoped: the watch loop
retained `mission_completions` to `c.day == today` at
startup and on UTC day rollover, so a long alert
completed yesterday lost its strike at midnight UTC even
though the alert was still live. Retention is now keyed
to the alert's lifetime instead of the day.

`elog.rs` gains a pure retention predicate driven by an
explicit `now` (fake clock):
`MissionCompletion::strikes_live_alert` /
`strikes_any_live_alert` / `retain_live_completions`. A
completion is kept only while it still strikes a live
alert — exact `alertId` or tag+node+type corroboration
(the activation guard is unchanged) *plus* a parseable
expiry after `now`. Empty expiry counts live (legacy
feed); present-but-unreadable expiry fails closed
(counts expired). A completion whose alert expired or
left the worldstate is pruned; a plain non-alert
completion, which the planner never strikes, is pruned
too.

`report::generate` prunes the ledger against the
worldstate it just fetched and feeds only the live list
to the planner; the pruned list rides a new
`ReportOutcome::retained_completions` field, which
`do_report`/`open_page` write back into the shared
ledger via `apply_retained_completions` — snapshot-only
drops, so a completion proved while a report was
generating survives. The watch loop adopts the shared
(pruned) ledger before persisting `state.json`, so
expired completions leave the file too. Deliberate
guard: pruning applies only on a fresh worldstate
(`Live`/`WfsLive`). A TTL-cached, fallback, or fixture
payload may predate the alert, so the ledger passes
through unpruned there and the next fresh report prunes
it.

Tests (5 new, all passing):
`retention_keeps_yesterdays_completion_for_a_still_live_alert`
(Lotus completed 2026-10-17 23:30, alert live until
2026-10-18 12:00, `now` 2026-10-18 08:00 → retained),
`retention_prunes_a_completion_whose_alert_has_expired`
(same completion, expiry 2026-10-18 00:00 → pruned;
alert absent from worldstate → pruned),
`retention_corroboration_path_is_liveness_gated_too`
(tag+node+type path live vs expired; non-alert
completion pruned),
`startup_keeps_yesterdays_completion` (yesterday-stamped
completion seeded in `state.json` is still in the shared
ledger after the watch loop starts),
`retained_writeback_drops_only_pruned_snapshot_entries`
(write-back drops only pruned snapshot entries; a
mid-report completion survives).

## 1b. Duration formatting uses days, two places of granularity

Seth's rule, verbatim: "keep only two places of
granularity, so d/h or h/m, not d/h/m". The shared
helper `plan::format_duration_mins` (pub(crate)):
under 60 minutes → `"25m"`; under 24 hours → `"5h30m"`;
24 hours and up → `"12d 14h"` (minutes dropped, never
d/h/m); exact whole days render bare (`"2d"`, no
`"0h"`). Applied at all four hours-only sites:
`mins_left_label` (plan.rs, Baro pill), the event-alert
chunk title (plan.rs), `days_until` (plan.rs,
resurgence pills), and `relative_countdown` (render.rs,
header pills). The alert title also zero-pads minutes
now (`5h5m` → `5h05m`), matching `mins_left_label`'s
existing `{:02}` and the render doc's `"1h05m"`.
`parse_left_minutes` was extended so the new `"12d 14h"`
labels still parse for the alert heat class (its doc
already claimed `"in 2d"` parsed; now it actually does
— day-long alerts stay cold, same outcome as before).

Judgment call, recorded: for `days_until` /
`relative_countdown`, below 24h the existing coarse
`"in Nh"` (one place, within the two-place cap) was
kept rather than adding minutes to header pills; only
`>=24h` changed to d/h. Consequence: the tuned
`days_until_hour_boundary = 48` in
`data/tuning/planner.toml` is no longer consulted by
`days_until` — the brief's verbatim 24h rule overrides
it. The field and TOML entry were left in place;
removing config is a cleanup-pass candidate, not this
pass.

Tests (4 new + 1 updated, all passing):
`format_duration_mins_keeps_two_places_of_granularity`
(302h16m → `"12d 14h"`, 25h → `"1d 1h"`, 24h → `"1d"`,
48h → `"2d"`, 5h30m → `"5h30m"`, 1h → `"1h00m"`,
23h59m, 25m, 59m), `days_until_renders_days_with_hours`
(30h → `"in 1d 6h"`, 11d → `"in 11d"`, 5h30m → `"in
5h"`, past → `"soon"`),
`relative_countdown_renders_days_with_hours` (302h16m →
`"in 12d 14h"`, 30h → `"in 1d 6h"`, 5h30m → `"in 5h"`,
25m → `"in 25m"`), `parse_left_minutes_handles_day_labels`
(`"12d 14h"` → 18120, `"in 2d"`/`"2d"` → 2880,
`"1h05m"` → 65, `"5h30m"` → 330, `"25m"` → 25).
Updated: `donow_views_split_alerts_and_gate_trader` now
expects `"Leaves in 1d 1h"` instead of `"Leaves in
25h00m"`.

## 2. Ctrl+A selects the text in the search-box overlay

The overlay had no text input at all: the query was a
JS string and keystrokes were captured at document level
(`search.rs` `client_script`), so Ctrl+A hit the
modifier early-return and did nothing, and there was no
editable field for the browser to select in. The query
line is now a real `<input type="text">`
(`SEARCH_OVERLAY_SHELL` in `src/render.rs`;
`autocomplete` off, `spellcheck` false).

In `src/search.rs` `client_script()`:
`openOverlay()` sets the value and calls
`queryEl.focus()`, so typing, Backspace, Ctrl+A, Ctrl+C,
and paste land on the input natively. An `input` event
listener syncs `query = queryEl.value` and re-filters on
every edit. The document `keydown` handler returns early
for keys whose target is the input (checked after
Escape, before the modifier early-return), so it can no
longer swallow Ctrl+A or double-append typed characters.
The old document-level path survives for keys landing
outside the input; Escape-to-close, blur-to-close, and
the click-to-open trigger are unchanged. `paint()` only
writes `queryEl.value` when it differs from the script
query, so it never clobbers the caret or a Ctrl+A
selection. The fiftieth-pass `#search-query:empty::before`
nbsp strut was retired (the input's own line box holds
the baseline) along with its render-test assertion.

Tests (1 new + 2 updated, all passing):
`client_script_lets_the_input_edit_natively` (pins the
`e.target === queryEl` fall-through, its ordering after
Escape and before the modifier return, and the guarded
value write in `paint()`),
`client_script_mirrors_the_matcher` (now pins value
sync, `queryEl.focus()`, the `input` listener, and
asserts `queryEl.textContent` is gone),
`search_overlay_shell_and_trigger_ship_with_the_page`
(asserts `<input id="search-query" type="text"` instead
of the strut). All 14 `search::` tests pass.

## 3. Tab title is just "Fracker"

`render.rs` now emits `<title>Fracker</title>` (was
`"Fracker — session plan"`); `server.rs` already read
`"Fracker"` on its refresh page. New test
`page_title_is_just_fracker` asserts the exact title and
the absence of the old string. `docs/demo-plan.html`
still contains the old title; it is a static demo export,
left alone.

## 4. Favicon: the Warframe logo

The logo Seth provided was downloaded and verified
before the pass: the real Warframe Lotus logo, 840×859
RGBA PNG with transparency, 180,602 bytes, now committed
as the repo asset `assets/warframe-logo.png`
(byte-identical to the provided file; code references
only the repo copy). `src/server.rs` embeds it via
`include_bytes!` as `FAVICON_PNG` and serves it at `GET
/favicon.ico` with `image/png` via the cacheable
`respond_cached` path (immutable, like the chime).
`src/render.rs` adds `<link rel="icon" type="image/png"
href="/favicon.ico">` to the page head. No Flatpak or
manifest change was needed — the asset is embedded in
the binary.

Tests (1 new, passing):
`favicon_route_serves_the_warframe_logo_png` spins up
the real server, GETs `/favicon.ico`, and asserts 200,
`Content-Type: image/png`, the immutable cache header,
PNG magic bytes, and byte-equality with the embedded
asset (a byte-level `http_get_bytes` helper was added,
since the existing `http_get` reads to `String`). The
render test for the page shell also asserts the head
link tag.

## 5. Nightwave done-state survives to the Sunday reset

Nightwave acts render as checkboxes `data-id="nw-{id}"`
with done from `ctx.proven_done`; manual ticks went to
the day-keyed TickStore ("Ticks belong to the daily
reset, so only the current day is kept") and proven
completions came from the today-scoped §4 mission
ledger — so an act ticked Monday was unticked Tuesday,
even though the acts live until the Sunday reset. The
week anchor is **Sunday 00:00 UTC** (corroborated by
the Warframe wiki's Nightwave page — Weekly/Elite acts
refresh every Sunday 00:00 UTC). Note: `WEEKLIES.md`
says Monday 00:00 UTC for the Cred Offerings shop
rotation, which is a different system; the acts follow
Sunday.

`nw-` ticks are now stored under an
`nw-week:<sunday>` bucket (`nightwave_week_sunday` /
`nightwave_week_key` in `ticks.rs`) and read back on
any later day of that week; the next Sunday starts
unticked. `is_done` / `ids_on` also read pre-pass
day-keyed `nw-` ticks from plain day keys within the
same week (migration). Daily ticks keep day scope
(`retain_day` renamed `retain_current`: keeps today +
the current week bucket). The page script in
`render.rs` merges the week bucket on load (plus the
legacy same-week day scan) and keys its `file://`
localStorage fallback by week for `nw-` ids; `POST
/tick` prunes stale scope on a current-day tick only (a
stale page ticking yesterday's day never prunes today's
ticks). Daily acts turn over by id in worldstate, so
week-scoping every `nw-` id is safe for them too.

Tests (7 new, all passing):
`nightwave_week_sunday_anchors_on_sunday` (Mon
2026-11-16 → Sun 2026-11-15; Sat 2026-11-21 →
2026-11-15; Sun 2026-11-22 → 2026-11-22),
`nightwave_tick_survives_the_week_and_clears_after_sunday_reset`
(tick Mon 11-16 → done Fri 11-20 and Sat 11-21,
cleared Sun 11-22 and Mon 11-23; a daily tick made the
same Monday does not survive to Tuesday),
`nightwave_tick_unticks_within_the_week`,
`legacy_day_keyed_nightwave_tick_reads_within_its_week`,
`retain_current_keeps_this_week_nightwave_and_drops_last_week`,
`nightwave_tick_posts_to_the_week_bucket` (POST Monday →
week bucket, done Friday, cleared Sunday, persisted
file agrees),
`tick_script_merges_the_nightwave_week_bucket` (pins the
JS wiring: bucket lookup, `weekSunday`, week-scoped
fallback key).

## 6. Backlog: dedupe Melee, add Pets and Amps

There is **no regen script in the repo** — the backlog
is reconciled by hand from a fresh profile pull against
the WFCD datasets (per `docs/second-pass.md` §5 and the
verification addendum). So the fix lives in the two
places a next regen touches: the checked-in
`data/backlog.json` itself, plus the load path every
consumer goes through (`Backlog::from_json` in
`src/model.rs`, used by `src/report.rs`). The next regen
was left a source for the new categories: new
`data/amps.toml` (prism roster keyed by barrel
uniqueName) and a `[regen]` derivation note in
`data/companions.toml`.

### 6a. Melee dedupe

Root cause confirmed: the 8 duplicates are all Zaw
strikes, each appearing exactly twice (the regen joins
two weapon sources that both carry them).
`Backlog::from_json` now dedupes every
`unmastered_by_category` list at load (first occurrence
wins, order preserved). The `UniverseScrub` is
untouched — its existing test pins that it keeps
non-alias duplicates by design, and that test still
passes. Before/after: Melee 75 entries / 67 unique →
**67 entries / 67 unique**; Mewan appears exactly once.
All other categories were already duplicate-free and
are unchanged.

Tests (2 new, all passing):
`backlog_from_json_dedupes_category_lists` (synthetic
duped JSON → deduped, order preserved, primes
untouched),
`backlog_data_has_no_duplicate_category_entries` (every
category in shipped data duplicate-free, Melee == 67,
Mewan count == 1).

### 6b. Pets

Method, validated against the repo's own methodology:
masterable universe = 22 entries with `masterable:
true` in `data/cache/pets.json`; mastery from the
profile fixture XPInfo at the note's ≥900k companion
threshold. The threshold was validated against knowns
first: Shade Prime 722,234 XP is `in_progress` in the
backlog, Diriga 949,824 is mastered, Nautilus absent →
unmastered — all consistent. No borderline cases:
mastered pets are all ≥1,036,910 XP, unmastered are all
absent from XPInfo (0 XP).

Pets (12 unmastered): Bhaira Hound, Crescent
Vulpaphyla, Dorma Hound, Lambeo Moa, Medjay Predasite,
Nychus Moa, Panzer Vulpaphyla, Para Moa, Pharaoh
Predasite, Sly Vulpaphyla, Vasca Kavat, Vizier
Predasite. Mastered (10, correctly absent): Adarza,
Chesa, Hec Hound, Helminth Charger, Huras, Oloro Moa,
Raksa, Sahasa, Smeeta, Sunika. Excluded, not
inventable: Venari / Venari Prime (`masterable=false`;
Venari Prime already excluded per `companions.toml`
addendum #22). Caveat: Hound/Moa/Vulpaphyla/Predasite
entries are keyed in the dataset by a component part
(e.g. Hec Hound = `ZanukaPetPartHeadC`), not a
powersuit — but that is how WFCD represents them and
XPInfo tracks XP on exactly those keys (Hec Hound head:
1,737,226 XP), so the mapping is the dataset's own, not
the author's.

### 6c. Amps

Method: an Amp's masterable unit is its prism — XPInfo
tracks lifetime XP per prism barrel. Roster = the 8
`* Prism` entries in `data/cache/item-names.json`
(there is no WFCD Amps.json in the cache; recorded in
new `data/amps.toml` with barrel uniqueNames).
Threshold = the note's ≥450k weapon threshold (amps
live under `/Lotus/Weapons/`). Again no borderline
cases: mastered are 473,990–6,568,447 XP, unmastered
are all absent (0 XP).

Amps (4 unmastered): Cantic Amp, Lega Amp, Rahn Amp,
Raplak Amp (named `<Prism> Amp`, matching the
established "Mote Amp" form). Mastered (4, correctly
absent): Granmu (5,615,678), Klamora (6,568,447), Mote
(473,990), Shwaak (612,375). Note the brief's examples
"Mote Amp, Sirocco, Shwaak" are all mastered, so none
of them belongs in an *unmastered* list: Mote and
Shwaak per the XP above, and **Sirocco is mastered
(1,265,897 XP)** — it is the Drifter's pistol
(`/Lotus/Weapons/Operator/Pistols/`), not a prism-built
Amp, and is recorded as such in `data/amps.toml` and
the backlog note.

Tests (1 new, passing):
`backlog_data_has_pets_and_amps_categories` pins the
exact 12-name Pets list and the exact 4-name Amps list
and cross-checks every name against the masterable set
in `data/cache/pets.json` (asserts universe == 22,
Venari Prime excluded) and each amp stem's `<stem>
Prism` in `item-names.json`. The prime-count test is
untouched and passing: roster 163, mastered 99,
unmastered 63.

## 7. Mastery rank displays Legendary ranks

New `render::mastery_rank_label`: 0–30 → `"MR{n}"`,
31+ → `"LR{n-30}"`. The header stamp
(`render.rs:827`) now renders the label instead of
`"MR{mr}"`; the caps math at `render.rs:1801` still
uses the raw numeric rank, untouched. Tests (2 new, all
passing): `mastery_rank_label_maps_legendary_ranks`
(0 → `"MR0"`, 30 → `"MR30"`, 31 → `"LR1"`, 36 →
`"LR6"`), `header_stamp_shows_legendary_rank` (renders
the page at rank 31, asserts the stamp contains `>LR1
&middot;` and no `"MR31"` anywhere; at rank 30 asserts
`>MR30 &middot;`).

## Verification

Integrated suite, `cargo test --locked --offline`,
exit 0: **535 passed, 0 failed** — lib 505 (base 481 +
24 new), ab_pass61 1, golden 15, packaging 7,
resident 6, tray_quit 1, doc-tests 0. All 24 new tests
confirmed present by name in the integrated tree and
passing in the full run. `cargo build --release
--locked --offline` green pre-tag (see addendum).
Version bump 0.5.56 → 0.5.57 in Cargo.toml plus
regenerated Cargo.lock in the same commit; lock diff =
version line only (`cargo generate-lockfile` re-resolved
dependencies, so the lock was restored and the fracker
entry edited surgically instead).

Live verification owed: Seth plays on this build and
confirms the Gift of the Lotus alert stays struck the
next day (and an expired alert's strike is gone),
durations read `"12d 14h"` / `"5h30m"`, Ctrl+A selects
the search query, the tab title reads "Fracker", the
tab shows the Lotus favicon, a Nightwave act ticked
Monday still shows done Friday and clears after the
Sunday reset, Melee lists Mewan once with Pets and Amps
categories present, and the header stamp reads LR1.
