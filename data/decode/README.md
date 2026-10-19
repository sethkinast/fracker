# Baked decode dictionaries (addendum #51)

Static decodes for DE's raw worldstate schema, baked from DE's
**Public Export** — the app never fetches dictionaries at runtime.

Refresh procedure:
1. Fetch the index: `https://origin.warframe.com/PublicExport/index_en.txt.lzma`
   (LZMA-compressed lines of `<file>!<hash>`).
2. Fetch payloads from the OTHER host pattern:
   `http://content.warframe.com/PublicExport/Manifest/<file>!<hash>`
3. Sources used here (2026-10-19):
   - `nodes.json` ← ExportRegions_en.json (269 nodes: name, system,
     missionIndex = the node's DEFAULT mission type index, factionIndex).
   - `nightwave.json` ← the ExportNightwave slice inside
     ExportSortieRewards_en.json (226 challenges by uniqueName).
   - `mission_types.json` ← hand-seeded MT_* display table (see its _note).

Note: a node's missionIndex is its default type; alerts/fissures carry
their own MissionType per instance and override it.
