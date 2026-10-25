# Pass #24 — batched OCR (addendum #81)

Seth's brief, 2026-10-24 (this pass is implemented by Blum
directly, not an Oracle worker — the brief below is Seth's,
the implementation notes follow it).

## Diagnosis (Seth)

`read_region` spawns a fresh `tesseract` process for every
region, each with a temp-file write, and `read_cell` does 2–3
of them per cell (name, optionally the upper name band,
count). A 77-cell screen is roughly 150–230 process launches,
each reloading the language model, run serially — seconds per
frame, so a continuous scroll outruns the reader and the
1-frame channel drops most viewports (addendum #80 defect b:
~152 of ~700 distinct relics captured).

## Brief

1. Add a trait method `ocr_cells_in` with a default
   (per-cell through `ocr_cell_in`), so other engines keep
   working unchanged.
2. `TesseractCliEngine`: add a crop-hash cache
   (`Mutex<OcrCache>` — names / uppers / counts). A scroll
   step re-shows most cells pixel-identical, so only the new
   row needs OCR. Batch the cache misses into ONE tesseract
   process per region class: tesseract accepts a text file
   listing images and prints the pages separated by `\f`,
   with the model loaded once. `OMP_THREAD_LIMIT=1` on every
   spawn. Split `read_region` into crop+upscale plus
   `read_prepared(sub, psm, wl)`, the existing body.
3. Override `ocr_cells_in` in `TesseractCliEngine` with the
   semantics of `read_cell` (same regions, same upper-band
   rule, same count parsing). The `raw` map in the sketch is
   leftover — delete it.
4. Call it once per frame: `capture.rs::read_frame` replaces
   the per-cell loop with
   `engine.ocr_cells_in(&layout.split(frame), layout)`.
   Also cache the header classification by crop hash and
   only re-OCR it when the header pixels change.

Expected: first frame ~3 process launches, each scroll step
~2. Seth has NOT compiled the sketch; borrow-checker details
may need small tweaks. The bigger step (libtesseract via
dlopen, ~20 ms/cell resident) is explicitly deferred until
after this change is measured.

## Implementation notes (Blum)

- As briefed, with two structural tweaks: the sketch's `fill`
  closure became an inherent `fill_cache` method (same
  behaviour, no closure-borrow fighting), and `crop_key` is
  a public function in `ocr.rs` (plus `region_key` for a
  fraction-of-image region) so `capture.rs` can key the
  header cache on the same hash.
- Header cache lives in `CaptureSession` (`last_header`):
  keyed by `region_key(frame, classifier.header)`; only a
  fresh classification appends to `header_samples`.
- `ingest_folder` (the one-shot bootstrap) also reads via
  `ocr_cells_in` — it is the same per-page loop and gets the
  same win for free.
- Any batch mismatch (spawn failure, non-zero exit, page
  count != image count) falls back to per-image reads, so a
  batch failure can never lose a read the old path made.
- Gates: `batched_reads_match_per_cell_reads` (batched ==
  per-cell on text/count/rank/confidence, incl. a repeated
  call served from cache), `crop_key_is_stable_and_discriminates`,
  `header_is_reread_only_when_its_pixels_change`.
- Ships as **v0.5.19**. Live measurement on Seth's machine
  is still owed (frames read/skipped are already logged at
  finalize per #80); that measurement decides the dlopen step.
