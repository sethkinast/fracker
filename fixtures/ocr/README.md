# OCR fixtures

`grid-2x2.ppm` — synthetic inventory-grid page rendered with a SYSTEM
font (DejaVu Sans, PIL), dark UI background + light labels, 2×2 cells:

- `Neo C5` | `Ivara Prime Chassis Blueprint`
- `Forma Blueprint` | `Lith K9`

Per the brief, the full pipeline assertion on system-font rendering
is engine-permitting: no OCR engine on this box reads DejaVu glyphs
(tesseract is not installed — see `src/ocr.rs::engine_status`), so the
fixture test pins what the box CAN prove: PGM/PPM parsing of a real
font rendering, even-grid segmentation geometry, and universe
validation of the known labels (reads stand in for the capture-session
engine). The bitmap-template path (`bitmap_font`) proves extraction
end to end without an engine.
