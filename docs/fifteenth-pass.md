# Worker brief #15 — Fracker fifteenth pass (Quit zombie, capture target, tab scope)

Branch: `opencode/fifteenth-pass` (created from master;
check it out). Read FIRST: `docs/verification-addendum.md`
items 52, 53 (incl. the 22:47 evidence), 54, 55.

Seth released this round on 2026-10-19. Three sections;
§1 and §2 are diagnosis-first — instrument, then fix.

## §1 Quit leaves a zombie (item 53)

Live behavior on v0.5.0, per Seth: Quit visibly STOPS the
resident operating (menu items dead, EE.log scraping
halted) but the process never exits and the tray icon
stays up. Pass 14's ordered teardown + explicit exit
therefore starts but never completes — a teardown step
hangs before exit is reached (or exit lives on a path the
running build doesn't take).

- Instrument every teardown step (start/done in the app
  log) so the hang point is visible in one run.
- Find and fix the hanging await / ordering bug.
- GATE (from the addendum, non-negotiable): a D-Bus-level
  test — resident spawned under a private D-Bus session,
  Quit activated through the tray's dbusmenu interface,
  process exit asserted within a bounded time. A pure
  logic test does not satisfy this gate; pass 14's did
  and the bug survived it.

## §2 Capture: wait for the node, then connect (items 54, 55)

v0.5.0 passes node 135 to the stream and still gets
"target not found", states 0->1->2 then 2->-1 ~5 ms in.
Leading hypothesis: the portal's node is not yet
registered on the PipeWire remote when connect fires;
known-good clients wait for the node global.

- Log the raw ScreenCast Start reply (the full streams
  payload) and the remote's registry globals as they
  appear.
- Wait until the target node exists in the registry
  before `pw_stream_connect`; connect to it then.
- If it still fails, the summary must say what the log
  rules in and out — no silent theory swap.
- Fix the stage mislabel from the same log: an error
  before the first frame must not be stamped "capturing".
- Constrain the portal request (item 55): source types
  WINDOW | MONITOR, cursor hidden. The flow's log line
  should tell the user to pick the Warframe window.

## §3 Tab persistence scoped to one generation (item 52)

The stored tab choice (pass 13's `data/plan-tab.json`)
survives only within one plan generation (its 60s
refreshes). A NEW regeneration resets to the starting
tab (First Crack). Implement by tying the stored choice
to the generation: a regen invalidates it.

## Gates and deliverables

- `cargo build` clean, `cargo test` green (incl. the new
  D-Bus-level Quit test); regenerate `docs/demo-plan.html`.
- Logical commits per section; PUSH
  `opencode/fifteenth-pass` and VERIFY with `git ls-remote`.
- Final summary to stdout: §1's hang point + fix + the
  D-Bus test's evidence, §2's registry findings (what the
  log proved, whether wait-for-node fixed the connect),
  §3's mechanism.
