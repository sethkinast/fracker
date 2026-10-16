# Worker brief #2 — Fracker second pass (data-true planner + page refinement)

Branch: `opencode/second-pass` (already created: first-pass +
master merged, .gitignore reconciled). Read FIRST, in this order:
`docs/verification-addendum.md` (authoritative — it overrides the
original brief where they disagree), `BRIEF.md`, `WEEKLIES.md`,
`data/backlog.json`, `data/wants-pruned.json`,
`data/taubound-shop.md`, `fixtures/VALIDATION.md`.

## 0. CORRECTNESS RULE — fissure era compatibility is a HARD gate

(Added mid-flight 2026-10-15; Seth caught the pass-1 demo sending a
mixed-era batch to Persto.) A void fissure accepts ONLY relics of
its own era (Lith/Meso/Neo/Axi/Requiem) — the exception is an
**Omnia** fissure (e.g. Conjunction Survival, and Omnia rotations
elsewhere), which accepts every era. Therefore:

- Filter fissure candidates by compatibility with the ENTIRE batch
  BEFORE any cascade scoring (expiry / minutes-per-crack / goal fit
  / SP tiebreak all operate only on compatible fissures).
- A mixed-era batch may ONLY be routed to a live Omnia fissure.
- If no compatible fissure is live, re-form the batch around a live
  era — never emit an action line that sends a relic to a fissure
  that cannot crack it.
- Persto (Deimos) is the regression case: the pass-1 demo paired
  Axi + Meso relics with it while its fissure was era-locked. Add a
  planner test reproducing exactly this and asserting the gate.

## Scope of THIS pass

1. **Targets = the reconciled backlog.** Goal cards derive from
   `data/backlog.json` (all 64 unmastered primes), not the raw
   Xuerian wants. Parts per parent from `data/cache/wishlist.json`.
   Non-prime unmastered gear appears in the Backlog tab grouped by
   category (list only this pass — no farm routing for them yet).
2. **Priority (addendum #10): unvaulted/current BEFORE vaulted.**
   Prime Resurgence rotation (world state `vaultTrader`) un-parks
   vaulted items into the current tier while the rotation lasts.
   Frames before weapons within a tier.
3. **Page v2 — tabs, terse, scannable.**
   - THREE tabs (Seth, 2026-10-15): **Do Now** (the focused slate),
     **Hunt** (fan-out: all prime goal cards with their relic /
     chance / stock rows, for when he wants more than the focus
     set), and **Backlog** (everything unmastered, all gear).
   - **Do Now targets AT MOST 3 parts at a time** (Seth's rule).
     Pick the 3 by priority (item 2), preferring a spread across
     different relic eras — era spread means a live fissure almost
     always serves one of them, and an Omnia fissure can serve all
     three at once. The slate's chunks pursue only these 3 parts;
     everything else waits in Hunt/Backlog. Exactly one loud
     action line; everything else quiet.
   - Copy goes TERSE everywhere: labels, numbers, pips, bars. No
     explanatory sentences. Seth's words: "way too many words."
   - Rows align into consistent columns (icon | item | parts pips |
     best relic + chance at chosen tier | stock | expected cracks)
     with steady rhythm and grouping by parent — the eye must be
     able to sweep. KEEP the pass-1 palette/art direction exactly
     (colors are right; structure was the problem).
4. **Icons (addendum #14).** Every frame/weapon card and row shows
   the PARENT ITEM's icon from the WFCD dataset: resolve parent →
   `imageName` → `https://cdn.warframestat.us/img/<imageName>`.
   A prime PART never shows the generic part/blueprint icon — it
   shows its parent item's icon. Report icon coverage (resolved /
   total) in your summary; unresolved items keep the monogram.
5. **Profile-backed state.** The profile fetcher gets a golden test
   against the REAL account (ID in addendum #1): record one live
   profile response into `fixtures/` (JSON commits are allowed;
   only EE.log files are gitignored) and test parsing of XPInfo,
   Affiliations, and the daily-cap fields. Daily bars render from
   `DailyAffiliation*` / `DailyFocus` REMAINING values (addendum
   #2) — no estimation anywhere.
6. **elog test corpus:** replace synthetic marker strings with the
   verbatim markers catalogued in `fixtures/VALIDATION.md`
   (label them transcribed-from-real-log in test comments). Session
   segmentation per the confirmed rule (split on `Logged in` +
   inactivity, never file boundaries).
7. **Taubound data pack v1 (TOML, per BRIEF pack design):** alerts
   as the tier-1 chunk source with the Moonflesh payouts (50 / 75
   SP); shop record from `data/taubound-shop.md`; Contegorus set
   gated on the community-goal progress in world state `events`
   (`health` on the Road To Tau entry); arcane shopping guidance
   (finish Melee Retaliation, then Melee Influence; skip
   Vortex/Surge/Fortification); pack expires 2026-11-17.
8. **Demo regenerated:** `fracker --demo` against LIVE world state
   + the reconciled backlog + a fresh profile pull; commit the new
   `docs/demo-plan.html` on this branch.

## Explicitly OUT of scope this pass

Tray/ksni runtime, Flatpak packaging, the EE.log live watch loop,
OCR inventory bootstrap (pass 3), Xuerian write-back, non-prime
farm routing.

## Gates and deliverables

- `cargo build` clean (warnings explained), `cargo test` green.
- ONE logical commit on `opencode/second-pass`, then PUSH the
  branch to origin (the repo exists now — pushing is required).
- Final summary to stdout: per-item status, what was cut and why,
  icon coverage, demo stats (cards/chunks/bars), any endpoint
  failures from this box.
