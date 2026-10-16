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

impl RelicTables {
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("relictables: {e}"))
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

#[derive(Debug, Clone, Deserialize)]
pub struct RosterPart {
    #[serde(default)]
    pub name: String,
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
// World state subset (api.warframestat.us)
// ---------------------------------------------------------------------------

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
    #[serde(default, rename = "voidTrader")]
    pub baro: Option<VoidTrader>,
    /// Full trader rotation (the feed carries both the singular and
    /// the list; either shape proves Baro is around).
    #[serde(default, rename = "voidTraders")]
    pub void_traders: Vec<VoidTrader>,
}

impl WorldState {
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("worldstate: {e}"))
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

/// Relic holdings. Unknown until the §8.1 OCR bootstrap resolves it —
/// then the planner ranks by coverage alone and labels "if you have it".
#[derive(Debug, Clone)]
pub enum RelicStock {
    Unknown,
    Known(HashMap<String, u32>),
}

impl RelicStock {
    pub fn count(&self, relic: &str) -> Option<u32> {
        match self {
            RelicStock::Unknown => None,
            RelicStock::Known(m) => Some(m.get(relic).copied().unwrap_or(0)),
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, RelicStock::Known(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn live_fixture_alerts_parse_tags_and_payouts() {
        let text = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/worldstate.json"),
        )
        .unwrap();
        let world = WorldState::from_json(&text).unwrap();
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
        let text = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/worldstate.json"),
        )
        .unwrap();
        let world = WorldState::from_json(&text).unwrap();
        let nw = world.nightwave.expect("recorded nightwave");
        assert!(!nw.active_challenges.is_empty());
        for act in &nw.active_challenges {
            assert!(!act.title.is_empty());
            assert!(!act.desc.is_empty(), "missing description: {}", act.id);
            assert!(act.standing > 0, "reputation parses: {}", act.id);
        }
    }

    #[test]
    fn trader_section_follows_the_live_window() {
        // Pass 4 Baro section: live only while a trader window covers
        // now (singular or list shape); gone when he leaves.
        let text = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/worldstate.json"),
        )
        .unwrap();
        let world = WorldState::from_json(&text).unwrap();
        assert_eq!(world.void_traders.len(), 1, "rotation list parses");
        let before: chrono::DateTime<chrono::Utc> =
            "2026-10-16T02:55:00Z".parse().unwrap();
        assert!(
            world.active_trader(&before).is_none(),
            "window opens 13:00 — section hides at 02:55"
        );
        let during: chrono::DateTime<chrono::Utc> =
            "2026-10-17T12:00:00Z".parse().unwrap();
        assert!(
            world.active_trader(&during).is_some(),
            "section renders mid-window"
        );
        let after: chrono::DateTime<chrono::Utc> =
            "2026-10-19T00:00:00Z".parse().unwrap();
        assert!(
            world.active_trader(&after).is_none(),
            "gone when he leaves"
        );
    }
}
