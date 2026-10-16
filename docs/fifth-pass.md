# Worker brief #5 — Fracker fifth pass (real OCR: engine + Seth's fixtures)

Branch: `opencode/fifth-pass` (created from master; check it out).
Read FIRST: `docs/verification-addendum.md` items 20, 21, 30 (the
OCR charter), `docs/third-pass.md` item 3 (the machinery you
built), and `fixtures/ocr-samples/NOTES.md` (ground truth for the
real screenshots — the fixtures are already in your workspace,
gitignored; NEVER commit them).

## Scope of THIS pass — OCR only

1. **Get a real tesseract running in user space.** No root on this
   box: fetch a static/AppImage aarch64 tesseract into
   `/opt/opencode/tools/` (or build leptonica + tesseract from
   source with the zig-cc shim if no static build works), plus
   `eng.traineddata`. This is the same binary class the Flatpak
   will bundle as a sidecar (addendum #30) — record its exact
   provenance and version in your summary.
2. **Bind the CLI engine** through the pass-3 trait: probe order =
   bundled sidecar path → `$PATH`. The stub must no longer be what
   a normal run gets when the sidecar exists.
3. **Run the real fixtures** (expectations in NOTES.md):
   - `relics-axi-02-clean.png`: all 77 cells extracted — names and
     counts (no count label = 1), validated against the item
     universe.
   - `relics-axi-01.png`: the tooltip-occluded cells flagged
     unreadable; every other cell agrees with the clean shot.
   - `parts-prime-01.png`: circled-badge counts, two-line part
     names, and the cut-off bottom row REJECTED as partial.
   Tune preprocessing (upscale / contrast / threshold) as needed,
   but the never-guess rule is absolute: low-confidence or
   unmatched reads are NOT held, and your summary reports the
   accuracy per fixture (cells correct / total) with the failure
   modes named.
4. Tests for the above, skipping gracefully when the fixtures or
   engine are absent (other machines won't have Seth's screens).

## Explicitly OUT of scope

The universe scrub, the First Crack rename, Do Now icons (pass 6,
already queued); tray/Flatpak; the live capture session with Seth.

## Gates and deliverables

- `cargo build` clean (warnings explained), `cargo test` green.
- ONE logical commit on `opencode/fifth-pass`, then PUSH.
- Final summary to stdout: engine provenance/version, per-fixture
  accuracy, failure modes, what was cut and why.
