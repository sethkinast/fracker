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
16. **Fissure era compatibility (Seth, 2026-10-15) — HARD planner
    constraint, not a scoring factor.** A fissure cracks only its
    own era's relics; Omnia fissures (Conjunction Survival et al.)
    accept all eras. Batches must be era-compatible with the target
    fissure as a whole; mixed-era batches route only to Omnia. The
    pass-1 demo violated this at Persto (Deimos) — regression test
    required (brief #2 §0).
17. **Do Now targets ≤ 3 parts (Seth, 2026-10-15), era-spread
    preferred.** Focus beats breadth for the brain-off slate; three
    parts across different relic classes means a live fissure
    almost always serves one, and Omnia serves all three.
18. **Three tabs (Seth, 2026-10-15):** Do Now (the ≤3-part slate) /
    Hunt (fan-out over all prime goal cards) / Backlog (everything
    unmastered). Supersedes the two-tab note in item 9.
19. **Merge policy (Seth, 2026-10-15): no PRs on this repo.** Worker
    pass branches merge straight into master (raw push) when a pass
    completes; Graf does the merge. Pass 1 merged 2026-10-15
    (master 1920d92).
20. **No game data hardcoded in Rust (Seth, 2026-10-15; queued for
    pass 3, do NOT retrofit into the running pass 2).** The pass-1
    code baked the weeklies into the source — that violates the
    data-pack principle. ALL content — weeklies, rotations, event
    packs, thresholds, target lists — lives in YAML/TOML/JSON files
    loaded at runtime; Rust only interprets. Updating game content
    must never require a code change.
21. **Inventory source ruling (Seth, 2026-10-15): OCR is the plan.**
    Sainan's warframe-api-helper was evaluated in full (read-only
    memory scan for the session nonce → inventory.php dump). Seth's
    call: back-pocket LAST RESORT only — not part of the build. The
    clean-sources rule stands; the pass-3 OCR bootstrap proceeds.
    The helper is revisited only if OCR genuinely fails AND Seth
    explicitly reopens it.
22. **Kavasa Prime excluded (Seth, 2026-10-15):** Seth owns the
    collar already; it's a companion collar, not a weapon — no
    mastery, a special case. Out of the backlog on every
    regeneration (unmastered primes 64 → 63).
23. **Demo deliverable must be self-contained (2026-10-15):** CDN
    hotlink icons don't render in Seth's review surface. The demo
    export embeds icons as ≤128px data-URI thumbnails; the product
    page itself keeps CDN hotlinks per BRIEF §4.2.
24. **Iteration discipline (Seth, 2026-10-15):** a pass is frozen
    at dispatch. New steering, fixtures, and ideas ENQUEUE for the
    next pass — never retrofit them into the running pass, never
    cram scope to "fit more in." Small, steerable passes are the
    point.
25. **Excalibur Prime excluded permanently (Seth, 2026-10-15):**
    Founder-exclusive and unobtainable — "he's gone forever."
    General rule for universe diffs: items with NO acquisition
    path are excluded, never listed as backlog. (Same scrub should
    catch non-frame entries squatting in the Warframes category —
    e.g. Helminth — queued for pass 4.)
26. **Do Now layout on regen (Seth, 2026-10-16; pass 4):**
    (a) A section for time-limited EVENT alerts; (b) a SEPARATE
    section for "Gift of the Lotus" alerts (none active at design
    time — empty sections hide); (c) then the ≤3 target relics with
    what they give (already built); (d) Varzia/Resurgence detection
    stays first-class — surfacing "Varzia sells relics for YOUR
    unowned vaulted items" is the tool's showcase value (Seth's
    words); (e) Nightwave acts render their DESCRIPTIONS, not just
    names — the name alone doesn't say what to do.
27. **Weekly box slimming + Baro (Seth, 2026-10-16; pass 4):**
    - Nightwave LEAVES the big weekly box entirely — it lives in
      its own section; no duplication.
    - Ditched outright (do not track): Conclave, Clan, Teshin,
      Helminth, and the NW-creds row.
    - Baro Ki'Teer gets his OWN section, present only when he's
      around (world state voidTraders) — gone when he leaves.
    - Pass 4 reconciles WEEKLIES.md and the weeklies data file
      against this list.
28. **Daily bars = one per standing pool (Seth, 2026-10-16).** A
    single syndicate bar is wrong: the profile carries FOURTEEN
    DailyAffiliation* pools (syndicates, Library/Simaris, Cetus,
    Quills, Solaris, Ventkids, Vox, Entrati, Necraloid, Zariman,
    Kahl, Cavia, Hex, PvP) — cap 31,000 each at MR 30 — plus
    DailyFocus (400,000). Render one slim bar per pool,
    remaining-of-cap; PvP is dropped per item 27's Conclave cut.
    Lifetime faction standings (41 Affiliations records incl. 16
    historical Nightwave seasons) are card data, not bars.
29. **Icons ARE hotlinked by design (settled 2026-10-16).** The
    WFCD CDN (cdn.warframestat.us/img/<imageName>) is published
    for exactly this consumption; the product page hotlinks.
    Embedding (item 23) is ONLY for demo/review exports into
    sandboxed surfaces. A local icon cache is a possible later
    robustness tweak, not a requirement.
30. **OCR ships BUNDLED (Seth, 2026-10-16).** No separate install:
    the Flatpak carries a static tesseract binary + eng tessdata
    as a sidecar; Fracker's engine trait probes it first (the
    pass-3 auto-probing CLI engine is the seam). Pass 5 validates
    the binding against Seth's real screenshot fixtures using a
    user-space static tesseract — the same binary the package
    will carry.
31. **Do Now gaps (Seth, 2026-10-16; queued):** (a) the Do Now
    tab renders NO icons — its target parts and chunk rows get
    the parent-item icons like everywhere else (addendum #14
    applies there too); (b) "Do Now" is renamed **First Crack**
    (Seth approved the pun, 2026-10-16: "send it"). The tab,
    its header, and any copy referencing "Do Now" as the tab
    name change with the rename.
32. **Pass sizing: FAT passes preferred (Seth, 2026-10-16).**
    Slow worker turns are fine — he'd rather each pass take
    EVERYTHING accumulated in the queue at once than run many
    small ones. (Supersedes the small-pass drift; passes still
    freeze at dispatch per item 24.) Observed 2026-10-16: the
    Oracle box idles through a run (load avg 0.22) — the wall
    clock is remote inference + serial tool round-trips, with
    local CPU only in compile bursts.
33. **Sequencing: runnable-here + inventory scrape BEFORE the IA
    pass (Seth, 2026-10-16).** The next major phase after the
    current queue is making Fracker run on Seth's machine (tray
    + live regen + the OCR bootstrap run against his real
    inventory, incl. the live capture session). The IA /
    scannability pass is explicitly deferred until that exists —
    using the tool with real stock data is what informs the IA.
