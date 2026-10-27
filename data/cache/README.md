# Baked cache files

Most files here are upstream snapshots refreshed by their own
fetchers. `relic-textures.json` is different: it is BAKED from
DE's **Public Export** and never fetched by the app at runtime
(the addendum #51 discipline — official-source data is baked in
at data-refresh time).

## relic-textures.json (pass 48)

Projection uniqueName -> `textureLocation`: the subset of
`ExportManifest.json` covering every projection that
`item-names.json` knows (all four refinement tiers). The app
joins it at report time (`model::relic_icon_urls`): relic name
-> projection uniqueName via item-names (Intact preferred,
then Exceptional, Flawless, Radiant) -> textureLocation ->
`http://content.warframe.com/PublicExport/<textureLocation>`,
the icon the search overlay's relic header shows.

Refresh procedure (two servers, per Seth 2026-10-27):

1. Fetch the language index from the ORIGIN server, HTTPS:
   `https://origin.warframe.com/PublicExport/index_en.txt.lzma`
   (LZMA-compressed lines of `<file>!<hash>`). Find the
   `ExportManifest.json!<hash>` line.
2. Fetch that manifest from the CONTENT server:
   `http://content.warframe.com/PublicExport/Manifest/ExportManifest.json!<hash>`
   Shape: `{"Manifest": [{"uniqueName": ..., "textureLocation": ...}]}`.
3. Keep the entries whose uniqueName appears in
   `item-names.json` as a `/Lotus/Types/Game/Projections/` key;
   write them as one flat JSON object, uniqueName ->
   textureLocation, keys sorted.

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
tables it resolves 800/800 relic names.
