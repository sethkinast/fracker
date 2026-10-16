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
