# Verification addendum — post-brief findings (2026-10-15 evening)

Everything here was verified AFTER Worker Brief #1 was dispatched;
it refines the brief where they disagree. For the next worker pass.

1. **Account ID RESOLVED.** `5420fba23846322173707618`, resolved
   from a real fissure log's reward relay (`VoidProjections: Client
   got reward info from <id>`) and confirmed by the profile
   endpoint (DisplayName Jenterrobahne, MR 30; the response's own
   `AccountId` field matches). §7.4 step 3 (profile confirmation)
   is proven, not hypothetical. Second name↔ID source in-log:
   `SendSessionUpdate` params carry `"memberAccountId"`.
2. **Daily bars come from the profile, not estimation.** The
   profile response carries remaining-cap fields:
   `DailyAffiliation` (faction pool; 6,300 of 31,000 remaining at
   pull time), per-family `DailyAffiliationCavia/Cetus/...`
   (31,000 = untouched), and `DailyFocus` (262,328 of 400,000
   remaining). Fetch and display; no XP derivation needed. (For
   reference, the derivation that DOES exist: 15% of affinity →
   pledged-syndicate standing.)
3. **Live targets supersede the raw 49.** `data/wants-pruned.json`:
   9 of the 49 Xuerian wants are stale (parents already in the
   profile's XPInfo = mastered: Equinox, Hildryn, Khora, Nidus,
   Fulmin, Scourge, Tipedo, Cobra & Crane Primes). 40 live wants.
   Planner tests should use the pruned set.
4. **Real fissure fixture exists** (Seth-side; EE.log files stay out
   of git per .gitignore). Every §7.2 reward marker verified
   verbatim — see `fixtures/VALIDATION.md`, third-fixture section.
   Synthetic elog test lines can be replaced by real excerpts.
5. **State model: track equips + picks.** Drop attribution (whose
   relic yielded what) is explicitly NOT needed: Seth picked the
   Protea part off a squadmate's crack while his own drop was
   something else. Consumed = his equipped relic; acquired = his
   pick. The "self first?" relay-ordering question is parked.
6. **Taubound event data:** `data/taubound-shop.md` — full price
   record from Seth's screenshots (core 500, shards 375, arcanes
   5/8 Moonflesh; rank-5 set = 21 copies), Contegorus set gated on
   the community goal (world state `/pc/events` carries progress),
   arcane verdicts (finish Melee Retaliation, then Melee Influence;
   skip Vortex/Surge/Fortification). This is the first data pack's
   real content.
7. **wf-log vendoring accepted.** Upstream was unreachable from the
   worker box (not on crates.io; no public warframe-lite repo
   found). The vendored §7.2 catalog with attribution stands until
   the upstream surfaces; keep the `classify()` swap-in seam.
8. **Demo review (Seth, 2026-10-15): way too many words.** Page copy
   goes terse: labels, numbers, pips — not sentences. The one Do-Now
   instruction stays a single line.
9. **Tabs:** a "do these right now" view vs the full wanted/backlog
   list — two tabs over the same data, not one long page.
10. **Priority inversion: unvaulted BEFORE vaulted.** Current items
    are time-bound (they vault eventually); vaulted items wait,
    except when Prime Resurgence un-parks them (that exception
    stands — Ivara now). The first pass had this backwards.
11. **Scannability:** the color scheme/palette is RIGHT (keep it);
    the lines are not scannable — rows need alignment, consistent
    columns, and grouping so the eye can sweep. Refine iteratively,
    bit by bit, with Seth reviewing passes.
12. **Backlog source of truth = profile-derived, NOT Xuerian.**
    `data/backlog.json` (2026-10-15): prime roster 163, mastered
    99, unmastered 64 — and 37 of the 64 are NOT on Seth's Xuerian
    list at all, including all seven current frames (Caliban,
    Citrine, Gyre, Lavos, Styanax, Voruna, Yareli) and 14 current
    weapons. Total unmastered across all gear: ~229 items. Xuerian
    is demoted to an input (his relic-stock tracking); Fracker's
    backlog derives from the profile vs the WFCD item universe.
    (Known gap: the pets category file failed to load — companion
    backlog incomplete; Kavasa Prime's status unverified.)
13. **Inventory bootstrap still owed:** relic stock + loose parts
    via the guided OCR scroll (brief design stands). Queued for
    pass 3.
14. **Icons (Seth, 2026-10-15):** frames and weapons show their real
    WFCD icons (`imageName` → cdn.warframestat.us/img/<imageName>).
    For a prime PART, use the icon of the ITEM ITSELF (the parent
    frame/weapon), never the generic part/blueprint icon.
15. **Iteration workflow (Seth, 2026-10-15):** Graf keeps cranking
    passes; Seth steers after each pass. No per-pass approval gate.
