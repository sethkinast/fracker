# Twenty-fifth pass — evidence dumps actually land (addendum #82)

Trigger: Seth's live v0.5.19 runs (2026-10-24 22:51–22:59 EDT)
verified pass 24 and then went looking for the capture-debug
folder to send Blum training frames. There was none, on any
run. No flag is involved — the dump path is unconditional.
Root cause, found in the code:

- `CaptureData::context()` set `debug_dir` to
  `data_dir/capture-debug`, where `data_dir` is
  `tuning::data_dir()` — `$FRACKER_DATA_DIR`, which the Flatpak
  launcher points at `/app/share/fracker/data`: the read-only
  bundle tree.
- `dump_debug_frame` is best-effort (`let _ =
  create_dir_all(...)`), so every write failed silently.
  On a dev checkout with a writable data dir it worked, which
  is how the defect survived to a release.

Fixes (v0.5.20):

1. `CaptureData` carries a dedicated `debug_dir`:
   `config::data_dir()/capture-debug` — the user data dir,
   beside `stock.json` (in the Flatpak:
   `~/.var/app/io.github.sethkinast.Fracker/data/fracker/`).
2. Sampling raised from one frame per name per session to up
   to 60, sequence-named `frame-<name>-<idx>.png`: the dumps
   now double as an OCR training set (a full parts scroll is
   ~14 pages).
3. PNG writes run on a detached thread. A full-res encode on
   the reader thread would worsen the frame starvation the
   evidence exists to diagnose.

Gate: `debug_dumps_land_in_the_given_dir_with_sequence_names`
plus the full suite.

Context recorded in addendum #82: the v0.5.19 live numbers
(relics 528 stacks at 18/18/0; arcanes 155 at 7/7/0; parts
140 of ~270 at 12 read / 377 seen on a slow scroll, with
page-specific glyph rejects). Parts coverage and glyph
accuracy work continues once the training frames land.
