# Worker brief #16 — Fracker sixteenth pass (review follow-ups)

Branch: `opencode/sixteenth-pass` (created from master;
check it out). Read FIRST: `docs/review-2026-10-20.md`
(the GLM review of passes 14–15) and verification
addendum item 56. Seth approved this round on 2026-10-20.
Model note: you are the DeepSeek implementer; the review
was independent. Environment note: a zig dist is staged
at `/opt/opencode/zig-dist` if the cc shim's target is
missing after the recent service restarts.

Four fixes, exactly the review's follow-ups:

## §1 SPA pod read uses the pod's own size (review §3, bug)

`on_param_changed` (`src/screencast.rs` ~1497) copies a
fixed 256 bytes from the SPA pod regardless of its real
size — an out-of-bounds read on any smaller pod. Read
the pod's size field and bound the copy by it (handle
the none/short cases the SPA API defines). Keep the
existing behavior for the normal ≥256B probe pods; add
a test with a deliberately small pod if the harness
allows, else a unit test of the size-extraction logic.

## §2 Visible loss in the DE parser (review §1, risks)

Follow `decode_tier`'s model — unknowns go to `gaps`,
visibly. Three sites:
- An unparseable/empty alert timestamp currently fails
  the activation guard OPEN (`src/elog.rs`
  `corroborates_activation`, `Err(_) => return true`).
  A degraded timestamp must surface (gaps entry and/or
  a conservative non-strike), never silently skip the
  guard. Preserve the documented legacy-feed behavior
  only where it is genuinely distinguishable.
- An unparseable fissure `expiry` currently drops the
  fissure from ranking silently (`src/plan.rs`
  `rank_fissures` and the remaining-live filter). Record
  the loss in gaps; a dropped fissure must not look like
  "DE has no fissures".
- Missing/renamed section fields degrading to "" via
  `de_str`/`de_date` (`src/model.rs`) get the same
  treatment where a section the plan renders is affected.

## §3 Capture Drop takes the thread-loop lock (review §3, risk)

`ScreenCastSource::Drop` (`src/screencast.rs` ~1316)
calls `pw_stream_disconnect` / `pw_core_disconnect`
without holding the thread-loop lock. Hold the lock for
the disconnect sequence (the same lock discipline the
rest of the file uses), so a future wiring can't race
the loop's callbacks against teardown.

## §4 Golden timestamp guard (review §6, risk)

`tests/golden.rs` hardcodes `golden_now()`; regenerating
fixtures (`FRACKER_REFRESH_FIXTURES=1`) would drift all
golden timestamps silently. Add the assertion the
review proposes: the golden "now" must match the
committed fixture's `Time`, failing loudly when they
diverge.

## Gates and deliverables

- `cargo build` clean, `cargo test` green (211 baseline
  plus new tests); regenerate `docs/demo-plan.html` if
  rendering changed (it should not).
- Logical commits per section; PUSH
  `opencode/sixteenth-pass` and VERIFY with
  `git ls-remote`.
- Final summary to stdout: per-section evidence,
  including how each silent path now reports.
