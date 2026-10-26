# Vendored font: Roboto Regular (2013 cut, "Old Roboto")

`Roboto-Regular.ttf` is the rasterisation reference for the
pass-35 analysis-by-synthesis reader (`src/synth.rs`). It is the
~2013 cut of Roboto — "Old Roboto" — that the Warframe inventory
UI renders item names in (see `docs/thirty-fifth-pass.md` for
the identification evidence).

Provenance:

- Source: Android Open Source Project,
  `platform/frameworks/base` repository, tag `android-4.3_r2.1`,
  path `data/fonts/Roboto-Regular.ttf` (fetched from the public
  AOSP mirror, github.com/aosp-mirror/platform_frameworks_base).
- Font self-identification (name table): full name
  "Roboto Regular", version string "Version 1.200310; 2013",
  unique ID "Google:Roboto:2013", copyright "Font data
  copyright Google 2013".
- SHA-256 of the vendored file:
  `00ae3fd7698bec219473edf68171a0f14e6cf68d363eea59373156e0b090de11`
- The identical version (1.200310; 2013) ships in Android
  4.4.2_r1; its ASCII glyph outlines are byte-identical to this
  file's (verified per glyph). The 2011 cut (Version 1.00000)
  and the 2014 redesign (Version 2.001047) differ measurably in
  the discriminating glyphs (capital R advance 1298 / 1356 /
  1261 units across the three cuts) and in held-line fit — see
  the pass doc.

License: Roboto is licensed under the Apache License, Version
2.0; the full text is in `LICENSE.txt` beside this file. The
font is embedded into the binary (`include_bytes!`) so the
Flatpak needs no runtime font lookup.
