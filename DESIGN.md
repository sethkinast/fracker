# Warframe session planner — design + verified sources

Settled in main chat 2026-10-15; execution lives in the "Warframe session
planner" side chat. Seth has the full star chart complete — no progression
gating; planning is timers × targets.

## Product shape (Seth's direction, 2026-10-15)

A KDE tray-resident binary (Flatpak) on Seth's Bazzite machine:

- Sandbox grant: read access to EE.log only (fallback if the game recreates
  the file per-launch: read-only grant on the containing `Warframe` folder).
  Path under Proton:
  `steamapps/compatdata/230410/pfx/drive_c/users/steamuser/AppData/Local/Warframe/EE.log`
- Watches EE.log; a new session's log activity = "session started".
- On session start it generates a flat HTML checklist page and opens it in
  the browser (via the OpenURI portal). The page is the UI; the tray is
  presence + verbs only (Open plan / Regenerate / Quit, status line).
  Seth explicitly preferred this over building UI into the tray.
- Mid-session, EE.log mission-end events trigger page regeneration with
  completed items struck through; page uses meta-refresh + localStorage
  checkboxes so manual ticks survive regeneration.
- Pops happen on session start only, never mid-session (no focus theft);
  tray toggle to silence pops.
- Language/stack: Rust + `ksni` (pure-Rust StatusNotifier over
  D-Bus, freedesktop runtime, no Qt) — CONFIRMED by Seth 2026-10-15.
  Spike first: tray icon visible from inside the Flatpak sandbox
  (session-bus proxy filtering is the known fiddle point).

## Hard constraints (Seth, non-negotiable)

Clean data only. NO process-memory scanning, NO session-nonce extraction,
NO `api.warframe.com/api/inventory.php`, NO Overwolf/AlecaFrame, NO
passwords. NO manual bookkeeping burden on Seth — state derives from the
sources below.

## Verified data sources (all checked live 2026-10-15)

1. **World state** — `https://api.warframestat.us/pc/` (200, ~186 KB).
   Sections: fissures, alerts, events, sortie, archonHunt, nightwave,
   archimedeas, steelPath, voidTrader, arbitration, invasions,
   syndicateMissions, open-world cycles. Taubound alerts appear in
   `/alerts` with the event resource as countedItem.
2. **Public profile** — `https://api.warframe.com/cdn/getProfileViewingData.php?playerId=<accountId>`
   (main chat's 404s were the wrong host/path: content.warframe.com/dynamic/).
   Account ID = 24-hex Mongo ObjectId, NOT the username. It appears in
   EE.log only situationally (no single authoritative line; squadmates'
   IDs appear too), so resolution = ranked candidates from the log
   confirmed against this API's DisplayName — full algorithm in
   BRIEF.md §7.4. Zero work for Seth either way. Bogus ID → HTTP 409
   "Could not find requested account" (verified). Response shape (per
   WFCD/profile-parser, via warframe-farming-planner source):
   `Results[0].LoadOutInventory.XPInfo[] = {ItemType: uniqueName, XP: int}`
   — every item ever affinity'd = built/owned at EQUIPMENT level.
   Component parts are NOT in the profile (Seth's framing: "items yes,
   parts no"). uniqueNames resolve to display names via the warframestat
   items dataset.
3. **Xuerian targets** — profiles are server-side; the page JS calls
   `POST https://wf.xuerian.net/api/load` with form field
   `tokens=<private bookmark token>` and gets back
   `{guid, public_key, name, wants[], has[], mastered[], forma[], private}`.
   Seth's real profile (pulled 2026-10-15; seed in `data/targets.json`):
   public_key `cp79x`, **49 wants**, has/mastered/forma all EMPTY — Seth
   maintains wants only. The private token is a credential (bookmark =
   read/write key): keep it only in the binary's local config, never in
   this workspace or git.
   CORRECTION: an earlier claim (nine parts — "Ivara Prime Systems,
   Chroma Prime Systems + Neuroptics, …") was NOT reproducible and is
   wrong. The real wants list is in `data/targets.json`.
4. **Drop data** — Xuerian's own static JSON, derived from DE's published
   drop tables, cached in `data/cache/`:
   - `relictables.json` (1.07 MB): `relics[era] = {rewards: [{name,
     chance: {Intact/Exceptional/Flawless/Radiant}}], vaulted, sources…}`,
     plus `sources` (mission tables → relics) and `items`. Covers both
     join directions: part → relics, relic → farm nodes.
   - `wishlist.json` (30 KB): the full prime roster — 177 items with
     `parts[]`, `vaulted`, `type`. This is the subtraction base against
     profile XPInfo.
   Refresh weekly; both are plain GETs from `wf.xuerian.net/json/`.

## EE.log (source 2 — event stream)

Not yet validated against a real log; Seth will supply one after his next
session. Documented patterns from open-source parsers (ASSUMPTIONS until
validated): reward-screen marker `OpenVoidProjectionRewardScreenRMI`
(wfinfo-go/wfinfo lineage); relic grid open via
`ThemedProjectionManager.lua: PopulateInventoryGrid` (warframe-lite);
account ID printed near session start (warframe-farming-planner sync.py).
EE.log contains NO inventory snapshot — events only: missions run, rewards
offered/picked, relic screens opened, trades completed. Part-level facts
accumulate from reward events; a prime appearing in the profile resolves
its wants as consumed.

## Plan logic

Ordered output, sized to a time budget (default: whole evening; page
regenerates as events complete):

1. Live event alert first when time-boxed (Taubound: flat 50 Moonflesh,
   75 on Steel Path — kills pay ~2–3/mission, negligible; event runs
   Oct 15 – Nov 17, 2026, never reruns; shop prices exist in-game only,
   never invent them).
2. Timer items worth doing inside the budget: Nightwave dailies/weeklies,
   sortie, archon hunt (weekly), Steel Path circuit/archimedea rotations.
3. Fissure cracks matched to targets: for each want, relics containing it
   (relictables.json) × live fissures of that era (world state) → crack
   suggestions; if no matching fissure is live, best farm node for the
   relic itself (sources).
4. Clock-sized filler (another crack set, a syndicate/arbitration) to
   fill remaining budget.

Pruning rule: any want whose parent item appears in profile XPInfo is
done — drop it (and optionally write the prune back to Xuerian via
`/api/store` — WRITE path, only with Seth's explicit OK; default off).
