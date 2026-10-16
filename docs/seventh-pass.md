# Worker brief #7 — Fracker seventh pass (the resident tray, for real)

Branch: `opencode/seventh-pass` (created from master; check it
out). Read FIRST: `docs/verification-addendum.md` items 24, 30,
35, 36, 37; `docs/sixth-pass.md` §3/§4 for what exists.

Context: pass 6 shipped the packaging and the EE.log watch
machinery, but the tray runtime is a STUB — only the Regenerate
signal seam is wired. Addendum #37 states the final-product
shape: Fracker STAYS RUNNING, detects a new EE.log on its own,
and produces a fresh report with no manual step. Seth installs
v0.1.2 tonight; this pass is what he upgrades to next. Nothing
here needs Seth or his machine — build it all box-side.

## Scope

1. **Real ksni tray runtime.** A resident process that owns:
   the tray icon on KDE Plasma (StatusNotifierItem via ksni),
   a menu with at least: Regenerate now, Open plan page, Quit;
   and a status line (last report time / watching state) as the
   tooltip or a disabled menu entry. Single-instance: a second
   launch focuses/exits cleanly instead of double-watching.
2. **Wire the §4 watch in-process.** The directory watch
   (addendum #35: directory events, drain-on-rotate via open FD,
   persisted (inode, offset), startup reconciliation) runs
   inside the resident process from launch:
   - a NEW EE.log appearing triggers a fresh report run
     automatically (item 37) — session start, not just
     mission-end;
   - mission-end events during the session regenerate too;
   - manual Regenerate forces a run.
3. **The generated page's home.** Reports render into the app
   data dir (`$XDG_DATA_HOME/fracker/` or the Flatpak
   equivalent); "Open plan page" opens the current report via
   the OpenURI portal. If no report exists yet, the menu action
   generates one first.
4. **Headless parity.** A `--headless` mode runs the identical
   watch + report loop with no tray (this is how you smoke-test
   on the box, and how it could run under a user service later).
   Tray absence must never change watch/report behavior.
5. **Autostart story.** Ship the desktop entry so KDE autostart
   can pick it up, and note the exact steps in the app README /
   docs (one short section — no new tooling).

## Hard constraints from the v0.1.x CI saga — do not regress

- The SDK extension for freedesktop 24.08 is
  `org.freedesktop.Sdk.Extension.rust-stable` (hyphenated;
  mount `/usr/lib/sdk/rust-stable/bin`). The unhyphenated
  `ruststable` does NOT exist for 24.08 — v0.1.1 died on this.
  The packaging tests assert the hyphenated form; keep them so.
- Cargo builds offline from the committed `cargo-sources.json`.
  If you change dependencies, regenerate it:
  `flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json`
  (see docs/flatpak-fix.md) and keep `cargo test` green locally.
- Leptonica/tesseract stay on the explicit out-of-tree cmake
  build commands — no cmake-ninja.

## Explicitly OUT of scope

- IA / scannability changes (gated on Seth running it with real
  stock — addendum #33).
- The Arcanes layout: provisional until Seth's capture session
  measures the real screen. Do not guess at it.
- New planner features.

## Gates and deliverables

- `cargo build` clean, `cargo test` green — including a
  synthetic-log integration test proving: resident process
  starts headless → new EE.log file appears in a watched temp
  dir → a report is generated WITHOUT any manual trigger.
- Logical commits per section; PUSH `opencode/seventh-pass`.
- Final summary to stdout: what the tray does now, how you
  smoke-tested headless mode on the box, and anything that
  still can only be verified on Seth's Plasma session.
