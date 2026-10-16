# EE.log validation

Fixture: `EE-2026-10-15-login.log` — Seth's real log, 2,596 lines /
247 KB, captured 2026-10-15 right after login (no missions run).
Seth notes the log "doesn't even clear that often" — one file can span
multiple sessions; session segmentation keys off `Logged in` lines and
inactivity gaps, not file boundaries.

## Confirmed on Seth's log

- **Line format**: `<elapsed.sec> <Subsystem> [<Level>]: <message>`
  (e.g. `14.148 Sys [Info]: Logged in Jenterrobahne`).
- **LoggedIn**: verbatim — `Logged in Jenterrobahne` (Sys/Info).
  Display name = Jenterrobahne.
- **LevelConnected**: line 1349 — `===[ Game successfully connected
  to: /Lotus/Levels/Proc/TheNewWar/PartTwo/TNWDrifterCampMain/
  BoC8ACA.lp ]===`. No MissionInfoSeen preceded it → hub transition
  (Drifter's Camp), exactly as the wf-log state machine predicts.
  BONUS: the line carries the level path, so hub/mission identity is
  recoverable, not just the boundary.
- **Build label** at log top (`2026.10.15.13.08 Retail Windows x64`)
  — usable to version-gate pattern changes.

## Second fixture: `EE-2026-10-15-taubound-alert.log` (8,638 lines)

Same session, one Taubound alert run (Moonflesh earned; one Nightwave
act completed → rank up, per Seth).

- **MissionInfoSeen CONFIRMED, and it's richer than documented**:
  `Client loaded {"name":"SolNode173_Alert"} with MissionInfo:` is
  followed by the FULL mission JSON — missionType (MT_EXTERMINATION),
  faction, location (SolNode173), levelOverride path, and the whole
  `missionReward` block including `countedItems` =
  RoadToTauEventResource ×50. → Mission identity AND event payout
  are exact, parsed from the log. No world-state inference needed
  for what he ran or what it paid.
- **Mission boundary state machine CONFIRMED**: LevelConnected ×3 —
  Camp (hub) → InfestedCorpusShipExterminate (mission, MissionInfo
  armed) → Camp (hub return). Durations derivable from the elapsed
  timestamps.
- **Mission XP CONFIRMED**: `Game [Info]: Mission progress XP:
  33867` on hub return — per-mission XP is logged.
- **RewardScreen for a plain alert: nothing.** No reward-screen or
  reward-claim event lines at mission end. Consistent with the
  design assumption: rewards are server-side; the log shows
  boundaries + XP, not grants.
- **Nightwave**: act completion has NO explicit log line. The only
  trace is the Amir voice-line asset queued ~50s after mission end
  (`DAmirNWWeeklyDone…`, later `DAmirNWReccuring…`) — a heuristic
  signal the client played a Nightwave-completion transmission, not
  structured data. Nightwave completion = NOT trackable from EE.log;
  stays manual-tick on the weekly board (as the brief already falls
  back to).
- **Standing / focus gains**: no gain lines anywhere (all "challenge"
  hits are netcode handshakes or asset paths). Daily bars stay
  estimates/checklists, not measurements.
- **Account ID: STILL absent.** Dozens of 24-hex runs exist now, but
  ranks 1–3 of the resolver all come up empty (no playerInfo lines,
  no VoidProjections lines, no line pairs his name with an ID — he
  played solo). The candidates present are session/content noise.
  Resolver outcome on real data: correctly returns "unresolved"
  rather than guessing. A fissure run (reward-relay lines) or any
  squad play should produce rankable candidates.

## Not answerable from a login-only log (need a fissure-session log)

- Mission/relic markers (MissionInfoSeen, RelicCrack,
  RelicRewardScreen, PopulateInventoryGrid): zero occurrences here,
  as expected — still rated "high confidence per wf-log's live
  captures, unvalidated on Seth's log".
- **Account ID: absent.** Zero lowercase 24-hex runs; the single
  24-hex hit is an uppercase content hash. Consistent with wf-log's
  account.rs: the ID appears only in situational mid-session lines.
  Resolver validation waits for a mission log.
- **Reward item names / standing / focus / Nightwave gains**: none
  present — every "focus"/"nightwave" hit in this file is an asset
  path, not a gain. Whether end-of-mission reports log gains remains
  OPEN until a session log with completed missions is inspected.

## Third fixture: EE-2026-10-15-fissure.log (added later same day)

46,272 lines — the FULL evening session (the earlier two files were
mid-session snapshots of this same session: identical login, session
IDs, and mm hashes). Segmentation rule CONFIRMED: one file, many
sessions/squads; split on `Logged in` + inactivity, never file edges.

- **Fissure reward markers — ALL VERIFIED VERBATIM:**
  `ProjectionRewardChoice.lua: Relic rewards initialized`;
  `VoidProjections: Client got reward info from <24-hex account ID>`
  (×4, one per squad member); `VoidProjections: Client has reward
  info for all players now`; `ProjectionRewardChoice.lua: Got
  rewards`; `ProjectionRewardChoice.lua: Selection countdown done`;
  `ProjectionRewardChoice.lua: Relic reward screen shut down`;
  `DVRCAftermath` transmission on return. The reward screen's four
  relics appear earlier as ResourceLoader projection paths.
- **ACCOUNT ID RESOLVED (2026-10-15, endpoint-confirmed):**
  `5420fba23846322173707618` — the first `got reward info from`
  candidate; the profile endpoint returns DisplayName Jenterrobahne,
  MR 30, and the response's own `AccountId` field matches. Control
  tests behaved (squadmates' IDs return their own names). A second
  name↔ID source also exists: `SendSessionUpdate` params carry
  `"memberAccountId"` for session members. Which relic of the four
  was Seth's own crack is NOT name-paired in the log — unresolved.
- **Profile pull works end to end** (`data/profile-raw.json`, local
  only): 610 XPInfo items, 470 mission nodes, affiliations, and —
  the big one — **daily cap state**: `DailyAffiliation: 6300`
  (faction standing REMAINING of the 31,000 MR30 cap),
  `DailyAffiliationCavia/Cetus: 31000` (untouched), `DailyFocus:
  262328` (remaining of 400,000). The daily bars need NO log
  derivation — the profile reports remaining caps directly.
- **Wants pruning works:** of the 49 Xuerian wants, 9 are stale —
  their parents (Equinox, Hildryn, Khora, Nidus, Fulmin, Scourge,
  Tipedo, Cobra & Crane Primes) are already in XPInfo = mastered.
  40 live wants remain (`data/wants-pruned.json`). Caveat: XPInfo
  absence means "never gained affinity," the design's mastery test.
