# Warframe weeklies — the definitive sweep

Compiled 2026-10-15 for the session planner (extends BRIEF.md §10 tier 4).
Anchor reset: **Monday 00:00 UTC** unless an item says otherwise.
Verified against the community wiki (Reset / activity / vendor pages),
the live world-state API (`api.warframestat.us/pc/`, sampled
2026-10-15), and the Update 41 / Update 44 patch notes.

**Observability legend** (can the planner *know*, not just list?):
- **(a)** — visible in the public world-state API (this week's content,
  rotation, or schedule; never personal completion).
- **(b)** — inferable from EE.log mission events, *tentative*: the log's
  MissionInfo lines should carry node/mission type (BRIEF §7.2), but no
  pattern is validated against a real log yet.
- **(c)** — checkbox-only: no clean source can prove completion
  (vendor purchases, orbiter interactions, personal progress).

## Tracked subset — pass-4 slimming (2026-10-16)

`data/weeklies.toml` IS the tracked list (16 entries): Archon Hunt,
Netracells, Deep + Temporal Archimedea, both Circuit tracks,
Descendia, Kahl, 1999 Calendar, Palladino, Acrithis, Yonta, Bird 3,
Chipper, Clem + Maroo (last, least important). Everything below in
this file remains the research record; only the TOML renders.

Cut, per addendum #27: Nightwave (own section now — no duplication),
Conclave, Clan, Teshin (rotating item + evergreen caps), Helminth,
and the NW-creds row (dropped outright, not tracked). Baro left the
box for his own section, rendered only while a void trader is live
in world state — gone when he leaves.

## Summary table

| # | Item | Limit | ~Time | Observable via |
|---|------|-------|-------|----------------|
| 1 | Archon Hunt | 1 clear/wk | 30–45m | (a) missions+boss; (b) completion |
| 2 | Netracells | shares 5 Search Pulses/wk | 10–15m each | (b) tentative; pulses left (c) |
| 3 | Deep Archimedea (+Elite) | rewards 1×/wk, 2 pulses | 30–45m | (a) missions/modifiers; completion (c) |
| 4 | Temporal Archimedea (+Elite) | rewards 1×/wk, 2 pulses | 30–45m | (a) missions/modifiers; completion (c) |
| 5 | The Circuit — Normal | 1 reward track/wk | 1.5–2h to Tier 10 | (c); (b) stage play only |
| 6 | The Circuit — Steel Path | 2 Incarnon picks/wk | 1.5–2.5h to Tier 10 | (c) (rotation is a fixed published table) |
| 7 | Descendia (Normal + Steel Path) | 10 rewards per difficulty/wk | 45–90m per difficulty, splittable | (a) weekly seed/challenges; completion (c) |
| 8 | Break Narmer (Kahl) | 1 mission set/wk, ≤115 Stock | 15–25m | (c) |
| 9 | Ayatan Treasure Hunt (Maroo) | 1 sculpture/wk | ~5m | (a) alert slot (unconfirmed in API); completion (c) |
| 10 | Help Clem (Darvo) | 1/wk | ~10m | (c) |
| 11 | Nightwave weekly + elite acts | 5 weekly + 2 elite/wk | mostly passive | (a) act list; completion (c) |
| 12 | 1999 Calendar weekly page | 1 page/wk | passive during 1999 play | (a) page contents; completion (c) |
| 13 | Clan Weekly Initiative | 4 reward tiers/wk (clan-wide) | passive | (a) region+thresholds; personal claim (c) |
| 14 | Conclave weekly challenges | 3 challenges + all-clear bonus/wk — **Fri reset** | varies | (a) challenge list; completion (c) |
| 15 | Teshin — Steel Path Honors rotation | 1 featured item/wk | 2m (purchase) | (a) current item + costs; purchase (c) |
| 16 | Teshin — evergreen caps | Veiled Riven Cipher 1/wk; 10k Kuva & Relic Pack ≤25/wk each | 2m | (a) stock list; purchases (c) |
| 17 | Palladino (Iron Wake) | each ware 1/wk (35 slivers buys all) | 2m | (c) |
| 18 | Acrithis weekly wares | 5 rotating wares, 1 each/wk — **Sun reset** | 2m | (c) |
| 19 | Archimedean Yonta — Forma | 3 Forma/wk | 2m | (c) |
| 20 | Bird 3 — Shiny Treasures shard | 1 Archon Shard/wk | 2m | (c) |
| 21 | Chipper — weekly Archon Shard | 1/wk (Garrison Rank 5) | 2m | (c) |
| 22 | Helminth Invigoration | 3 frames get options/wk | 2m (orbiter) | (c) |
| 23 | Nightwave Cred Offerings rotation | shop subset rotates/wk | 2m | (c) |
| 24 | Baro Ki'Teer | **biweekly**, Fri 13:00–Sun 13:00 UTC | 5m | (a) schedule, location, inventory |

