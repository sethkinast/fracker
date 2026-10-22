# Worker brief #21 — Fracker twenty-first pass (matcher predicate, format negotiation)

Branch: `opencode/twenty-first-pass` (created from master;
check it out). Read FIRST: `docs/verification-addendum.md`
items 64 and 65. Ships as **v0.5.8**.

## §1 Alert corroboration: the full instance predicate (item 64)

Event alerts rerun on a small handful of nodes all day,
so neither node-only (pass 17, too weak — same-node
siblings of a different type get struck) nor node+type
alone is instance identity across repeated instances.
The corroboration predicate for `matches_alert`
(src/elog.rs) becomes, all conjunctively:

- same event tag (as today),
- same SolNode (when both sides carry one),
- same mission type (when both sides carry one —
  node-only fallback only when a type is absent),
- AND the existing pass-12 activation guard unchanged:
  the completion must be observed at/after the alert's
  activation (unparseable activation stays fail-closed).

Regression tests, both required:
- same-tag, same-node, DIFFERENT-type siblings, one
  completion -> exactly one struck;
- same node AND type, completion observed BEFORE the
  alert's activation (a rerun instance) -> NOT struck.

## §2 Complete the PipeWire format negotiation (item 65)

v0.5.6 connects on attempt 1, then dies +40 ms in:
state 2 -> -1, "no more input formats". Two gaps are
verified absent in src/screencast.rs:

(a) The app never confirms a format. Add
    `pw_stream_update_params` to the FFI surface and
    the pinned symbol manifest (it is a standard
    exported symbol), and in `on_param_changed`, after
    parsing the node's proposed format, confirm it back
    with a Format pod built from the negotiated values.
(b) The offered EnumFormat pod carries no modifier
    property. Enumerate `SPA_FORMAT_VIDEO_modifier` in
    the offer (choice enum; verify the property id and
    value encoding against the upstream headers, the
    pass-20 discipline — cite them in your summary).

Also log the negotiated format on success (fourcc,
size, framerate): besides diagnosis, a 2560x1440 game
surface in the log is the user's practical evidence
the restored portal source is the right window.

## Gates and deliverables

- `cargo build` clean, `cargo test` green; bump
  Cargo.toml to **0.5.8**.
- Logical commits per section; PUSH
  `opencode/twenty-first-pass` and VERIFY with
  `git ls-remote`. Final summary: §1 predicate as
  shipped, §2 header citations for the modifier values.
