# Review brief — independent review of passes 14 and 15 (2026-10-20)

You are the REVIEWER, not the implementer. Do not change
any code. Your only deliverable is a review document,
committed on this branch as `docs/review-2026-10-20.md`
and pushed.

## Scope

Review the non-merge commits of passes 14 and 15 — the
range `v0.4.2..v0.5.1` on master (notably 65b0f58,
ce3bf05, 3937730, faeacf6, dc9ac56, 63c4c78, fc409a7,
7c29936, 06731d6) — against their briefs
(`docs/fourteenth-pass.md`, `docs/fifteenth-pass.md`)
and verification-addendum items 48–55. The full test
suite is green (211 tests, independently re-run); your
job is what tests can't say.

## Questions to answer explicitly

1. **DE worldstate parser (pass 14 §1):** are the schema
   assumptions sound? Check the $date/$numberLong parsing,
   missing/renamed section handling, fissure tier mapping
   from `Modifier`, Steel Path detection, and the
   freshness/staleness logic (payload `Time`, the ~10 min
   threshold, the labeled-stale cache path). Does anything
   silently degrade instead of failing visibly, against
   the addendum's "no silent substitution" rule?
2. **Quit fix (passes 14 §2 + 15 §1):** is the pass-15
   restructure (reports as tasks, bounded teardown) sound?
   Does `tests/tray_quit.rs` actually prove process exit
   under the wedged-report condition, or could it pass
   while the real binary still zombies? Any remaining
   unbounded await on the shutdown path?
3. **Capture (passes 13–15):** the PipeWire FFI and the
   new registry wait-for-node: bounds handling, timeout
   behavior, symbol/structure assumptions against
   PipeWire 1.x headers, and whether the instrumentation
   will actually discriminate the "target not found"
   cause on the next live run.
4. **Tab scoping (pass 15 §3):** is the generation token
   airtight (regen vs refresh distinguished correctly,
   legacy file handling)?
5. **Weeklies audit (pass 14 §5):** spot-check three
   verdicts of your choice against the wiki and say
   whether the audit's standard of evidence was met.
6. **Anything else:** regressions vs the briefs, dead
   code left behind (e.g. the warframestat worldstate
   path), data-pack rule violations (content in src/).

## Deliverable format

`docs/review-2026-10-20.md`: one section per question,
findings graded **bug / risk / nit** with file:line
references and the evidence for each. End with a
one-paragraph overall verdict. Commit ONLY that file,
push the branch, verify with `git ls-remote`, and print
the verdict paragraph to stdout.
