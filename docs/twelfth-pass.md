# Worker brief #12 — Fracker twelfth pass (strict mock port, open discipline, reset-aware standing, alert-strike strictness)

Branch: `opencode/twelfth-pass` (created from master; check
it out). Read FIRST: `docs/verification-addendum.md` items
42, 43, 44, 45.

Seth live-tested v0.4.0. Four findings, four sections. §1 is
the big one; he was blunt: the pass-10 IA "doesn't match my
mock at all." This time the mock is not inspiration — it is
the spec, verbatim where possible.

## §1 Port the mock STRICTLY (item 42, round 2)

Seth's reference file is on the box at
`/opt/opencode/ia-reference-seth.html` (111,308 bytes —
CONFIRM it exists and matches that size before starting; if
it is missing, say so in the summary and still do your best
from this brief, do not silently proceed as if you had it).
Pass 10 missed it because the file landed after that worker
checked. Graf has verified it is present NOW.

- **Steal the CSS and markup.** Adopt the reference's
  stylesheet as the renderer's stylesheet (extend only for
  dynamic states the mock can't show: struck/done rows, tick
  states, live values) and emit the reference's markup
  patterns per component. Do not redesign, reinterpret, or
  "improve" the layout.
- Seth's named gaps from the v0.4.0 test — ALL must land:
  - the **multicolumn layout** (the mock's g2/g3 grids on
    First Crack, Backlog, and wherever else it uses them);
  - the **backlog pips** (the component-progress pips beside
    the 0/4-style counts in Backlog);
  - the **Hunt filter tabs** (the in-page filter controls
    the mock carries inside Hunt — find them in the file and
    port them as-is, CSS-driven like the main tabs);
  - the **reduced wordiness in First Crack** (the mock's
    compact row phrasing for alerts and the crack queue).
- Data completeness stays the app's (the mock was
  hand-trimmed); structure, hierarchy, density, and phrasing
  shape are the mock's.
- Deliverable: a component mapping table in the final
  summary (mock component → render site → data source), and
  tests asserting the structural facts: four tabs in order,
  grid classes on the right pages, Hunt filter controls
  present, backlog pips present, standing/weeklies absent
  from First Crack and present in Routine.

## §2 Auto-open discipline (item 43)

v0.4.0 behavior Seth saw: the plan opened the moment the APP
started (before any game), then opened twice more while
Warframe was starting.

- Auto-open fires ONLY on game-session start: a new EE.log
  session (login), never at app startup.
- Exactly ONCE per session. Warframe startup churns the log
  (file created/recreated in bursts before login settles):
  debounce and coalesce file events, treat the session as
  started when it has settled (e.g. first login line /
  stability window), and remember that this session already
  got its open. Regenerations never re-open; only the tray's
  Open action opens on demand.
- Fixture for the startup sequence:
  `fixtures/ocr-samples/ee-log-2026-10-18.log` (his
  startup-only session from the v0.4.0 test).
- Test: simulated event burst (multiple new-file events +
  app start with no new log) yields exactly one open, at
  session start only.

## §3 Reset-aware standing (item 44)

- Daily reset boundary: 00:00 UTC. When the profile/cache
  being rendered was fetched BEFORE the most recent reset,
  the daily pools (every `DailyAffiliation*` + `DailyFocus`)
  render as reset — remaining = cap — until a fresh
  post-reset fetch lands. A fresh fetch always wins verbatim
  (pass 9 §3 rule stands).
- Test: fixture profile timestamped pre-reset rendered after
  the boundary shows caps for all daily pools; a post-reset
  fetch renders verbatim.

## §4 Alert strikes: instance-strict (item 45)

v0.4.0 false positives: three LIVE Road To Tau alerts
(Isos Survival, Saxis Mobile Defense, Naeglar Hive) rendered
struck with "log ✓" — matched via the tag/reward/type
corroboration paths against YESTERDAY's same-event
completions. The worldstate values themselves were correct.

- ID match (`alertId` ↔ worldstate `id`) stays as-is.
- Every non-ID corroboration path gains an
  **activation-time guard**: the completion's observed time
  must be at/after the alert's activation time (small
  epsilon for clock skew). A completion that predates the
  alert instance can never strike it. Completions therefore
  need trustworthy observed-at timestamps in the ledger —
  add them if missing.
- Regression tests: (a) yesterday's same-tag completion vs
  today's alert → NOT struck; (b) completion after activation
  with corroboration → struck; (c) ID match → struck.

## Gates and deliverables

- `cargo build` clean, `cargo test` green; regenerate
  `docs/demo-plan.html` from the new renderer.
- Logical commits per section; PUSH `opencode/twelfth-pass`
  and VERIFY with `git ls-remote` before declaring done.
- Final summary to stdout: the §1 component mapping table,
  reference-file confirmation (size seen), and per-section
  gate evidence.
