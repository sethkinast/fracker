//! Shared data model: relic/drop tables, prime roster, targets,
//! world-state subset, profile snapshot, relic stock.
//!
//! Thin-layer principle (BRIEF §16 decision log): game knowledge lives
//! in maintained remote datasets (Xuerian, warframestat, WFCD). This
//! module only models their shapes.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tolerant primitives: the datasets use `null` where a default belongs.
// ---------------------------------------------------------------------------

/// Deserialize `null` (or missing) as `T::default`.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Refinement
// ---------------------------------------------------------------------------

/// Relic refinement tier. Trace costs: 0/25/50/100 (BRIEF §17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Refinement {
    Intact,
    Exceptional,
    Flawless,
    Radiant,
}

impl Refinement {
    pub fn all() -> [Refinement; 4] {
        [
            Refinement::Intact,
            Refinement::Exceptional,
            Refinement::Flawless,
            Refinement::Radiant,
        ]
    }

    /// Trace costs live in `data/tuning/refinement.toml`
    /// (`RefinementTuning::trace_cost`); this name table is protocol,
    /// not content.
    pub fn name(self) -> &'static str {
        match self {
            Refinement::Intact => "Intact",
            Refinement::Exceptional => "Exceptional",
            Refinement::Flawless => "Flawless",
            Refinement::Radiant => "Radiant",
        }
    }
}

// ---------------------------------------------------------------------------
// Relic tables (Xuerian relictables.json)
// ---------------------------------------------------------------------------

/// One farm node entry from the top-level `sources` map.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FarmNode {
    #[serde(default, deserialize_with = "null_default")]
    pub planet: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub mode: String,
    #[serde(default, deserialize_with = "null_default")]
    pub mode_name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub faction_name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub min_enemy_level: u32,
    #[serde(default, deserialize_with = "null_default")]
    pub max_enemy_level: u32,
}

/// Per-table drop info for one relic: rotation -> chance %.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelicSourceTable {
    #[serde(default)]
    pub total_rotations: u32,
    #[serde(default)]
    pub rotations: HashMap<String, f64>,
}

/// One relic entry: rewards with per-refinement chances, vault flag,
/// and farm sources (drop table -> rotation chances).
#[derive(Debug, Clone, Deserialize)]
pub struct RelicEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub rewards: Vec<RelicReward>,
    #[serde(default)]
    pub vaulted: bool,
    #[serde(default, deserialize_with = "sources_map_or_empty")]
    pub sources: HashMap<String, RelicSourceTable>,
}

/// Vaulted relics carry `"sources": []` while dropping relics carry a
/// table map — accept both, normalizing lists to empty.
fn sources_map_or_empty<'de, D>(
    d: D,
) -> Result<HashMap<String, RelicSourceTable>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{SeqAccess, Visitor};
    use std::fmt;
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = HashMap<String, RelicSourceTable>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a source-table map or an empty list")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut m: M,
        ) -> Result<Self::Value, M::Error> {
            let mut out = HashMap::new();
            while let Some((k, v)) = m.next_entry::<String, RelicSourceTable>()? {
                out.insert(k, v);
            }
            Ok(out)
        }
        fn visit_seq<S: SeqAccess<'de>>(self, _s: S) -> Result<Self::Value, S::Error> {
            Ok(HashMap::new())
        }
    }
    d.deserialize_any(V)
}

/// One reward row. `chance` maps refinement name -> percent.
#[derive(Debug, Clone, Deserialize)]
pub struct RelicReward {
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub chance: HashMap<String, f64>,
}

impl RelicReward {
    /// Chance (%) of this reward at the given refinement.
    /// Missing tiers fall back to Intact (older rows only list Intact).
    pub fn chance_at(&self, r: Refinement) -> f64 {
        if let Some(v) = self.chance.get(r.name()) {
            return *v;
        }
        self.chance.get("Intact").copied().unwrap_or(0.0)
    }
}

impl RelicEntry {
    /// Era = first token of the relic name ("Lith"/"Meso"/"Neo"/"Axi"/"Requiem").
    /// "Requiem" relics have no era fissure match except Requiem tier itself.
    pub fn era(&self) -> &str {
        self.name.split_whitespace().next().unwrap_or("")
    }

    /// Chance (%) of `part` in THIS relic at refinement `r`.
    /// Rarity is a property of the (part, relic) pair (BRIEF §17).
    pub fn chance_for(&self, part: &str, r: Refinement) -> f64 {
        self.rewards
            .iter()
            .find(|rw| rw.name == part)
            .map(|rw| rw.chance_at(r))
            .unwrap_or(0.0)
    }

    pub fn contains(&self, part: &str) -> bool {
        self.rewards.iter().any(|rw| rw.name == part)
    }
}

/// Full relic dataset.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RelicTables {
    #[serde(default)]
    pub relics: HashMap<String, RelicEntry>,
    #[serde(default)]
    pub sources: HashMap<String, Vec<FarmNode>>,
}

/// Bake-time era corrections for the relic tables (addendum #66).
///
/// The upstream WFCD/Xuerian cache still labels four Perita Rebellion
/// relics `Vanguard C1/E1/M1/P1` even though "Vanguard" is not a fissure
/// era. Seth established they are legacy Axi relics. The corrected names
/// keep the qualifier so they stay distinct from the existing Axi
/// C1/E1/M1/P1 entries in the same table.
pub const LEGACY_RELIC_ERA_REMAP: [(&str, &str); 4] = [
    ("Vanguard C1", "Axi Vanguard C1"),
    ("Vanguard E1", "Axi Vanguard E1"),
    ("Vanguard M1", "Axi Vanguard M1"),
    ("Vanguard P1", "Axi Vanguard P1"),
];

/// Apply [`LEGACY_RELIC_ERA_REMAP`] to a relic-tables payload in place.
/// Renames the top-level relic key and its `name`, plus the per-item
/// relic map keys, so every consumer sees the corrected era token.
/// Returns the number of relic entries renamed.
pub fn remap_legacy_relic_names(root: &mut serde_json::Value) -> usize {
    let mut renamed = 0;
    for (from, to) in LEGACY_RELIC_ERA_REMAP {
        if let Some(relics) = root.get_mut("relics").and_then(|v| v.as_object_mut()) {
            if let Some(mut entry) = relics.remove(from) {
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert("name".to_owned(), serde_json::Value::String(to.to_owned()));
                }
                relics.insert(to.to_owned(), entry);
                renamed += 1;
            }
        }
        if let Some(items) = root.get_mut("items").and_then(|v| v.as_object_mut()) {
            for item in items.values_mut() {
                if let Some(relics) =
                    item.get_mut("relics").and_then(|v| v.as_object_mut())
                {
                    if let Some(value) = relics.remove(from) {
                        relics.insert(to.to_owned(), value);
                    }
                }
            }
        }
    }
    renamed
}

/// Bake a relic-tables payload: apply the era remap and return the
/// normalized cache. This is the one maintenance path that rewrites the
/// committed seed (`fracker --bake-relics`), so the artifact is never
/// hand-edited.
///
/// The rename is a token-preserving replacement rather than a
/// parse/serialize round-trip: the upstream file's key order and
/// formatting stay intact, so regenerating the cache yields a minimal,
/// reviewable diff.
pub fn bake_relic_tables(text: &str) -> Result<String, String> {
    let mut out = text.to_owned();
    for (from, to) in LEGACY_RELIC_ERA_REMAP {
        out = out.replace(
            &format!("\"{from}\""),
            &format!("\"{to}\""),
        );
    }
    // Validate that the rewrite is still a well-formed relic payload.
    serde_json::from_str::<serde_json::Value>(&out)
        .map_err(|e| format!("relictables: {e}"))?;
    Ok(out)
}

