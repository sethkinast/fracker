# Worker brief #10 — Fracker tenth pass (IA restructure, Seth's design)

Branch: `opencode/tenth-pass` (created from master; check it
out). Read FIRST: `docs/verification-addendum.md` item 42.

Seth iterated the plan page's information architecture
himself, by hand, in a worked HTML file. It is on the box at
`/opt/opencode/ia-reference-seth.html` (his live-data export,
hand-edited — NOT a fixture, never commit it). **That file is
the spec.** This pass ports its structure, hierarchy, and
density into `src/render.rs` (and the demo export) without
changing any data source, scoring, or behavior. Where his
file shows content, find its existing data source in the app;
if a string in his file has NO data source in the app, flag
it in your summary — do not invent data to match the mock.

## The target IA (from his file)

- **Four tabs**: First Crack / Hunt / Backlog / **Routine**
  (new). Same CSS-radio tab mechanism; the server-side tick
  store keys remain stable across the restructure (ticks for
  moved items must not lose their identity).
- **Header**: one slim row — brand, three status pills (Baro
  countdown, Varzia summary, Targets summary), one-line stamp
  (MR, generated time, backlog count, profile state). The old
  multi-block header boilerplate goes away.
- **First Crack is action-only**: event alerts (payout,
  node, mission, time), the crack queue (mission, relic +
  refinement, target part + also-drops, chance/traces/time),
  Varzia strip, event pack strip (incl. the remaining-arcanes
  line), closing one-liner. NO standing bars, NO weeklies,
  nothing routine.
- **Routine tab** owns the daily/weekly surface:
  - "Today": standing + focus LEFT as compact inline numbers
    (label + value chips) with slim bars — not the old huge
    bar stack. Sortie line with its reset time.
  - "This week": the weeklies board (the existing WEEKLIES
    content set) as compact tick rows with time estimates —
    server-side ticks as of pass 9.
- **Hunt / Backlog**: keep their content and order; apply the
  same density treatment (spacing, type scale, bar height).
- **Density everywhere**: progress bars 5px, tighter card
  padding and vertical rhythm, smaller kickers — match the
  reference's CSS values where they exist rather than
  inventing a parallel scale.

## Fidelity rules

- His file was hand-trimmed (111KB vs the full render); do
  not read missing rows as "delete this data" — structure and
  hierarchy are the spec, data completeness stays the app's.
- Every behavior from passes 1–9 keeps working: log-struck
  completions strike in the new layout, standing values are
  the profile's verbatim remaining (pass 9 §3), ticks POST
  and persist, the page is served from the loopback server,
  meta-refresh stays.
- Content stays data-owned (item 20): new labels/strings the
  IA needs go to the data files, not src/ literals, beyond
  pure layout chrome.

## Gates and deliverables

- `cargo build` clean, `cargo test` green; update golden /
  render tests to the new structure (section placement tests:
  standing + weeklies render under Routine and NOT under
  First Crack; four tabs present in order).
- Regenerate the demo export (`docs/demo-plan.html`) from the
  new renderer.
- Logical commits; PUSH `opencode/tenth-pass` and VERIFY with
  `git ls-remote` before declaring done.
- Final summary to stdout: section-by-section mapping from
  the reference to the renderer, anything in his file with no
  data source (flagged, not faked), and the tick-identity
  story for moved items.
