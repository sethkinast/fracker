# Worker brief #17 — Fracker seventeenth pass (capture round 4, alert strike)

Branch: `opencode/seventeenth-pass` (created from master;
check it out). Read FIRST: `docs/verification-addendum.md`
items 59 and 60 (all three parts). Seth released this
round on 2026-10-21 and wants a fast turnaround. Scope
is tight: capture (§1–§3) and the alert strike (§4).
Items 57/58 stay queued — do not build them.

## §1 Registry through exported symbols only (item 60a)

v0.5.2 died at "connecting": `pw_core_get_registry` is
not exported by Seth's libpipewire. Header-declared is
not library-exported — that distinction now rules this
module. Either obtain the registry through a mechanism
that exists as an exported symbol on current PipeWire
(verify against the library's exported-symbol reality,
not headers alone), or drop the registry dependency and
retry `pw_stream_connect` on target-not-found (bounded
retries over a few seconds). Keep the raw Start reply +
registry/connect logging from pass 15. The v0.5.0
target-not-found at connect is still unexplained; your
summary must say what this round's evidence does to
that question.

## §2 Resolve every symbol up front (item 60b)

Before the portal flow begins, resolve ALL capture
symbols and, if any are missing, fail with ONE error
naming every missing symbol. No more one-symbol-per-
live-run discovery. The pinned manifest
(`data/capture/pipewire-symbols.txt`) and its agreement
test must cover every name the module resolves,
including any §1 adds or removes.

## §3 Persist the restore token (item 60c)

Confirmed defect: the portal issues a restore_token in
the Start reply (the user checks "remember for future
sessions") and the app never persists or replays it, so
every run demands full consent. Store the token from
the Start reply in the app-data store; pass it back
(with persist mode) on subsequent SelectSources calls;
log token events with only the last few characters —
the pass-15 raw-reply logging currently prints the
whole token, redact it there too.

## §4 Alert strike: SolNode gates corroboration (item 59)

`matches_alert` (src/elog.rs) ORs tag+node, tag+type,
tag+shared-payout. The payout path strikes every
same-tag sibling when the event pays a shared currency
(Seth's live case: one completion struck all four Road
To Tau alerts). Rule: when both the completion and the
alert carry a SolNode, ONLY the node (or the exact
alert id) corroborates; type/reward paths apply only
when node data is absent on either side. Regression
test = the live case: four same-tag siblings, distinct
nodes/types, shared currency reward, one completion →
exactly one struck.

## Gates and deliverables

- `cargo build` clean, `cargo test` green; regenerate
  `docs/demo-plan.html` only if rendering changed.
- Logical commits per section; PUSH
  `opencode/seventeenth-pass` and VERIFY with
  `git ls-remote`.
- Final summary to stdout: per-section evidence, and
  for §1 the exact mechanism chosen and why it exists
  in the installed library.
