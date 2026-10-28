# Baked cache files

Most files here are upstream snapshots refreshed by their own
fetchers. `relic-textures.json` is different: it is BAKED from
DE's **Public Export** and never fetched by the app at runtime
(the addendum #51 discipline — official-source data is baked in
at data-refresh time).

## relic-textures.json (pass 48; class-table shape, pass 52)

The relic-icon CLASS table, baked from `ExportManifest.json`.
DE ships one relic icon per (era, refinement tier): measured
against the pass-48 per-projection bake, all 3,204 records
collapse to 21 distinct textures, so the bake is keyed by
class instead of by projection uniqueName:

```json
{
  "classes":  { "Lith": {"Intact": "<textureLocation>", ...},
                "Meso": {...}, "Neo": {...}, "Axi": {...},
                "Vanguard": {...}, "Requiem": {...} },
  "pickup":   "<textureLocation>",
  "tierless": { "Requiem Eterna": "<textureLocation>" }
}
```

- `classes[era][tier]`: the era's icon at that refinement
  tier (Intact / Exceptional / Flawless / Radiant). The
  Vanguard row carries the Axi textures and the Requiem row
  the RelicImmortal ones — those eras share an icon family,
  which the manifest's per-projection records confirm group
  by group.
- `pickup`: the texture of the un-tiered GENERIC base
  projection items (the plain "Lith Relic", "Requiem
  Relic", ... displays) — all of them wear the shared
  OmegaIsotope pickup icon.
- `tierless`: NAMED single-variant relics (no refinement
  tiers; today only "Requiem Eterna") keep a per-name row —
  unlike the generic base items they wear a real relic icon.

The app joins it at report time (`model::relic_icon_urls`):
relic name -> era + finest tier it has via item-names ->
class textureLocation ->
`http://content.warframe.com/PublicExport/<textureLocation>`,
the icon the search overlay's relic header shows. The
equivalence gate (`relic_icon_urls_match_the_pre_class_table_baseline`
in src/model.rs) pins all 800 relic names to the URLs the
old per-item bake produced (fixtures/relic-icon-baseline.json).

Refresh procedure (two servers, per Seth 2026-10-27):

1. Fetch the language index from the ORIGIN server, HTTPS:
   `https://origin.warframe.com/PublicExport/index_en.txt.lzma`
   (LZMA-compressed lines of `<file>!<hash>`). Find the
   `ExportManifest.json!<hash>` line.
2. Fetch that manifest from the CONTENT server:
   `http://content.warframe.com/PublicExport/Manifest/ExportManifest.json!<hash>`
   Shape: `{"Manifest": [{"uniqueName": ..., "textureLocation": ...}]}`.
3. Take the entries whose uniqueName appears in
   `item-names.json` as a `/Lotus/Types/Game/Projections/`
   key, and group them by (era, tier): era = the first word
   of the projection's item-names display base ("Meso C11
   Intact" -> Meso), tier = the display name's tier suffix.
   Every group MUST name exactly one textureLocation — that
   singularity is what makes the class table sound; if a
   future manifest breaks it, stop and re-measure instead
   of baking. Write the groups as `classes`. Tier-less
   projections (display ends in " Relic", no tier suffix):
   a bare-era base ("Lith", "Requiem", ...) must name the
   one shared pickup texture -> `pickup`; a named relic
   (multi-word base, e.g. "Requiem Eterna") goes to
   `tierless` under its display base name.

Freshness: the hashed manifest name rotates whenever DE ships
an export update, so a refresh ALWAYS starts from the live
index (step 1) — never reuse a remembered manifest URL. The
texture assets themselves are hash-addressed
(`.../RelicMesoD.png!00_...`) and may be cached forever; if a
texture's contents change it gets a new hashed name in the new
manifest, so a stale bake can only go stale by missing NEW
content, which the next re-bake picks up. There is no runtime
TTL: this file refreshes on the same manual cadence as the
other baked DE data (cf. data/decode/).

Baked 2026-10-27 from manifest
`ExportManifest.json!00_ctbDG9gRd4GzkO0LrBP9Kw`: 3,204
projection entries; joined against item-names and the relic
tables it resolves 800/800 relic names. Collapsed to the
class table 2026-10-28 (pass 52) from that same bake — no
new manifest fetch: 24 (era, tier) class rows + the pickup
texture + 1 tier-less row, 21 distinct textures, still
800/800, equivalence-pinned per above.
