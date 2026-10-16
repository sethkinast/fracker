# Build brief — Fracker (Warframe session planner)

Name (Seth, 2026-10-15): **Fracker** — fracking + tracker, because
cracking relics is basically fracking. Repo: `fracker`; app id and
binary derive from the name.

Status: BRIEF ONLY — no build until Seth says go (2026-10-15).
Stack decision: Rust. Companion docs: `DESIGN.md` (short summary),
`data/targets.json` (seeded real targets), `data/cache/` (drop data).

## 1. What this is

A small tray-resident desktop app for Seth's Bazzite/KDE machine that
watches Warframe's own log file and, when a play session starts, shows
him a generated checklist page: what is worth doing this session, in
order, sized to the time he has. It keeps its own books from public data
and log events so Seth never hand-maintains anything.

One-line shape: **EE.log watcher + public-data planner + static HTML
checklist, wearing a tray icon.**

## 2. Hard constraints (do not cross, do not "improve" past)

- Clean data only. NO process-memory scanning or reading, NO
  session-nonce extraction, NO `api.warframe.com/api/inventory.php`,
  NO Overwolf/AlecaFrame, NO credentials of any kind (no password, no
  cookies — the Xuerian bookmark token in local config is the sole
  stored secret, and it is a read/write key for a wishlist, nothing
  else).
- Read-only everywhere. The app never writes to the game, the log, or
  any account. One optional exception, OFF by default and gated behind
  an explicit setting: pruning completed items from the Xuerian
  wishlist via its `/api/store`.
- Least privilege on disk: the sandbox sees EE.log and the app's own
  config/data dirs. Nothing else.
- Zero bookkeeping burden on the user (see §8 — all state is derived).
- Explicitly NOT tracked (Seth, 2026-10-15): **syndicate missions** —
  dead content at his stage, below the value threshold unless he is
  actively farming them, in which case he'll say so. Do not surface
  them in the plan, the week section, or filler. (Syndicate STANDING
  the daily bar is different — that IS tracked, §8.)
- Companion-app route, settled with correct mechanics (Seth's
  correction, 2026-10-15): the first-party Android app DOES log in
  on its own with account credentials — no PC client involved; its
  session is independent (and conflicts with a live game session).
  That does not open a route for us: third-party programmatic login
  to DE's auth endpoint sits behind Akamai bot defense and is
  effectively dead (per finneritter/warframe-item-tracker's
  GAME_INVENTORY_IMPORT.md, researched 2026-10), and reading the
  app's stored session off the phone would still be nonce reuse —
  the thing ruled out. The only working programmatic inventory
  route remains memory-scanning the running game client
  (Sainan/warframe-api-helper + WFHelper pattern). Out, unchanged.
  Inventory cold-start = the §8.1 OCR dump.
- No daemon outside the app, no systemd units, no cron. The tray app
  is the whole thing; when it's not running, nothing happens, and
  that's fine.

## 3. Environment assumptions (verify in spike phase)

- Bazzite (Fedora Atomic), KDE Plasma, Wayland session.
- Warframe via Steam + Proton. EE.log expected at:
  `~/.local/share/Steam/steamapps/compatdata/230410/pfx/drive_c/users/steamuser/AppData/Local/Warframe/EE.log`
  Variants to handle: alternate Steam library roots
  (`~/.steam/steam/…` symlink farm, mounted libraries under
  `/var/mnt/…` or `~/Games/…`). Config takes an explicit path override;
  auto-detection scans the standard candidates.
- The game recreates/truncates EE.log at launch (assumption — the
  watcher design in §7 must survive both, plus mid-session rotation).

## 4. User experience

### 4.1 Tray (StatusNotifier item, via `ksni`)

- States: idle (no session), session-active, plan-stale/error badge.
- Menu (complete list — nothing more):
  - Status line (disabled item): e.g. "Session active — plan updated
    12 min ago" / "No session detected" / "Inventory not scanned
    yet — open Inventory → Prime Parts in game" (§8.1).
  - "Open plan" — opens current checklist via OpenURI portal.
  - "Regenerate plan" — re-fetch world state, re-render. This is
    the core loop (Seth, 2026-10-15): the plan is a slate of
    self-contained chunks, not a time-boxed schedule; when he's
    finished a bunch of chunks he hits Regenerate and gets fresh
    ones. There is NO explicit time-budget input anywhere.
  - "Pause pop-ups" checkbox.
  - "Quit".
- No settings window in v1. Configuration is a TOML file (see §9)
  that the app watches and reloads; the tray menu links to it
  ("Edit config" opens it in the default text editor).

### 4.2 The checklist page (the actual UI)

- A single self-contained HTML file written to the app's data dir
  (inline CSS, no external assets, works from `file://`).
- Opened automatically ONCE per session start (portal `xdg-open`),
  unless pop-ups are paused. Never re-opened, never focus-stolen
  after that.
- Regenerated on: session start, every mission-end transition,
  manual Regenerate, and at most every 5 minutes while a session is
  active if world state changed materially (a fissure from the plan
  expired, the event alert rotated).
- `<meta http-equiv="refresh" content="60">` so an open tab converges
  without a server. Checkbox state persists in `localStorage` keyed
  by plan-item ID + in-game date, so regeneration does not un-tick
  the user's manual ticks; items the log proves complete render
  struck-through and pre-ticked regardless.
