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
34. **Arcane state source — VERIFIED GAP (Seth's direction
    2026-10-16: reconcile from data, not debriefs).** Checked
    2026-10-16 against the real payloads: (a) the public profile
    carries NO arcane ownership/rank data — its only "Arcane"
    strings are a challenge name and his Arcane Collector title;
    arcanes grant no XP, so XPInfo never sees them, and
    LoadOutInventory is equipped-gear only. (b) EE.log arcane
    mentions are resource-loader noise; MissionInfo would only
    ever show DROP-acquired copies, and Taubound arcanes are SHOP
    purchases, which no elog marker covers. CONCLUSION: neither
    clean source reconciles arcanes. Steady state = the OCR
    bootstrap extended to the Arcanes inventory screen (same
    capture class as relics/parts) in the runnable-here phase.
    Until then, Seth's debriefs are the stopgap — labeled as
    such, never silently presented as data-derived.
35. **EE.log is UNLINKED and recreated every session (Seth,
    confirmed 2026-10-16).** The live watch must NOT be a naive
    timer-poll or path-tail: the final lines of a session (the
    mission-end picks!) can be lost when the file is replaced.
    Required design: (a) watch the containing DIRECTORY for file
    events (the folder read grant is what makes this possible —
    it is the primary grant, not a fallback); (b) on recreation,
    DRAIN the old inode to EOF via its still-open FD before
    letting go, then open the new file; (c) persist (inode,
    offset) and reconcile on startup by processing any
    unprocessed tail. Polling may exist only as a fallback with
    inode-change detection.
36. **Dispatch needs no separate go-ahead (Seth, 2026-10-16).**
    Once a pass brief is written and presented, Graf dispatches
    the worker directly — no approval tap per pass. Steering
    still lands as addendum items; passes still freeze at
    dispatch (#24).
37. **Final-product behavior: resident + auto-report (Seth,
    2026-10-16).** Fracker stays running (tray-resident). When a
    NEW EE.log appears (new game session), that detection alone
    triggers a fresh report run automatically — the plan page
    is already current when Seth sits down, no manual step.
    This is in addition to mission-end regenerations during the
    session (item 35 watch) and the manual Regenerate verb.
38. **First live-run fixes (Seth, 2026-10-16 evening, v0.2.0).**
    (a) "Open plan page" does nothing — the OpenURI portal call
    from the sandbox silently fails and the xdg-open fallback
    does not fire either. The report itself generates fine
    (~/.var/app/<id>/data/fracker/plan.html). Fix the open path
    and make failures visible (status line / notification),
    never silent.
    (b) Daily-cap labels must be player-facing. The renderer
    passes the profile DailyAffiliation* suffixes through raw:
    the bare pool shows as just "Standing" (it is the main six
    syndicates shared pool) and "Library" is Cephalon Simaris
    under his internal faction name. Add a data-owned label
    map: main pool -> "Syndicates", Library -> "Simaris", and
    audit every other family suffix the same way.
39. **Capture Inventory is a tray flow, not files (Seth,
    2026-10-16).** Seth rejected the screenshot-folder + CLI
    ingest UX after one use. Final-product flow: "Capture
    Inventory" verb in the tray; he then simply scrolls his
    inventory in-game while Fracker watches the screen (XDG
    ScreenCast portal + PipeWire, one consent), classifies
    which inventory screen he is on (relics / parts / arcanes),
    and collects stacks as they scroll past, deduped by stack
    identity. The CLI --ocr-ingest path remains as a debug /
    calibration tool only.
40. **Live-test round 2 (Seth, 2026-10-17, v0.3.0).** (a) Capture
    Inventory: consent granted, then nothing — no state change,
    no feedback. The capture lifecycle must be a visible state
    machine in the tray (consent -> connecting -> capturing with
    live counts -> error WITH the reason), and logged to a file.
    (b) On a new EE.log the plan should not only regenerate, it
    should OPEN (his expectation at game start). Opening must
    land in his browser with no app-chooser dialog, every time.
    (c) Standing math is wrong live: the profile field
    DailyAffiliation IS the remaining amount (7823 tonight,
    matching his in-game readout) yet the app showed 0 left for
    Syndicates. (d) Alerts he completed this session still show
    undone after regeneration despite the resident watching
    since session start — log-derived completions must reach
    the report the resident generates.
41. **The plan is served, not a file (Seth, 2026-10-18).**
    localStorage under file:// is browser-dependent and
    effectively broken (opaque origins in Chrome, partitioned
    storage in Firefox), so tick persistence cannot live
    there, and the file handoff is what produced the
    every-time app chooser. The resident serves the plan from
    a tiny loopback HTTP server in-process: stable origin,
    ticks POST to the server and persist in app data,
    opening is a plain http:// URL in the default browser.
42. **IA pass — Seth's design (2026-10-18).** Seth authored the
    target IA himself in a worked HTML file (his session-plan
    export, iterated by hand). Decisions: FOUR tabs — First
    Crack / Hunt / Backlog / **Routine** (new). First Crack is
    action-only: event alerts, crack queue, Varzia strip, pack
    strip — no standing bars, no weeklies. Routine owns the
    daily/weekly surface: standing + focus as compact numbers
    with slim bars, sortie, the weeklies board. Slim header:
    one row, brand + status pills (Baro / Varzia / Targets) +
    one-line stamp. Density down everywhere: 5px bars, tighter
    spacing, less boilerplate per card. His file is the spec;
    the renderer conforms to it (pass 10).
43. **Auto-open discipline (Seth, 2026-10-18, v0.4.0 test).**
    The plan opens ONLY at game-session start (new EE.log
    with login) — never at app start — and exactly ONCE per
    session. Warframe startup churns the log file (create /
    recreate bursts); opens must be debounced/coalesced to
    the settled session, not fired per file event.
44. **Standing is daily-reset aware (Seth, 2026-10-18).**
    Daily pools (DailyAffiliation* + DailyFocus) reset at
    00:00 UTC. A profile/cache fetched before the most recent
    reset renders those pools as reset (remaining = cap)
    until a fresh post-reset fetch lands; a fresh fetch
    always wins verbatim (item 40c).
45. **Alert strikes are instance-strict (Seth, 2026-10-18).**
    Three live Road To Tau alerts rendered struck from
    YESTERDAY's same-event completions via the tag/reward/
    type corroboration paths. A completion can only strike
    the alert instance it belongs to: non-ID matches must
    also satisfy an activation-time guard (completion
    observed at/after the alert's activation). Yesterday's
    runs must never strike today's alerts.
46. **Refresh must not reset the tab (Seth nit, 2026-10-18;
    held for the round after pass 12).** The 60s meta-refresh
    reloads the page and the CSS-radio tab state resets to
    First Crack, kicking him out of Hunt/Backlog/Routine.
    Since the page is served by the app (item 41), persist
    the active tab server-side or via cookie so a refresh
    lands back on the tab he was viewing.
47. **Capture: libpipewire symbol skew (Seth, 2026-10-18,
    v0.4.0 test).** The FD fix works live (consent -> FD ->
    connecting); the consumer then dies at connect:
    "libpipewire missing symbol pw_thread_loop_get_context"
    (as rendered). The dlopen FFI resolves symbols by name
    at runtime, so a wrong/hallucinated symbol name compiles
    clean and only fails on a real desktop. Fix: audit every
    dlsym name in the capture code against the real PipeWire
    public headers, correct the construction sequence, and
    pin the required-symbol list in a test.
48. **Tray Quit does not quit (Seth, 2026-10-19).** The
    tray menu's Quit item does not terminate the resident.
    Diagnose why the activation never reaches the event
    loop's exit (or exits and is restarted/held by the
    single-instance machinery). Queued 2026-10-19 at Seth's
    request — do not build until he releases the round.
49. **Capture stream connects targetless (Seth, 2026-10-19,
    v0.4.2 test).** Full chain now works — consent, FD,
    symbols, connect — then the stream errors "target not
    found" ~30ms in: pw_stream_connect was never given the
    node_id the ScreenCast Start reply carries in its
    streams array, so AUTOCONNECT had no valid target on the
    portal remote. Fix: parse the node_id from Start's
    streams and connect the stream to it. Log also shows the
    state machine declaring "capturing" before any frame
    landed and flipping through a stale capturing stage on
    the error — capturing starts at the first real frame,
    and an error supersedes. Queued with #48; build when
    Seth releases the round.
50. **Sortie placement + weeklies data truth (Seth,
    2026-10-19).** (a) The Sortie row belongs with the
    cadence checklist in Routine, not in the Today standing
    block — queued for the next round. (b) "Yonta Forma
    3/wk" was fabricated data (WEEKLIES.md SS19 invented it;
    wiki-verified 2026-10-19: Yonta has no Forma and no
    weekly ware). Removed from data/weeklies.toml +
    WEEKLIES.md by Graf same day. Follow-up queued: audit
    the remaining WEEKLIES entries against the wiki — one
    confident fabrication indicts the research pass, not
    just the entry.
