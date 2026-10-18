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

## §2 Session start: regenerate AND open; opening must just work (items 37, 40b)

- On new-EE.log detection the resident already regenerates;
  Seth expects the plan to OPEN at game start too. Do both.
  (If the game is focused fullscreen, the browser opening
  behind it is fine — the page must be open when he alt-tabs.)
- The current Open path makes him pick a program for the HTML
  file EVERY time. Diagnose why (portal OpenFile handing off
  without a usable content-type/default association on KDE)
  and make opening land in his default browser with no
  chooser, from inside the sandbox, every time. The §4/pass-8
  response-tracking stays; a failed open still surfaces.

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
  needed).

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
