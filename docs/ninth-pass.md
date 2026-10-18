# Worker brief #9 — Fracker ninth pass (live-test round 2 fixes)

Branch: `opencode/ninth-pass` (created from master; check it
out). Read FIRST: `docs/verification-addendum.md` items 35, 37,
38, 39, 40. Seth ran v0.3.0 live tonight; four defects, all
reproducible from the evidence below. FAT pass.

A live profile snapshot from tonight is on the box at
`fixtures/ocr-samples/profile-2026-10-17.json` (gitignored —
never commit). Ground truth from it: `DailyAffiliation = 7823`,
every other DailyAffiliation* pool = 31000, `DailyFocus =
357977`.

## §1 Capture Inventory dies silently after consent (item 40a)

Seth clicks Capture Inventory, grants the portal consent, and
then NOTHING: the menu item never flips to Stop, no status
change, no error. The capture lifecycle after consent —
ScreenCast Start, OpenPipeWireRemote, the dlopen PipeWire
consumer's first frames — is failing or stalling invisibly.

- Instrument the whole lifecycle: every state transition and
  failure (with the D-Bus error / PipeWire error verbatim)
  appended to a log file in the app data dir, AND reflected in
  the tray status line.
- The tray capture control becomes an explicit state machine:
  Idle -> Consent pending -> Connecting -> Capturing (with
  live per-bucket counts) -> Finalizing -> Idle; any failure
  lands in a visible Error state naming the failing stage.
  Seth must be able to tell, from the tray alone, whether it
  is capturing.
- Find and fix the actual post-consent failure. Suspects to
  rule in or out with evidence, not vibes: the Start call's
  session/source negotiation, the restore-token path when no
  token exists yet (first run), the FD handling from
  OpenPipeWireRemote, the hand-laid SPA pod / dlopen symbol
  set failing at stream connect, or the classification loop
  never receiving a frame and the UI never saying so.
- Box gate: the state machine + failure surfacing are unit
  tested with a fake frame source that fails at each stage;
  the happy path still passes the §2-sequence harness from
  pass 8.

## §2 Serve the plan over loopback HTTP; session start opens it (items 37, 40b, 41)

Seth's catch (item 41): the plan page's checkboxes persist via
localStorage, but the page is a file:// URL — Chrome treats
each file:// document as an opaque origin (localStorage is
dead there) and Firefox partitions file storage, so ticks are
fragile-to-broken depending on browser. And the file handoff
is why opening asks him to pick a program every time. One
move fixes both: **the resident serves the plan itself.**

- Embed a tiny HTTP server in the resident process, bound to
  127.0.0.1 only (the manifest already has --share=network,
  so the sandbox shares the host net namespace and the host
  browser can reach loopback). Near-zero footprint: serve the
  current report HTML at a stable URL (e.g.
  `http://127.0.0.1:<port>/` — port from config with a sane
  stored default; if the port is taken, fall back and record
  the live URL where the tray's Open action reads it).
- "Open plan page" and the session-start auto-open both
  become OpenURI on that http:// URL: default browser, no
  chooser, no FD handoff, no sandbox path nonsense. A failed
  open still surfaces in the tray status.
- Tick persistence moves server-side: checkbox changes POST
  to the server (e.g. /tick) and are stored in the app data
  dir keyed by item id + day, re-applied at render — ticks
  survive regenerations and are browser-independent.
  localStorage may remain as a progressive nicety but is no
  longer the store of record. The 60s meta-refresh stays.
- On new-EE.log detection: regenerate AND open the URL at
  game start (item 40b). The browser opening behind a
  fullscreen game is fine.

## §3 Standing math: the profile field IS the remaining (item 40c)

The app showed Syndicates at "0 left" while the live profile
says DailyAffiliation = 7823 and Seth's in-game readout says
7,823 remaining. The field semantics (verified tonight against
the live endpoint): DailyAffiliation* values ARE remaining
amounts; cap is 31,000 at MR30; DailyFocus likewise (357,977
of 400,000 tonight).

- Find the transform that zeroed a partial value and fix it.
  Pools at exactly cap rendered fine, so the defect hides in
  the partial-value path — check the bare-pool pairing after
  the pass-8 label-map change and every arithmetic step
  between fetch and render.
- Audit ALL pools + Focus against the fixture profile: the
  test asserts rendered remaining == the fixture's raw values
  for every family, including the bare pool.

## §4 Log-derived completions must reach the resident's report (item 40d)

Seth ran alerts this session with the resident watching since
session start; after regeneration the alerts still show
undone. The design (items 26/35) says log-proven completions
strike regardless of manual ticks: MissionInfo / reward lines
in EE.log identify completed missions.

- Trace the state flow end to end: watch parses the log (for
  triggers) -> where does parsed mission state accumulate? ->
  what does report generation actually read? Fix the wiring
  so the resident's accumulated log state is an input to
  every report it generates (auto, manual, and session-start),
  persisting across regenerations and app restarts
  (today-scoped: completions belong to the daily reset).
- Match completions to worldstate alerts by mission identity
  (node + mission type/faction as the data allows) — an alert
  is struck only on a positive log match, never inferred.
- Gate: an integration test runs the resident headless
  against a fixture EE.log containing a completed alert
  mission and asserts the generated report strikes that alert
  (the fixtures/ EE.log samples on the box carry real
  MissionInfo lines; synthesize the worldstate side as
  needed). Seth's own current-session EE.log may land at
  `fixtures/ocr-samples/ee-log-2026-10-17.log` mid-run (Graf
  syncs it) — if present, use it as an additional fixture:
  it is the exact log his resident watched while the alerts
  went unstuck.

## Hard constraints (unchanged)

rust-stable extension id for 24.08; out-of-tree cmake;
offline cargo from committed cargo-sources.json (regenerate
if deps change); fixtures and the live profile stay
gitignored; no new network calls.

## Gates and deliverables

- `cargo build` clean, `cargo test` green incl. the new §1,
  §3, §4 gates.
- Logical commits per section; PUSH `opencode/ninth-pass`
  (verify the push landed — `git ls-remote` the branch —
  before declaring done; pass 8's push silently didn't).
- Final summary to stdout: the §1 root cause with evidence,
  the §2 open mechanism, the §3 transform that was wrong,
  the §4 wiring gap, and what only Seth's machine can verify.