- Page sections, top to bottom:
  1. **Header**: session date/time, "updated HH:MM" stamp.
  2. **Do first — time-boxed**: live event alert (Taubound: node,
     mission type, Moonflesh payout, minutes left), expiring
     fissures that match targets, Baro if present and leaving < 24h.
  3. **Session plan** (ordered chunks): the planner output (§10),
     each chunk with an expected-minutes label (information, not a
     budget) and a one-line why ("Neo fissure, Taranis — Axi R10 can
     drop Khora Prime Blueprint").
  4. **This week** (status board, NOT plan chunks — Seth
     2026-10-15): every tracked weekly from `WEEKLIES.md` with
     done / not-done state and the weekly reset countdown —
     Clem, Maroo's Ayatan hunt, Archon Hunt, both Archimedeas,
     Netracells/Search Pulses, Circuit track, Kahl, vendor
     rotations, Nightwave weekly acts. Within the board, Clem and
     Maroo render LAST — Seth (2026-10-15): they are the least
     important items on it, "just there if you're passing by."
     Items check themselves off
     when a source proves completion (observability column in
     WEEKLIES.md); the rest are manual checkboxes. The board never
     enters the chunk queue and never blocks it — it's a "hasn't
     been done this week" reminder surface only.
  5. **Targets snapshot**: wants remaining (count + top few), wants
     completed this week. Mostly a reassurance footer.
- Visual bar: dark, quiet, readable at a glance from a couch —
  this is a checklist, not a dashboard. No charts in v1.

## 5. Architecture

Single Rust binary, `tokio` runtime, modules:

- `watch` — EE.log tail (offset-tracked reads, `notify` for wakeups
  + 2s poll fallback; handles truncate/recreate via inode + size
  checks).
- `elog` — thin wrapper over the upstream **`wf-log`** crate from
  the warframe-lite project (MIT, actively maintained, deps: just
  `tracing`): it provides the line parser, the event classifier (§7
  catalog), and the account-ID candidate scanner (§7.4). Ours adds
  only the session state machine (§7.3) and the profile-API
  confirmation step. Seth's call (2026-10-15): reuse beats
  maintaining our own patterns — upstream fixes and new events land
  for free. NOT on crates.io (verified) → Cargo **git dependency**,
  pinned to a rev, bumped deliberately. `cargo vendor` covers it for
  the offline Flatpak build. If upstream ever dies, the fallback is
  vendoring the crate into the repo (MIT permits it) — still not a
  from-scratch rewrite.
- `fetch` — HTTP (`reqwest`, rustls) for the four sources (§6), each
  with on-disk cache + TTL; all fetches fail-soft to cache.
- `state` — derived state store (§8): JSON file, atomic writes.
- `plan` — the planner (§10): pure function of (world state, profile,
  targets, drops, derived state, budget) → ordered item list.
- `render` — HTML template → file write (atomic rename).
- `tray` — `ksni` item; owns the menu actions; talks to the rest via
  channels.
- `portal` — OpenURI via `ashpd` (or `xdg-open` subprocess fallback
  when unsandboxed dev builds need it).

Threading: single async process; the watcher must never block on
network — plan regeneration is a spawned task, coalesced (max one
in flight, dirty-flag re-run).

## 6. Data sources (all verified live 2026-10-15)

| # | Source | Mechanics | TTL |
|---|--------|-----------|-----|
| 1 | World state | GET `api.warframestat.us/pc/` (~186 KB JSON) | 5 min during session |
| 2 | Public profile | GET `api.warframe.com/cdn/getProfileViewingData.php?playerId=<ID>`; 409 = no such account | 30 min |
| 3 | Xuerian targets | POST `wf.xuerian.net/api/load`, form `tokens=<token>` → `{wants, has, mastered, forma, public_key}` | 1 h |
| 4 | Relic/drop data | GET `wf.xuerian.net/json/relictables.json` (1.07 MB) + `json/wishlist.json` (177-item prime roster) | 7 days |
| 5 | Item names + images | WFCD `warframe-items` dataset (uniqueName → display name, `imageName`, components). Images served from `https://cdn.warframestat.us/img/<imageName>` — verified live 2026-10-15 (real 512×512 PNG for a relic icon). NO wiki scraping anywhere. The page hotlinks or locally caches icons from this CDN. | 7 days |

Profile response: `Results[0].LoadOutInventory.XPInfo[] =
{ItemType, XP}` — equipment-level affinity only; no components, no
resources. That limitation is a design input (§10 pruning works at
equipment level; part-level facts come from §7 events).

## 7. EE.log specification

### 7.1 Line format

Timestamped lines tagged by subsystem (`Sys`, `Game`, `Script`, …)
and level. Parsing and classification come from the upstream `wf-log`
crate (§5): (timestamp, subsystem, message) extraction, substring
classification per the catalog below. The crate's authors confirmed
the patterns against live captures — but they are still ASSUMPTIONS
for our purposes until validated against Seth's real log (§12): his
game version and platform are what the app actually runs against.

### 7.2 Event catalog

| Event | Marker (substring on message) | Source | Confidence |
|---|---|---|---|
| LoggedIn(name) | Sys subsystem, message starts `Logged in ` | wf-log | high |
| MissionInfoSeen | `with MissionInfo:` | wf-log (live capture 2026-08) | high |
| LevelConnected | `Game successfully connected to:` | wf-log | high |
| RelicRewardScreen | `ProjectionRewardChoice.lua: Relic rewards initialized` or `…: Got rewards` | wf-log | high |
| RelicCrack | `DVRCAftermath` | wf-log | high |
| RelicInventoryOpen | `ThemedProjectionManager.lua: PopulateInventoryGrid` | wf-log | high |
| InventorySellOpen | `InventoryTest.lua: PopulateGrid` | wf-log | high |
| HostMigration | Game subsystem, contains `HostMigration` | wf-log | medium |

Explicitly NOT relied on (wf-log's authors found them unreliable):
`FrameworkCmd::OpenLevel` for mission starts (missing entirely on
matchmaking joins).

Mission boundary state machine (from wf-log): `MissionInfoSeen` arms
"mission incoming"; next `LevelConnected` consumes it → mission
START. A `LevelConnected` with nothing armed → hub transition =
mission END (or abort/hub hop). RelicRewardScreen implies a fissure
mission is in its reward phase.

### 7.3 Session detection

- Session start: file (re)creation/first write after ≥ 30 min of
  inactivity, confirmed by a `LoggedIn` line. Triggers: account-ID
  resolution (if unconfigured), full fetch, plan, page pop.
- Session end: no writes for 30 min, or the tray app just notes
  staleness — nothing user-visible depends on a precise end.
- Log flushes arrive in bursts; debounce event processing 1–2 s.

### 7.4 Account-ID resolution (self-configuration)

There is NO single "your account id" line in EE.log (wf-log
account.rs is explicit about this). Algorithm:
1. Take local display name from the last `LoggedIn` line.
2. Collect candidate IDs = lowercase hex runs of exactly 24 chars
   (uppercase `mm=` matchmaking GUIDs excluded), ranked:
   (a) on lines that also contain the local display name;
   (b) on `VoidProjections:` lines containing `to host!` or
       `Client got reward info from`;
   (c) on `Updated playerInfo for player` lines with `Host=true`.
   (Squadmates' IDs appear in the same file — ranking alone is not
   proof.)
3. Confirm: fetch the public profile for candidates in rank order;
   the right one's `DisplayName` equals the logged-in name. Cache
   the confirmed ID in config permanently.
4. If no candidate confirms, the app runs without profile data
   (targets-only planning) and the tray status says so. Never guess.

### 7.5 Known unknowns (validate on Seth's log, §12)

- Whether the reward screen's offered/picked ITEM NAMES appear in
  log lines near `RelicRewardScreen` (would give exact part-gain
  facts). wf-log does not extract them; WFInfo-lineage tools OCR the
  screen instead — so plan A is they are NOT in the log, and part
  gains are inferred only at coarse grain ("a fissure reward was
  claimed in a mission whose relic could drop X"). If Seth's log
  proves otherwise, upgrade the inference.
- Mission-end reward identification for non-fissure missions
  (rotation rewards, relic drops) — same status: unconfirmed.
- Trades: one Linux overlay claims trade history is parseable from
  EE.log; not needed for v1 planning, note as a possible v2 source.
- Mission-end standing/affinity deltas: does EE.log carry the
  end-of-mission summary numbers (per-syndicate standing gained,
  focus gained)? None of the surveyed parsers extract them, so
  assume NOT until the fixture says otherwise. Fixture task: inspect
  lines around mission-end transitions for standing values. This
  decides whether the daily bars in §8 are measured or estimated.

## 8. Derived state (the "bookkeeping" — Seth does none)

`state.json` in the app data dir, all derived, all rebuildable:

- `account_id`, `display_name` (from §7.4).
- `profile_snapshot`: last XPInfo pull (timestamped).
- `targets_snapshot`: last Xuerian pull.
- `relic_stock`: counts by relic, when known. Acquisition is the
  one genuinely hard input: no API or log source lists holdings.
  Precedent (warframe-lite): when `RelicInventoryOpen` fires, OCR
  the relic grid while the user scrolls it and persist counts — a
  screen-content read, inside the clean-data rules (no memory, no
  nonce). SPIKE-GATED (§13 #5): Wayland/portal screenshot capture
  from the sandbox + OCR accuracy on the grid. Until/unless the
  spike passes, stock = unknown and §10 ranks cracks by coverage
  alone, labeling each "if you have it".
- `parts_held`: counts by prime part — the cold-start gap. No
  source lists held parts (profile = built equipment only; Xuerian
  `has` is empty). Bootstrap = the ONE-TIME DUMP (§8.1); afterwards
  maintained incrementally from reward events + profile pulls.
- `session_log`: per-session record — missions seen (start/end,
  node/type if recoverable from MissionInfo JSON), relic cracks,
  reward screens. Rolling 90 days.
- `weekly_ledger`: which weekly items the log proves done this week
  (sortie/archon/archimedea completions are inferred conservatively
  — only when the mission identity is known; otherwise the page asks
  via checkbox rather than asserting).
- `daily_ledger`: the two daily bars Seth wants tracked (reset
  00:00 UTC): **syndicate standing** vs cap 16,000 + 500 × MR
  (MR30 → 31,000; MR comes from the profile) and **focus** vs cap
  250,000 + 5,000 × MR (MR30 → 400,000). Whether these are MEASURED
  from log mission-end deltas or ESTIMATED depends on the §7.5
  fixture check; either way the page labels them as what they are,
  never as exact.
- `moonflesh_estimate`: Taubound alerts completed (counted from
  alert-mission completions in world-state-matched windows) × payout
  — an estimate, labeled as such on the page. Kill drops are NOT
  counted (unobservable, negligible).

### 8.1 One-time bootstrap dump (Seth, 2026-10-15)

Ten years of inventory cannot be typed in, and must not be: the
dump is a guided scroll, not data entry. Flow:

1. Tray status reads "Inventory not scanned yet" until done.
2. Seth opens Inventory → Prime Parts in game. The watcher sees
   `InventorySellOpen` (`InventoryTest.lua: PopulateGrid`, §7.2)
   and enters capture mode.
3. He scrolls slowly to the bottom. Each grid page is captured
   (screenshot portal) and OCR'd; names are matched against the
   known part universe (`wishlist.json` roster + items dataset)
   with fuzzy matching — OCR can never invent an item that doesn't
   exist; counts read from the stack badges.
4. Same pass on the Relics screen (`RelicInventoryOpen`) fills
   `relic_stock`.
5. The checklist page shows capture progress + a short
   "uncertain reads" list. Error asymmetry rule: an uncertain cell
   defaults to NOT held (a redundant crack suggestion is cheap;
   wrongly pruning a needed part is not). Seth may glance-fix the
   uncertain list; zero fixes is an acceptable outcome.
6. Baseline saved. Expected total effort: a few minutes of
   scrolling, once. Cross-check: parts belonging to equipment
   already in XPInfo are marked consumed automatically.

Pruning: a want is "done" when its parent equipment appears in
XPInfo (§10) or when a reward event + later profile pull makes it
certain. Uncertain dones are shown as "likely done — confirm?"
never silently dropped. Nothing is ever written back to Xuerian in
v1.

## 9. Configuration (`config.toml`, XDG config dir)

```toml
ee_log_path = ""            # empty = auto-detect
pop_on_session_start = true
xuerian_token = ""          # bookmark token; treat as secret
# account_id is learned (§7.4) and cached in state, not config
```

First run with no token: targets come from the bundled seed list
(49 wants, 2026-10-15) and the tray status notes "Xuerian token not
set — targets may go stale". The token is the only thing Seth ever
pastes, once, into a text file on his own machine.

## 10. Planner algorithm

Session model (Seth, 2026-10-15): NO explicit time budget. The
plan is an ordered slate of self-contained CHUNKS — one chunk = one
activity block (an alert run, a crack set on one fissure, a sortie
chain, two Netracells). Seth works through chunks in any order or
his own, and hits Regenerate when he wants fresh ones; completed
chunks fall out via the log/ledger state, new ones slide in. The
planner emits a useful slate (roughly 8–12 chunks across tiers),
not an exhaustive schedule and never a minute total.

Input: world state W; profile XPInfo P; targets T (wants, pruned);
relic tables R; derived state S; now.

Output: ordered list of chunks {title, minutes_label, why, kind,
expiry?}. Pure function — same inputs, same plan (testable).

Ordering (strict tiers; within a tier, earliest expiry first, then
highest value density). Weekly-cycle content never appears here at
any tier — it is board-only (§4.2 section 4).

1. **Expiring event value.** If a Taubound Operation Alert is live:
   item = run it now (est. by mission type, ~10–15 min). Payout line:
   50 Moonflesh (75 Steel Path variant). Repeat for each distinct
   live alert (typically a 1–3 chunk block). Event ends 2026-11-17 — after that this tier is
   driven by whatever `events`/`alerts` carry instead; code the tier
   generically (any alert with a counted-item event resource), with
   Taubound as the configured instance.
2. **Target-matched fissures.** Want prioritization (Seth,
   2026-10-15): he's a completionist — every want matters eventually,
   so ordering is by class and closeness, not worth: **Warframes
   before weapons**, then fewest parts remaining, and a `pins` list
   in config overrides everything ("working on these two items" —
   a pin is how he says so without a UI). For each want w: relics
   containing w (R.relics rewards), filtered to
   unvaulted-or-owned-unknown, era E. For each live fissure of era E
   in W: candidate crack. Score = (number of distinct wants the
   relic can drop) × (best rarity chance) ÷ (mission duration
   estimate), multiplied up when the relic is known IN STOCK (§8
   relic stock) — cracking beats farming, always. Steel Path
   fissures get a small bonus (better traces/essence) but are never
   required. Greedy pick, removing covered wants optimistically but
   keeping alternates visible in the "why" ("or Neo N12, same
   part"). Expected-run math shown honestly: at Intact, common
   25.33% / uncommon 11% / rare 2% per crack (Radiant: 40/20/10) —
   the page may show "≈9 cracks at Intact, ≈3 as Radiant 4-stack"
   for rare parts; never promise a drop.
3. **Relic acquisition** (only if tier 2 is empty or thin): for the
   highest-value want relics, best farm nodes from R.sources —
   prefer fast missions (Capture/Exterminate/Disruption rotations)
   with the relic in early rotations; show rotation + chance.
4. **Cap-and-bar progress** (Seth, 2026-10-15: "anything that
   requires you to fill up a bar"). WEEKLY-cycle content is NOT in
   this tier — per Seth's ruling it lives on the This Week board
   (§4.2), never in the chunk queue: Archon Hunt, Deep + Temporal
   Archimedea, Netracells (shared Search Pulse pool — see
   WEEKLIES.md), Circuit track, Kahl, Clem, Maroo, Iron Wake
   riven, vendor rotations, Nightwave weekly acts. What remains
   here, as chunks, is the daily/limited set:
   - Nightwave daily act if quick; weekly acts surface on the
     board and may be called out when they complete incidentally
     during tiers 1–3.
   - Daily bars: sortie (one chunk, ~20–30 min), Steel Path incursions (5/day),
     focus cap (daily: 250k + 5k per mastery rank — Seth's ~350k
     figure matches MR20; focus state is NOT visible in any source,
     so the page shows the cap and the best focus-farm route, never
     a fake progress number), syndicate standing caps (same
     honesty rule), Simaris daily target.
   Completion state comes from S.weekly_ledger where the log proves
   it, else from the page's checkboxes — never asserted from
   nothing.
5. **Filler chunks** when the slate runs thin: best remaining
   fissure (any era, highest ducat/plat density per R + optional
   market data later), or an arbitration staple. Filler
   is labeled filler. (Never syndicate missions — §2 exclusion.)

Duration model: per-mission-type base estimates (Capture 3m,
Exterminate 5m, Survival 5m/rotation-unit, Defense 5 waves ≈ 7m,
Disruption round ≈ 4m, Hive ≈ 10m, alert missions by type) +
2m overhead per mission (loading/orbiter). Labels only — there is
no budget for them to feed. Seth's full star chart is complete,
Steel Path unlocked — no gating checks needed.

Pruning vs profile P: want w belongs to equipment e (via roster);
if e ∈ P (any XP), w is done. Whole-equipment wants (e.g. "Khora
Prime Blueprint" is a PART — the roster's parts list defines
parentage; a part want is done iff its parent equipment ∈ P, or the
part itself was certainly gained per §8 inference rules).

## 11. Flatpak packaging

- Runtime: `org.freedesktop.Platform` 24.08 (no Qt — ksni speaks
  D-Bus directly). SDK: freedesktop + rust-stable extension.
- Build: cargo vendored (offline build per Flatpak norms);
  `flatpak-builder` manifest in the repo.
- finish-args (each justified):
  - `--share=network` — the four HTTP sources.
  - `--socket=session-bus` — actually prefer the narrow route:
    ksni needs to own `org.kde.StatusNotifierItem-*` and talk to the
    watcher; start with `--talk-name=org.kde.StatusNotifierWatcher`
    + `--talk-name=org.freedesktop.Notifications`… if the proxy
    blocks item registration, fall back and document why. THIS IS
    SPIKE #1.
  - `--filesystem=<EE.log path>:ro` — the one data grant. If the
    recreation problem (§7) bites, widen to the `Warframe` dir :ro
    and document the change.
  - `--talk-name=org.freedesktop.portal.Desktop` — OpenURI portal
    for the page pop.
  - XDG config/data via the app's own sandboxed dirs (default).
- Autostart: portal-based (Background portal request) or a
  user-copied `.desktop` in `~/.config/autostart` — decide in spike;
  prefer the portal, fall back to instructions.

## 12. Validation plan

- SETH DELIVERABLE (one thing): a copy of a real EE.log after a
  session that includes at least one fissure mission. Drop it in
  `~/workspace/warframe-planner/fixtures/` (from his side) — it
  becomes the parser's golden fixture, scrubbed of nothing (it's a
  game log) but never committed to a public repo.
- Parser acceptance: on the fixture, the classifier must find
  LoggedIn, ≥1 mission start/end pair, ≥1 RelicRewardScreen; the
  account-ID resolver must produce a candidate that the profile API
  confirms (DisplayName match) — run once, by hand, before the
  resolver is trusted in-app.
- Planner acceptance: golden tests from recorded world-state JSON
  (capture several `/pc/` responses at different times of day/week)
  + the real targets — eyeball three plans with Seth before calling
  the ordering "right".
- Page acceptance: renders from a fixture plan with no network;
  checkboxes persist across regeneration.

## 13. Spikes, in order (before real code)

1. Tray icon visible + menu functional from inside the sandbox on
   Seth's Plasma (bus permission shape). Kill criterion: if
   StatusNotifier can't work sandboxed without `--socket=session-bus`
   wholesale, decide whether that's acceptable or the app ships as a
   plain (non-Flatpak) binary instead.
2. EE.log watch semantics: truncate vs recreate at game launch
   (observable from Seth's fixture + one live launch).
3. Account-ID resolution on the fixture (§7.4 end-to-end).
4. OpenURI portal pop from the sandbox.
5. Screen-capture OCR (see §8/§8.1): portal screenshot capture
   from the sandbox + OCR of (a) the relic grid on
   `RelicInventoryOpen`, (b) the Prime Parts grid on
   `InventorySellOpen` for the bootstrap dump. Kill criterion:
   portal capture or OCR accuracy fails → ship without stock and
   without the dump; planner degrades to coverage ranking +
   profile-only pruning (the pre-dump cold-start state).

## 14. Build phases (after brief approval — not before)

1. Skeleton: tray + config + watcher + event classifier on fixture.
2. Fetchers + caches; profile/Xuerian/world-state/drops live.
3. Planner v1 (tiers 1–3 + pruning) + page render + pop.
4. Weekly tier + derived-state ledger + regeneration triggers.
5. Flatpak manifest, autostart, install on Seth's machine, live
   session test.

## 15. Open questions for Seth

1. ~~Name~~ RESOLVED (Seth, 2026-10-15): **Fracker** (fracking +
   tracker). Repo `fracker`; app id derives from it.
2. ~~Budget default~~ RESOLVED (Seth, 2026-10-15): no explicit
   time bounding at all — chunked slate + tray Regenerate is the
   model. Duration labels stay as information only.
3. The "likely done — confirm?" pruning prompts (§8): on the page
   only, or also allowed as a tray badge? (Brief assumes page only —
   zero interruptions.)
4. Repo: private GitHub under his account (like grafmote), built
   locally — CI optional later.

## 16. Long-term projects: bespoke material tracker (Seth, 2026-10-15)

Scope CONFIRMED by Seth: track ONLY the grind-gate materials — the
resources DE makes exclusive to a (usually new) frame and its
signature weapons, precisely so ten years of stockpiled commodities
can't insta-build new releases. Bulk resources (ferrite, circuits,
polymer…) are NEVER tracked; a veteran account is never short of
them. Explicit non-goal, Seth's example: base materials like
**Plastids** are out — if a build cost includes them, the tracker
silently ignores that line. Example: Dagath + Dorrclave ← Vainthorn (Abyssal Zone, Ceres;
Seth has already done this farm — it's the shape, not a live need).

- **Requirements are machine-readable.** The WFCD items dataset
  (already our names/images source) carries full build trees via
  uniqueName joins: frame → component blueprints → resource costs,
  counts included.
- **Farm locations are machine-readable too.** Bespoke resource
  items carry their source in the item description — verified live:
  Vainthorn's entry reads "Location: Abyssal Zone on Ceres (requires
  Abyssal Beacons to access)". No wiki scraping.
- **The bespoke set is derivable, then curated once**: resources
  whose sinks are a single frame family (frame + signature weapons).
  Bounded — on the order of a couple dozen resources, not hundreds.
- **Holdings are the one soft spot** (resources appear in NO clean
  source — same wall as parts/relics). Two options, decision open:
  (a) holdings unknown — the tracker shows cost + farm per project,
  completion inferred when the frame/weapon appears in the public
  profile (built = done); (b) seed counts for just the bespoke list
  during the one-time guided inventory capture (the scroll already
  happens for parts/relics; the bespoke list is short enough for
  the Resources tab pass to cover it), then decrement on builds.
  Brief's lean: (b) — the capture exists anyway, and "68 Vainthorn
  to go" is the difference between a tracker and a poster.
- **Presentation**: a "Long-term projects" board on the page,
  alongside the week board — NOT in the chunk queue, except that a
  project's farm may appear as a labeled FILLER chunk when the
  slate runs thin. Each project: frame, missing parts (from §10
  targets/pruning), missing bespoke resources with counts, farm
  node/activity, and a done state from the profile.

## 17. Companion-app API — investigated, closed (2026-10-15)

Seth's correction, verified: the Warframe **companion app
authenticates standalone on the phone** (account credential login,
no PC client involved) and holds its own session. DE enforces a
single session per account — logging into the app can kick a live
game session (forum-documented behavior, with an in-app warning
that in-game progress may not save).

What authorizes the inventory endpoint is the same regardless of
client: `inventory.php?accountId=<id>&nonce=<session nonce>` on
`api.warframe.com` / `mobile.warframe.com` (reconstructed in
warframe-lite's `docs/research/mobile-inventory-api-coverage.md`).

Both piggyback paths stay closed under §2:
1. **Credential login** — our code would hold/transmit Seth's
   account password and impersonate the first-party app to mint a
   session. (warframe-lite's ADR-0001 declined this exact path:
   "we will not hold or transmit account credentials.") Bonus
   problem: minting a session can terminate his running game's.
2. **Nonce relay** — reading the session nonce out of the running
   client's memory (the warframe-api-helper / WFHelper approach;
   warframe-lite ADR-0013). Memory access — already ruled out.

Conclusion: no piggyback. The one-time OCR inventory capture
remains the holdings source; nothing about the planner depends on
this API.

Follow-up Seth raised (2026-10-15): does the app PERSIST the
inventory on-device somewhere readable? Investigated: the app
(package `com.digitalextremes.warframenexus`) does keep a large
external footprint under `Android/data/com.digitalextremes.
warframenexus/` — but the evidence says that's the static content
cache (Public Export item databases + engine assets, i.e. the
same catalog data WFCD already gives us), not account data.
Account/session state on Android lives in the app-private
internal sandbox, which a stock non-rooted phone exposes to no
one but the app; reaching it means root or extraction tricks —
the shenanigans class, out. Cannot prove the negative from
outside, so one empirical check remains, and it's Seth-side and
rule-clean (his phone, his files): install/open the app, log in,
plug in USB, browse `Android/data/com.digitalextremes.
warframenexus/` for anything that looks like account inventory
JSON. If it exists, this section reopens; until then, closed.

## 18. Collection scope: primes AND base frames (Seth, 2026-10-15)

The collect list covers **both variants of every frame** — base
frames have their own grinds and mastery is mastery (Seth is
collecting everything). The list itself is derived: full roster
(WFCD items dataset) minus profile XPInfo, frames before weapons,
with Seth's pins on top (§10 ordering is unchanged in spirit).

Non-prime acquisition comes in three shapes, and the planner
models each frame with one acquisition record:
- **A — part drops**: node/boss/rotation drops for each component
  blueprint (drop-data mission rewards + item component drops).
  Chunk: "Farm: <frame> — <node> (<type>), drops <missing part>,
  ≈runs at <chance>".
- **B — bespoke currency → vendor**: parts are BOUGHT with an
  exclusive resource. This is §16's tracker wearing a frame's
  clothes — the frame's "missing parts" ARE a resource shortfall.
  Chunk: "Farm: <resource> — <activity> (≈<have>/<need>)".
- **C — fixed source**: quest reward, dojo lab, syndicate vendor,
  market blueprint. Bounded, curated once per roster; the page
  shows the source, no farm chunk (nothing to grind repeatedly).

Worked example — the frame Seth was reaching for: **Narin**, the
ice frame (Update 44, Sep 2026). Type B: Entropic Kuva drops from
Entropic Eximus in Zariman missions; 790 total buys her full set
(400 main + 3×130) from Cephalon Melica at Yuvan's Peak (needs
Angels of the Zariman + Whispers in the Walls done — Seth's star
chart/quest state makes this a non-issue, but the record carries
prereqs so the planner never suggests a locked farm). There's
also an RNG part-drop route; the record notes it, the currency
route is the plannable one.

Session slate consequence: every slate can offer both a crack
chunk (primes, §10 tier 2) and a farm chunk (next base frame on
the collect list) — Seth picks per session. Farm chunks rank by:
pinned > fewest-missing-parts > newest release (new frames are
the usual live goal).

## 16. Decision log

- 2026-10-15 (voice call): **Storage split for relic data.** Owned relic
  stock (counts by relic + refinement) lives LOCALLY in the app's state
  (§8) — it changes constantly (every crack, every farm), and the app must
  write it from EE.log events in real time without depending on a third
  party's uptime or risking a stale read-modify-write clobbering Seth's
  manual edits. Xuerian remains the source for (a) targets/wants (read via
  `/api/load`, §6) and (b) relic reference data — `relictables.json`
  rewards/chances AND `sources` (mission-table → node list with planet,
  mode, faction, levels; verified in the cached file) — cached locally,
  refreshed weekly, so Xuerian's data updates flow in automatically.
  Optional future extra, off by default: one-way publish of owned stock
  into Xuerian's `has`. Never the reverse as source of truth.
- 2026-10-15 (voice call): **Thin-layer principle (Seth).** Wherever a
  maintained remote thing exists, depend on it instead of owning the
  knowledge: Warframe changes fast and often, and every dataset or parser
  we maintain ourselves is a thing that silently rots. Concretely:
  EE.log parsing → the `wf-log` crate from warframe-lite if consumable
  (check crates.io / git dependency at build time; vendor its pattern
  catalog with attribution only if it isn't), world state → warframestat
  API, item names/images → WFCD datasets/CDN, drop data + targets →
  Xuerian, profile → DE's public endpoint. Our code owns only: the
  session state machine glue, the planner's ordering choices, Seth's
  personal state (§8), and the page render. The owned-stock-local
  decision above is the deliberate exception — personal state isn't game
  knowledge and no maintained remote holds it for him.

## 17. Goal cards + action derivation (2026-10-15, voice call)

The display unit is the GOAL (a frame/weapon), never a raw part name.

Card anatomy:
- Header: name + WFCD image, VAULTED badge when applicable, progress
  (parts n/N).
- Part rows: state per part — have / need / unknown (from §8 sources;
  unknown until profile + OCR bootstrap resolve it).
- For each needed part: rarity, its drop relics (compact list), owned
  count per relic from local stock ("?" until the OCR bootstrap),
  best-odds line (e.g. "rare — 2% Intact / 10% Radiant").

The card's ACTION line is verb-first and derived, in this order:
1. Owns a relic containing a needed part AND a fissure for its era —
   or an Omnia fissure (any era) — is live → "Crack <relic> at <node>
   (<era> <mission type>, live N more min)". Earliest-expiring fissure
   wins; faster mission type breaks ties.
2. Owns the relic, no matching fissure live → no queue entry; card
   shows next fissure ETA for that era.
3. Owns none, relic unvaulted → farm action from `sources` (best node /
   rotation for that relic).
4. Owns none, relic vaulted → no action exists; card says so plainly
   ("vaulted — crack-only, none held"). The planner never suggests
   trading/market purchases.

The session queue ranks ACTIONS across all cards: live-fissure cracks
first (expiring value), then actions covering multiple wants (a relic
whose other rewards are also wanted), frames before weapons (Seth's
rule), goals with fewer parts remaining, rare-part cracks ahead of
common-part ones while their fissure is up (bottleneck first).

Worked example (verified live 2026-10-15 ~17:30 ET): Ivara Prime —
vaulted; Seth's only tracked want is the Chassis Blueprint, the RARE
drop (2% Intact / 10% Radiant) in Lith I1, Meso I3, Neo I1/I2/I3 — all
vaulted, so farming is impossible; the card's action is a crack if he
holds any of the five (Neo fissures, incl. Hepit (Void) Capture, and
Omnia fissures were live at check time), else the goal is parked as
crack-only/none-held.

Addendum to §17 (same call): **matching is scored, never blind.** A
part's rarity is a property of the (part, relic) pair, not the part —
verified in `relictables.json`: 88 of 608 catalog parts sit at
different Intact chances in different relics, including Seth's own
wants (Khora Prime Blueprint: 11% in Neo C5 vs 2% in Lith K9/Meso K4;
Akarius Prime Receiver: 11% in Neo G10 vs 2% in Lith A6/A7/A10; Baza
Prime Blueprint: 11% in Meso I3/Axi S13 vs 2% in Lith B7). The engine
therefore scores every candidate crack as: chance of the target part
in THAT relic at the chosen refinement × live-fissure availability ×
owned stock, plus a coverage bonus for the relic's other rewards
that are also wanted, and shows expected cracks (≈1/p) rather than a
bare match. Refinement advice follows the same math: Radiant
multiplies rares (2%→10%) but *lowers* commons (25.33%→16.67%), so
the action line names the refinement per target ("run it Radiant" /
"run it Intact"), and stock is tracked per refinement where known.

Addendum to §17 (same call): **Void Trace investment is part of every
recommendation (Seth's North Star).** The action line is a complete
instruction, e.g. "Grab 3× Neo C5, refine to Flawless, run Hepit
(Void) — Capture." Three decisions are computed, not defaulted:
- *Refinement*: costs 0/25/50/100 traces (Intact/Exceptional/Flawless/
  Radiant); chances per rarity come from `relictables.json` (rare
  2/4/6/10%, uncommon 11/13/17/20%, common 25.33/23.33/20/16.67%).
  Rule of thumb the engine formalizes: scarce relic stock → refine
  harder (maximize success per copy); plentiful stock → bias DOWN a
  tier (Seth's example: 25 copies held → Flawless, not Radiant —
  volume substitutes for per-crack odds, and unspent traces stay
  banked); the crossover is computed from stock vs expected cracks
  at each tier, not a fixed threshold; commons are run Intact (refining
  *hurts* them); rares justify Flawless/Radiant, with the expected
  trace spend per success shown (≈625 traces Exceptional, ≈850
  Flawless, ≈1000 Radiant per expected rare, before trace refunds
  from cracking itself).
- *Quantity*: batch = min(owned stock, expected cracks at the chosen
  refinement, one slate chunk) — enough to be a meaningful run,
  never "all of them."
- *Trace balance is unobservable* (not in EE.log or the profile), so
  the line states the recommendation's trace cost ("3× Radiant =
  300 traces") and lets Seth veto against his actual balance; the
  planner never assumes a balance.
North Star, verbatim from Seth: "we log in and get told immediately:
grab relic X, upgrade it to [quality], play this node."

Addendum to §17 (same call): **Relic sourcing + co-yield bias (Seth:
"Warframe is all about efficient farming").** When a needed relic is
out of stock, the action becomes a farm recommendation built from
`relictables.json`: each unvaulted relic carries `sources` = {drop
table → per-rotation chances}, and the top-level `sources` maps each
table to its nodes (planet, node, mode, levels). Only 34 of ~800
relics are currently unvaulted/dropping at all. Farm scoring:
- Per node: expected wanted-relics per hour = Σ over ALL wanted
  relics dropping at that node (not just the one that triggered the
  farm) of rotation chance × rotations/hour for that mission type.
  A node yielding three wanted relics beats the single best node for
  one — co-yield is a heavy bias, per Seth.
- Rotation matters (chances differ per rotation, e.g. 14.29% vs
  2.44% across tables for the same relic): prefer nodes where the
  wanted relics sit in early/frequent rotations; endless missions
  that cycle the right rotation score up.
- Reality check on Seth's current 49 wants: every one sits in a
  vaulted relic (verified against the cache) — so today the sourcing
  engine returns "nothing to farm" for all of them (crack-or-park,
  §17 rule 4). Sourcing activates for current-rotation goals: new
  primes as they're added to targets, and general relic farming.
- 2026-10-15 (voice call): **Prime Resurgence is a sourcing channel.**
  Seth flagged that vaulted relics also return via the Resurgence
  rotation (Varzia, Maroo's Bazaar) — buried in-game, worth surfacing.
  It IS in the world state: `vaultTrader` (verified live) carries the
  rotation window, inventory, and forward schedule. Rule: when a
  vaulted goal's gear is in the live rotation, its relics are
  purchasable for Aya — the goal un-parks and gains a time-boxed
  action ("Rotation ends Oct 29 — Varzia has Ivara's relics, 1 Aya
  each"); rotation matches rank with the expiring-value tier in
  §10/§17. LIVE FINDING at design time: the Oct 1–29 rotation is
  Protea + **Ivara** Prime, and its weapons include **Baza** and
  **Aksomati** — three of Seth's wanted lines (Ivara Chassis; Baza
  Blueprint + Stock; Aksomati Barrel + Link + Receiver) are covered
  by this rotation's relics.
- 2026-10-15 (voice call): **Nightwave handling.** Seth wants it
  tracked weekly; EE.log trackability is doubtful and stays an open
  validation item (no Nightwave events in the wf-log catalog). What
  world state gives (verified live): season + active challenges with
  titles and standing values (Intermission season 18 at check time;
  dailies 1000, weeklies 4500). Design: Nightwave lives on the weekly
  board (not the blocking queue, per Seth's earlier ruling); acts
  whose completion is activity-shaped (mission counts, sortie done)
  get conservatively auto-checked from log events; the rest are
  manual ticks on the page. His standing/rank progress is NOT
  observable from any clean source — the page shows acts, never a
  fabricated standing number.
  Nightwave wrinkle (Seth, same call): the "Recovered" mechanic —
  finishing the current week's acts unlocks backfilled acts missed in
  prior weeks, so the effective act list can exceed the world-state
  list and the recovery order/logic is NOT exposed anywhere we can
  read. Implication: the board presents the world-state acts as a
  floor, never claims a complete weekly picture, and recovery-act
  modeling is deferred until it can be observed (Seth: "we'll get
  there when we get there").

Addendum to §17 (same call): **Mission selection — one answer, not a
menu (Seth's North Star: "turn off my brain and just do the farm").**
Given the slate's target relics, fissures of the needed eras (Omnia
counts for every era) are ranked by a deterministic cascade:
1. *Expiry*: a matching fissure expiring within ~15 min jumps first
   (if comfortably runnable), use-it-or-lose-it.
2. *Minutes per crack for the planned batch* — the dominant factor.
   Sprint missions (Capture ≈ 2–3 min, Exterminate, Sabotage) win
   for small batches; ENDLESS missions (Survival/Defense/Interception,
   ~5 min per crack unit) win for batches ≥ ~3 because there's no
   orbiter/load/requeue overhead between cracks — the instruction
   becomes "stay for N rotations." Computed per batch, not per
   mission type in the abstract.
3. *Goal fit*: era of the highest-priority (bottleneck) relic first
   when several eras are targeted.
4. *Steel Path fissures*: tiebreak bonus (more traces/essence) for
   sprint missions; endless defaults to normal path (brain-off
   beats scaling enemies). Config toggle exists.
5. *Void Storms* (Railjack): last resort — slowest per crack.
Presentation rule: the page's top line is exactly ONE mission with
its relic + quantity + refinement ("Run Hepit (Void), Neo Capture —
bring 4× Neo C5, Radiant"). Alternates render collapsed beneath,
never as competing equals. Re-picks happen at mission end (the
regeneration trigger), never mid-mission — an endless run in
progress is never yanked.

Addendum to §17 (same call): **Squad optimization — the relic share.**
The people-side lever is the 4-player same-relic share: everyone
brings the same relic at the same refinement, every crack rolls four
rewards, and each player picks the best — so the target part's
per-crack chance becomes 1−(1−p)⁴ (Radiant rare: 10% → ~34%,
expected cracks ~10 → ~2.9; Intact rare: 2% → ~7.8%, ~50 → ~12.9).
The planner computes both numbers (solo vs 4-share) and puts the
share in the action line when it wins after LFG formation time
(~5–10 min): typically rare parts at Flawless/Radiant, and scarce
stock generally (sharing stretches expensive relics; bonus traces
when squadmates pick your reward are a side benefit). It does NOT
recommend shares for cheap volume runs (commons at Intact, deep
stock) — pubs/solo start instantly and the math doesn't repay the
coordination.
Seth's gate on top of the math (2026-10-15): LFG is choosy in
practice — recruiting only actually fills for the CURRENT prime
release or one back. So the share verdict additionally requires
the relic's gear to be within the last two prime releases
(derivable from the roster/release data); for anything older, the
action line stays solo/pubs no matter how good the share math is,
because a group that never forms saves zero cracks. The action line renders the LFG-ready form: "Best as
a radshare: LF Neo C5 rad." The tool never does the recruiting —
it just does the math and writes the line.
- 2026-10-15 (voice call): **Governing principle (Seth, verbatim
  intent).** The problem being solved is twofold: on login he has to
  (1) remember what he needs to do, and (2) decide how best to do
  it. The tool owns BOTH. Success = "I log in and just get told: go
  use this relic — and I get right to the fun part, which is
  actually playing the game." Every feature is judged against this:
  anything that hands a decision back to him (menus of equals,
  unweighted options, analysis he must interpret) is a defect, no
  matter how good the analysis is. Casual relic-cracking to see
  what others get is explicitly NOT the target-farm use case. The
  radshare verdict survives only as a pre-decided line inside the
  instruction ("best as a radshare"), never as a choice presented
  to him.

Addendum (2026-10-15, voice call): **Page sections, settled list**
(Seth enumerated, Graf completed):
1. **Do now** (top band): the ONE current instruction, plus what's
   expiring — live alerts/Taubound, a slate fissure about to die,
   closing windows (e.g. "Varzia: Ivara leaves Oct 29"). Countdowns,
   not prose.
2. **The hunt**: relic goal cards + the current slate (the main
   event).
3. **Farms**: non-prime frame farms and bespoke bottleneck
   materials (the Dagath/Veinthorn class) when targeted.
4. **Nightwave**: the act board (§16 handling: world-state acts,
   conservative auto-checks, manual ticks, no standing total,
   Recovered-backfill caveat).
5. **This week**: Clem, Maroo's Ayatan hunt, Archon Hunt, Circuit,
   Kahl, Netracells/Deep Archimedea, Iron Wake, Baro countdown.
   Shown-not-queued, per Seth's earlier ruling.
6. **Today's bars**: Focus cap, per-syndicate standing caps
   (estimates labeled as estimates until the log proves otherwise),
   Sortie done/not — the daily-reset gauges (Seth confirms these
   reset daily "for the most part").
Deliberately NOT sections: trading/market (standing non-goal),
an inventory browser (the tool decides; he doesn't browse), and
loadout/build advice (scope creep, hands decisions back).

Addendum (2026-10-15, voice call): **Data model — activities are
first-class; obligations are predicates over them (Seth's callout).**
Nightwave acts can demand a Rescue mission or a Duviri run; the
same run can crack a wanted relic, satisfy an act, and fill standing
at once. Modeling that overlap is the core design problem, and the
shape is: (1) every open obligation — a wanted part, a Nightwave
act, a weekly, a daily cap — is expressed as a small predicate over
activity facets (mission type, era, node/planet, region incl.
Duviri, Steel Path, endless, count). (2) Every candidate run the
planner considers carries the facet set it would satisfy. (3) The
planner's score for a run is the weighted sum of EVERY obligation
it advances — expiry-weighted — which is exactly the relic-coverage
scoring of §17 generalized from "which relic covers most wants" to
"which run covers most obligations across all systems." One engine,
not per-section heuristics. Care points: facet vocabulary stays
small and closed; act→predicate mapping is a small maintained table
keyed by challenge id (thin-layer exception — nobody publishes it),
and an unmapped act renders display-only and is NEVER scored or
auto-checked; progress within an act is unobservable, so predicates
match on activity identity, not counts claimed.

## 19. Visual design / art direction (2026-10-15, voice call)

Goal, Seth's words: nice, not "a spreadsheet in space." His hard
requirements: dark mode (it lives on a second screen mid-game — no
flash-banging), a real background, considered fonts, generous
padding, and the WFCD item icons used where they identify things.
NOT an image gallery — restraint is the brief.

Why pages look like spreadsheets, and the counter for each:
uniform rows → one hero + cards at varied scale; visible gridlines
→ separation by space and panel tone only; everything the same
weight → strict type/luminance hierarchy; text-only → icons as
identification; crammed edges → whitespace as a layout material.

- **Palette (low-luminance register throughout):** page bg a deep
  blue-black gradient (#0a0d12 → #11151d, faint radial vignette);
  panels #151a23 with 1px hairline borders (#232b38); body ink
  #c9d2de (never pure white); muted #7e8898. Accents are rationed:
  prime gold #bfa05a, rarity bronze/silver/gold for chance chips,
  pale cyan reserved for the ONE current action, amber → red only
  on expiring countdowns. Done items dim to ~45% + strike — no
  bright green checkmarks anywhere.
- **Background:** layered CSS gradients only (no asset): vignette
  + an extremely faint star speckle. Present, quiet, free.
- **Type:** Rajdhani (Google Fonts) for headings, labels, and
  numerals — it's the closest freely available face to the game's
  UI type; body in the system sans stack. Google Fonts <link> with
  full system fallback (offline/file:// still renders cleanly).
  Section heads are 12–13px uppercase, letter-spaced, muted; the
  Do Now instruction is 30px+; nothing else exceeds ~20px.
- **Layout & spacing:** max width ~1080px, centered; section gaps
  28–36px; card padding 20–24px; line-height ≥1.45. The decision
  content (Do Now + slate) fits the first screen; boards below get
  progressively smaller and quieter.
- **Shape language:** cards, never tables. Chamfered corners via
  `clip-path` (an 8–10px corner cut) echo the game's tech frames
  without copying assets. Progress is shapes first: part pips
  (filled = have, hollow = need), 6px bars for the daily caps,
  countdown chips — numerals support the shape, never replace it.
- **Icons (WFCD CDN, §6 source 5):** one portrait per goal card,
  the relic icon on action lines, small icons on weekly chips
  where cheap. Identification only — no per-row thumbnails, no
  decoration. Broken/offline image = the layout holds without it.
- **The refresh-flash rule (load-bearing):** the page meta-refreshes
  every 60s; a default white repaint would flash-bang him once a
  minute. `background` is set on the root element AND
  `color-scheme: dark` is declared, so every repaint — including
  scrollbars and checkboxes — stays dark. This is a requirement,
  not a nicety.
- **Presentation rules already settled stay in force:** exactly
  ONE loud element (the Do Now instruction); alternates collapsed
  via <details>; Clem/Maroo smallest and last (§4.2); no charts,
  no dashboards, nothing that asks him to interpret instead of act.

## 20. Information architecture: content is data (Seth, 2026-10-15)

Seth's structural requirement, stated while pass 1 was already
running: Warframe changes constantly, and the repo must NOT need
its guts pulled apart every three months. New events, new farms,
new mechanics slot in; they do not get welded in.

**The engine knows four things, and only four:**
1. **Activities** — candidate runs, each carrying a facet set
   (§17 data-model addendum: mission type, era, node, planet,
   region, Steel Path, endless, faction…). Activities are built in
   ONE place (an activity factory over world state + static node
   data); nothing downstream touches raw world-state JSON. This is
   the collation seam: facets are what obligations intersect with.
2. **Obligations** — wants, acts, weeklies, caps, event goals —
   each a predicate over facets + a weight + an expiry. (Same
   addendum; this section makes it load-bearing architecture
   rather than a planner trick.)
3. **Scoring** — weighted, expiry-aware coverage of obligations by
   activities (§10/§17 rules are the current scoring policy).
4. **Sections** — the page is a declared layout: ordered slots,
   each with a renderer kind (instruction / cards / board / bars).
   A new section is a layout entry + an existing renderer kind in
   almost every case.

**Everything game-specific is content:** TOML packs in a `content/`
directory, bundled with the binary and overridable from the config
dir. A pack declares: id, validity window, obligations, how
world-state items map to activities/facets, copy, section
placement. The facet vocabulary itself stays small, closed, and
versioned; content referencing an unknown facet degrades to
display-only, never breaks planning.

**Canonical example — Taubound/Moonflesh is a pack, not code:**
~30 lines declaring: match live alerts whose counted item is the
event resource; payout 50 (75 Steel Path); weight high and
expiry-weighted; renders in Do Now; `valid_to = 2026-11-17`. When
the date passes, the pack self-retires — no code removal, no
zombie event in December, nothing to remember to clean up. The
pass-1 planner's "tier 1 with Taubound as the configured instance"
(§10) is the seed of this; the pack is its proper form.

**On "plugins":** a dynamic plugin system (dylib/wasm) is
explicitly NOT the mechanism — single-user app, and DE's actual
change pattern (new event, new frame, new node, new resource) is
data-shaped, which packs cover entirely. A genuinely new BEHAVIOR
(a scoring verb or activity source the engine has never seen) is
the rare code case, and it lands as one new provider module behind
the activity/obligation traits — bounded addition, not
rearchitecture. If that boundary starts getting crossed often, the
design gets revisited with evidence, not before.

**Pass-1 consequence:** the Oracle worker is building against the
pre-refinement brief (hardcoded-ish event tier). That's accepted
for a first cut whose job is to be reacted to. The content-pack
refactor is pass 2's first item and the standing law for all
structure decisions from here: when in doubt, it's content until
proven behavior.
