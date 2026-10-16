# Worker brief #4 — Fracker fourth pass (Do Now spec + weekly/bars correctness)

Branch: `opencode/fourth-pass` (created from master; check it out).
Read FIRST: `docs/verification-addendum.md` items 24–29 (this
pass's charter), then `BRIEF.md` and the pass-3 state.

## Scope of THIS pass

1. **Do Now sections (addendum #26).** On regen, Do Now is:
   (a) time-limited EVENT alerts section; (b) a SEPARATE
   Gift-of-the-Lotus alerts section — sections hide when empty;
   (c) the ≤3 target relics with what they give (exists);
   (d) Varzia/Resurgence detection rendered as its own first-class
   element (rotation ∩ Seth's unmastered vaulted items);
   (e) Nightwave acts show name + DESCRIPTION everywhere they
   render.
2. **Weekly box slimming (addendum #27).** Remove Nightwave from
   the weekly box (it has its own section). Delete the Conclave,
   Clan, Teshin, Helminth, and NW-creds entries outright.
   Reconcile `WEEKLIES.md` and `data/weeklies.toml` so the data
   file IS the slimmed list (per addendum #20, content lives in
   data — edit the data, not the code). Baro Ki'Teer: his own
   section, rendered only while a void trader is active in world
   state.
3. **Daily bars per pool (addendum #28).** One slim bar per
   DailyAffiliation* pool in the profile (13 bars — PvP excluded
   per the Conclave cut) + the DailyFocus bar. Remaining-of-cap,
   terse labels. Full pools may dim/collapse; the pattern is the
   information.
4. **Demo regenerated**, self-contained per addendum #23 (embedded
   thumbnails, zero CDN references), committed on the branch.

## Explicitly OUT of scope this pass

OCR fixture tests + engine binding and the universe scrub
(Helminth/duplicates) — that's pass 5, already queued. Tray,
Flatpak, live watch loop, live OCR capture, IA redesign beyond
the spec items above.

## Gates and deliverables

- `cargo build` clean (warnings explained), `cargo test` green.
- ONE logical commit on `opencode/fourth-pass`, then PUSH.
- Final summary to stdout: per-item status, what was cut and why,
  the weeklies before/after entry lists, demo stats (bars count,
  sections present, embedded icons).