---

## A. Weekly activities

### 1. Archon Hunt
- **What:** A 3-mission Sortie-like chain (two missions, then an
  Archon Showdown assassination) against Narmer.
- **Reward:** 1 Archon Shard of the boss's color (Boreal→Azure,
  Nira→Amber, Amar→Crimson); 20% base Tauforged chance with a pity
  ramp (+20% per non-Tauforged, tracked per color). Rotating
  frame/weapon damage bonuses each week.
- **Limit:** Rewards once per week. **Time:** ~30–45m.
- **Prereqs:** The New War + Veilbreaker quests. **Island:** Origin System.
- **Observable:** (a) world-state `archonHunt` gives boss, nodes,
  mission types; (b) completion plausibly inferable from the
  Showdown mission in EE.log — unvalidated.

### 2. Netracells
- **What:** High-level vault mission in Albrecht's Laboratories
  (Deimos): carry debuff Keyglyphs, kill enemies inside the circle.
- **Reward:** Per clear, rolls on the Netracell pool — melee arcanes
  (incl. Legendary), Archon Shards (~52.5% normal / 12.5% Tauforged
  per clear), adapters.
- **Limit:** Each rewarded clear consumes **1 Search Pulse** from the
  weekly pool of **5, shared with both Archimedeas**. Replays allowed
  after pulses run out, but unrewarded.
- **Time:** ~10–15m per run. **Prereqs:** Whispers in the Walls.
  **Island:** Deimos / Sanctum Anatomica.
- **Observable:** (b) tentative — Netracell missions should be
  identifiable in EE.log; remaining pulse count is (c) unless the
  planner's own ledger tracks spend.

### 3. Deep Archimedea (+ Elite Archimedea)
- **What:** A fixed weekly gauntlet of 3 consecutive Laboratories
  missions with preset loadout options, deviations, risk variables
  and personal modifiers; score Research Points to unlock reward
  tiers. Elite is the harder, better-paying version (permanently
  unlocked by a 25-point Deep run once).
- **Reward:** Multiple rolls scaling with points — Archon Shards
  (top tiers Tauforged-guaranteed territory), Legendary melee
  arcanes, Vosfor.
- **Limit/cost:** Unlocking the week's rewards costs **2 Search
  Pulses** (shared pool of 5). Rewards pay once per week.
- **Time:** ~30–45m. **Prereqs:** Whispers in the Walls + Cavia
  Rank 5. **Island:** Deimos / Sanctum Anatomica (via Necraloid).
- **Observable:** (a) world-state `archimedeas` lists both
  Archimedeas' missions, deviations, risks (type key `CT_LAB`).
  Completion: (c) — the underlying mission types also exist as
  normal missions, so EE.log can't reliably prove an Archimedea run.
- **Recent change:** Update 44 (Sep 2026) restructured personal
  modifiers into 4 buckets, one drawn from each per week.

### 4. Temporal Archimedea (+ Elite)
- **What:** The 1999 twin of Deep Archimedea — 3 fixed Höllvania
  missions with modifiers, plus Peely Pix buffs.
- **Reward:** Same tier structure (shards incl. Tauforged chances,
  arcanes) plus **Pix** currency, which Kaya exchanges for 1999
  arcanes and goods.
- **Limit/cost:** **2 Search Pulses** — the same weekly pool of 5.
  Doing both Elites (4 pulses) leaves exactly 1 Netracell.
- **Time:** ~30–45m. **Prereqs:** The Hex quest + The Hex Rank 5
  (via Kaya, Höllvania Central Mall). **Island:** Höllvania 1999.
- **Observable:** (a) `archimedeas` (type key `CT_HEX`) lists its
  missions/modifiers; completion (c).