51. **Worldstate freshness is judged by the DATA, not the
    fetch (Seth, 2026-10-19).** His regen kept showing a
    stale alert generation while the game had new ones.
    Verified from Graf's side: warframestat.us's payload
    timestamp was frozen at 03:29:40Z (75 min old, listing
    an alert 74 min past expiry) while DE's own feed
    (api.warframe.com/cdn/worldState.php) was live with the
    current alerts. The app calls any HTTP-200 worldstate
    "live" regardless of payload age — that is the defect.
    Fix, queued: (a) payload-age validation — stale payload
    marks the plan's worldstate STALE in the stamp, never
    silently fresh; (b) add DE's cdn worldState.php as a
    second source and prefer the fresher payload (at least
    for alerts, where staleness bites). Also corrected same
    day: 1999 Calendar is 1 season/wk, not 1 page/wk
    (data + WEEKLIES.md).
    Audit (Graf, 2026-10-19, live-fetched all three): DE
    Time was 69 SECONDS old; oracle.browse.wf alerts were
    byte-identical to DE (fresh proxy) but its min.json
    OMITS PrimeVaultTraders (Varzia) and SeasonInfo
    (Nightwave), so it cannot be the sole source.
    warframestat was still frozen at 03:29:40Z (~90 min).
    DE carries every section the app consumes (Alerts,
    ActiveMissions w/ VoidT tier in Modifier, Events/Goals,
    PrimeVaultTraders, VoidTraders, Sorties, LiteSorties,
    SeasonInfo) plus unused extras (Descents,
    KnownCalendarSeasons, Conquests). Migration cost is a
    DE-schema parser + static SolNode/MT_ mapping tables;
    bonus: EE.log and DE both speak SolNode keys, making
    alert matching exact. Direction for the round: DE
    primary, warframestat fallback at most.
    DIRECTION UPDATE (Seth, 2026-10-19): NO fallback chain
    at all — DE only; a failed fetch shows as a failure,
    never a silent substitution. Decodes are BAKED IN from
    official sources at data-refresh time, never fetched
    by the app at runtime. Official source verified by
    Graf 2026-10-19: DE Public Export
    (origin.warframe.com/PublicExport/index_en.txt.lzma;
    16 exports incl. ExportRegions, ExportRelicArcane,
    ExportUpgrades, ExportSortieRewards). Gaps Public
    Export may not cover (MT_* names, Nightwave challenge
    titles) to come from browse.wf's vended dicts — Seth
    offered to supply those URLs.
    Decode coverage (Seth, 2026-10-19): ExportRegions
    carries the MT_* mission-type enum; ExportSortie-
    Rewards contains the Nightwave decode inside it.
    Public Export therefore covers every decode the DE
    migration needs — no community dictionaries required.
    Bake-time note (Graf): the export payloads 403 to
    scripted fetches from every egress tried (index .lzma
    serves fine); pull them via browse.wf's cached copies
    or a real browser session when baking.
    CORRECTION (Seth + Graf verification, 2026-10-19):
    payloads live at http://content.warframe.com/
    PublicExport/Manifest/<ExportFile>!<hash-from-index>
    — they fetch cleanly (Regions 49 KB, SortieRewards
    79 KB, HTTP 200 from Graf's VM). The earlier 403s were
    Graf's wrong URL (origin host, no /Manifest/), NOT CDN
    fingerprinting. Verified contents: ExportRegions =
    269 nodes {uniqueName SolNode*, name, systemName,
    missionIndex (the MT enum as an index — crosswalk
    derivable from known nodes), factionIndex, levels};
    the SortieRewards file bundles ExportNightwave
    alongside ExportSortieRewards/Railjack/Intrinsics.
52. **Tab persistence is scoped to ONE generation (Seth,
    2026-10-19, v0.5.0 test).** The stored tab choice must
    survive only a plan's own 60s refreshes; a NEW
    regeneration resets to the starting tab (First Crack).
    Today the choice survives regens, so the plan can open
    on a stale tab.
53. **Tray Quit STILL broken in v0.5.0 (Seth, 2026-10-19)** —
    pass 14's teardown + explicit exit changed nothing live,
    so the patched path is likely not the one the running
    resident takes, or the activation never reaches it. The
    next attempt instruments FIRST (log on Quit activation
    receipt) and gates on a D-Bus-level test: resident
    under dbus-run-session, Quit activated via dbusmenu,
    process exit asserted. A logic test is not evidence.
54. **Capture STILL "target not found" in v0.5.0 (Seth's log,
    2026-10-19 22:14)** — with node 135 now passed (§3
    landed). States 0->1->2, then 2->-1 ~5ms in. Leading
    hypothesis: the portal node is not yet registered on
    the remote when connect is attempted — known-good
    clients wait for the node global. The next diagnostic
    pass logs the raw Start streams payload + registry
    globals and waits for the node before connecting. The
    error line also still mislabels the stage "capturing"
    before any frame arrived.
55. **Capture source guidance (Seth, 2026-10-19).** The
    portal dialog offers every source type because the app
    doesn't constrain them. It should request WINDOW |
    MONITOR with the cursor hidden, and the flow should
    steer Seth to pick the Warframe WINDOW: window frames
    are the pure game surface (no overlapping windows,
    notifications, or extra monitors) at the 2560x1440
    geometry the layouts were tuned on — materially better
    OCR input than a full-screen share. (For the connect
    itself the choice is neutral; it's an OCR-quality and
    robustness call.)
    Key evidence (Seth, 2026-10-19 22:47): Quit DOES stop
    operations — menu items go dead, EE.log scraping halts —
    but the process and tray icon remain. So the shutdown
    sequence starts (watch released, loop ends) and then
    hangs before process exit; the icon outlives its menu.
    Instrument each teardown step; the gate stays the
    D-Bus-level exit assertion.
56. **Review follow-ups (GLM review of passes 14-15,
    2026-10-20; Seth approved the round same day).**
    (a) on_param_changed must read the SPA pod's own size,
    not a fixed 256 bytes (screencast.rs:1497). (b) Visible
    loss, never silent: unparseable alert timestamps must
    not fail the activation guard open, unparseable fissure
    expiries must not silently drop fissures, renamed or
    missing section fields must surface via the gaps
    channel (decode_tier is the model). (c) Capture Drop
    takes the thread-loop lock before disconnecting.
    (d) Golden guard: goldens assert their "now" matches
    the committed fixture's Time, so fixture regeneration
    cannot silently drift them. Recorded, NOT scheduled:
    the unbounded frame channel and tray_quit.rs's hard
    dbus-daemon requirement.
57. **Version in the tray menu (Seth, 2026-10-21).** A
    disabled menu item showing the running build
    ("Fracker v0.5.2") in the tray menu. Source of truth:
    the crate version — release merges bump Cargo.toml's
    version in step with the tag so the menu can never
    lie about what's installed.
58. **Push plan updates over WebSocket (Seth, 2026-10-21).**
    The plan server (loopback, :8791) should push a new
    generation to open pages the moment a regen lands,
    instead of pages discovering it at the 60s meta-
    refresh. Reload-on-push is safe: ticks live server-
    side and tab scope is generation-aware (item 52), so
    a pushed reload lands on First Crack like a manual
    one. Keep the meta-refresh as the backstop for a
    dropped socket (with reconnect), not as the primary
    mechanism.
59. **One completion struck ALL FOUR event alerts (Seth,
    2026-10-21, v0.5.2).** He ran one Road To Tau alert;
    every sibling rendered struck "log". Mechanism (Graf,
    src/elog.rs matches_alert): corroboration is an OR of
    tag+SolNode, tag+mission-type, tag+shared-payout. The
    payout path treats any shared counted-item uniqueName
    as identity evidence — but all Road To Tau alerts pay
    the SAME event resource (50 Moonflesh), so one
    completion's reward list matches every sibling. The
    pass-12 activation guard doesn't help: all four were
    live before he ran his. Fix rule: when both the
    completion and the alert carry a SolNode, ONLY the
    node (or the exact id) corroborates; the type/reward
    paths are legacy fallbacks for node-less data only.
    Regression test = the screenshot: four same-tag
    siblings with distinct nodes/types and a shared
    currency reward, one completion -> exactly one struck.
60. **Capture round 3 (Seth's log, 2026-10-21, v0.5.2).**
    Consent, window pick, and node selection all worked
    (Start selected node 151, source_type 2 = window);
    the run then died at "connecting": **libpipewire
    missing symbol pw_core_get_registry** — the pass-15
    registry step never ran, so the registration-race
    question remains OPEN (v0.5.0's target-not-found at
    connect is still unexplained by it). Lessons:
    (a) The symbol audits validated names against HEADERS;
    for a dlopen FFI, header-declared is not the same as
    library-exported — the installed .so's symbol table
    is the only ground truth, and this name isn't in
    Seth's. Obtain the registry through an exported
    mechanism, or drop the registry dependency and retry
    the connect itself on target-not-found.
    (b) META-FIX: resolve EVERY capture symbol up front,
    before the portal flow starts, and report ALL missing
    names in one error — three live runs have each died
    on a different stage because discovery is serialized.
    (c) Question for the code: the Start reply CARRIES a
    restore_token, yet the log says "restore_token
    unchanged" on a first run — if the reply's token is
    not persisted, every capture demands full consent,
    against the pass-9 design.
    RESOLVED (Seth, 2026-10-21, v0.5.2): Quit confirmed
    working live. The pass-15 restructure (reports as
    tasks, bounded teardown, D-Bus-gated test) closed
    items 48/53 after two earlier attempts missed.
    CONFIRMED (Seth, 2026-10-21): a fresh run asked for
    full consent again despite the portal dialog's
    "remember for future sessions" being checked. The
    portal issues the token (it is in the Start reply);
    the app never persists or replays it. Defect, not a
    question: capture the Start reply's restore_token
    into the app-data store, pass it with persist_mode
    on later SelectSources calls, and log only its last
    few characters — the pass-15 raw-reply logging
    currently prints the whole token.
61. **HOTFIX: plan page reload storm (Seth, live on
    v0.5.4, 2026-10-21).** The page reloads every few
    milliseconds — flickering, re-fetching every icon
    until the Warframe image CDN rate-limited his IP.
    Browser Stop pauses it briefly. Root area: pass 18's
    WebSocket connect path — the server names the current
    generation on connect and the client's reload
    predicate can never settle (or fires on any message),
    so every load self-reloads. Pass 19: fix the
    predicate (reload only on a genuinely different
    generation; hello with equal generation is a no-op)
    AND fail-safe the client independent of server
    behavior — minimum interval between push reloads plus
    a circuit breaker that drops to meta-refresh-only
    after repeated push reloads in a window. Graf's miss:
    the diff review praised the reconnect-hello detail
    without interrogating the comparison semantics.
62. **POLISH, DEFERRED (Seth, 2026-10-21): in-place plan
    updates instead of reload-on-push.** The WebSocket
    push currently reloads the whole page. Nothing
    fundamental requires it — reload was the cheap
    mechanism because the renderer emits whole documents
    and server-side ticks made reload safe. The polish:
    push the newly rendered plan fragment and swap it
    into the page in place (ticks are links/forms or can
    be event-delegated, so no rebinding problem), keeping
    scroll position and avoiding the reload flash. Small
    scope; Seth explicitly deferred it — do not build
    until released.
63. **CAPTURE ROOT CAUSE FOUND (Graf, 2026-10-21, from
    Seth's v0.5.4 log).** The token and symbol fixes both
    verified live (token issued/redacted/persisted; no
    symbol died). The connect then failed "target not
    found" deterministically across all 20 retries —
    killing the registration-race theory for good. The
    defect is in src/screencast.rs: `PW_DIRECTION_INPUT`
    is hand-defined as **1**, but PipeWire's headers say
    SPA_DIRECTION_INPUT = **0** and OUTPUT = 1
    (spa/utils/defs.h:95-96; pw_stream_connect takes
    enum pw_direction, stream.h:547 — verified against
    the upstream repo). Every capture since pass 9 has
    connected an OUTPUT (producer) stream and asked the
    session manager to link it INTO the portal's source
    node, which has no input ports — "target not found",
    instantly and forever. It survived every audit
    because audits verified symbol NAMES and pod shapes;
    an enum VALUE in our own source was tested by
    nothing. Fix = the constant (1 -> 0), plus a full
    audit of every hand-defined FFI enum/flag value in
    the capture module against the real headers, and
    correcting the pass-9 doc comment whose private-
    remote theory rationalized the failure. Ships as
    pass 20 (v0.5.6) right behind the WS hotfix.
64. **Two alerts struck at fresh launch, fresh EE.log
    (Seth, 2026-10-22, v0.5.6).** Mechanism (Graf, code
    read): the resident seeds its completion ledger from
    persisted state at startup, retaining records whose
    UTC day is today (resident.rs) — a fresh EE.log does
    not zero it, by design (restart continuity). Seth's
    evening session and his after-midnight session share
    one UTC day, so the earlier session's completions
    re-strike at launch. The wrongful second strike: the
    pass-17 rule (Graf's brief) lets a SolNode ALONE
    corroborate within a tag — a same-node sibling alert
    (same tag, same node, DIFFERENT mission type) is
    struck by its sibling's completion. Both sides carry
    mission type; corroboration must require node AND
    type when both are present, falling back to node-only
    only when type is absent on a side. Regression test:
    same-tag, same-node, different-type siblings, one
    completion -> exactly one struck.
65. **Capture round 5 (Seth's log, 2026-10-22, v0.5.6).**
    Token replay CONFIRMED (silent Start, ~20 ms, no
    picker) and the connect CONFIRMED (node 74, attempt
    1 — direction fix live). New failure one stage
    deeper, +40 ms into negotiation: state 2 -> -1
    "no more input formats". Two gaps verified absent in
    src/screencast.rs: (a) the FFI never declares
    pw_stream_update_params and on_param_changed only
    parses/stores the format — the app never CONFIRMS a
    chosen format back to the node; (b) the offered
    EnumFormat pod carries no modifier property, which
    KWin's screencast node (KPipeWire) may require for
    any intersection. Fix both: complete the negotiation
    (confirm the format; pw_stream_update_params is a
    standard exported symbol — add it to the FFI +
    manifest) and enumerate modifiers in the offer.
    Seth's refinement (2026-10-22): event alerts rerun
    on a small handful of nodes all day, so node+type
    still is not instance identity across REPEATED
    instances — the time dimension is the activation
    guard's (pass 12): a completion corroborates only
    at/after the alert's activation. Full predicate =
    tag AND node AND type AND activation window; the
    rerun regression test (same node+type, completion
    observed before activation -> NOT struck) rides
    with the sibling test.
66. **"Vanguard" relics (Seth, 2026-10-22 — QUEUED, not
    this session).** Some relics display with the era
    label "Vanguard", which does not exist. Seth's
    taxonomy: the relic eras are Lith / Meso / Neo / Axi
    (plus Requiem/Omnia tiers; Kuva relics exist but
    carry no prime parts). Source found by Graf: the
    baked relic-tables cache (data/cache/relictables.
    json, WFCD-derived) names four relics "Vanguard C1 /
    E1 / M1 / P1" — mixed pools of Caliban / Protea /
    Ember / Mesa / Ash / Volt Prime parts, flagged
    vaulted, no sources. They appear tied to The Perita
    Rebellion activity drops (whose mission set includes
    "Recall: Vanguard"). Impact beyond the label: era is
    the relic name's first token, so "Vanguard" stock
    can never match a fissure era or a crack plan, and
    the names will never match OCR text from a real
    inventory screen. Future session: establish the true
    in-game names/eras for these four (DE drop tables /
    Seth's inventory), then remap or drop the pseudo-era
    where the cache is baked.
    RESOLVED by Seth (2026-10-22): they are legacy AXI
    relics that do not currently drop (a Varzia return
    is possible later). Fix: map the four to Axi in the
    data pipeline — era, fissure matching and display
    all become Axi; their unobtainable status is item
    68's concern, not the era label's.
67. **Chance text is dead weight in relic reward rows
    (Seth, 2026-10-22).** Within a rarity band every
    item carries the same chance (commons 25.33% Intact,
    etc.), so per-item strings like "Int 25.33% ~4" are
    noise repeated down the slate. Slim the display to
    the refinement label alone (Int / Rad / etc.) — the
    item's rarity is already carried by its color.
68. **Relic source priority + unobtainable treatment
    (Seth, 2026-10-22).** The part->relic pick already
    chooses the lowest rarity across relics (correct,
    keep). Verify end-to-end — and extend where missing —
    this source priority: (1) owned relics, (2)
    unvaulted (currently dropping), (3) vaulted but
    available (Varzia / Prime Resurgence), (4) currently
    unobtainable (not dropping, not at Varzia) — tier 4
    gets a NEW visual treatment (likely strikethrough).
    Downrank items that are even partially unobtainable
    (their viable sources include tier-4 relics) in
    target/backlog ordering.
69. **Capture: external review triage (Claude via Seth,
    2026-10-22; every claim verified by Graf against
    master before adoption).**
    (a) CONFIRMED, the headline: pod::choice wraps its
    children in a Struct pod — a real SPA Choice body is
    {kind, flags, child header, RAW values}. Every choice
    in the EnumFormat offer (format, size, framerate,
    modifier) is malformed; the pinning test enshrines
    the wrong layout. Sufficient explanation for "no
    more input formats" in every build so far.
    (b) CONFIRMED: negotiate() drops its zbus Connection
    on return — Negotiated keeps only fd/node/token, so
    the portal session is orphaned at consent. Keep the
    Connection + session path alive until capture ends;
    Session.Close at teardown.
    (c) CONFIRMED mechanism: nothing in the module ever
    mmaps buffer memory, so SpaData.data is null for
    every portal buffer (MemFd and DmaBuf alike) and
    convert_buffer can never have produced a frame. The
    fix is buffer mapping (with SpaData.fd corrected to
    i64 — currently i32, harmless only by alignment
    luck); the modifier stance follows from it.
    (d) SUBSTANTIALLY RIGHT: the pass-21 Format echo via
    pw_stream_update_params is not the canonical accept
    flow (consumers answer with Buffers/Meta params) —
    it was Graf's brief, and it goes. Not implicated in
    any live failure (v0.5.8 never ran capture live).
    (e) Riders, all verified: pw_stream_new properties
    are NULL (pass OBS-style media.type/category/role);
    frame width/height derive from stride (wrong under
    padding — use the negotiated format); the framerate
    offer is a pinned 30/1 fraction, not a range from
    0/1; the frame channel is unbounded (drop-oldest
    bounded queue instead). Ships as pass 23 (v0.5.10).
70. **Capture SOLVED at the protocol layer (2026-10-22,
    Seth's v0.5.12 diagnostic run).** The link existed
    all along (pw-link: kwin_wayland:output_1 ->
    fracker.bin:input_1) and KWin WAS proposing fixed
    Format pods — parse_video_info rejected every one:
    libspa wraps each value of a fixed Format in a
    Choice pod (kind None, sometimes several
    near-duplicate values), while the parser only
    accepted bare scalar pods — the same self-consistent
    fiction as the old choice test, one layer up. The
    captured 328-byte KWin pod (2560x1440, BGRx, modifier
    INVALID) is now a regression fixture in the test
    suite. Fix: the parser unwraps Choice values to
    their first entry. Handshake completion: the
    consumer now answers a parsed Format with
    SPA_PARAM_Buffers via pw_stream_update_params
    (buffers/blocks/size/stride/dataType from the
    negotiated geometry) — the canonical accept whose
    absence left KWin cycling proposals and io_changed
    id=1 forever. Item 69d refined, not reversed: the
    Format ECHO stays out; Buffers is the answer.
    io_changed logging is transition-only now. Ships
    as v0.5.13.
71. **Standing shows pre-reset values until the next
    regeneration trigger (Seth, 2026-10-22).** Root
    cause (Graf, code-verified): the resident
    regenerates the plan ONLY on SessionStart /
    EE.log FileChanged / MissionEnd / Manual triggers —
    there is no periodic and no reset-boundary trigger.
    Across the 00:00 UTC reset, an idle app keeps
    rendering the last pre-reset snapshot (his syndicate
    pool at 0 remaining, Necraloid at 1k) until he plays
    or regenerates by hand. Expected post-reset values
    were 19,586 syndicate / 31k Necraloid. Fix shape: a
    reset-boundary regeneration trigger in the resident
    loop. Secondary defect in the same area: the live
    fetch path stamps fetched_at = now even when
    fetch_profile served its any-age CacheFallback, so
    profile_context's reset rule can never fire on that
    path — fetch_profile should surface the snapshot's
    true fetch time (cache mtime), as profile_fallback
    already does. QUEUED by Seth; not yet released.
    Correction (Seth, same night): the launch WAS fresh
    — a SessionStart regeneration happened post-reset
    and the plan still rendered 0. The missing-trigger
    theory is dead; the SNAPSHOT itself is stale. The
    fork is now: (a) our cache chain served a pre-reset
    file as current, or (b) DE's profile endpoint itself
    lags the reset and keeps returning pre-reset Daily
    values. Discriminators: the page's profile status
    line, the cache file's mtime + stored Daily values,
    and a direct curl of the endpoint. Still queued.
    RESOLVED (same night, hard evidence): the plan's
    status line reads "profile live", generated
    2026-10-23 02:48 UTC; the cache file (written by a
    LIVE fetch at 02:39 UTC) contains yesterday's
    document byte-for-byte (DailyAffiliation 0,
    Necraloid 1000, DailyFocus 232630, Ventkids 30994 —
    the 6-point spend is yesterday's fingerprint),
    while a direct curl at 03:04 UTC returns current
    values (19586 / 31000 / 387278). DE's endpoint
    served a pre-reset replica for ~3h after the reset
    and rolled over between 02:48 and 03:04 UTC. The
    app chain is exonerated — it fetched live and
    rendered faithfully. The app-side defect that
    remains: profile_context judges freshness by OUR
    fetch clock, but the endpoint's data has its own
    age. Fix shape (same queued item): treat a
    post-reset snapshot whose DailyCaps are identical
    to the last pre-reset snapshot's as pre-reset
    (render full caps) — content age, not fetch age.
72. **Crack queue errata (Seth, 2026-10-22, with
    screenshot).** (a) The NEXT row's accent
    misaligns: `.ch.next` adds `padding-left:10px`
    (render.rs style block) that plain `.ch` rows
    lack, shifting its content instead of compensating
    — keep the content box uniform; the inset shadow
    + gradient carry the highlight. (b) Caliban Prime
    renders in the queue struck-through (relics
    vaulted, none owned) — a frame with zero parts
    obtainable from owned relics must not enter the
    crack queue at all. (c) Part names need the rarity
    coloring the Hunt page already has (`rarity_css`,
    render.rs) — the queue renderer doesn't apply it.
    (d) Remove the time estimates and the "for 4"
    framing everywhere in the queue (header "stay for
    4", row "~32m for 4", the 400 tr prep framing):
    the plan's EV math is not a commitment to prep
    four relics. QUEUED by Seth ("next in queue").
    Seth's shape (2026-10-22, supersedes the fingerprint
    as the primary fix): the rule is about the snapshot's
    DATE, not its content. A snapshot dated pre-reset
    must be ignored post-reset. The current live path
    violates this by construction: fetch_profile's TTL
    branch discards the snapshot's true age and
    report.rs stamps fetched_at = now, so a document
    fetched 23:50 and served from the 30-min cache at
    00:10 renders pre-reset values as current — wrong
    for 20 minutes in the NORMAL case, no endpoint lag
    required. Fix: (1) fetch_profile surfaces the true
    fetch time (network = now, cache hit = file mtime)
    and report.rs passes it through (profile_fallback
    already does); (2) the profile TTL clamps at the
    reset boundary — a cache entry never counts as
    fresh across 00:00 UTC, so the first post-reset
    generate refetches instead of coasting. With true
    dating, the existing profile_context reset rule
    does the rest. Named limit: tonight's incident (a
    live post-reset fetch returning pre-reset DATA)
    is not covered by dating alone — Seth's call
    whether the content check joins it.
    Final shape (Seth, 2026-10-22): drop the app's
    30-minute profile cache entirely — DE already
    caches the profile for ~5 minutes (Cache-Control:
    public, max-age=276, observed live); use theirs.
    fetch_profile always fetches; the disk file remains
    only as the failure fallback, dated by its mtime
    (the fallback path already dates it truthfully).
    With no app-side TTL there is nothing to clamp, and
    the reset rule keys on true dates everywhere.
    Scope: the profile only — worldstate and static
    caches are separate designs, untouched.
73. **Varzia timer copy (Seth, 2026-10-22):** the
    resurgence pill reads "IN 6D LEFT". plan.rs builds
    ends_in WITH the prefix ("in 6d", plan.rs's
    countdown formatter) because the header pill wants
    "N primes · in 6d"; the resurgence panel
    (render.rs) appends " left" to the same value, and
    the goal footer ("In Varzia · {ends} left") does
    it too. Fix: the "…left" sites use a bare duration
    ("6d left"); the header keeps "in 6d". QUEUED with
    #72.
74. **Blueprint sorts first on goal cards (Seth,
    2026-10-22).** Part names on a goal card are
    sorted plain-alphabetically (plan.rs's goal-parts
    builder: `names.sort()`), so "Kestrel Prime
    Blueprint" lands between Blade and Grip. Warframe
    convention — and Seth's ask: the item's own
    blueprint ("<Gear> Blueprint") always heads the
    list; the rest stay alphabetical. (Component
    blueprints — Chassis Blueprint etc. — are not the
    item blueprint.) QUEUED with #72/#73.
75. **Capture, round 3 (Seth's v0.5.13 live log,
    2026-10-22).** The Choice-unwrap worked — formats
    parse and get answered — but KWin re-proposed the
    Format ~500 times (modifier INVALID, then LINEAR)
    and never allocated a buffer; the stream never
    left PAUSED. Two canonical consumer steps were
    still missing, both verified against OBS's
    linux-pipewire source: (1) OBS's SPA_PARAM_Buffers
    object carries ONLY dataType (bare Int) — count,
    size and stride derive from the Format; v0.5.13
    sent buffers=0/blocks/size/stride, and a literal
    zero buffer count can never allocate. (2) OBS
    calls pw_stream_set_active(stream, true) right
    after connect; we never activated the stream at
    all. v0.5.14: Buffers = dataType only,
    pw_stream_set_active after connect (new FFI
    symbol), and the negotiated-format line logs only
    on change.
76. **The Varzia inference is wrong (Seth, 2026-10-22;
    supersedes #68's rarest-reward rule).** "Velox
    Prime is obtainable, therefore Axi V12 is
    obtainable" is false: Axi V12 is vaulted, and this
    rotation's Velox-Barrel relic is Meso V16. The
    rotation sells a FIXED SET of relics — the
    manifest's Void Projection items (T1-T4, Vault
    A/B; six this loop) — not every relic that happens
    to contain a featured part. Content overlap is not
    availability. Verified in data: the tables mark
    even Meso V16 vaulted with no sources (Resurgence
    availability lives nowhere in the drop data), and
    the manifest names the sold relics only as
    projection SKUs, never by relic name. The correct
    fix needs a projection -> relic-name mapping from
    an items source (candidates: the items dataset's
    per-relic uniqueNames, warframestat, the wiki's
    resurgence list) — until then the rarest-reward
    promotion must not stand as-is. Seth's call on the
    interim: disable the inference, or keep it until
    the mapping lands. QUEUED.
    Resolution (Graf, same night): the mapping was
    already in the app's own data. item-names.json
    resolves projection uniqueNames to relic names —
    the manifest's "/Lotus/StoreItems/Types/Game/
    Projections/T2VoidProjectionProteaIvaraVaultB
    Bronze" is the item-names key "/Lotus/Types/Game/
    Projections/T2VoidProjectionProteaIvaraVaultB
    Bronze" -> "Meso V16 Intact" (Seth's example,
    exactly). This rotation's sold set resolves to
    Lith B12, Meso I3, Meso V16, Neo P11, Neo O4,
    Axi A23. Fix shape: VarziaAvailability resolves
    manifest projections through item-names (match on
    the path tail, any refinement variant, strip the
    refinement word) and the #68 inference (direct-name
    + rarest-reward clauses) is deleted outright.
77. **Full per-item part resolution (Seth's design,
    2026-10-22; LOGGED, DO NOT BUILD YET — waits for
    faithful relic data (the #76 fix) and Seth's
    worked example cases).** For a goal item (Protea
    Prime as the example):
    1. Parts already owned are filtered out. Per
       remaining part:
    2. Owned relics for the part are collected. If
       none, continue down the chain.
    3. Part vaulted and NOT in Varzia: owned relics
       are the ONLY current source — flag them
       LIMITED; run these relics only as a group,
       never burn them casually/solo.
    4. Otherwise find the relic(s) where the part
       sits at the LOWEST rarity (Protea Prime
       Blueprint is Common in Meso V1, Neo C6,
       Meso K7, Meso W4, Axi N13, Lith W4, Meso V14).
    5. Among those, match the most desirable farming
       mission: A = capture / rescue / survival;
       B = exterminate; C = conjunction survival;
       D = mobile defense / defense. Show a relic
       from the best available tier; all relics of a
       given class have relatively equivalent value.
    6. ADVANCED (later): weigh how many OTHER needed
       parts the relic also contains (Meso V14 covers
       two other needed items) — "this gets crazy
       fast"; needs worked examples before building.
78. **Capture round 4 (v0.5.15, from Seth's v0.5.14
    live log, 2026-10-22).** v0.5.14 changed the
    shape — two Format proposals, then quiet, instead
    of ~500 — but still no add_buffer, no frame.
    Diffing our answer against OBS's linux-pipewire
    param_changed handler (fetched live): OBS answers
    a fixed Format with FOUR params in ONE
    update_params call — SPA_PARAM_Meta VideoCrop
    (size 16), SPA_PARAM_Meta Cursor (size = choice
    range 16432/52/4194352), SPA_PARAM_Buffers
    (dataType bare Int; MemPtr, plus DmaBuf iff the
    Format carries a modifier), SPA_PARAM_Meta Header
    (size 32). We sent the Buffers pod alone. v0.5.15
    sends the canonical four; every constant
    header-verified (spa/param/buffers.h spa_param_meta
    enum; spa/buffer/meta.h meta types + struct sizes;
    type ids ParamMeta 0x40005 / SPA_PARAM_Meta 6).
    Success signature unchanged: negotiated-format
    line, then add_buffer, then first frame.
79. **Capture round 5 (v0.5.17, from Seth's v0.5.15/16
    live log, 2026-10-23).** The full OBS param set
    changed nothing: two modifier proposals (INVALID,
    LINEAR), no add_buffer. Evidence gathered before
    theorizing: the captured KWin Format fixture
    decoded — its modifier prop carries flags 0 (NO
    DONT_FIXATE) with a single-value Choice(INVALID),
    so the fixation dance a review suggested is NOT
    what KWin is asking for. Two deltas shipped
    together (bisect later if it works; Seth's run
    cycles are the scarce resource): (a) the offer is
    SHM-only — the #65b DRM modifier enum is removed.
    Every stalled round negotiated on KWin's DMA-BUF
    path, and this reader cannot consume DmaBuf
    buffers anyway (no GBM/EGL path); OBS always
    includes an SHM offer alongside its DMA-BUF ones.
    dataType mask is now MemPtr|MemFd. (b)
    pw_stream_set_active moves from connect-time to
    after the first answered Format — v0.5.13 (never)
    cycled proposals forever, v0.5.14+ (at connect)
    went quiet after two; the canonical lifecycle
    activates once params are set. Plus a one-shot
    hex dump of the first Format pod. If this still
    stalls, the next instrument is the server's own
    log (PIPEWIRE_DEBUG / KWin), not more client
    theory. Parked for when frames flow: skip frames
    whose chunk size is 0 (KWin's no-change signal)
    instead of re-OCR-ing stale pixels.
80. **Capture WORKS live (v0.5.17, Seth's runs
    2026-10-23 ~02:05–02:13 EDT).** Round 5 landed:
    state 2→3, add_buffer ×3, frames drained, clean
    teardown. Seth's captures wrote real stock —
    Arcanes (75 stacks with per-rank copies), Relics
    (per-tier counts). Two live defects remain:
    (a) Prime Parts never classifies — its header
    strip OCRs to nothing the parts keywords match,
    so every frame fell to Other (finalize said
    "nothing collected", never "0 parts"). (b) Lossy
    coverage: ~700 distinct relics, slow scroll,
    152 captured = ~2 viewports of 77. Mechanism is
    in the code: read_frame spawns tesseract per
    cell (name + count ≈ 150–230 spawns per 77-cell
    screen = many seconds per frame) while the frame
    channel holds ONE frame (sync_channel(1) +
    try_send drops the rest); a continuous scroll
    outruns the reader. Step-and-hold works today
    (the channel always holds the newest frame).
    The durable fix is batched OCR: one tesseract
    TSV pass per grid, word boxes mapped to cells.
    v0.5.18 ships the evidence for both defects —
    header-OCR samples + frames seen/read/skipped
    logged at every finalize, and evidence PNGs
    (first frame, first frame per recognised class)
    under <data>/capture-debug/ — so the Parts fix
    lands as data (screens.toml keywords) read off
    the actual header text, and the batching win is
    measurable. Also in v0.5.18 (Seth, same night):
    Farm rows OUT of the crack queue — the tier-3
    farm chunks and the appended "Farm:" rows both
    removed; score_farms/FarmHint data kept for a
    future presentation. Box-suite note: the OCR
    fixture tests are multi-minute each on the
    aarch64 box (a single capture-sequence test
    exceeds 150 s alone) — gate them on CI, not by
    watching a box suite and calling it hung.
81. **Batched OCR (pass 24, Seth's brief 2026-10-24;
    implemented by Blum).** The #80(b) durable fix, in the
    shape Seth specified — NOT the TSV word-box idea #80
    sketched: per-region crops are keyed by a crop hash
    (dims + salt, pixels quantised `>> 4`), repeats are
    served from an engine-side cache, and each region
    class's cache misses go through ONE tesseract process
    (list-file mode, pages split on `\f`, model loaded
    once, `OMP_THREAD_LIMIT=1`). Semantics are pinned to
    the old per-cell path by an equivalence gate
    (batched == per-cell on text/count/rank/confidence);
    any batch mismatch falls back to per-image reads.
    The capture header classification is likewise cached
    by crop hash in the session, so a scroll step pays
    ~2 launches (new row's names + counts) instead of
    150–230 per frame. Ships in v0.5.19. Still owed:
    Seth's live measurement (frame stats at finalize,
    #80) to confirm the scroll coverage fix and decide
    whether the deferred libtesseract-via-dlopen step
    (~20 ms/cell, model resident; needs the lib in the
    Flatpak) is warranted. Defect #80(a) — Prime Parts
    header classification — is untouched by this pass;
    it waits on the v0.5.18 evidence captures.
82. **Live verdict on pass 24 (Seth, 2026-10-24, v0.5.19),
    and the missing-evidence defect.** Three captures:
    relics 18 seen / 18 read / 0 skipped -> 528 relic stacks;
    arcanes 7/7/0 -> 155 arcanes; parts 130/4/126 -> 77, then
    on a deliberately slower scroll 377/12/365 -> 140 of
    ~270 actual parts. So #80(b) is confirmed fixed at
    relic/arcane pacing, #80(a) did NOT reproduce (Parts
    classified and read; pass 24 changed no classification
    code, so (a) is unconfirmed rather than closed), and the
    residual parts loss is twofold: per-page OCR still takes
    seconds on all-unique-cell pages (coverage is
    scroll-pacing-bound), and this page's glyphs reject at
    the fuzzy matcher more than the tuned screens. New
    defect found while collecting training data: the
    evidence dumps never landed in the Flatpak. debug_dir
    pointed into the install tree (`tuning::data_dir()` =
    /app/share/fracker/data, read-only in the bundle) and the
    best-effort write swallowed the failure. Pass 25
    (v0.5.20) moves dumps to the user data dir beside
    stock.json, raises sampling to 60 frames per name
    (sequence-named), and writes PNGs off the reader thread.
    Glyph/throughput work on Parts follows once the training
    frames are in hand.
83. **The capture that ended itself (Seth, 2026-10-24,
    v0.5.20): header misclassification, root-caused from his
    own frames.** A parts capture finalised on its own at
    ~34 s (161 seen / 2 read / 159 skipped, 25 parts) with no
    tray stop. Mechanism, verified by running Seth's three
    uploaded frames through the production classifier: the
    header matcher used exact substring search, and the title
    font defeats it — "INVENTORY" read back as "VENTORY" in
    all three frames (the IN ligature drops), so Parts rode
    entirely on "SELL"; the frame whose read split it ("SEL
    L") classified Other. Consequences compounded: the #81
    header cache made the bad class sticky per header-hash,
    and the idle-stop (90 consecutive idle frames, "left the
    inventory") counted every subsequent frame toward
    auto-finalise. This also explains #80(a): under v0.5.18
    the Parts header simply never won its OCR roll. Pass 26
    (v0.5.21): (1) classify_text normalises to squashed
    alphanumerics and adds a confusable-aware fuzzy window
    match (allowance 1 edit, 2 for keywords >= 8 chars) —
    all three real reads now classify Parts, pinned as
    fixtures; (2) idle accrues only after 3 CONSECUTIVE fresh
    Other classifications, and a fresh real classification
    resets the streak — one misread can no longer end a
    capture (a perfectly static exit screen no longer
    auto-stops; animated menus/gameplay still do, and the
    tray toggle remains the primary stop); (3) finalize now
    logs header samples and the debug-frame list on EVERY
    capture, not only zero-collection ones — the partial case
    is exactly where that evidence matters.
84. **Captured part stock never reaches Hunt/Backlog
    (Seth, 2026-10-25).** Now that capture produces a real
    parts inventory, the plan ignores it: "Dual Zoren Prime
    Handle" is in stock.json's parts bucket, yet Dual Zoren
    shows no filled pips in Backlog, and Hunt gives no
    indication a part is already collected. Mechanism
    (Blum, code-verified): report.rs loads StockState and
    converts it with `to_relic_stock()` — only the relics
    bucket survives; PlanInput carries no part stock at
    all. Downstream, part_rows_for_gear derives PartState
    solely from the want list (in wants -> Need, else
    Unknown); PartState::Have exists and
    GoalCard.parts_have counts it for the Backlog pips,
    but the goal-card builder never constructs Have — the
    pips are unreachable, not merely unrendered. Fix
    shape: carry part stock (name -> count) through
    PlanInput; a part whose captured count meets its
    required count is Have (required > 1 exists —
    dual-wield components), a wanted part short of that
    stays Need; render Have distinctly in Hunt part rows
    (Need/Unknown are the only states a row can show
    today, which is why "collected" has no denotation
    there). QUEUED for the next build at Seth's
    direction; not in the current capture/OCR build.
85. **Baro pill while he is HERE (Seth, 2026-10-25).**
    With Baro active the header pill reads "BARO
    EarthHUB · in 0d". Two defects: (a) the countdown is
    donow_baro's days_until on his DEPARTURE, at day
    granularity — an active visit runs < 2 days, so its
    last day renders the useless "in 0d"; while a trader
    window is live the pill should state the leaving
    time at hour granularity ("Leaves in 18h" — the
    mins_left_label shape already exists). (b) No visual
    distinction: the pill should PULSE while Baro is
    active. The location shows the raw worldstate node
    id (EarthHUB is a pinned decode gap — the app never
    invents a name); noted, not prescribed. QUEUED for
    the next build.
86. **Tab still resets on refresh (Seth, 2026-10-25) —
    #46/#52 not holding live.** Design intent: the
    remembered tab survives a plan's own 60s refreshes
    (#46) and resets only on a NEW generation (#52,
    Seth's rule). Live, refreshes still land on First
    Crack. The build must first pin which side fails:
    (i) the generation token changes more often than "a
    new report" — worldstate-tick regenerations and the
    WebSocket push reloads (item 58) are each a new
    generation by construction and indistinguishable
    from a refresh to the reader; or (ii) scope_to
    clears on serves it should not. If (i), the #52
    rule itself needs revisiting: an unrequested regen
    is not the "new report" moment #52 aimed at (that
    was the session-start open). QUEUED for the next
    build.
87. **Crack queue NEXT accent still misaligned (Seth,
    2026-10-25, with screenshot).** The accent bar for
    the NEXT row renders offset from the row it marks
    (screenshot: Citrine Prime heading a queue at
    Adrastea (Jupiter) Steel Path; the bar sits up/left
    of the row's box). The accent is `.ch.next` in the
    render.rs style block (background gradient +
    `box-shadow: inset 3px 0 var(--cy)`) — the same
    accent #72(a) already fixed once for a different
    cause (a padding-left asymmetry, since removed).
    Root cause to be pinned against the live DOM in the
    build; the mock's alignment is the reference.
    QUEUED for the next build.
88. **Varzia supersedes the vault badge in Hunt (Seth,
    2026-10-25).** A vaulted goal that is live at Varzia
    shows BOTH the "V" badge and the "◆ In Varzia"
    footer. Rule: when a goal is resurgence_live, the
    Varzia marker replaces the V — never both.
    Mechanism (Blum, code-verified): the goal-card
    renderer emits the V badge on `g.vaulted`
    unconditionally, independently of the Varzia footer
    below it; the Backlog section split already treats
    Varzia-live as its own class — the badge never got
    the same condition. QUEUED for the next build.
89. **Built components are first-class capture stock
    (Seth, 2026-10-25; resolves the census's open
    product question).** A built component and its
    blueprint are both valid evidence of progress on
    the parent item, so capture must recognise BOTH
    forms. The Parts screen shows them as distinct
    stacks — e.g. "Gauss Prime Chassis Blueprint" and
    the built "Gauss Prime Chassis" — while the roster
    (and therefore the OCR universe built from it)
    carries only the blueprint forms, which is why the
    census found built components unvalidatable by
    construction. Rules: (1) the screen-scoped Parts
    universe in pass 27 gains the built form of every
    roster component blueprint (a part named "<Gear>
    <Component> Blueprint"; the gear's own "<Gear>
    Blueprint" has no loose built counterpart — its
    built result is the gear itself); (2) stock.json
    keeps the two forms as separate keys under their
    exact screen names — the dump stays literal, no
    folding at capture time; (3) progress consumption
    (#84) canonicalises the two forms onto the same
    roster part and SUMS their counts toward the
    required quantity (built x1 + blueprint x1 = two
    component-equivalents). The Kavasa exclusion (#22)
    is a backlog decision and is not disturbed by this
    item. Folded into pass 27's scope.
90. **Parts census resolution (pass 27, v0.5.22,
    2026-10-25).** The census replayed Seth's 16
    capture-debug frames through production code: 269
    distinct parts visible, production held 108 with 8
    false accepts and 0 of 61 Neuroptics instances.
    RESOLVED by pass 27 (docs/twenty-seventh-pass.md):
    (a) parts name band y 0.53-1.00 + PSM 11 + upscale 4
    (tuned on the Neuroptics subset through the
    production matcher: 0 -> 24/61 cell instances;
    dual-PSM measured at +1 and rejected; line-split
    and upscale-6 measured worse); (b) screen-scoped
    validation universes, parts including the #89 built
    forms; (c) the matcher tries ALL leading-token
    suffix trims and strip_tier_label strips only
    balanced [...] spans; (d) session counts resolve by
    vote (mode, ties to max) instead of max-merge.
    Production replay of the same frames after the
    pass: 127 correct + 0 false accepts; Neuroptics
    13/28 distinct items; the frame-11 mid-scroll
    composite holds zero cells. DEFERRED: scroll-offset
    handling — no cheap reliable signal; scoped
    universes already neutralise composite phantoms,
    and a speculative offset search would multiply
    per-frame OCR time. NOTED LIMIT: a stack observed
    exactly once keeps its count (the Trumna x8 phantom
    is single-observation on these frames and
    survives); MATCH_MARGIN was not retuned — the
    dominant residual Neuroptics loss is margin refusal
    among genuine siblings (20/37 misses), which is
    the never-guess guard working as designed.
