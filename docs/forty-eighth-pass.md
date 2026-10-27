# Forty-eighth pass — real relic icons in the search overlay

Verification addendum: #114.

## The ask

Pass 47 shipped the overlay with one data-forced substitution:
the relic result header carried a letter monogram chip because
no relic image source existed in the app's data (data/icons.json
is gear-only, and the relic tables carry no image field). Seth
supplied the source and the access procedure (2026-10-27): DE's
Public Export. This pass closes exactly that gap.

## Seth's source specification

- `ExportManifest.json` pairs every item's `uniqueName` with a
  `textureLocation`; `http://content.warframe.com/PublicExport/`
  + textureLocation is the item's image asset (512×512; older
  assets 512×342 — variance is fine at chip size).
- Two-server access: the language index lives on the ORIGIN
  server over HTTPS
  (`https://origin.warframe.com/PublicExport/index_en.txt.lzma`,
  LZMA-compressed lines of `<file>!<hash>`); everything else —
  manifests and textures — comes from the CONTENT server
  (`http://content.warframe.com/PublicExport/Manifest/<file>!<hash>`
  for manifests). Hashed names rotate on DE updates, so the
  index is consulted at refresh time; a hashed asset itself may
  be cached forever (its contents changing means a new name).
- The repo already documents this pattern (data/decode/README.md,
  addendum #51) — followed here.

## What shipped

**Baked data** (`data/cache/relic-textures.json`, refresh
procedure in `data/cache/README.md`): projection uniqueName ->
textureLocation — the ExportManifest subset covering the 3,204
projections `item-names.json` knows. Baked, never fetched at
runtime, per the addendum #51 discipline for official-source
data. The textures are NOT downloaded or cached locally; the
rendered blocks hotlink the content server, exactly as gear
icons hotlink their CDN.

**The join** (`model::relic_icon_urls`, built once per report
generation in report.rs from `fetch::load_relic_textures` +
the `item_names` map the planner already loads):

relic display name -> projection uniqueName (via item-names'
tier-suffixed display names, "Meso C11 Intact" -> base "Meso
C11") -> textureLocation -> absolute content-server URL.

- Tier: ONE icon per relic, the **Intact** projection's
  texture (per Seth's pass-47 ruling the result ignores
  refinement tier); Exceptional -> Flawless -> Radiant
  substitute only when a finer tier carries no texture.
- Two source naming quirks are normalized in the join, never
  guessed around: relic-tables roman numerals are title-case
  ("Requiem Ii") where item-names uses upper ("Requiem II") —
  the map is keyed case-folded; and the Vanguard relics are
  "Axi Vanguard C1" in the tables but plain "Vanguard C1" as
  projections — Vanguard bases are also keyed with the "Axi "
  prefix. A tier-less single-variant display ("Requiem Eterna
  Relic") keys its base minus " Relic" at the Intact slot.
- Resolution coverage on current data: **800/800** relic
  names. Colliding projections (5 base+tier groups, e.g. two
  Sevagoth Prime variants of Lith G12) share the identical
  texture, so the join's output is deterministic.

**Render** (`render::search_relic_html`): the header chip takes
the gear icons' exact markup — `<img>` over the monogram
initial, `onerror` removes the image — fed by
`SearchData::relic_icons`. A relic the join does not resolve
keeps the monogram alone; no URL is ever invented. Gear icons,
reward rows (still text + rarity colour), and every other
pass-47 behaviour are unchanged.

**Freshness / cache behaviour**: there is no runtime fetch and
no runtime TTL. The texture map refreshes by re-baking per
data/cache/README.md, which always starts from the LIVE index
— when DE rotates the manifest hash, the next bake picks up
the new manifest and its current texture locations; a stale
bake is never pinned by the app, and hash-addressed texture
URLs stay valid for as long as DE serves them. This is the
same manual cadence as the other baked DE data (data/decode/).

## Live spot-check (2026-10-27, during the build)

- Index: `GET https://origin.warframe.com/PublicExport/index_en.txt.lzma`
  -> HTTP 200, 16 lines; the manifest line is
  `ExportManifest.json!00_ctbDG9gRd4GzkO0LrBP9Kw`.
- Manifest: `GET http://content.warframe.com/PublicExport/Manifest/ExportManifest.json!00_ctbDG9gRd4GzkO0LrBP9Kw`
  -> HTTP 200, 3,838,178 bytes, 20,153 entries.
- Meso C11 Intact = `/Lotus/Types/Game/Projections/T2VoidProjectionCitrinePrimeABronze`
  -> textureLocation `/Lotus/Interface/Icons/Relics/RelicMesoD.png!00_YnW-MWrqd3JE32EROH2M1g`.
- Texture: `GET http://content.warframe.com/PublicExport/Lotus/Interface/Icons/Relics/RelicMesoD.png!00_YnW-MWrqd3JE32EROH2M1g`
  -> HTTP 200, PNG signature, 512×512, 77,697 bytes. (The
  content server sends no Content-Type header; the payload is
  a PNG by signature and dimensions.)
- End-to-end on real data: the offline demo's sidecar renders
  861 blocks (800 relics + 61 items) and all 800 relic blocks
  carry a content-server `<img>`; the Meso C11 block carries
  exactly the URL above.

## Gates

- `model::tests::relic_icon_urls_resolve_through_the_manifest_join`:
  fixture item-names + fixture manifest textures — Intact
  preferred, Radiant substitute when Intact has no texture,
  Vanguard alias, roman-numeral case-fold, tier-less variant,
  unresolvable relic absent.
- `render::tests::search_index_relic_block_shows_the_real_icon`:
  the Meso C11 block contains the resolved `<img>` URL and NOT
  the monogram span; an unresolvable relic renders the
  monogram and no `<img>`.
- Pass-47 gates green untouched (matching tests, relic spec,
  item block, sidecar serving); full suite green (see
  addendum #114 for totals); census gates untouched.