impl RelicTables {
    pub fn from_json(text: &str) -> Result<Self, String> {
        // Normalize the legacy era alias on every load, including a fresh
        // upstream pull that still carries the pseudo-era.
        let mut root: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("relictables: {e}"))?;
        remap_legacy_relic_names(&mut root);
        serde_json::from_value(root).map_err(|e| format!("relictables: {e}"))
    }

    /// All relics containing `part`, with the per-(part, relic) Intact chance.
    pub fn relics_for_part(&self, part: &str) -> Vec<(&RelicEntry, f64)> {
        let mut out: Vec<(&RelicEntry, f64)> = self
            .relics
            .values()
            .filter(|r| r.contains(part))
            .map(|r| (r, r.chance_for(part, Refinement::Intact)))
            .collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.name.cmp(&b.0.name))
        });
        out
    }

    pub fn unvaulted_relics(&self) -> Vec<&RelicEntry> {
        let mut out: Vec<&RelicEntry> = self
            .relics
            .values()
            .filter(|r| !r.vaulted)
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

// ---------------------------------------------------------------------------
// Prime roster (Xuerian wishlist.json)
// ---------------------------------------------------------------------------

fn default_required() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
pub struct RosterPart {
    #[serde(default)]
    pub name: String,
    /// Copies of this part the foundry recipe consumes (pass 42,
    /// addendum #107). The roster feed carries names only, so this
    /// defaults to 1 and is populated from `data/required.toml`
    /// (see [`Roster::apply_required_counts`]); only doubled parts
    /// (dual-wield components and kin) ever exceed 1.
    #[serde(default = "default_required")]
    pub required: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RosterItem {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vaulted: bool,
    #[serde(default)]
    pub parts: Vec<RosterPart>,
    #[serde(default, rename = "type")]
    pub kind: String,
}

impl RosterItem {
    /// Warframes rank before weapons (BRIEF §10).
    pub fn is_warframe(&self) -> bool {
        self.kind.eq_ignore_ascii_case("warframe")
    }

    /// Companion gear (prime Sentinels from the universe feed).
    pub fn is_companion(&self) -> bool {
        matches!(
            self.kind.to_lowercase().as_str(),
            "sentinel" | "sentinels" | "pet" | "pets" | "companion" | "companions"
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct Roster {
    pub items: Vec<RosterItem>,
}

impl Roster {
    pub fn from_json(text: &str) -> Result<Self, String> {
        let items: Vec<RosterItem> =
            serde_json::from_str(text).map_err(|e| format!("wishlist: {e}"))?;
        Ok(Self { items })
    }

    pub fn by_name(&self, name: &str) -> Option<&RosterItem> {
        self.items.iter().find(|i| i.name == name)
    }

    /// Apply required build quantities (pass 42): part display
    /// name -> copies the recipe consumes. Names absent from the
    /// map keep the default of 1; a map value below 1 is clamped
    /// to 1 (a listed part is consumed at least once by
    /// definition, and 0 would make satisfaction vacuous).
    pub fn apply_required_counts(&mut self, counts: &HashMap<String, u32>) {
        for item in &mut self.items {
            for part in &mut item.parts {
                if let Some(&n) = counts.get(&part.name) {
                    part.required = n.max(1);
                }
            }
        }
    }

    /// Parentage: part display name -> parent equipment names.
    /// A part want is done iff its parent equipment is owned (BRIEF §10).
    pub fn parentage(&self) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for item in &self.items {
            for part in &item.parts {
                map.entry(part.name.clone())
                    .or_default()
                    .push(item.name.clone());
            }
        }
        map
    }

    /// Parent equipment for a want. Whole-equipment wants (e.g. a part
    /// name that is itself gear) resolve to themselves when present.
    pub fn parents_of(&self, want: &str) -> Vec<String> {
        let mut out = Vec::new();
        if self.by_name(want).is_some() {
            out.push(want.to_owned());
        }
        for item in &self.items {
            if item.parts.iter().any(|p| p.name == want)
                && !out.contains(&item.name)
            {
                out.push(item.name.clone());
            }
        }
        out
    }
}

/// The built (crafted) form of a roster part name, when the part is a
/// COMPONENT blueprint: "<Gear> <Component> Blueprint" displays on the
/// Parts screen without the suffix once the component is crafted
/// ("Gauss Prime Chassis Blueprint" -> "Gauss Prime Chassis"), and
/// both forms are first-class capture stock (addendum #89). The
/// gear's own "<Gear> Blueprint" has no loose built counterpart —
/// its built result is the gear itself — so it yields None, as does
/// any part that is not a blueprint at all.
pub fn built_component_form(gear: &str, part: &str) -> Option<String> {
    if part == format!("{gear} Blueprint") {
        return None;
    }
    part.strip_suffix(" Blueprint").map(str::to_owned)
}

// ---------------------------------------------------------------------------
// Required build quantities (data/required.toml)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
struct RequiredFile {
    #[serde(default)]
    required: HashMap<String, u32>,
}

/// Parse `data/required.toml` (pass 42, addendum #107): the
/// curated table of parts whose foundry recipe consumes more
/// than one copy, extracted from the WFCD items dataset (the
/// roster feed itself carries no quantities). Returns part
/// display name -> required copies; parts absent from the table
/// require exactly 1.
pub fn load_required_counts(text: &str) -> Result<HashMap<String, u32>, String> {
    let file: RequiredFile =
        toml::from_str(text).map_err(|e| format!("required.toml: {e}"))?;
    Ok(file.required)
}

// ---------------------------------------------------------------------------
// Companion universe manifest (data/companions.toml)
// ---------------------------------------------------------------------------

/// Companion-dataset manifest: the WFCD category files behind the pet
/// universe and the Prime-named entries that are members but never
/// wants (non-masterable). The exclusion list itself lives in
/// `data/exclusions.toml` (see [`UniverseScrub`]).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CompanionsManifest {
    #[serde(default)]
    pub unmasterable: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct CompanionsFile {
    #[serde(default)]
    pub unmasterable: CompanionsNames,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct CompanionsNames {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub reason: String,
}

impl CompanionsManifest {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: CompanionsFile =
            toml::from_str(text).map_err(|e| format!("companions: {e}"))?;
        Ok(Self {
            unmasterable: file.unmasterable.names,
        })
    }
}

// ---------------------------------------------------------------------------
// Universe scrub (data/exclusions.toml — sixth pass §1)
// ---------------------------------------------------------------------------

/// Data-owned corrections to the item universe: entries that must never
/// appear (with a per-entry reason) and duplicate names that collapse
/// onto one canonical spelling. Applied everywhere the universe is
/// consumed (backlog wants, category lists, OCR universe).
#[derive(Debug, Clone, Default)]
pub struct UniverseScrub {
    /// Excluded name -> reason.
    excluded: HashMap<String, String>,
    /// Non-canonical name -> canonical spelling.
    aliases: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ScrubFile {
    #[serde(default)]
    exclude: Vec<ScrubEntry>,
    #[serde(default)]
    alias: Vec<ScrubAlias>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ScrubEntry {
    #[serde(default)]
    name: String,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ScrubAlias {
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
}

impl UniverseScrub {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: ScrubFile =
            toml::from_str(text).map_err(|e| format!("exclusions: {e}"))?;
        let mut excluded = HashMap::new();
        for e in file.exclude {
            if !e.name.trim().is_empty() {
                excluded.insert(e.name, e.reason);
            }
        }
        let mut aliases = HashMap::new();
        for a in file.alias {
            if !a.from.trim().is_empty() && !a.to.trim().is_empty() {
                aliases.insert(a.from, a.to);
            }
        }
        Ok(Self { excluded, aliases })
    }

    /// A name that is excluded outright.
    pub fn is_excluded(&self, name: &str) -> bool {
        self.excluded.contains_key(name)
    }

    /// The recorded reason for an excluded name.
    pub fn reason(&self, name: &str) -> Option<&str> {
        self.excluded.get(name).map(String::as_str)
    }

    /// Canonical spelling for `name` (identity when not an alias).
    pub fn canonical(&self, name: &str) -> String {
        self.aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    }

    /// An excluded name or any of its parts (`"<name> …"`).
    fn covers(&self, want: &str) -> bool {
        self.excluded
            .keys()
            .any(|x| want == x || want.starts_with(&format!("{x} ")))
    }

    /// Apply the scrub to one name list: drop excluded entries (and
    /// their parts), canonicalize duplicates. Order is preserved; an
    /// alias whose canonical spelling is already present collapses onto
    /// it. Non-alias duplicate names are left untouched (they are
    /// distinct universe entries).
    pub fn scrub_names(&self, names: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for n in names {
            if self.covers(n) {
                continue;
            }
            let c = self.canonical(n);
            if c != *n && out.iter().any(|x| *x == c) {
                continue;
            }
            out.push(c);
        }
        out
    }

    /// Planner wants (same rules as [`Self::scrub_names`], named for the
    /// call site).
    pub fn scrub_wants(&self, wants: &[String]) -> Vec<String> {
        self.scrub_names(wants)
    }

    /// Scrub every category list; empty categories are dropped.
    pub fn scrub_categories(
        &self,
        categories: &HashMap<String, Vec<String>>,
    ) -> HashMap<String, Vec<String>> {
        categories
            .iter()
            .map(|(k, v)| (k.clone(), self.scrub_names(v)))
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Arcane item set (data/arcanes.toml — sixth pass §5)
// ---------------------------------------------------------------------------

/// Arcane universe (`data/arcanes.toml`). A LABELED stopgap: the names
/// are transcribed from Seth's spoken debrief until his first Arcanes
/// inventory capture lands. Consumed as the arcane-name set for OCR
/// validation/routing (neither the profile nor EE.log carries arcane
/// state — addendum #34).
#[derive(Debug, Clone, Default)]
pub struct ArcaneManifest {
    /// Human-readable provenance label ("stopgap, not a capture").
    pub label: String,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ArcaneFile {
    #[serde(default)]
    stopgap: ArcaneStopgap,
    #[serde(default)]
    arcanes: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ArcaneStopgap {
    #[serde(default)]
    label: String,
}

impl ArcaneManifest {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: ArcaneFile =
            toml::from_str(text).map_err(|e| format!("arcanes: {e}"))?;
        Ok(Self {
            label: file.stopgap.label,
            names: file.arcanes,
        })
    }

    /// Names as a lookup set (for OCR universe + bucket routing).
    pub fn name_set(&self) -> std::collections::HashSet<String> {
        self.names.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Targets (Xuerian /api/load or seed data/targets.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Targets {
    #[serde(default)]
    pub wants: Vec<String>,
    #[serde(default)]
    pub has: Vec<String>,
    #[serde(default)]
    pub mastered: Vec<String>,
}

impl Targets {
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("targets: {e}"))
    }

    pub fn from_seed_json(text: &str) -> Result<Self, String> {
        Self::from_json(text)
    }
}

// ---------------------------------------------------------------------------
// World state (DE's own feed: api.warframe.com/cdn/worldState.php)
//
// Addendum #51: DE is the ONLY source. The app parses DE's raw schema into
// this model using decode tables baked under `data/decode/` (fetched at
// data-refresh time, never at runtime). No second source, no silent
// substitution: a failed fetch renders as unavailable/stale in the report
// stamp. The legacy warframestat endpoint is gone.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NodeDecode {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub system: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NightwaveDecode {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: u32,
    #[serde(default)]
    pub standing: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MissionTypeDecode {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub observed: bool,
}

/// Baked decode dictionaries (addendum #51). Loaded once per report run
/// from `data/decode/`; the app never fetches these at runtime.
#[derive(Debug, Clone, Default)]
pub struct Decodes {
    /// `SolNode*` -> display name + system.
    pub nodes: HashMap<String, NodeDecode>,
    /// Nightwave challenge uniqueName -> name/description/required/standing.
    pub nightwave: HashMap<String, NightwaveDecode>,
    /// `MT_*` -> display name.
    pub mission_types: HashMap<String, MissionTypeDecode>,
    /// Hub node key -> display location (evidenced entries only).
    pub hubs: HashMap<String, String>,
}

impl Decodes {
    pub fn load(data_dir: &std::path::Path) -> Result<Self, String> {
        let read = |name: &str| -> Result<String, String> {
            let path = data_dir.join("decode").join(name);
            std::fs::read_to_string(&path)
                .map_err(|e| format!("decode {}: {e}", path.display()))
        };
        // mission_types.json carries a `_note` string alongside the MT
        // entries; skip any non-object/underscore key rather than failing.
        let mission_types: HashMap<String, MissionTypeDecode> =
            serde_json::from_str::<HashMap<String, serde_json::Value>>(&read(
                "mission_types.json",
            )?)
            .map_err(|e| format!("decode mission_types: {e}"))?
            .into_iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .filter_map(|(k, v)| {
                serde_json::from_value(v)
                    .ok()
                    .map(|d| (k, d))
            })
            .collect();
        Ok(Self {
            nodes: serde_json::from_str(&read("nodes.json")?)
                .map_err(|e| format!("decode nodes: {e}"))?,
            nightwave: serde_json::from_str(&read("nightwave.json")?)
                .map_err(|e| format!("decode nightwave: {e}"))?,
            mission_types,
            hubs: serde_json::from_str(&read("hubs.json")?)
                .map_err(|e| format!("decode hubs: {e}"))?,
        })
    }

    pub fn node(&self, key: &str) -> Option<&NodeDecode> {
        self.nodes.get(key)
    }

    pub fn mission_type(&self, key: &str) -> Option<&str> {
        self.mission_types.get(key).map(|d| d.name.as_str())
    }

    pub fn hub(&self, key: &str) -> Option<&str> {
        self.hubs.get(key).map(String::as_str)
    }
}

/// Camel-split the tail of a DE language/<uniqueName> key into the display
/// vocabulary the recorded warframestat fixtures used:
/// `/Lotus/.../RoadToTauEventResource` -> "Road To Tau Event Resource",
/// `EventAlertTitle` -> "Event Alert Title", `MPVProteaPrimeSinglePack` ->
/// "M P V Protea Prime Single Pack". This is presentation of the feed's own
/// key, never a guessed game name.
pub fn humanize_language_key(key: &str) -> String {
    let tail = key.rsplit('/').next().unwrap_or(key);
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in tail.chars() {
        if ch.is_ascii_uppercase() && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words.join(" ")
}

fn push_gap(gaps: &mut Vec<String>, key: &str) {
    if !key.is_empty() && !gaps.iter().any(|g| g == key) {
        gaps.push(key.to_owned());
    }
}

/// One DE date field: `{"$date":{"$numberLong":"<unix ms>"}}` -> RFC 3339.
fn de_date(v: Option<&serde_json::Value>) -> String {
    let ms = v
        .and_then(|v| v.get("$date"))
        .and_then(|d| d.get("$numberLong"))
        .and_then(|s| s.as_str())
        .and_then(|s| s.parse::<i64>().ok());
    match ms.and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis) {
        Some(dt) => dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        None => String::new(),
    }
}

/// One DE Mongo id field: `{"$oid":"..."}`.
fn de_oid(v: Option<&serde_json::Value>) -> String {
    v.and_then(|v| v.get("$oid"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_owned()
}

fn as_array<'a>(v: Option<&'a serde_json::Value>) -> &'a [serde_json::Value] {
    v.and_then(|v| v.as_array()).map(Vec::as_slice).unwrap_or(&[])
}

/// A section the planner/renderer consumes. DE's payload is a fixed
/// schema, so a missing key is a rename/shape change, not "no data";
/// record it as a gap and let the caller keep the empty default
/// (addendum #56b: no silent empty plan).
fn required_section<'a>(
    root: &'a serde_json::Value,
    key: &str,
    gaps: &mut Vec<String>,
) -> Option<&'a serde_json::Value> {
    let value = root.get(key);
    if value.is_none() {
        push_gap(gaps, &format!("section:{key} missing"));
    }
    value
}

fn de_str<'a>(v: Option<&'a serde_json::Value>, key: &str) -> &'a str {
    v.and_then(|v| v.get(key))
        .and_then(|x| x.as_str())
        .unwrap_or("")
}

/// A DE string field the plan renders. An absent/renamed or empty field
/// degrades to `""` exactly as before, but the loss is recorded in the
/// worldstate gaps channel instead of disappearing (addendum #56b).
fn de_str_tracked<'a>(
    v: Option<&'a serde_json::Value>,
    key: &str,
    label: &str,
    gaps: &mut Vec<String>,
) -> &'a str {
    let field = v.and_then(|v| v.get(key));
    let out = de_str(v, key);
    if out.is_empty() {
        let state = match field {
            None => "missing",
            Some(f) if f.as_str() == Some("") => "empty",
            Some(_) => "not a string",
        };
        push_gap(gaps, &format!("{label}.{key} {state}"));
    }
    out
}

/// A DE date field the plan renders. `de_date` still degrades to `""`
/// (never a guessed date); failures land in the gaps channel with the
/// section/entry that lost them (addendum #56b).
fn de_date_tracked(
    v: Option<&serde_json::Value>,
    key: &str,
    label: &str,
    gaps: &mut Vec<String>,
) -> String {
    let field = v.and_then(|v| v.get(key));
    let out = de_date(field);
    if out.is_empty() {
        let state = if field.is_none() { "missing" } else { "unreadable" };
        push_gap(gaps, &format!("{label}.{key} {state}"));
    }
    out
}

/// Stable identity for a gap label: prefer the display node, then the
/// Mongo id, then a positional fallback so nothing is swallowed.
fn gap_identity(prefix: &str, node: &str, id: &str, index: usize) -> String {
    if !node.is_empty() {
        format!("{prefix}:{node}")
    } else if !id.is_empty() {
        format!("{prefix}:{id}")
    } else {
        format!("{prefix}:#{index}")
    }
}

/// Node display = `name (system)` as today; an unmapped key stays raw and
/// is recorded as a decode gap (never a guessed name).
fn decode_node_display(
    key: &str,
    decodes: &Decodes,
    gaps: &mut Vec<String>,
) -> String {
    if key.is_empty() {
        return String::new();
    }
    match decodes.node(key) {
        Some(n) => format!("{} ({})", n.name, n.system),
        None => {
            push_gap(gaps, key);
            key.to_owned()
        }
    }
}

fn decode_mission_type(
    key: &str,
    decodes: &Decodes,
    gaps: &mut Vec<String>,
) -> String {
    if key.is_empty() {
        return String::new();
    }
    match decodes.mission_type(key) {
        Some(name) => name.to_owned(),
        None => {
            push_gap(gaps, key);
            key.to_owned()
        }
    }
}

fn decode_hub(key: &str, decodes: &Decodes, gaps: &mut Vec<String>) -> String {
    if key.is_empty() {
        return String::new();
    }
    match decodes.hub(key) {
        Some(name) => name.to_owned(),
        None => {
            push_gap(gaps, key);
            key.to_owned()
        }
    }
}

/// DE fissure tier (`Modifier`/`ActiveMissionTier` `VoidT1..VoidT6`).
/// Crosswalk verified against warframestat `tierNum` 1..6 in the recorded
/// fixture (Lith/Meso/Neo/Axi/Requiem/Omnia).
fn decode_tier(key: &str, gaps: &mut Vec<String>) -> String {
    let tier = match key {
        "VoidT1" => "Lith",
        "VoidT2" => "Meso",
        "VoidT3" => "Neo",
        "VoidT4" => "Axi",
        "VoidT5" => "Requiem",
        "VoidT6" => "Omnia",
        _ => {
            if !key.is_empty() {
                push_gap(gaps, key);
            }
            key
        }
    };
    tier.to_owned()
}

/// Payload age beyond which the stamp must say STALE (~10 minutes).
pub const WORLDSTATE_STALE_AFTER_MINUTES: i64 = 10;

/// The worldstate stamp label. `fetched_live` is true only when this run
/// got a fresh HTTP 200 from DE; a disk-cache/fixture view is always
/// labeled STALE even when its payload is recent (addendum #51).
pub fn worldstate_status_label(
    world: &WorldState,
    now: chrono::DateTime<chrono::Utc>,
    fetched_live: bool,
) -> String {
    let Ok(stamp) = world.timestamp.parse::<chrono::DateTime<chrono::Utc>>() else {
        return "worldstate unavailable".to_owned();
    };
    let age = now.signed_duration_since(stamp);
    let secs = age.num_seconds().max(0);
    if fetched_live && age <= chrono::Duration::minutes(WORLDSTATE_STALE_AFTER_MINUTES) {
        format!("worldstate live ({secs}s)")
    } else {
        format!("worldstate STALE ({}m)", age.num_minutes().max(0))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fissure {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub mission_type: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub expiry: String,
    #[serde(default)]
    pub is_storm: bool,
    #[serde(default)]
    pub is_hard: bool,
}

impl Fissure {
    /// Omnia fissures accept every era (BRIEF §17).
    pub fn matches_era(&self, era: &str) -> bool {
        self.tier.eq_ignore_ascii_case("omnia") || self.tier.eq_ignore_ascii_case(era)
    }

    /// HARD gate (brief #2 §0): a fissure accepts a batch ONLY if it
    /// accepts EVERY era in the batch. Mixed-era batches route solely
    /// to live Omnia fissures. This filters candidates BEFORE any
    /// cascade scoring — never emit a line sending a relic to a
    /// fissure that cannot crack it.
    pub fn accepts_eras(&self, eras: &[&str]) -> bool {
        if eras.is_empty() {
            return false;
        }
        if self.tier.eq_ignore_ascii_case("omnia") {
            return true;
        }
        eras.iter().all(|e| self.tier.eq_ignore_ascii_case(e))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CountedItem {
    #[serde(default)]
    pub unique_name: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub count: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertReward {
    #[serde(default)]
    pub counted_items: Vec<CountedItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertMission {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub node: String,
    /// Internal node key (`SolNode175`). EE.log and DE both speak these,
    /// so alert completion matching keys on them exactly where the data
    /// allows (addendum #51 bonus).
    #[serde(default)]
    pub sol_node: String,
    #[serde(default, rename = "type")]
    pub mission_type: String,
    #[serde(default)]
    pub faction: String,
    #[serde(default)]
    pub reward: AlertReward,
    /// Older captures flattened counted items onto the mission itself;
    /// accepted here so both shapes parse.
    #[serde(default, rename = "countedItems")]
    pub counted_items_flat: Vec<CountedItem>,
}

impl AlertMission {
    /// Event-resource payouts live under `reward.countedItems` in the
    /// live API (flattened in older captures).
    pub fn counted_items(&self) -> &[CountedItem] {
        if self.reward.counted_items.is_empty() {
            &self.counted_items_flat
        } else {
            &self.reward.counted_items
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub tag: String,
    /// When this alert instance went live (worldstate `activation`). The
    /// twelfth-pass §4 activation guard: a completion may only corroborate
    /// an alert it could actually belong to — observed at/after this time.
    #[serde(default)]
    pub activation: String,
    /// True when DE sent an `Activation` field this parser could not read.
    /// The guard then refuses corroboration instead of failing open; a
    /// genuinely absent field stays the documented legacy path (empty
    /// string, not degraded). Set by `WorldState::from_de_json` only.
    #[serde(skip)]
    pub activation_degraded: bool,
    #[serde(default)]
    pub expiry: String,
    #[serde(default)]
    pub mission: AlertMission,
}

impl Alert {
    /// Time-limited EVENT alert (pass 4 Do Now §a): carries a counted
    /// event-resource payout. Gift-of-the-Lotus alerts (no counted
    /// payout, tag/description match) are a separate section.
    pub fn is_event(&self) -> bool {
        !self.mission.counted_items().is_empty() && !self.is_lotus()
    }

    /// Gift-of-the-Lotus alert (pass 4 Do Now §b): the feed tags them
    /// (`tag` carries the Lotus gift marker) or the mission
    /// description names the gift. Protocol heuristic, not content:
    /// match text is the feed's own vocabulary.
    pub fn is_lotus(&self) -> bool {
        let hay = format!("{} {}", self.tag, self.mission.description).to_lowercase();
        hay.contains("lotus") || hay.contains("gift")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultTraderItem {
    #[serde(default)]
    pub unique_name: String,
    #[serde(default)]
    pub item: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultTrader {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub activation: String,
    #[serde(default)]
    pub expiry: String,
    #[serde(default)]
    pub character: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub inventory: Vec<VaultTraderItem>,
}

impl VaultTrader {
    /// Normalized gear tokens from uniqueNames for rotation matching.
    /// e.g. "/Lotus/StoreItems/Powersuits/Ranger/IvaraPrime" -> "ivara prime".
    pub fn gear_tokens(&self) -> HashSet<String> {
        self.inventory
            .iter()
            .map(|i| normalize_gear_token(&i.unique_name))
            .filter(|t| !t.is_empty())
            .collect()
    }
}

/// "/Lotus/.../IvaraPrime" or "MPVIvaraPrimeSinglePack" -> "ivara prime".
pub fn normalize_gear_token(unique_name: &str) -> String {
    let tail = unique_name.rsplit('/').next().unwrap_or(unique_name);
    let s = tail.to_owned();
    // Split CamelCase-ish tokens; pack wrappers ("...SinglePack") still
    // contain the gear names, and matching is substring-based.
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_uppercase() && !cur.is_empty() {
            words.push(cur.clone());
            cur.clear();
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let joined = words.join(" ").to_lowercase();
    // Pack entries ("m p v protea ivara prime dual pack") still contain
    // the gear names; keep the whole token — matching is substring-based.
    joined
    .replace("m p v ", "")
    .replace(" single pack", "")
    .replace(" dual pack", "")
    .replace(" pack", "")
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ")
}

/// Community/event entry from world state `/pc/events` (event packs:
/// a community-gated set rides on the community-goal `health` progress
/// of the pack's event entry).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldEvent {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub health: Option<f64>,
    #[serde(default)]
    pub is_community: bool,
    #[serde(default)]
    pub expiry: String,
    #[serde(default)]
    pub activation: String,
}

impl WorldEvent {
    /// Pack-defined event match (the match text lives in the pack TOML,
    /// never in Rust): case-insensitive substring on the description.
    pub fn matches_pack_event(&self, needle: &str) -> bool {
        let d = self.description.to_lowercase();
        !needle.is_empty() && d.contains(&needle.to_lowercase())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NightWaveChallenge {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// Act description: the name alone doesn't say what to do, so the
    /// page renders name + description everywhere acts render
    /// (pass 4 Do Now §e).
    #[serde(default)]
    pub desc: String,
    /// The feed's key is `reputation`; `standing` stays as the alias
    /// so both shapes parse.
    #[serde(default, alias = "reputation")]
    pub standing: u32,
    #[serde(default)]
    pub is_daily: bool,
    #[serde(default)]
    pub is_elite: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NightWave {
    #[serde(default)]
    pub season: u32,
    #[serde(default)]
    pub active_challenges: Vec<NightWaveChallenge>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortieVariant {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub mission_type: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoidTrader {
    #[serde(default)]
    pub activation: String,
    #[serde(default)]
    pub expiry: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub active: bool,
}

impl VoidTrader {
    /// Live window check (pass 4 Baro section): the feed's `active`
    /// flag when present, else now inside [activation, expiry). The
    /// section renders only while a trader is around — gone when he
    /// leaves.
    pub fn is_live(&self, now: &chrono::DateTime<chrono::Utc>) -> bool {
        if self.active {
            return true;
        }
        let open = self
            .activation
            .parse::<chrono::DateTime<chrono::Utc>>()
            .ok();
        let close = self.expiry.parse::<chrono::DateTime<chrono::Utc>>().ok();
        match (open, close) {
            (Some(a), Some(e)) => *now >= a && *now < e,
            (None, Some(e)) => *now < e,
            _ => self.active,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sortie {
    #[serde(default)]
    pub expiry: String,
    #[serde(default)]
    pub variants: Vec<SortieVariant>,
}

/// World-state subset the planner consumes. Unknown sections default
/// to empty so parsing never breaks on API additions.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldState {
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub fissures: Vec<Fissure>,
    #[serde(default)]
    pub alerts: Vec<Alert>,
    #[serde(default)]
    pub events: Vec<WorldEvent>,
    #[serde(default)]
    pub vault_trader: Option<VaultTrader>,
    #[serde(default)]
    pub nightwave: Option<NightWave>,
    #[serde(default)]
    pub sortie: Option<Sortie>,
    /// The Archon weekly (`LiteSorties`). Parsed now; not consumed by
    /// the planner yet (the weekly board tracks it as a manual item).
    #[serde(default)]
    pub archon: Option<Sortie>,
    #[serde(default, rename = "voidTrader")]
    pub baro: Option<VoidTrader>,
    /// Full trader rotation (the feed carries both the singular and
    /// the list; either shape proves Baro is around).
    #[serde(default, rename = "voidTraders")]
    pub void_traders: Vec<VoidTrader>,
    /// DE keys this run could not decode (nodes/hubs/mission types).
    /// Surfaces in tests/stderr; never a guessed display name.
    #[serde(skip)]
    pub decode_gaps: Vec<String>,
}

impl WorldState {
    /// Parse DE's raw worldState.php payload into the app model using the
    /// baked decode tables. Sections the app does not consume (Discord
    /// `Events`, Descents, etc.) are ignored; every consumed section is
    /// mapped here. Unmapped keys are recorded in `decode_gaps`.
    pub fn from_de_json(text: &str, decodes: &Decodes) -> Result<Self, String> {
        let root: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("worldstate (DE): {e}"))?;
        if !root.is_object() {
            return Err("worldstate (DE): payload is not an object".to_owned());
        }
        let mut world = WorldState::default();
        let mut gaps: Vec<String> = Vec::new();

        // DE's payload carries `Time` in unix seconds; the render clock is
        // judged against this stamp, never against the fetch time.
        match root.get("Time").and_then(|v| v.as_i64()) {
            Some(secs) => {
                if let Some(dt) = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0) {
                    world.timestamp = dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                } else {
                    push_gap(&mut gaps, "Time out of range");
                }
            }
            None => push_gap(&mut gaps, "Time missing/unreadable"),
        }

        // Alerts.
        for (index, a) in
            as_array(required_section(&root, "Alerts", &mut gaps)).iter().enumerate()
        {
            let mission = a.get("MissionInfo");
            let id = de_oid(a.get("_id"));
            let tag_raw = de_str(Some(a), "Tag");
            let label = gap_identity(
                "alert",
                "",
                if !id.is_empty() { &id } else { tag_raw },
                index,
            );
            let mission_label = format!("{label}.MissionInfo");
            let location = de_str_tracked(mission, "location", &mission_label, &mut gaps);
            let counted_items = as_array(
                mission
                    .and_then(|m| m.get("missionReward"))
                    .and_then(|r| r.get("countedItems")),
            )
            .iter()
            .enumerate()
            .filter_map(|(item_index, c)| {
                let unique_name = de_str_tracked(
                    Some(c),
                    "ItemType",
                    &format!("{mission_label}.countedItems[{item_index}]"),
                    &mut gaps,
                );
                if unique_name.is_empty() {
                    return None;
                }
                Some(CountedItem {
                    unique_name: unique_name.to_owned(),
                    kind: humanize_language_key(unique_name),
                    count: c.get("ItemCount").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                })
            })
            .collect();
            // A present-but-unreadable Activation must not silently take
            // the legacy fail-open path; an absent field is the real
            // legacy feed (addendum #56b).
            let activation_field = a.get("Activation");
            let activation = de_date(activation_field);
            let activation_degraded = activation_field.is_some() && activation.is_empty();
            if activation_degraded {
                push_gap(&mut gaps, &format!("{label}.Activation unreadable"));
            }
            let tag = de_str_tracked(Some(a), "Tag", &label, &mut gaps);
            world.alerts.push(Alert {
                id,
                tag: tag.to_owned(),
                activation,
                activation_degraded,
                expiry: de_date_tracked(Some(a), "Expiry", &label, &mut gaps),
                mission: AlertMission {
                    description: humanize_language_key(de_str_tracked(
                        mission,
                        "descText",
                        &mission_label,
                        &mut gaps,
                    )),
                    node: decode_node_display(location, decodes, &mut gaps),
                    sol_node: location.to_owned(),
                    mission_type: decode_mission_type(
                        de_str_tracked(mission, "missionType", &mission_label, &mut gaps),
                        decodes,
                        &mut gaps,
                    ),
                    faction: de_str_tracked(mission, "faction", &mission_label, &mut gaps)
                        .to_owned(),
                    reward: AlertReward { counted_items },
                    counted_items_flat: Vec::new(),
                },
            });
        }

        // ActiveMissions -> fissures. `Hard` is the entry's own Steel Path
        // flag; `Modifier` carries the era tier.
        for (index, m) in
            as_array(required_section(&root, "ActiveMissions", &mut gaps)).iter().enumerate()
        {
            let id = de_oid(m.get("_id"));
            let nod = de_str(Some(m), "Node");
            let label = gap_identity("fissure", nod, &id, index);
            world.fissures.push(Fissure {
                id,
                node: decode_node_display(
                    de_str_tracked(Some(m), "Node", &label, &mut gaps),
                    decodes,
                    &mut gaps,
                ),
                mission_type: decode_mission_type(
                    de_str_tracked(Some(m), "MissionType", &label, &mut gaps),
                    decodes,
                    &mut gaps,
                ),
                tier: decode_tier(
                    de_str_tracked(Some(m), "Modifier", &label, &mut gaps),
                    &mut gaps,
                ),
                expiry: de_date_tracked(Some(m), "Expiry", &label, &mut gaps),
                is_storm: false,
                is_hard: m.get("Hard").and_then(|v| v.as_bool()).unwrap_or(false),
            });
        }

        // VoidStorms -> fissures with `is_storm`. The storm payload carries
        // only Node/Tier/Activation/Expiry (no mission type), so the type
        // stays empty rather than being inferred.
        for (index, s) in
            as_array(required_section(&root, "VoidStorms", &mut gaps)).iter().enumerate()
        {
            let id = de_oid(s.get("_id"));
            let nod = de_str(Some(s), "Node");
            let label = gap_identity("void-storm", nod, &id, index);
            world.fissures.push(Fissure {
                id,
                node: decode_node_display(
                    de_str_tracked(Some(s), "Node", &label, &mut gaps),
                    decodes,
                    &mut gaps,
                ),
                mission_type: String::new(),
                tier: decode_tier(
                    de_str_tracked(Some(s), "ActiveMissionTier", &label, &mut gaps),
                    &mut gaps,
                ),
                expiry: de_date_tracked(Some(s), "Expiry", &label, &mut gaps),
                is_storm: true,
                is_hard: false,
            });
        }

        // Goals -> events (community/event progress incl. Road To Tau).
        // DE's top-level `Events` array is Discord/community messaging, not
        // game events, and is deliberately ignored.
        for (index, g) in as_array(required_section(&root, "Goals", &mut gaps)).iter().enumerate() {
            let desc = de_str(Some(g), "Desc");
            let label = gap_identity("goal", desc, "", index);
            world.events.push(WorldEvent {
                description: humanize_language_key(de_str_tracked(
                    Some(g),
                    "Desc",
                    &label,
                    &mut gaps,
                )),
                health: g.get("HealthPct").and_then(|v| v.as_f64()),
                is_community: g
                    .get("Community")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                expiry: de_date_tracked(Some(g), "Expiry", &label, &mut gaps),
                activation: de_date_tracked(Some(g), "Activation", &label, &mut gaps),
            });
        }

        // PrimeVaultTraders (Varzia).
        if let Some(vt) =
            as_array(required_section(&root, "PrimeVaultTraders", &mut gaps)).first()
        {
            let id = de_oid(vt.get("_id"));
            let label = gap_identity("vault-trader", de_str(Some(vt), "Node"), &id, 0);
            let inventory = as_array(vt.get("Manifest"))
                .iter()
                .enumerate()
                .filter_map(|(item_index, item)| {
                    let unique_name = de_str_tracked(
                        Some(item),
                        "ItemType",
                        &format!("{label}.Manifest[{item_index}]"),
                        &mut gaps,
                    );
                    if unique_name.is_empty() {
                        return None;
                    }
                    Some(VaultTraderItem {
                        item: humanize_language_key(unique_name),
                        unique_name: unique_name.to_owned(),
                    })
                })
                .collect();
            world.vault_trader = Some(VaultTrader {
                id,
                activation: de_date_tracked(Some(vt), "Activation", &label, &mut gaps),
                expiry: de_date_tracked(Some(vt), "Expiry", &label, &mut gaps),
                character: String::new(),
                location: decode_hub(
                    de_str_tracked(Some(vt), "Node", &label, &mut gaps),
                    decodes,
                    &mut gaps,
                ),
                inventory,
            });
        }

        // VoidTraders (Baro). `active` is derived from the window at render
        // time by `is_live`, so the DE entry leaves it false.
        for (index, t) in
            as_array(required_section(&root, "VoidTraders", &mut gaps)).iter().enumerate()
        {
            let label = gap_identity("void-trader", de_str(Some(t), "Node"), "", index);
            world.void_traders.push(VoidTrader {
                activation: de_date_tracked(Some(t), "Activation", &label, &mut gaps),
                expiry: de_date_tracked(Some(t), "Expiry", &label, &mut gaps),
                location: decode_hub(
                    de_str_tracked(Some(t), "Node", &label, &mut gaps),
                    decodes,
                    &mut gaps,
                ),
                active: false,
            });
        }
        world.baro = world.void_traders.first().cloned();

        // Sorties.
        if let Some(s) = as_array(required_section(&root, "Sorties", &mut gaps)).first() {
            world.sortie = Some(Sortie {
                expiry: de_date_tracked(Some(s), "Expiry", "sortie", &mut gaps),
                variants: as_array(s.get("Variants"))
                    .iter()
                    .enumerate()
                    .map(|(index, v)| {
                        let label = format!("sortie.variants[{index}]");
                        SortieVariant {
                            node: decode_node_display(
                                de_str_tracked(Some(v), "node", &label, &mut gaps),
                                decodes,
                                &mut gaps,
                            ),
                            mission_type: decode_mission_type(
                                de_str_tracked(Some(v), "missionType", &label, &mut gaps),
                                decodes,
                                &mut gaps,
                            ),
                        }
                    })
                    .collect(),
            });
        }

        // LiteSorties -> the Archon weekly (parsed; planner later).
        if let Some(s) = as_array(required_section(&root, "LiteSorties", &mut gaps)).first() {
            world.archon = Some(Sortie {
                expiry: de_date_tracked(Some(s), "Expiry", "archon", &mut gaps),
                variants: as_array(s.get("Missions"))
                    .iter()
                    .enumerate()
                    .map(|(index, v)| {
                        let label = format!("archon.missions[{index}]");
                        SortieVariant {
                            node: decode_node_display(
                                de_str_tracked(Some(v), "node", &label, &mut gaps),
                                decodes,
                                &mut gaps,
                            ),
                            mission_type: decode_mission_type(
                                de_str_tracked(Some(v), "missionType", &label, &mut gaps),
                                decodes,
                                &mut gaps,
                            ),
                        }
                    })
                    .collect(),
            });
        }

        // SeasonInfo -> Nightwave. Titles/descriptions/standing come from
        // the baked Nightwave decode; `|COUNT|` is substituted with the
        // challenge's required count. `WeeklyHard` marks elites.
        if let Some(si) = required_section(&root, "SeasonInfo", &mut gaps) {
            let mut active_challenges = Vec::new();
            for c in as_array(si.get("ActiveChallenges")) {
                let id = de_oid(c.get("_id"));
                let label = gap_identity("nightwave", "", &id, active_challenges.len());
                let key = de_str_tracked(Some(c), "Challenge", &label, &mut gaps);
                let (title, desc, standing) = match decodes.nightwave.get(key) {
                    Some(d) => (
                        d.name.clone(),
                        d.description.replace("|COUNT|", &d.required.to_string()),
                        d.standing,
                    ),
                    None => {
                        push_gap(&mut gaps, key);
                        (humanize_language_key(key), String::new(), 0)
                    }
                };
                active_challenges.push(NightWaveChallenge {
                    id: de_oid(c.get("_id")),
                    title,
                    desc,
                    standing,
                    is_daily: c.get("Daily").and_then(|v| v.as_bool()).unwrap_or(false),
                    is_elite: key.contains("WeeklyHard"),
                });
            }
            world.nightwave = Some(NightWave {
                season: si.get("Season").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                active_challenges,
            });
        }

        world.decode_gaps = gaps;
        Ok(world)
    }

    /// The live void trader, if any (pass 4 Baro section): the
    /// singular entry when its window is live, else the first live
    /// entry of the rotation list. None when he is gone.
    pub fn active_trader(&self, now: &chrono::DateTime<chrono::Utc>) -> Option<&VoidTrader> {
        if let Some(b) = &self.baro {
            if b.is_live(now) {
                return Some(b);
            }
        }
        self.void_traders.iter().find(|t| t.is_live(now))
    }
}

// ---------------------------------------------------------------------------
// Reconciled backlog (data/backlog.json — profile-derived, NOT Xuerian)
// ---------------------------------------------------------------------------

/// One unmastered prime from the reconciled backlog (addendum #12).
#[derive(Debug, Clone, Deserialize)]
pub struct BacklogEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vaulted: bool,
    #[serde(default, rename = "type", deserialize_with = "null_default")]
    pub gear_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub wants: String,
    #[serde(default)]
    pub parts_wanted: Vec<String>,
    #[serde(default)]
    pub all_parts: Vec<String>,
}

impl BacklogEntry {
    pub fn is_warframe(&self) -> bool {
        self.gear_type.eq_ignore_ascii_case("warframe")
    }
}

/// The reconciled backlog: prime roster vs profile XPInfo.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Backlog {
    #[serde(default)]
    pub primes: BacklogPrimes,
    #[serde(default)]
    pub unmastered_by_category: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BacklogPrimes {
    #[serde(default)]
    pub roster: u32,
    #[serde(default)]
    pub mastered: u32,
    #[serde(default)]
    pub unmastered: Vec<BacklogEntry>,
}

impl Backlog {
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("backlog: {e}"))
    }

    /// Planner wants derived from the reconciled backlog: every part of
    /// every unmastered prime (parts per parent come from the roster at
    /// card-build time; this flat list feeds coverage scoring).
    pub fn wants(&self) -> Vec<String> {
        let mut out = Vec::new();
        for e in &self.primes.unmastered {
            for p in &e.all_parts {
                if !out.contains(p) {
                    out.push(p.clone());
                }
            }
        }
        out
    }

    pub fn gear_names(&self) -> Vec<String> {
        self.primes.unmastered.iter().map(|e| e.name.clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// Icons (addendum #14)
// ---------------------------------------------------------------------------

/// Parent-item icon URL from the WFCD dataset:
/// parent name -> `imageName` -> `https://cdn.warframestat.us/img/<imageName>`.
/// A prime PART always shows its PARENT item's icon, never the generic
/// part/blueprint icon. Unresolved items keep the monogram (None).
pub fn icon_url(parent: &str, icons: &HashMap<String, String>) -> Option<String> {
    icons
        .get(parent)
        .map(|img| format!("https://cdn.warframestat.us/img/{img}"))
}

pub fn load_icons(text: &str) -> Result<HashMap<String, String>, String> {
    serde_json::from_str(text).map_err(|e| format!("icons: {e}"))
}

/// DE Public Export content-server base (pass 48, Seth's source
/// spec 2026-10-27): an ExportManifest `textureLocation` appended
/// to this prefix is the item's image asset. The locations carry
/// their own `!<hash>` suffix and are hash-addressed — DE rotates
/// them only by shipping a new manifest (see the refresh
/// procedure in data/cache/README.md).
pub const PUBLIC_EXPORT_BASE: &str = "http://content.warframe.com/PublicExport";

/// Refinement tiers in relic-icon preference order (pass 48): the
/// overlay shows ONE icon per relic — the Intact projection's
/// texture; the coarser tiers substitute only when a finer one
/// carries no texture.
const RELIC_ICON_TIERS: [&str; 4] = ["Intact", "Exceptional", "Flawless", "Radiant"];

/// The baked relic-texture CLASS table
/// (`data/cache/relic-textures.json`, pass 52): DE ships one
/// relic icon per (era, refinement tier) — the 3,204-record
/// per-projection bake of pass 48 collapsed to 21 distinct
/// textures once measured, so the bake is keyed by class
/// instead of by projection uniqueName. Baked, never fetched
/// at runtime (addendum #51 discipline); refresh per
/// data/cache/README.md.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RelicTextureClasses {
    /// Era name as it appears in relic display names ("Lith",
    /// "Meso", "Neo", "Axi", "Vanguard", "Requiem") ->
    /// refinement tier -> Public Export `textureLocation`.
    /// Vanguard rows carry the Axi textures and Requiem rows
    /// the RelicImmortal ones: those eras share an icon
    /// family, which the manifest's own per-projection
    /// records confirm group by group.
    #[serde(default)]
    pub classes: HashMap<String, HashMap<String, String>>,
    /// The un-tiered base projection items' texture: every
    /// era's generic projection item (the plain "Lith Relic",
    /// "Requiem Relic", ...) shows the shared OmegaIsotope
    /// pickup icon.
    #[serde(default)]
    pub pickup: Option<String>,
    /// Tier-less NAMED relics — single-variant projections
    /// such as "Requiem Eterna" — by case-folded base name ->
    /// textureLocation. Unlike the generic base items these
    /// wear a real relic icon, so they keep a per-name row.
    #[serde(default)]
    pub tierless: HashMap<String, String>,
}

/// Parse the baked relic-texture class table
/// (`data/cache/relic-textures.json`). The tier-less keys
/// fold on load, like every other relic-name key in the join.
pub fn load_relic_textures(text: &str) -> Result<RelicTextureClasses, String> {
    let mut parsed: RelicTextureClasses =
        serde_json::from_str(text).map_err(|e| format!("relic textures: {e}"))?;
    parsed.tierless = parsed
        .tierless
        .into_iter()
        .map(|(name, loc)| (name.to_lowercase(), loc))
        .collect();
    Ok(parsed)
}

/// One base relic name's icon candidacy, collected from
/// item-names: the era word the class lookup keys on, which
/// refinement tiers the base actually has, and whether an
/// un-tiered variant of the base exists.
struct IconCandidate<'a> {
    era: &'a str,
    tiers: [bool; 4],
    tierless: bool,
}

impl<'a> IconCandidate<'a> {
    fn new(era: &'a str) -> Self {
        IconCandidate {
            era,
            tiers: [false; 4],
            tierless: false,
        }
    }
}

/// Build the relic icon map: relic display name -> absolute
/// Public Export texture URL (pass 48; class-table join,
/// pass 52). Keys are CASE-FOLDED base names; resolve through
/// [`relic_icon_url`], never by raw key.
///
/// The join: `item_names` maps each projection uniqueName
/// (`/Lotus/Types/Game/Projections/...`) to a display name —
/// tier-suffixed ("Meso C11 Intact") or bare ("Requiem
/// Eterna Relic"). A tiered base resolves through the class
/// table at the finest tier it HAS whose (era, tier) class
/// exists — with a complete bake that is its Intact class,
/// the texture every one of its projections carried in the
/// old per-item map (each (era, tier) group in the manifest
/// names exactly one texture; the equivalence gate in the
/// tests pins all 800 relic names to their pre-collapse
/// URLs). An un-tiered base resolves through the tier-less
/// rows: a named single-variant relic's own texture, else
/// the shared pickup icon of the generic base items. Two
/// naming quirks in the sources are normalized here, never
/// guessed around: relic-tables roman numerals are
/// title-case ("Requiem Ii") where item-names uses upper
/// ("Requiem II") — the case-folded key absorbs that; and
/// the Vanguard relics are "Axi Vanguard C1" in the tables
/// but plain "Vanguard C1" as projections — Vanguard bases
/// are ALSO keyed with the "Axi " prefix. A relic nothing
/// resolves a texture for is simply absent — the caller
/// keeps its monogram.
pub fn relic_icon_urls(
    item_names: &HashMap<String, String>,
    textures: &RelicTextureClasses,
) -> HashMap<String, String> {
    // Case-folded base name -> candidacy (see IconCandidate).
    let mut candidates: HashMap<String, IconCandidate> = HashMap::new();
    fn add<'a>(
        candidates: &mut HashMap<String, IconCandidate<'a>>,
        key: String,
        era: &'a str,
        rank: Option<usize>,
    ) {
        let cand = candidates
            .entry(key)
            .or_insert_with(|| IconCandidate::new(era));
        match rank {
            Some(rank) => cand.tiers[rank] = true,
            None => cand.tierless = true,
        }
    }
    for (unique, display) in item_names {
        if !unique.contains("/Projections/") {
            continue;
        }
        let mut parsed: Option<(&str, Option<usize>)> = None;
        for (rank, tier) in RELIC_ICON_TIERS.iter().enumerate() {
            if let Some(base) = display
                .strip_suffix(tier)
                .and_then(|s| s.strip_suffix(' '))
            {
                parsed = Some((base, Some(rank)));
                break;
            }
        }
        if parsed.is_none() {
            if let Some(base) = display.strip_suffix(" Relic") {
                parsed = Some((base, None));
            }
        }
        let Some((base, rank)) = parsed else {
            continue;
        };
        let era = base.split(' ').next().unwrap_or(base);
        add(&mut candidates, base.to_lowercase(), era, rank);
        if base.starts_with("Vanguard") {
            let alias = format!("axi {}", base.to_lowercase());
            add(&mut candidates, alias, era, rank);
        }
    }
    let mut out = HashMap::new();
    for (key, cand) in &candidates {
        let mut loc: Option<&str> = None;
        for (rank, tier) in RELIC_ICON_TIERS.iter().enumerate() {
            if cand.tiers[rank] {
                if let Some(found) = textures
                    .classes
                    .get(cand.era)
                    .and_then(|row| row.get(*tier))
                {
                    loc = Some(found);
                    break;
                }
            }
        }
        if loc.is_none() && cand.tierless {
            loc = textures
                .tierless
                .get(key)
                .map(String::as_str)
                .or(textures.pickup.as_deref());
        }
        if let Some(loc) = loc {
            out.insert(
                key.clone(),
                format!("{PUBLIC_EXPORT_BASE}/{}", loc.trim_start_matches('/')),
            );
        }
    }
    out
}

/// One relic's icon URL from the map [`relic_icon_urls`] built
/// (pass 48), mirroring [`icon_url`]: unresolved relics keep
/// the monogram (None) — never an invented URL.
pub fn relic_icon_url(relic: &str, icons: &HashMap<String, String>) -> Option<String> {
    icons.get(&relic.to_lowercase()).cloned()
}

// ---------------------------------------------------------------------------
// Profile + stock
// ---------------------------------------------------------------------------

/// Equipment-level ownership: display names of built gear (resolved via
/// the items dataset at the fetch/state layer; the planner takes names).
#[derive(Debug, Clone, Default)]
pub struct OwnedEquipment {
    pub names: HashSet<String>,
}

impl OwnedEquipment {
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Relic holdings, tier-aware (eighth pass §1). The refinement screen
/// shows one cell per (relic x tier); the planner consumes per-relic
/// TOTALS plus the RADIANT split (radiant piles are cracked as-is;
/// otherwise the tier is chosen by the refinement ladder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownStock {
    /// relic -> total copies across all tiers.
    pub totals: HashMap<String, u32>,
    /// relic -> copies already at Radiant (cracked without spending traces).
    pub radiant: HashMap<String, u32>,
}

impl KnownStock {
    pub fn from_totals(totals: HashMap<String, u32>) -> Self {
        Self {
            totals,
            radiant: HashMap::new(),
        }
    }
}

/// Relic holdings. Unknown until the §8.1 OCR bootstrap resolves it —
/// then the planner ranks by coverage alone and labels "if you have it".
#[derive(Debug, Clone)]
pub enum RelicStock {
    Unknown,
    Known(KnownStock),
}

impl RelicStock {
    pub fn count(&self, relic: &str) -> Option<u32> {
        match self {
            RelicStock::Unknown => None,
            RelicStock::Known(k) => Some(k.totals.get(relic).copied().unwrap_or(0)),
        }
    }

    /// Copies already at Radiant for `relic` (`None` = stock unknown).
    pub fn radiant_count(&self, relic: &str) -> Option<u32> {
        match self {
            RelicStock::Unknown => None,
            RelicStock::Known(k) => Some(k.radiant.get(relic).copied().unwrap_or(0)),
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, RelicStock::Known(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pass 48 gate, class-table shape (pass 52): a relic
    /// resolves through the (era, tier) class table to the
    /// expected content-server URL — the finest tier it HAS
    /// with a class row wins, the source naming quirks are
    /// normalized, tier-less relics take their own row, and an
    /// unresolvable relic is absent (monogram), never a
    /// guessed URL.
    #[test]
    fn relic_icon_urls_resolve_through_the_class_join() {
        let item_names: HashMap<String, String> = HashMap::from([
            // Meso C11, all four tiers (real uniqueNames).
            (
                "/Lotus/Types/Game/Projections/T2VoidProjectionCitrinePrimeABronze".to_owned(),
                "Meso C11 Intact".to_owned(),
            ),
            (
                "/Lotus/Types/Game/Projections/T2VoidProjectionCitrinePrimeASilver".to_owned(),
                "Meso C11 Exceptional".to_owned(),
            ),
            (
                "/Lotus/Types/Game/Projections/T2VoidProjectionCitrinePrimeAGold".to_owned(),
                "Meso C11 Flawless".to_owned(),
            ),
            (
                "/Lotus/Types/Game/Projections/T2VoidProjectionCitrinePrimeAPlatinum".to_owned(),
                "Meso C11 Radiant".to_owned(),
            ),
            // Tier-fallback relic: only its Radiant tier carries
            // a texture in the fixture manifest.
            (
                "/Lotus/Types/Game/Projections/T4VoidProjectionABronze".to_owned(),
                "Axi A99 Intact".to_owned(),
            ),
            (
                "/Lotus/Types/Game/Projections/T4VoidProjectionAPlatinum".to_owned(),
                "Axi A99 Radiant".to_owned(),
            ),
            // The Vanguard alias and the roman-numeral casing.
            (
                "/Lotus/Types/Game/Projections/T4VoidProjectionVanguardCBronze".to_owned(),
                "Vanguard C1 Intact".to_owned(),
            ),
            (
                "/Lotus/Types/Game/Projections/T5VoidProjectionImmortalBBronze".to_owned(),
                "Requiem II Intact".to_owned(),
            ),
            // The single-variant projection: no tier suffix.
            (
                "/Lotus/Types/Game/Projections/T5VoidProjectionImmortalOmniA".to_owned(),
                "Requiem Eterna Relic".to_owned(),
            ),
            // A non-projection entry: never part of the join.
            (
                "/Lotus/Powersuits/Bard/OctaviaPrime".to_owned(),
                "Octavia Prime".to_owned(),
            ),
        ]);
        // The class table, fixture locations. The Axi row
        // deliberately lacks an Intact class, so Axi A99 —
        // which HAS an Intact projection — must fall through
        // to the finest tier it has whose class exists
        // (Radiant), mirroring the per-item map's missing-
        // texture substitute. Vanguard is its own era row
        // carrying an Axi-family texture, as in the real bake.
        let textures = RelicTextureClasses {
            classes: HashMap::from([
                (
                    "Meso".to_owned(),
                    HashMap::from([(
                        "Intact".to_owned(),
                        "/Lotus/Interface/Icons/Relics/RelicMesoD.png!00_fixturehash"
                            .to_owned(),
                    )]),
                ),
                (
                    "Axi".to_owned(),
                    HashMap::from([(
                        "Radiant".to_owned(),
                        "/Lotus/Interface/Icons/Relics/RelicAxiR.png!00_fixturehash"
                            .to_owned(),
                    )]),
                ),
                (
                    "Vanguard".to_owned(),
                    HashMap::from([(
                        "Intact".to_owned(),
                        "/Lotus/Interface/Icons/Relics/RelicAxiD.png!00_fixturehash"
                            .to_owned(),
                    )]),
                ),
                (
                    "Requiem".to_owned(),
                    HashMap::from([(
                        "Intact".to_owned(),
                        "/Lotus/Interface/Icons/Relics/RelicImmortalB.png!00_fixturehash"
                            .to_owned(),
                    )]),
                ),
            ]),
            pickup: None,
            tierless: HashMap::from([(
                // As the loader leaves it: keys case-folded.
                "requiem eterna".to_owned(),
                "/Lotus/Interface/Icons/Relics/RelicImmortalC.png!00_fixturehash".to_owned(),
            )]),
        };
        let icons = relic_icon_urls(&item_names, &textures);
        assert_eq!(
            relic_icon_url("Meso C11", &icons).as_deref(),
            Some(
                "http://content.warframe.com/PublicExport/Lotus/Interface/Icons/Relics/RelicMesoD.png!00_fixturehash"
            ),
            "the Intact class texture, on the content server"
        );
        assert_eq!(
            relic_icon_url("Axi A99", &icons).as_deref(),
            Some(
                "http://content.warframe.com/PublicExport/Lotus/Interface/Icons/Relics/RelicAxiR.png!00_fixturehash"
            ),
            "the Axi row has no Intact class: the Radiant substitute"
        );
        assert_eq!(
            relic_icon_url("Axi Vanguard C1", &icons).as_deref(),
            Some(
                "http://content.warframe.com/PublicExport/Lotus/Interface/Icons/Relics/RelicAxiD.png!00_fixturehash"
            ),
            "the tables' Axi-prefixed Vanguard name resolves"
        );
        assert_eq!(
            relic_icon_url("Requiem Ii", &icons).as_deref(),
            Some(
                "http://content.warframe.com/PublicExport/Lotus/Interface/Icons/Relics/RelicImmortalB.png!00_fixturehash"
            ),
            "title-case roman numerals in the tables still resolve"
        );
        assert_eq!(
            relic_icon_url("Requiem Eterna", &icons).as_deref(),
            Some(
                "http://content.warframe.com/PublicExport/Lotus/Interface/Icons/Relics/RelicImmortalC.png!00_fixturehash"
            ),
            "the tier-less single-variant projection resolves"
        );
        assert_eq!(
            relic_icon_url("Lith Z9", &icons),
            None,
            "an unresolvable relic stays absent — monogram, never a guess"
        );
    }

    /// Pass 52 equivalence gate: against the REAL baked data,
    /// every one of the 800 relic-table names resolves through
    /// the class-table join to the SAME content-server URL the
    /// pre-collapse per-item join produced — the baseline in
    /// fixtures/relic-icon-baseline.json was generated by the
    /// old code from the old 3,204-record bake before the
    /// collapse. The edge cases ride along in the 800: Requiem
    /// II/III/IV numeral folding, the dual-keyed Vanguard
    /// bases, tier-less "Requiem Eterna", and the bare
    /// "Requiem" whose icon is the pickup texture.
    #[test]
    fn relic_icon_urls_match_the_pre_class_table_baseline() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let read = |rel: &str| {
            std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
        };
        let item_names: HashMap<String, String> =
            serde_json::from_str(&read("data/cache/item-names.json")).unwrap();
        let textures =
            load_relic_textures(&read("data/cache/relic-textures.json")).unwrap();
        let baseline: HashMap<String, String> =
            serde_json::from_str(&read("fixtures/relic-icon-baseline.json")).unwrap();
        assert_eq!(baseline.len(), 800, "the full relic-table name list");
        let icons = relic_icon_urls(&item_names, &textures);
        for (name, want) in &baseline {
            assert_eq!(
                relic_icon_url(name, &icons).as_ref(),
                Some(want),
                "{name} must keep its pre-collapse icon URL"
            );
        }
    }

    #[test]
    fn universe_scrub_excludes_and_canonicalizes() {
        // Sixth pass §1: the data-owned scrub drops excluded entries
        // (and their parts) and collapses duplicate name orders. The
        // collar and Helminth stay out of every regeneration; the
        // duplicate frame name resolves to one canonical spelling.
        let text = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data/exclusions.toml"),
        )
        .unwrap();
        let s = UniverseScrub::from_toml(&text).unwrap();
        assert!(s.is_excluded("Helminth"));
        assert!(s.is_excluded("Kavasa Prime"));
        assert!(s.is_excluded("Excalibur Prime"));
        assert!(!s.is_excluded("Nautilus Prime"));
        assert!(s.reason("Helminth").is_some());
        let names = vec![
            "Kavasa Prime".to_owned(),
            "Kavasa Prime Band".to_owned(),
            "Helminth".to_owned(),
            "Orion & Sirius".to_owned(),
            "Sirius & Orion".to_owned(),
            "Nautilus Prime Blueprint".to_owned(),
        ];
        assert_eq!(
            s.scrub_names(&names),
            vec!["Orion & Sirius".to_owned(), "Nautilus Prime Blueprint".to_owned()],
        );
    }

    #[test]
    fn scrub_removes_only_scrubbed_entries_from_categories() {
        // Delta discipline: applying the scrub changes counts ONLY by
        // the scrubbed entries (excluded names + one alias collapse).
        let text = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data/exclusions.toml"),
        )
        .unwrap();
        let s = UniverseScrub::from_toml(&text).unwrap();
        let cats: HashMap<String, Vec<String>> = [
            (
                "Warframes".to_owned(),
                vec![
                    "Baruuk".to_owned(),
                    "Helminth".to_owned(),
                    "Orion & Sirius".to_owned(),
                    "Sirius & Orion".to_owned(),
                ],
            ),
            (
                "Melee".to_owned(),
                // A repeated non-alias name is a distinct entry: kept.
                vec!["Balla".to_owned(), "Balla".to_owned()],
            ),
        ]
        .into_iter()
        .collect();
        let scrubbed = s.scrub_categories(&cats);
        assert_eq!(
            scrubbed.get("Warframes").unwrap(),
            &vec!["Baruuk".to_owned(), "Orion & Sirius".to_owned()],
        );
        assert_eq!(
            scrubbed.get("Melee").unwrap(),
            &vec!["Balla".to_owned(), "Balla".to_owned()],
            "unrelated duplicates survive"
        );
    }

    #[test]
    fn backlog_data_keeps_prime_counts_after_scrub() {
        // Sixth pass §1 regression gate: the prime roster (163/99/63)
        // is untouched; only the scrubbed category entries changed.
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(base.join("data/backlog.json")).unwrap();
        let backlog = Backlog::from_json(&text).unwrap();
        assert_eq!(backlog.primes.roster, 163);
        assert_eq!(backlog.primes.mastered, 99);
        assert_eq!(backlog.primes.unmastered.len(), 63);
        let scrub = UniverseScrub::from_toml(
            &std::fs::read_to_string(base.join("data/exclusions.toml")).unwrap(),
        )
        .unwrap();
        let scrubbed = scrub.scrub_categories(&backlog.unmastered_by_category);
        let frames = scrubbed.get("Warframes").expect("Warframes list");
        assert_eq!(frames.len(), 8, "10 → 8 after two scrubbed entries");
        assert!(!frames.iter().any(|n| n == "Helminth"));
        assert!(!frames.iter().any(|n| n == "Sirius & Orion"));
        assert!(frames.iter().any(|n| n == "Orion & Sirius"));
        // Re-scrubbing is idempotent: the edited data already reflects it.
        let again = scrub.scrub_categories(&scrubbed);
        assert_eq!(again, scrubbed);
    }

    #[test]
    fn arcane_manifest_carries_the_full_universe() {
        // Eighth pass §3: the arcane set is now the real, data-owned
        // universe derived from the item dataset (172 names), superseding
        // the spoken-debrief stopgap. OCR reads validate against it.
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let m = ArcaneManifest::from_toml(
            &std::fs::read_to_string(base.join("data/arcanes.toml")).unwrap(),
        )
        .unwrap();
        assert_eq!(m.names.len(), 172, "the arcane universe");
        assert!(!m.label.is_empty());
        assert!(m.name_set().contains("Melee Influence"));
        // Names visible in Seth's real Arcanes capture must validate.
        for n in [
            "Arcane Avenger",
            "Arcane Acceleration",
            "Akimbo Slip Shot",
            "Arcane Blade Charger",
        ] {
            assert!(m.name_set().contains(n), "missing captured name {n}");
        }
    }

    #[test]
    fn required_counts_table_pins_the_multi_unit_population() {
        // Pass 42 (addendum #107): the shipped data/required.toml
        // is a mechanical extraction from the WFCD items dataset
        // (see the file's provenance header). Pin its size, its
        // all-x2 shape, and the known examples so a bad edit to
        // the table fails loudly here rather than misstating
        // progress on the page.
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let counts = load_required_counts(
            &std::fs::read_to_string(base.join("data/required.toml")).unwrap(),
        )
        .unwrap();
        assert_eq!(counts.len(), 49, "the enumerated multi-unit population");
        assert!(
            counts.values().all(|&n| n == 2),
            "every known doubled part is x2"
        );
        for name in [
            "Quassus Prime Blade",
            "Quassus Prime Handle",
            "Akarius Prime Barrel",
            "Akarius Prime Receiver",
            "Dual Kamas Prime Blade",
            "Orthos Prime Blade",
            "Venka Prime Blades",
        ] {
            assert_eq!(counts.get(name), Some(&2), "{name}");
        }
        // Zhuge Prime is NOT in the table: WFCD's recipe export
        // and the wiki manufacturing table both list every Zhuge
        // component at x1 (5 units total), against Seth's
        // recollection of six — flagged in
        // docs/forty-second-pass.md for his ruling.
        assert!(!counts.keys().any(|k| k.starts_with("Zhuge Prime")));

        // Applied to the real seed roster, the counts land on the
        // right parts and everything else keeps the default of 1.
        let wish =
            std::fs::read_to_string(base.join("data/cache/wishlist.json")).unwrap();
        let mut roster = Roster::from_json(&wish).unwrap();
        roster.apply_required_counts(&counts);
        let req = |gear: &str, part: &str| {
            roster
                .by_name(gear)
                .and_then(|i| i.parts.iter().find(|p| p.name == part))
                .map(|p| p.required)
        };
        assert_eq!(req("Quassus Prime", "Quassus Prime Blade"), Some(2));
        assert_eq!(req("Quassus Prime", "Quassus Prime Handle"), Some(2));
        assert_eq!(req("Quassus Prime", "Quassus Prime Blueprint"), Some(1));
        assert_eq!(req("Zhuge Prime", "Zhuge Prime String"), Some(1));
        assert_eq!(
            req("Caliban Prime", "Caliban Prime Chassis Blueprint"),
            Some(1),
            "warframe components are never doubled"
        );
    }

    #[test]
    fn sentinel_kinds_count_as_companions() {
        let item = RosterItem {
            name: "Nautilus Prime".into(),
            vaulted: true,
            parts: vec![],
            kind: "sentinel".into(),
        };
        assert!(item.is_companion());
        assert!(!item.is_warframe());
    }

    #[test]
    fn refinement_trace_costs_from_data() {
        let t = crate::tuning::Tuning::for_tests();
        assert_eq!(t.refinement.trace_cost(Refinement::Intact), 0);
        assert_eq!(t.refinement.trace_cost(Refinement::Exceptional), 25);
        assert_eq!(t.refinement.trace_cost(Refinement::Flawless), 50);
        assert_eq!(t.refinement.trace_cost(Refinement::Radiant), 100);
    }

    #[test]
    fn omnia_matches_every_era() {
        let f = Fissure {
            tier: "Omnia".into(),
            ..Default::default()
        };
        assert!(f.matches_era("Lith"));
        assert!(f.matches_era("Axi"));
        let g = Fissure {
            tier: "Neo".into(),
            ..Default::default()
        };
        assert!(g.matches_era("Neo"));
        assert!(!g.matches_era("Axi"));
    }

    #[test]
    fn chance_is_per_pair_with_intact_fallback() {
        let r = RelicReward {
            name: "X".into(),
            chance: [("Intact".to_owned(), 2.0)].into_iter().collect(),
        };
        assert_eq!(r.chance_at(Refinement::Intact), 2.0);
        assert_eq!(r.chance_at(Refinement::Radiant), 2.0);
    }

    #[test]
    fn gear_token_normalizes_packs() {
        assert_eq!(
            normalize_gear_token("/Lotus/StoreItems/Powersuits/Ranger/IvaraPrime"),
            "ivara prime"
        );
        let t = normalize_gear_token(
            "/Lotus/Types/StoreItems/Packages/MegaPrimeVault/MPVIvaraPrimeSinglePack",
        );
        assert!(t.contains("ivara"), "pack token keeps gear name: {t}");
    }

    #[test]
    fn era_gate_rejects_empty_and_locked() {
        let omnia = Fissure {
            tier: "Omnia".into(),
            ..Default::default()
        };
        let meso = Fissure {
            tier: "Meso".into(),
            ..Default::default()
        };
        assert!(omnia.accepts_eras(&["Axi", "Meso"]));
        assert!(meso.accepts_eras(&["Meso"]));
        assert!(!meso.accepts_eras(&["Axi"]));
        assert!(!meso.accepts_eras(&["Axi", "Meso"]));
        assert!(!meso.accepts_eras(&[]));
        assert!(!omnia.accepts_eras(&[]));
    }

    #[test]
    fn alert_sections_split_event_from_lotus() {
        // Pass 4 Do Now §§a–b: counted-payout alerts are events; tagged
        // gift alerts are their own section; bare alerts are neither.
        let event = Alert {
            tag: "RoadToTauEvent".to_owned(),
            mission: AlertMission {
                description: "Event Alert Title".to_owned(),
                reward: AlertReward {
                    counted_items: vec![CountedItem {
                        kind: "Road To Tau Event Resource".to_owned(),
                        count: 50,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(event.is_event());
        assert!(!event.is_lotus());
        let gift = Alert {
            tag: "GiftOfTheLotus".to_owned(),
            mission: AlertMission {
                description: "Gift of the Lotus".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(gift.is_lotus());
        assert!(!gift.is_event());
        assert!(!Alert::default().is_event());
        assert!(!Alert::default().is_lotus());
    }

    fn de_fixture() -> WorldState {
        let text = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/worldstate.json"),
        )
        .unwrap();
        let decodes = Decodes::load(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data"),
        )
        .unwrap();
        WorldState::from_de_json(&text, &decodes).unwrap()
    }

    #[test]
    fn de_fixture_parses_every_consumed_section() {
        let world = de_fixture();
        // `Time` (unix seconds) is the freshness stamp.
        assert_eq!(world.timestamp, "2026-10-19T05:16:44Z");
        // 22 ActiveMissions + 12 VoidStorms.
        assert_eq!(world.fissures.len(), 34);
        assert_eq!(world.fissures.iter().filter(|f| f.is_storm).count(), 12);
        assert!(world.fissures.iter().any(|f| f.is_hard), "Hard = Steel Path");
        // Tier crosswalk from `Modifier`.
        let tiers: HashSet<&str> =
            world.fissures.iter().map(|f| f.tier.as_str()).collect();
        for tier in ["Lith", "Meso", "Neo", "Axi", "Requiem", "Omnia"] {
            assert!(tiers.contains(tier), "missing tier {tier}: {tiers:?}");
        }
        // Node display = `name (system)`.
        assert!(world
            .fissures
            .iter()
            .any(|f| f.node == "Dakata (Kuva Fortress)"));

        // Alerts: DE MissionInfo -> model, SolNode retained for matching.
        assert_eq!(world.alerts.len(), 3);
        assert!(world.alerts.iter().all(|a| a.is_event()));
        assert!(!world.alerts.iter().any(|a| a.is_lotus()));
        let alert = world
            .alerts
            .iter()
            .find(|a| a.mission.sol_node == "SolNode175")
            .expect("Decoded SolNode175 alert");
        assert_eq!(alert.mission.node, "Naeglar (Eris)");
        assert_eq!(alert.mission.mission_type, "Mobile Defense");
        assert_eq!(alert.mission.description, "Event Alert Title");
        assert_eq!(alert.mission.counted_items()[0].count, 50);
        assert!(alert.expiry > alert.activation);

        // Goals -> events (Road To Tau community progress).
        assert_eq!(world.events.len(), 4);
        let tau = world
            .events
            .iter()
            .find(|e| e.description.contains("Road To Tau"))
            .expect("Road To Tau goal");
        assert!(tau.is_community);
        assert!(tau.health.is_some());

        // Varzia (PrimeVaultTraders) + Baro (VoidTraders).
        let vt = world.vault_trader.as_ref().expect("Varzia rotation");
        assert_eq!(vt.location, "Maroo's Bazaar (Mars)");
        assert_eq!(vt.inventory.len(), 22);
        assert!(vt.gear_tokens().contains("ivara prime"));
        assert_eq!(world.void_traders.len(), 1);
        assert_eq!(world.baro.as_ref().unwrap().expiry, "2026-10-25T13:00:00.000Z");

        // Sortie + Archon Hunt (LiteSorties).
        let sortie = world.sortie.as_ref().expect("sortie");
        assert_eq!(sortie.variants.len(), 3);
        assert_eq!(sortie.variants[0].node, "Venera (Venus)");
        assert_eq!(sortie.variants[0].mission_type, "Interception");
        let archon = world.archon.as_ref().expect("archon hunt");
        assert_eq!(archon.variants.len(), 3);
        assert_eq!(archon.variants[0].node, "Tharsis (Mars)");

        // SeasonInfo -> Nightwave with baked name/description/standing.
        let nw = world.nightwave.as_ref().expect("nightwave season");
        assert_eq!(nw.season, 18);
        assert_eq!(nw.active_challenges.len(), 10);
        assert_eq!(
            nw.active_challenges.iter().filter(|c| c.is_daily).count(),
            3
        );
        assert_eq!(
            nw.active_challenges.iter().filter(|c| c.is_elite).count(),
            2
        );
        let swatter = nw
            .active_challenges
            .iter()
            .find(|c| c.title == "Swatter")
            .expect("baked challenge");
        assert_eq!(
            swatter.desc,
            "Kill 3 Drones or Ospreys with your Melee Weapon."
        );
        assert_eq!(swatter.standing, 1000);
    }

    #[test]
    fn de_fixture_decode_gaps_are_recorded_not_guessed() {
        // Unmapped keys keep their raw DE key for display and land in
        // decode_gaps; the app never invents a name. EarthHUB is
        // deliberately unbaked (the Baro relay varies per visit) and the
        // Empyrean storm nodes are absent from ExportRegions.
        let world = de_fixture();
        assert!(world.decode_gaps.iter().any(|g| g == "EarthHUB"));
        for g in &world.decode_gaps {
            if g.starts_with("CrewBattleNode") {
                assert!(
                    world.fissures.iter().any(|f| f.node == *g),
                    "raw storm node displayed: {g}"
                );
            }
        }
        assert!(
            !world.decode_gaps.iter().any(|g| g.starts_with("SolNode")),
            "every consumed SolNode decoded: {:?}",
            world.decode_gaps
        );
        assert_eq!(
            world.decode_gaps.iter().filter(|g| g.starts_with("CrewBattleNode")).count(),
            12
        );
        assert_eq!(world.decode_gaps.len(), 13);
    }

    #[test]
    fn degraded_de_fields_surface_in_gaps_instead_of_disappearing() {
        // Addendum #56b: a renamed/missing field the plan renders must
        // land in the gaps channel. This payload also proves the
        // activation guard can tell a present-but-unreadable timestamp
        // from the legacy "field absent" feed, and that an unreadable
        // fissure expiry is reported before ranking drops the fissure.
        let json = r#"{
            "Time": 1792387004,
            "Alerts": [{
                "_id": {"$oid": "alert-1"},
                "Tag": "RoadToTauEvent",
                "Activation": {"$date": {"$numberLong": "not-a-number"}},
                "Expiry": {"$date": {"$numberLong": "1792389600000"}},
                "MissionInfo": {
                    "location": "SolNode1",
                    "missionType": "MT_SURVIVAL",
                    "descText": "Event Alert Title",
                    "missionReward": {"countedItems": [{"ItemType": "/Lotus/X", "ItemCount": 5}]}
                }
            }, {
                "_id": {"$oid": "alert-legacy"},
                "Tag": "RoadToTauEvent",
                "Expiry": {"$date": {"$numberLong": "1792389600000"}},
                "MissionInfo": {
                    "location": "SolNode1",
                    "missionType": "MT_SURVIVAL",
                    "faction": "FC_INFESTATION",
                    "descText": "Event Alert Title"
                }
            }],
            "ActiveMissions": [{
                "_id": {"$oid": "fissure-1"},
                "Node": "SolNode2",
                "MissionType": "MT_SURVIVAL",
                "Modifier": "VoidT1",
                "Expiry": {"$date": {"$numberLong": "still-not-a-number"}}
            }]
        }"#;
        let world = WorldState::from_de_json(json, &Decodes::default()).unwrap();

        // A renamed/absent top-level section is visible even when it
        // would otherwise just parse as an empty array.
        assert!(world
            .decode_gaps
            .iter()
            .any(|g| g == "section:Goals missing"));
        assert!(world
            .decode_gaps
            .iter()
            .any(|g| g == "section:VoidStorms missing"));

        let degraded = &world.alerts[0];
        assert!(degraded.activation_degraded);
        assert!(world
            .decode_gaps
            .iter()
            .any(|g| g.contains("alert:alert-1.Activation unreadable")));
        // The renamed/missing faction is visible, not silently "".
        assert!(world
            .decode_gaps
            .iter()
            .any(|g| g.contains("alert:alert-1.MissionInfo.faction missing")));

        let legacy = &world.alerts[1];
        assert!(!legacy.activation_degraded, "absent Activation is legacy");
        assert!(
            !world
                .decode_gaps
                .iter()
                .any(|g| g.contains("alert:alert-legacy.Activation")),
            "legacy absence is not reported as a degraded timestamp"
        );

        let fissure = &world.fissures[0];
        assert!(fissure.expiry.is_empty());
        assert!(world
            .decode_gaps
            .iter()
            .any(|g| g.contains("fissure:SolNode2.Expiry unreadable")));
        let tuning = crate::tuning::Tuning::load(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data"),
        )
        .unwrap();
        let now = "2026-10-19T05:17:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap();
        let ranked = crate::plan::rank_fissures(
            &world.fissures,
            &["Lith"],
            1,
            now,
            false,
            &tuning,
        );
        assert!(
            ranked.is_empty(),
            "the unreadable fissure cannot rank — but the gap above records why"
        );
    }

    #[test]
    fn mission_type_decodes_match_the_recorded_warframestat_vocabulary() {
        // Addendum #51: validated against the recorded warframestat
        // fixture (it carries display names). Names the Public Export
        // enum does not cover are flagged, never guessed.
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let decodes = Decodes::load(&dir.join("data")).unwrap();
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("fixtures/warframestat-worldstate.json")).unwrap(),
        )
        .unwrap();
        let mut observed: HashSet<String> = HashSet::new();
        for f in as_array(raw.get("fissures")) {
            let t = de_str(Some(f), "missionType");
            if !t.is_empty() {
                observed.insert(t.to_owned());
            }
        }
        for a in as_array(raw.get("alerts")) {
            let t = de_str(a.get("mission"), "type");
            if !t.is_empty() {
                observed.insert(t.to_owned());
            }
        }
        for s in as_array(raw.get("sortie")) {
            for v in as_array(s.get("variants")) {
                let t = de_str(Some(v), "missionType");
                if !t.is_empty() {
                    observed.insert(t.to_owned());
                }
            }
        }
        for h in as_array(raw.get("archonHunt")) {
            for m in as_array(h.get("missions")) {
                let t = de_str(Some(m), "type");
                if !t.is_empty() {
                    observed.insert(t.to_owned());
                }
            }
        }
        // Display names with no MT enum entry in the baked table. These
        // are real DE display names but the Public Export enum has no
        // matching MT_* key; flagged rather than fabricated.
        let allowlisted_unmapped: HashSet<&str> = ["Assault", "Skirmish", "Volatile"]
            .into_iter()
            .collect();
        let decoded: HashSet<&str> = decodes
            .mission_types
            .values()
            .map(|d| d.name.as_str())
            .collect();
        let mut unmapped: Vec<&str> = observed
            .iter()
            .map(String::as_str)
            .filter(|name| !decoded.contains(name))
            .collect();
        unmapped.sort_unstable();
        let mut expected: Vec<&str> = allowlisted_unmapped.iter().copied().collect();
        expected.sort_unstable();
        assert_eq!(
            unmapped, expected,
            "recorded display names must map or be explicitly flagged"
        );
        // And every allowlisted gap really has no MT_* entry.
        for gap in allowlisted_unmapped {
            assert!(
                !decoded.contains(gap),
                "{gap} is in the MT decode table now — drop the allowlist"
            );
        }
    }

    #[test]
    fn freshness_stamp_marks_live_stale_and_unavailable() {
        let mut world = WorldState {
            timestamp: "2026-10-19T05:16:44Z".to_owned(),
            ..Default::default()
        };
        let now: chrono::DateTime<chrono::Utc> = "2026-10-19T05:17:00Z".parse().unwrap();
        assert_eq!(worldstate_status_label(&world, now, true), "worldstate live (16s)");
        // Same payload, nine minutes later: still live.
        let nine: chrono::DateTime<chrono::Utc> = "2026-10-19T05:25:44Z".parse().unwrap();
        assert_eq!(worldstate_status_label(&world, nine, true), "worldstate live (540s)");
        // Past ~10 minutes: STALE with the data's age.
        let stale: chrono::DateTime<chrono::Utc> = "2026-10-19T05:30:44Z".parse().unwrap();
        assert_eq!(worldstate_status_label(&world, stale, true), "worldstate STALE (14m)");
        // A cache/fixture view is never called live, even when fresh.
        assert_eq!(worldstate_status_label(&world, now, false), "worldstate STALE (0m)");
        // No payload at all.
        world.timestamp.clear();
        assert_eq!(worldstate_status_label(&world, now, true), "worldstate unavailable");
    }

    #[test]
    fn live_fixture_alerts_parse_tags_and_payouts() {
        let world = de_fixture();
        assert!(!world.alerts.is_empty());
        assert!(
            world.alerts.iter().all(|a| a.is_event()),
            "fixture alerts all carry the event payout"
        );
        assert!(
            !world.alerts.iter().any(|a| a.is_lotus()),
            "no gift alerts live in this fixture — section hides"
        );
    }

    #[test]
    fn live_fixture_nightwave_carries_descriptions() {
        // Pass 4 §e: name + description everywhere acts render.
        let world = de_fixture();
        let nw = world.nightwave.expect("recorded nightwave");
        assert!(!nw.active_challenges.is_empty());
        for act in &nw.active_challenges {
            assert!(!act.title.is_empty());
            assert!(!act.desc.is_empty(), "missing description: {}", act.id);
            assert!(act.standing > 0, "standing from the baked decode: {}", act.id);
        }
    }

    #[test]
    fn legacy_vanguard_relics_bake_to_axi() {
        // Addendum #66: the four pseudo-era relics are legacy Axi. The
        // bake renames both the top-level relic keys and the per-item
        // relic maps, and stays idempotent on a second pass.
        let text = r#"{
            "relics": {
                "Vanguard C1": {"name": "Vanguard C1", "vaulted": true, "sources": {},
                    "rewards": [{"name": "Caliban Prime Blueprint", "chance": {"Intact": 25.33}}]}
            },
            "items": {
                "Caliban Prime Blueprint": {"name": "Caliban Prime Blueprint",
                    "relics": {"Vanguard C1": {"Intact": 25.33}}}
            },
            "sources": {}
        }"#;
        let baked = bake_relic_tables(text).unwrap();
        let root: serde_json::Value = serde_json::from_str(&baked).unwrap();
        assert!(root["relics"].get("Vanguard C1").is_none());
        assert_eq!(root["relics"]["Axi Vanguard C1"]["name"], "Axi Vanguard C1");
        assert!(
            root["items"]["Caliban Prime Blueprint"]["relics"]
                .get("Axi Vanguard C1")
                .is_some(),
            "per-item relic keys follow the rename"
        );
        let tables = RelicTables::from_json(&baked).unwrap();
        let relic = &tables.relics["Axi Vanguard C1"];
        assert_eq!(relic.era(), "Axi");
        assert_eq!(bake_relic_tables(&baked).unwrap(), baked, "idempotent bake");
    }

    #[test]
    fn committed_relic_cache_has_no_vanguard_era() {
        // The artifact itself must be baked, not just normalized on load.
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("data/cache/relictables.json");
        let text = std::fs::read_to_string(path).unwrap();
        let root: serde_json::Value = serde_json::from_str(&text).unwrap();
        for name in ["Vanguard C1", "Vanguard E1", "Vanguard M1", "Vanguard P1"] {
            assert!(
                root["relics"].get(name).is_none(),
                "unbaked pseudo-era key in the committed cache: {name}"
            );
        }
        for (key, relic) in root["relics"].as_object().unwrap() {
            assert!(
                !relic["name"].as_str().unwrap_or(key).starts_with("Vanguard "),
                "pseudo-era survives in relic {key}"
            );
        }
    }

    #[test]
    fn trader_section_follows_the_live_window() {
        // Pass 4 Baro section: live only while a trader window covers
        // now (singular or list shape); gone when he leaves.
        let world = de_fixture();
        assert_eq!(world.void_traders.len(), 1, "rotation list parses");
        let before: chrono::DateTime<chrono::Utc> =
            "2026-10-22T13:00:00Z".parse().unwrap();
        assert!(
            world.active_trader(&before).is_none(),
            "window opens Oct 23 13:00 — section hides before"
        );
        let during: chrono::DateTime<chrono::Utc> =
            "2026-10-24T12:00:00Z".parse().unwrap();
        assert!(
            world.active_trader(&during).is_some(),
            "section renders mid-window"
        );
        let after: chrono::DateTime<chrono::Utc> =
            "2026-10-25T13:00:00Z".parse().unwrap();
        assert!(
            world.active_trader(&after).is_none(),
            "gone when he leaves"
        );
    }
}
