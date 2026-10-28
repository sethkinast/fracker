# Fifty-second pass — icon class table, coverage phrase, pip titles

Verification addendum: #118.

## The ask

Three cleanup items from the queue Seth closed on
2026-10-28 (record: `QUEUE_BUILD_PLAN.md` in the frames
workspace):

1. **Relic-icon bake collapse (queue A-1)** — queued
   2026-10-27 after Seth observed that all relics of an era
   share one icon per refinement class: replace the
   3,204-record per-projection bake with the era × tier
   class table, gated on equivalence with the current map.
2. **Coverage phrase (queue A-2)** — the crack-queue rows'
   `+N also` becomes `+N other needed`. Wording only; the
   count is unchanged.
3. **Pip titles (Seth, 2026-10-28)** — Backlog pips get a
   `title` attribute whose text is the part name, exactly
   (his example: "Lavos Prime Neuroptics"), so hovering a
   pip says which part it is — and what he is missing —
   without opening Hunt or the overlay. No state wording:
   the pip's fill already shows state.

## 1. The class table (`data/cache/relic-textures.json`, `src/model.rs`, `src/fetch.rs`)

Measured against the shipped bake first: the 3,204
projection uniqueName -> textureLocation records collapse
to **21 distinct textures** — RelicLith/Meso/Neo/Axi A–D,
RelicImmortal A–D, and the OmegaIsotope pickup icon. The
letter is the refinement tier (Intact = D, Exceptional = A,
Flawless = B, Radiant = C); every (era, tier) group in the
manifest names exactly one texture. Vanguard projections
carry the Axi set; Requiem projections carry the
RelicImmortal set.

New bake shape (2,579 bytes, down from 455,373):

- `classes[era][tier]` — era as named in relic display
  names (Lith, Meso, Neo, Axi, Vanguard, Requiem; Vanguard
  rows hold the Axi locations, Requiem rows the Immortal
  ones), tier -> textureLocation.
- `pickup` — the one texture every un-tiered GENERIC base
  projection item wears (the plain "Lith Relic", "Requiem
  Relic", ... displays): the OmegaIsotope pickup icon.
- `tierless` — NAMED single-variant relics keep a per-name
  row (today exactly one: "Requiem Eterna" ->
  RelicImmortalC). They wear a real relic icon, unlike the
  generic base items — including the bare "Requiem" table
  relic, whose icon stays the pickup texture, as shipped.

The join (`model::relic_icon_urls`) still enumerates base
names from item-names — tier presence, the case-folded
keys, and the dual-keyed Vanguard aliasing are unchanged —
but resolves each base through the classes: the finest
tier the base HAS whose (era, tier) class row exists (with
the complete bake, its Intact class — the same texture
every one of its projections carried in the per-item map),
else the tier-less rows. Fallback now lives at class
granularity: a missing class row substitutes the next
tier, mirroring the old missing-texture substitute.
`load_relic_textures` returns the new `RelicTextureClasses`
struct (serde-shaped like the file; tier-less keys fold on
load); fetch's wrapper and the render fixture test follow.

**Equivalence gate.** Before the rewrite, the OLD code ran
against the OLD bake and its per-name output for the 800
relic-table names was frozen into
`fixtures/relic-icon-baseline.json`. (An independent Python
port of the old join reproduced that baseline bit-for-bit —
809 map keys, zero diffs — before the Rust dump was even
taken.) The new test
`relic_icon_urls_match_the_pre_class_table_baseline` loads
the real item-names + new bake and asserts all 800 names
resolve to their frozen URLs. The edge cases ride along in
the 800: Requiem II/III/IV numeral folding, the dual-keyed
Vanguard bases (C1/E1/M1/P1), tier-less "Requiem Eterna",
and the bare "Requiem" on the pickup icon. **800/800,
no exceptions.** The unit gate from pass 48 was re-shaped
to class-table inputs with its assertions intact (Intact
preference, class-level tier substitution, Vanguard alias,
numeral folding, Eterna, monogram-for-absent).

Refresh honesty: `data/cache/README.md` now describes the
class shape and a refresh step 3 that groups the manifest
subset by (era, tier) and REQUIRES the one-texture-per-group
singularity — if a future manifest breaks it, the
procedure says stop and re-measure rather than bake.

## 2. Coverage phrase (`src/render.rs`, `src/plan.rs`)

One production site renders the compact phrase — the
crack-queue row in `crack_queue_panel`: `+{covers} also` ->
`+{covers} other needed`, gated on `covers > 0` as before,
count untouched (pass 44's Need-only `covers`). The queue
render pin now asserts "Systems · +2 other needed"; the
plan/render doc comments that quote the copy follow. (The
long-form chunk copy — "Covers N other needed part(s) in
the same relic." — already used these words; the compact
row now matches it. No other copy changed.)

## 3. Pip titles (`src/render.rs`)

`seg_html` — the shared pip renderer — now emits
`<i class="seg ..." title="<part name>"></i>` for every
segment, the name attribute-escaped via the existing
`esc`. A multi-unit part's segments all carry the part's
name (they are the same part, per unit). Because the
renderer is shared, the titles landed on **Backlog** (the
`.bg` sections Seth asked about), **Hunt cards**, and the
**First Crack goal cards** — the brief anticipated exactly
this for the shared renderer. The existing exact-string
pip assertions were updated to the titled shape (Have,
⍻-dimmed, Need, and the segmented ×2 counts), and a new
pin, `pip_titles_name_the_part_exactly`, asserts the title
is the part name exactly — Seth's "Lavos Prime
Neuroptics" — with an &-bearing name ("Silva & Aegis Prime
Blade") escaping to `&amp;` in the attribute.

## Gates

- Suite: **420 passed / 0 failed** (lib 392; golden 15;
  packaging 7; resident 5; tray_quit 1), +2 over v0.5.46's
  418 — the equivalence gate and the pip-title pin are
  new; the class-join gate replaces the manifest-join gate
  one for one. Census/reward gates untouched.
- `cargo build --release --locked --offline` green
  pre-tag; the Cargo.lock diff is the version line only.

Live verification owed: Seth hovers a Backlog pip and
sees the part name; the queue shows "+N other needed" in
the wild; overlay relic headers unchanged (spot-check a
Vanguard and the Requiem Eterna header against the old
build if desired — the gate already proves the URLs).
