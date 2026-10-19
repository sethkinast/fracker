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