### 5. The Circuit — Normal
- **What:** Duviri's Undercroft stage chain with loaner/owned gear;
  earn Circuit Progress through stages to climb a 10-tier weekly
  reward track.
- **Reward:** One Warframe of the week's 3 choices — its full part
  set is distributed across the tiers (Neuroptics T2, Chassis T5,
  Systems T8, main BP T10), plus mods/arcanes/resources.
- **Limit:** Tier rewards claimable once per week; progress resets
  Monday 00:00 UTC. (A bonus 50 progress/day exists — daily, see
  appendix.) **Time:** ~1.5–2h to Tier 10, can be split across days.
- **Prereqs:** The Duviri Paradox quest. **Island:** Duviri.
- **Observable:** (c). The weekly frame trio is a fixed rotation but
  is not in the world-state API.

### 6. The Circuit — Steel Path
- **What:** Same track on Steel Path. Before the week's first run
  you pick **2 of 5 Incarnon Genesis Adapters** as your Tier 5 and
  Tier 10 rewards.
- **Reward:** The 2 chosen adapters (rotation of 5-adapter sets,
  8+ sets long); at Drifter Opportunity Rank 9, Tier 9 can instead
  be a Riven or 20,000 Kuva.
- **Limit:** Once per week; the adapter set rotates Monday 00:00 UTC.
  **Time:** ~1.5–2.5h to Tier 10.
- **Prereqs:** Duviri Paradox + Steel Path access + one Normal
  Circuit/Lone Story completion. **Island:** Duviri.
- **Observable:** (c) for progress/picks; the rotation itself is a
  fixed published table, so the planner can hard-code it.

### 7. Descendia (Normal + Steel Path)
- **What:** The Old Peace (Update 41, Dec 2025) roguelite tower:
  descend 21 "Infernum" floors of mini-missions with Penance
  modifiers; every 7th floor is a Devil's Triad checkpoint where
  progress can be banked and the run exited.
- **Reward:** **10 rewards per difficulty per week** from that
  difficulty's table (credits/Endo/Vosfor, Ignia & Maphica trade
  currency, better rolls on Steel Path); floor 21 guarantees a
  Uriel or Vinquibus component (not weekly-capped).
- **Limit:** Each difficulty's 10 weekly rewards are earnable once;
  resets Monday 00:00 UTC. **Time:** ~45–90m per full difficulty,
  explicitly designed to be split at checkpoints.
- **Prereqs:** The Old Peace quest (a Mission Preview skip exists
  for newer accounts). **Island:** Tau / The Dark Refractory.
- **Observable:** (a) world-state `descendia` exposes the weekly
  seed and all 21 floor challenges for both difficulties.
  Completion: (c) (floor clears may appear in EE.log; unvalidated).
- **Stale-check:** absent from anything pre-Dec-2025 — the single
  biggest miss on older checklists.

### 8. Break Narmer (Kahl's weekly)
- **What:** One weekly Kahl stealth/action mission (rotates through
  Sneaky Sabotage / Junk Run / Prison Break by personal progress)
  with 6 side challenges.
- **Reward:** **Stock** currency: 15 per challenge, 30 for the
  Deathless challenge — 105/wk from challenges, +10 from in-mission
  pickups (115 max). Also Garrison rank-up until Rank 5.
- **Limit:** Challenge rewards once per week; replays allowed to
  mop up unfinished challenges (progress carries between runs).
  **Time:** ~15–25m.
