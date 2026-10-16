# Worker brief #6 — Fracker sixth pass (FAT pass: universe scrub,
# First Crack, icons, and the runnable-here phase)

Branch: `opencode/sixth-pass` (created from master; check it out).
This is a fat pass by design (addendum #32): everything queued
below ships in ONE pass. Read FIRST: `docs/verification-addendum.md`
items 14, 20, 24, 29, 30, 31, 32, 33, 34, 35; `docs/fifth-pass.md`
for what pass 5 landed (real tesseract binding, fixture-validated,
`data/capture/layouts.toml`); `BRIEF.md` / `DESIGN.md` for the
tray + generated-page architecture.

Sections in order. Logical commits per section are fine; push
when all gates are green.

## 1. Universe scrub (data quality)

The item universe (WFCD category data + `data/backlog.json`)
carries known defects. Fix them as DATA (item 20 — no game
content in `src/`), with the fixes owned by data files so a
universe rebuild keeps them fixed:

- **Helminth** sits in the Warframes category. It is a system,
  not a frame: it must not appear in the frame universe, the
  backlog, or any mastery math.
- **"Orion & Sirius"** appears duplicated under both name
  orders. One canonical entry, one name.
- **Non-acquirable / non-mastery entries:** generalize the
  hand-applied exclusions (Kavasa Prime Collar — owned, no
  mastery; Excalibur Prime — unobtainable) into a data-owned
  exclusion list with a per-entry reason, applied everywhere
  the universe is consumed.
- Regression gates: prime counts stay **163 roster / 99
  mastered / 63 unmastered**; total backlog counts change ONLY
  by the scrubbed entries, and your summary names each delta.

## 2. First Crack rename + Do Now icons (addendum #31)

- The **Do Now tab is renamed "First Crack"** — tab label, page
  header, and every copy reference to the tab name. (Internal
  identifiers may stay if renaming them is churn, but nothing
  user-visible may still say "Do Now" for this tab.)
- The First Crack tab renders **parent-item icons** for its
  target parts and chunk rows, like every other tab (#14, #29 —
  hotlinked WFCD CDN images in the product page).

## 3. Flatpak packaging (addendum #30, #33)

Make Fracker installable on Seth's Bazzite/KDE machine:

- **Manifest in-repo** (app id `io.github.sethkinast.Fracker`),
  freedesktop runtime + `ruststable` SDK extension. The build
  compiles the Rust binary AND the tesseract sidecar: leptonica
  + tesseract built as manifest modules, installed where the
  pass-3/pass-5 engine probes first, with `eng` tessdata
  bundled. No OCR = a packaging failure, not a degraded mode.
- **Sandbox permissions:** session bus + tray
  (StatusNotifierWatcher on KDE Plasma), network for the
  worldstate/profile/icon fetches, and a filesystem grant for
  the EE.log CONTAINING FOLDER (item 35 — the folder grant is
  primary). The log path is configurable in-app; bake the
  Steam/Proton default as data.
- **GitHub Actions workflow** (`.github/workflows/flatpak.yml`)
  using the Flathub-infra flatpak-builder action/container:
  on manual dispatch and on tags, produce a single-file
  `.flatpak` bundle and attach it to the workflow run / release.
  Seth installs with `flatpak install --user <bundle>`.
- Acceptance you CAN verify from the box: manifest parses,
  `flatpak-builder` lint passes if available, workflow YAML is
  valid. The bundle build itself runs in CI (this box is
  aarch64; do NOT try to build the x86_64 bundle locally).

## 4. Live EE.log watch (addendum #35 — exact requirements)

- Watch the containing DIRECTORY for file events (inotify or
  equivalent), never a naive path tail.
- On unlink/recreate: DRAIN the old inode to EOF through its
  still-open FD (the session tail — mission-end picks — lives
  there), then open the new file.
- Persist (inode, offset) in the app state dir; on startup,
  reconcile by processing any unprocessed tail.
- Polling exists only as a fallback and must carry
  inode-change detection.
- Mission-end events from the log drive an automatic page
  regeneration; the tray's manual Regenerate forces one too.

## 5. OCR bootstrap, extended to Arcanes (addendum #34)

- Wire the pass-5 engine into the app as the one-time inventory
  bootstrap: user supplies screenshots (relic era tabs, parts
  categories), Fracker builds local stock state. Validated,
  never-guess behavior from pass 5 is not negotiable.
- EXTEND the capture layouts to the **Arcanes inventory
  screen** (new entry in `data/capture/layouts.toml` + universe
  validation against the arcane item set). Neither the profile
  nor EE.log carries arcane state (#34) — this screen is the
  steady-state source. Seth's spoken debriefs remain in the
  data as a LABELED stopgap until his first arcane capture.
- Deliver a **capture checklist** in `docs/` for Seth's live
  session: one shot per relic era tab (with the filters NOTES.md
  established), scrolled shots per parts category, the Arcanes
  screen. The session itself is acceptance testing with Seth —
  it is NOT something you can do from the box.

## Explicitly OUT of scope

- The IA / scannability pass (#33 — deferred until Fracker has
  run on Seth's machine with real stock).
- Any new planner features beyond sections 1–2.
- Committing fixtures or screenshots (gitignored, always).

## Gates and deliverables

- `cargo build` clean (warnings explained), `cargo test` green,
  including the no-content-in-src test and new tests for the
  section 1 scrub and section 4 rotation handling (synthetic
  log files — drain-on-rotate must be proven by test, not by
  assertion).
- Refreshed self-contained demo export (rename + icons visible)
  left in the workspace for review, per the established pattern.
- Push `opencode/sixth-pass`. Final summary to stdout: per-section
  results, the app id + workflow name, what CI must produce, and
  honest notes on anything verifiable only on Seth's machine.