- **Prereqs:** Veilbreaker quest; Drifter's Camp access.
  **Island:** Origin System (Drifter's Camp, Earth).
- **Observable:** (c) — Kahl missions don't present as normal
  star-chart missions; treat as checkbox.

### 9. Ayatan Treasure Hunt (Maroo)
- **What:** Talk to Maroo (Maroo's Bazaar, Mars); run a timed
  obstacle-room mission in the Void/Derelict and grab the sculpture.
- **Reward:** 1 Ayatan sculpture (socket with stars → Endo).
- **Limit:** 1 per week, Monday 00:00 UTC. **Time:** ~5m.
- **Prereqs:** Stolen Dreams quest. **Island:** Origin System.
- **Observable:** appears as an alert in the in-game world-state
  window; whether warframestat's `alerts` carries it is unconfirmed
  — plan on (c) for completion either way.

### 10. Help Clem (Darvo's weekly)
- **What:** Repeatable weekly mission from Darvo in any Relay:
  rescue/escort Clem on a survival-style mission.
- **Reward:** Clem Clone Specter blueprint.
- **Limit:** 1 per week. **Time:** ~10m.
- **Prereqs:** Complete Darvo's one-time "A Man of Few Words" quest
  first. **Island:** Origin System (Relay).
- **Observable:** (c).

### 11. Nightwave weekly & elite acts
- **What:** 5 weekly acts (4,500 standing each; three are permanent —
  complete 15 missions, kill 500 enemies, kill 30 Eximus) + 2 elite
  weekly acts (7,000 each). Daily acts (1,000) are separate — see
  appendix.
- **Reward:** Nightwave standing toward season ranks (reward track +
  Creds at prestige ranks).
- **Limit:** Each act once per week; unfinished acts can be
  recovered in later weeks when behind. **Time:** mostly passive
  during other play. **Prereqs:** none. **Island:** all.
- **Observable:** (a) world-state `nightwave` lists active acts and
  the season; per-act completion is account state → (c).

### 12. 1999 Calendar (weekly page)
- **What:** The POM-2 calendar advances one in-game week per real
  week: days carry To Do challenges (small Höllvania tasks),
  Override buffs, and Big Prize days.
- **Reward:** Big Prize items (e.g. Exilus Weapon Adapter BP,
  primary arcane unlocker), weekly Override buffs for 1999 play,
  arcane rewards.
- **Limit:** One page per week, Monday 00:00 UTC. **Time:** passive
  alongside normal 1999 play.
- **Prereqs:** The Hex quest (1999 access). **Island:** Höllvania 1999.
- **Observable:** (a) world-state `calendar` gives the current
  week's days, challenges, overrides and prizes; which To Dos
  *you* finished is (c).

### 13. Clan Weekly Initiative
- **What:** 2026 clan-rework system (The Shadowgrapher update):
  each week designates a bonus region; clan members' mission play
  earns initiative points toward 4 shared thresholds.
- **Reward:** Per-threshold personal rewards (research resources,
  Kuva, etc. — this week's live example: 500/1000/1500/2000 points
  → component ×5, 7,000 Kuva, 14,000 Kuva, 200 Distill Points).
  Exact point-earning rates per mission are not well documented —
  flagged unsure.
- **Limit:** 4 tiers per week, clan-wide progress. **Time:** passive.
- **Prereqs:** Be in a clan. **Island:** rotates (bonus region).
- **Observable:** (a) world-state `clanWeeklyInitiative` gives
  week, bonus region, thresholds; the public feed's per-reward
  `RewardClaimed` flags are defaults, not Seth's account state —
  personal claims are (c).

### 14. Conclave weekly challenges
- **What:** 3 weekly PvP challenges (win 6 matches, complete 20,
  complete 10 daily challenges) under a root "Weekly Mission".
- **Reward:** Conclave standing; completing all three grants the
  root bonus of 50,000 standing.
- **Limit:** Once per week — **resets Friday 00:00 UTC** (an
  exception to the Monday rule; the API showed a 01:00 UTC boundary
  the sampled week, so treat the exact Friday hour as ±1h).
- **Time:** varies; only relevant if playing Conclave.
- **Prereqs:** Conclave access (Teshin). **Island:** Origin System (PvP).
- **Observable:** (a) world-state `conclaveChallenges` lists them;
  completion (c).

---

## B. Weekly vendors & purchase limits

### 15–16. Teshin — Steel Path Honors (any Relay)
- **Rotating item (limit 1/week):** an 8-week rotation — Umbra
  Forma BP (150 Steel Essence), 50,000 Kuva (55), Kitgun Riven (75),
  3× Forma (75), Zaw Riven (75), 30,000 Endo (150), Rifle Riven
  (75), Shotgun Riven (75).
- **Weekly-capped evergreens:** Veiled Riven Cipher (20 SE,
  1/week); 10,000 Kuva (15 SE) and Relic Pack (15 SE) capped at
  25/week each (cap added in Update 32.3).
- **Observable:** (a) world-state `steelPath` carries the current
  rotating item, full rotation and evergreen list with costs and
  the reset timer. Purchases: (c).
- **Prereqs:** Steel Path unlocked. **Time:** ~2m at a Relay.

### 17. Palladino — Iron Wake (Earth)
- **Weekly wares, each 1/week:** 2× Veiled Riven Mod (10 Riven
  Slivers each), 35,000 Kuva (10 slivers), 150,000 Credits
  (5 slivers). Full buyout = 35 slivers/week.
- **Prereqs:** Chains of Harrow (Iron Wake access). **Time:** ~2m.
- **Observable:** (c).
- **Stale-check:** pre-Update 32.3 sources say Palladino reset on
  **Thursday** — wrong since Feb 2023; it's Monday 00:00 UTC now.

### 18. Acrithis — Duviri / Dormizone
- **Weekly rotation:** 5 wares drawn from a pool — Forma BP,
  5,000 Kuva, Veiled Rivens (Rifle/Melee/Companion Weapon), Orokin
  Reactor/Catalyst, Exilus & Arcane adapters — priced 10–25
  **Pathos Clamps** each, 1 purchase each.
- **Reset is the odd one: Sunday 00:00 UTC** (confirmed on the wiki
  vendor page and by a 37.0.9 hotfix note) — a day *before* the
  main weekly reset.
- Her separate **daily** offerings and the once-daily arcane are
  daily systems (appendix). **Prereqs:** The Duviri Paradox.
- **Observable:** (c) — weekly stock is not in the world-state API.

### 19. Archimedean Yonta — Chrysalith, Zariman
- **Weekly ware:** Forma, up to **3 per week**, 5 Voidplume Pinions
  each. **Time:** ~2m. **Prereqs:** Angels of the Zariman.
- **Observable:** (c).

### 20. Bird 3 — Sanctum Anatomica ("Shiny Treasures")
- **Weekly ware:** 1 **Archon Shard** for 30,000 Cavia standing;
  color rotates weekly (Crimson/Amber/Azure), never Tauforged.
- **Prereqs:** Whispers in the Walls + Cavia Rank 5. **Time:** ~2m.
- **Observable:** (c).
- **Stale-check:** this replaced Chipper's weekly shard in Dante
  Unbound (2024) — older checklists pointing at Chipper for "the
  weekly shard" are half-right (see #21: Chipper still sells one,
  but for Stock, at max Garrison rank).

### 21. Chipper — Drifter's Camp
- **Weekly ware:** 1 **Archon Shard** for 90 Stock, at Kahl's
  Garrison Rank 5 (Home). The rest of Chipper's stock is one-time
  purchases, not weekly.
- **Observable:** (c).

### 22. Helminth Invigoration (Orbiter)
- **What:** Each week, 3 random Warframes in your arsenal receive
  Invigoration options (two buff choices each); applying one costs
  Helminth resources and lasts until the next weekly reset.
- **Limit:** The offered set refreshes Monday 00:00 UTC; an applied
  invigoration lasts the week.
- **Prereqs:** Helminth Invigoration Segment (from Son, Entrati
  standing) installed. **Time:** ~2m at the Helminth chair.
- **Observable:** (c) — the offered frames are account-specific and
  in no public feed. (Buff choice itself is a preference, not a
  completion; the planner item is "check this week's invigorations".)

### 23. Nightwave Cred Offerings rotation
- **What:** A rotating subset of the Nightwave store (aura mods,
  alt helmets, weapon BPs, etc., bought with season Creds) refreshes
  weekly, Monday 00:00 UTC, alongside the acts.
- **Observable:** (c) — the rotation is not exposed in the
  world-state API's nightwave section.

### 24. Baro Ki'Teer — Void Trader (**biweekly**)
- **What:** Arrives at a Relay every other **Friday 13:00 UTC**,
  leaves Sunday 13:00 UTC, selling primed mods, Prisma/Baro
  weapons and cosmetics for Ducats + Credits.
- **Observable:** (a) world-state `voidTrader` gives schedule,
  location, and inventory once he arrives. Purchases: (c).
- Included per the brief despite the biweekly cadence — his
  "is Baro here / leaving soon" state is exactly the kind of thing
  the planner's Do-first section wants.

---

## C. Appendix — NOT weekly (do not track as weeklies)

Daily (reset 00:00 UTC unless noted):
- **Sortie** — daily, but on its own **17:00 UTC** reset (16:00
  during DST). Syndicate mission set also resets 17:00 UTC.
- **Syndicate standing caps** — the "turn items in for standing"
  mechanic Seth remembered is **daily-capped** (cap scales with MR);
  there is no weekly standing cap anywhere. Medallion turn-ins draw
  from the same daily cap.
- **Focus cap** — daily: 250,000 + 5,000 × MR (so ~350k at MR20).
- **Steel Path Incursions** — 5 daily alerts (world-state
  `steelPath.incursions`).
- **Nightwave daily act** — 1,000 standing; note daily acts stay
  completable for 3 days in the API.
- **Simaris daily synthesis target** (world-state `simaris`).
- **Daily Tribute** (login), **trade limit**, **Argon Crystal
  decay**, **Circuit Stage-4 bonus progress** (+50/day),
  **Acrithis daily offerings** + her once-daily arcane,
  **Hex KIM chemistry/gifting** (1999), **Darvo's daily deal**.
- **Faceoff bonus windows** (1999 PvPvE) — recur several times a
  day (world-state `faceoffBonus`); not weekly.

Hourly / event-driven / longer cycles:
- **Arbitration** — hourly rotation. **Void Fissures** — ~hourly.
- **Invasions, Fomorian, Razorback** — community-progress driven.
- **Ghoul Purge / Thermia Fractures** — roughly monthly recurrences.
- **Varzia — Prime Resurgence** (world-state `vaultTrader`) —
  ~4-week rotation, not weekly.
- **Gift of the Lotus alerts** — follow devstreams, irregular.

## D. The Reddit infographic, and what it gets stale

The checklist Seth remembers most plausibly traces to r/Warframe's
"The Ultimate Warframe Daily Tasks Checklist" (Mar 2023) and the
u/Joewoof infographic family it links. Accurate for its era, but a
2023 checklist is wrong or silent on all of this by Oct 2026:

- **Missing entirely:** Temporal Archimedea (2025), Descendia
  (Dec 2025), the 1999 Calendar (Dec 2024), Clan Weekly Initiative
  (2026), Deep Archimedea + Search Pulses (2024).
- **Moved:** Palladino's reset Thursday → Monday (Update 32.3).
- **Moved:** the Cavia weekly shard — Bird 3 sells it now (Dante
  Unbound); Chipper's is a separate Stock purchase at Rank 5.
- **Changed:** Archimedea personal modifiers restructured into
  buckets (Update 44, Sep 2026); Search Pulse pool is shared three
  ways (Netracells + both Archimedeas), not "5 Netracells".
- **Reset exceptions it flattens:** Acrithis weekly = Sunday,
  Conclave weekly = Friday, Sortie = daily at 17:00 UTC.

## Sources

- Wiki — Reset: https://warframe.fandom.com/wiki/Reset
- Wiki — Deep Archimedea: https://warframe.fandom.com/wiki/Deep_Archimedea
- Wiki — Netracells: https://warframe.fandom.com/wiki/Netracells
- Wiki — The Circuit: https://warframe.fandom.com/wiki/The_Circuit
- Wiki — Acrithis: https://warframe.fandom.com/wiki/Acrithis
- Wiki — Palladino: https://warframe.fandom.com/wiki/Palladino
- Wiki — Break Narmer: https://warframe.fandom.com/wiki/Break_Narmer
- Wiki — Vendors data module: https://warframe.fandom.com/wiki/Module:Vendors/data
- Dexerto weekly reset guide: https://www.Dexerto.com/gaming/warframe-weekly-reset-guide-2920630/
- Sportskeeda weekly resets checklist: https://www.SportsKeeda.com/mmo/warframe-weekly-resets-checklist-everything-new-week
- Sportskeeda — The Old Peace (Update 41) patch notes: https://www.sportskeeda.com/mmo/warframe-the-old-peace-update-41-patch-notes
- Sportskeeda — Temporal Archimedea guide: http://sportskeeda.com/mmo/warframe-temporal-archimedea-guide-requirements-mechanics-rewards
- Forums — Update 44 (Archimedea modifier buckets): https://forums.warframe.com/topic/1523958-nsw-update-44-iceblade-of-narin/
- WarframeStatus API docs: https://docs.warframestat.us/
- Live world-state sample: `api.warframestat.us/pc/` fetched 2026-10-15
  (archonHunt, archimedeas, steelPath, calendar, descendia,
  clanWeeklyInitiative, conclaveChallenges, voidTrader sections).
- Reddit — The Ultimate Warframe Daily Tasks Checklist (2023,
  archived): https://archive-box.yurigamma.org/archive/1726010007.860536/output.pdf
