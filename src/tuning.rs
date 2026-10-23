//! Runtime tuning tables (addendum #20: no game data hardcoded in Rust).
//!
//! Every estimate, threshold, window, and content list the planner and
//! renderer interpret lives in `data/` TOML and is loaded here at
//! startup. Rust never embeds game-shaped content: changing a value
//! under `data/` changes the next generated plan with no recompilation.
//!
//! Files:
//! - `data/weeklies.toml` — the weekly board (order = render order).
//! - `data/tuning/missions.toml` — mission duration model + farm rates.
//! - `data/tuning/refinement.toml` — refinement ladder + rarity bands.
//! - `data/tuning/planner.toml` — batch bounds, windows, scores, caps.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::Refinement;

// ---------------------------------------------------------------------------
// Data-dir resolution
// ---------------------------------------------------------------------------

/// Repo `data/` dir: `$FRACKER_DATA_DIR` when set (tests, installs),
/// otherwise the `data` dir beside the current working directory.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("FRACKER_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    PathBuf::from("data")
}

// ---------------------------------------------------------------------------
// Weeklies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct WeeklyDef {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub limit: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub obs: String,
    /// Least-important items render last and visually quiet.
    #[serde(default)]
    pub deprioritized: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct WeekliesFile {
    #[serde(default)]
    pub weekly: Vec<WeeklyDef>,
}

// ---------------------------------------------------------------------------
// Missions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct MissionsFile {
    #[serde(default)]
    pub overhead: f64,
    #[serde(default)]
    pub endless_unit_minutes: f64,
    #[serde(default)]
    pub default_minutes: f64,
    #[serde(default)]
    pub default_rotations_per_hour: f64,
    #[serde(default)]
    pub minutes: HashMap<String, f64>,
    #[serde(default)]
    pub endless: Vec<String>,
    #[serde(default)]
    pub rotations_per_hour: HashMap<String, f64>,
}

#[derive(Debug, Clone)]
pub struct MissionTuning {
    pub overhead: f64,
    pub endless_unit_minutes: f64,
    pub default_minutes: f64,
    pub default_rotations_per_hour: f64,
    minutes: HashMap<String, f64>,
    endless: HashSet<String>,
    rotations_per_hour: HashMap<String, f64>,
}

fn norm_key(s: &str) -> String {
    s.to_lowercase()
}

impl MissionTuning {
    fn from_file(f: MissionsFile) -> Self {
        Self {
            overhead: f.overhead,
            endless_unit_minutes: f.endless_unit_minutes,
            default_minutes: f.default_minutes,
            default_rotations_per_hour: f.default_rotations_per_hour,
            minutes: f
                .minutes
                .into_iter()
                .map(|(k, v)| (norm_key(&k), v))
                .collect(),
            endless: f.endless.into_iter().map(|s| norm_key(&s)).collect(),
            rotations_per_hour: f
                .rotations_per_hour
                .into_iter()
                .map(|(k, v)| (norm_key(&k), v))
                .collect(),
        }
    }

    /// Base minutes per mission by type (+ overhead applied by callers).
    pub fn base_minutes(&self, mission_type: &str) -> f64 {
        self.minutes
            .get(&norm_key(mission_type))
            .copied()
            .unwrap_or(self.default_minutes)
    }

    /// Endless missions crack once per rotation unit with no
    /// orbiter/load overhead between cracks.
    pub fn is_endless(&self, mission_type: &str) -> bool {
        self.endless.contains(&norm_key(mission_type))
    }

    /// Total minutes for a batch on one mission type.
    pub fn minutes_for_batch(&self, mission_type: &str, batch: u32) -> f64 {
        let b = batch.max(1) as f64;
        if self.is_endless(mission_type) {
            b * self.endless_unit_minutes + self.overhead
        } else {
            b * (self.base_minutes(mission_type) + self.overhead)
        }
    }

    pub fn minutes_per_crack(&self, mission_type: &str, batch: u32) -> f64 {
        self.minutes_for_batch(mission_type, batch) / batch.max(1) as f64
    }

    pub fn rotations_per_hour(&self, mode_name: &str) -> f64 {
        self.rotations_per_hour
            .get(&norm_key(mode_name))
            .copied()
            .unwrap_or(self.default_rotations_per_hour)
    }
}

// ---------------------------------------------------------------------------
// Refinement + rarity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rarity {
    #[default]
    Common,
    Uncommon,
    Rare,
}

impl Rarity {
    pub fn name(self) -> &'static str {
        match self {
            Rarity::Common => "common",
            Rarity::Uncommon => "uncommon",
            Rarity::Rare => "rare",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RefinementFile {
    #[serde(default)]
    pub common_min: f64,
    #[serde(default)]
    pub uncommon_min: f64,
    #[serde(default)]
    pub trace_cost_per_relic: HashMap<String, u32>,
    #[serde(default)]
    pub ladder: HashMap<String, HashMap<String, f64>>,
}

#[derive(Debug, Clone)]
pub struct RefinementTuning {
    pub common_min: f64,
    pub uncommon_min: f64,
    trace_costs: HashMap<String, u32>,
    ladder: HashMap<String, HashMap<String, f64>>,
}

impl RefinementTuning {
    fn from_file(f: RefinementFile) -> Self {
        Self {
            common_min: f.common_min,
            uncommon_min: f.uncommon_min,
            trace_costs: f.trace_cost_per_relic,
            ladder: f.ladder,
        }
    }

    /// Rarity is a property of the (part, relic) pair, banded off the
    /// Intact chance.
    pub fn rarity_of(&self, intact_chance: f64) -> Rarity {
        if intact_chance >= self.common_min {
            Rarity::Common
        } else if intact_chance >= self.uncommon_min {
            Rarity::Uncommon
        } else {
            Rarity::Rare
        }
    }

    pub fn trace_cost(&self, tier: Refinement) -> u32 {
        self.trace_costs.get(tier.name()).copied().unwrap_or(0)
    }

    /// Refined chance when the table only guarantees the Intact value.
    /// Intact is always passthrough of the table value.
    pub fn fallback_chance(&self, intact: f64, tier: Refinement) -> f64 {
        if tier == Refinement::Intact {
            return intact;
        }
        let band = match self.rarity_of(intact) {
            Rarity::Common => "common",
            Rarity::Uncommon => "uncommon",
            Rarity::Rare => "rare",
        };
        self.ladder
            .get(band)
            .and_then(|m| m.get(tier.name()))
            .copied()
            .unwrap_or(intact)
    }
}

// ---------------------------------------------------------------------------
// Planner constants
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct PlannerFile {
    #[serde(default)]
    pub focus_max_parts: usize,
    #[serde(default)]
    pub unknown_default_batch: u32,
    #[serde(default)]
    pub max_batch: u32,
    #[serde(default)]
    pub expiry_jump_mins: f64,
    #[serde(default)]
    pub tier1_crack_mins: f64,
    #[serde(default)]
    pub max_crack_chunks: usize,
    #[serde(default)]
    pub thin_tier2_below: usize,
    #[serde(default)]
    pub farm_chunks: usize,
    #[serde(default)]
    pub score_resurgence: f64,
    #[serde(default)]
    pub score_farm: f64,
    #[serde(default)]
    pub score_wait_fissure: f64,
    #[serde(default)]
    pub score_parked: f64,
    #[serde(default)]
    pub known_stock_factor: f64,
    #[serde(default)]
    pub unknown_stock_factor: f64,
    #[serde(default)]
    pub lfg_minutes: f64,
    #[serde(default)]
    pub radshare_min_solo_cracks: f64,
    #[serde(default)]
    pub scarce_max_held: u32,
    #[serde(default)]
    pub vendor_window_hours: i64,
    #[serde(default)]
    pub days_until_hour_boundary: i64,
    #[serde(default)]
    pub steel_path_label: String,
    #[serde(default)]
    pub vendor: VendorTuning,
    #[serde(default)]
    pub nightwave: NightwaveTuning,
    #[serde(default)]
    pub alert_sections: AlertSectionsTuning,
    #[serde(default)]
    pub resurgence: ResurgenceTuning,
    #[serde(default)]
    pub daily_caps: DailyCapsTuning,
    #[serde(default)]
    pub sortie: SortieTuning,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DailyCapsTuning {
    #[serde(default)]
    pub standing_base: u32,
    #[serde(default)]
    pub standing_per_mr: u32,
    #[serde(default)]
    pub focus_base: u32,
    #[serde(default)]
    pub focus_per_mr: u32,
    #[serde(default)]
    pub fallback_standing_label: String,
    #[serde(default)]
    pub fallback_focus_label: String,
    #[serde(default)]
    pub families: Vec<String>,
    /// Player-facing names for the profile's internal `DailyAffiliation*`
    /// suffixes (`main` = the bare shared pool). Data-owned: no display
    /// string lives in `src/`.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

impl DailyCapsTuning {
    /// The player-facing label for a profile pool key. `main` is the bare
    /// `DailyAffiliation` shared pool; other keys are family suffixes. An
    /// unmapped suffix renders raw (visible, never blank) — never guess.
    pub fn label_for<'a>(&'a self, key: &'a str) -> &'a str {
        self.labels.get(key).map(String::as_str).unwrap_or(key)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SortieTuning {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub reset_label: String,
    #[serde(default)]
    pub ledger_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VendorTuning {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NightwaveTuning {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AlertSectionsTuning {
    /// Do Now §a header: time-limited event alerts.
    #[serde(default)]
    pub event_header: String,
    /// Do Now §b header: separate gift-alert section (hides when empty).
    #[serde(default)]
    pub lotus_header: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResurgenceTuning {
    #[serde(default)]
    pub contact: String,
    #[serde(default)]
    pub currency: String,
}

#[derive(Debug, Clone)]
pub struct PlannerTuning {
    pub focus_max_parts: usize,
    pub unknown_default_batch: u32,
    pub max_batch: u32,
    pub expiry_jump_mins: f64,
    pub tier1_crack_mins: f64,
    pub max_crack_chunks: usize,
    pub thin_tier2_below: usize,
    pub farm_chunks: usize,
    pub score_resurgence: f64,
    pub score_farm: f64,
    pub score_wait_fissure: f64,
    pub score_parked: f64,
    pub known_stock_factor: f64,
    pub unknown_stock_factor: f64,
    pub lfg_minutes: f64,
    pub radshare_min_solo_cracks: f64,
    pub scarce_max_held: u32,
    pub vendor_window_hours: i64,
    pub days_until_hour_boundary: i64,
    pub steel_path_label: String,
    pub vendor: VendorTuning,
    pub nightwave: NightwaveTuning,
    pub alert_sections: AlertSectionsTuning,
    pub resurgence: ResurgenceTuning,
    pub daily_caps: DailyCapsTuning,
    pub sortie: SortieTuning,
}

impl PlannerTuning {
    fn from_file(f: PlannerFile) -> Self {
        Self {
            focus_max_parts: f.focus_max_parts,
            unknown_default_batch: f.unknown_default_batch,
            max_batch: f.max_batch,
            expiry_jump_mins: f.expiry_jump_mins,
            tier1_crack_mins: f.tier1_crack_mins,
            max_crack_chunks: f.max_crack_chunks,
            thin_tier2_below: f.thin_tier2_below,
            farm_chunks: f.farm_chunks,
            score_resurgence: f.score_resurgence,
            score_farm: f.score_farm,
            score_wait_fissure: f.score_wait_fissure,
            score_parked: f.score_parked,
            known_stock_factor: f.known_stock_factor,
            unknown_stock_factor: f.unknown_stock_factor,
            lfg_minutes: f.lfg_minutes,
            radshare_min_solo_cracks: f.radshare_min_solo_cracks,
            scarce_max_held: f.scarce_max_held,
            vendor_window_hours: f.vendor_window_hours,
            days_until_hour_boundary: f.days_until_hour_boundary,
            steel_path_label: f.steel_path_label,
            vendor: f.vendor,
            nightwave: f.nightwave,
            alert_sections: f.alert_sections,
            resurgence: f.resurgence,
            daily_caps: f.daily_caps,
            sortie: f.sortie,
        }
    }
}

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Tuning {
    pub weeklies: Vec<WeeklyDef>,
    pub missions: MissionTuning,
    pub refinement: RefinementTuning,
    pub planner: PlannerTuning,
}

impl Tuning {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let weeklies_text = std::fs::read_to_string(dir.join("weeklies.toml"))
            .map_err(|e| format!("weeklies data/weeklies.toml: {e}"))?;
        let weeklies: WeekliesFile =
            toml::from_str(&weeklies_text).map_err(|e| format!("weeklies: {e}"))?;
        let missions_text = std::fs::read_to_string(dir.join("tuning/missions.toml"))
            .map_err(|e| format!("missions data/tuning/missions.toml: {e}"))?;
        let missions: MissionsFile =
            toml::from_str(&missions_text).map_err(|e| format!("missions: {e}"))?;
        let refinement_text =
            std::fs::read_to_string(dir.join("tuning/refinement.toml"))
                .map_err(|e| format!("refinement data/tuning/refinement.toml: {e}"))?;
        let refinement: RefinementFile =
            toml::from_str(&refinement_text).map_err(|e| format!("refinement: {e}"))?;
        let planner_text = std::fs::read_to_string(dir.join("tuning/planner.toml"))
            .map_err(|e| format!("planner data/tuning/planner.toml: {e}"))?;
        let planner: PlannerFile =
            toml::from_str(&planner_text).map_err(|e| format!("planner: {e}"))?;
        Ok(Self {
            weeklies: weeklies.weekly,
            missions: MissionTuning::from_file(missions),
            refinement: RefinementTuning::from_file(refinement),
            planner: PlannerTuning::from_file(planner),
        })
    }

    pub fn load_defaults() -> Result<Self, String> {
        Self::load(&data_dir())
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        Self::load(&dir).expect("test tuning loads from data/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuning_loads_all_four_files() {
        let t = Tuning::for_tests();
        assert_eq!(t.weeklies.len(), 14);
        // Least-important items render last (flagged in data, not code).
        let tail: Vec<&str> = t.weeklies.iter().rev().take(2).map(|w| w.id.as_str()).collect();
        assert!(tail.contains(&"clem") && tail.contains(&"maroo"));
        assert!(t.weeklies.iter().filter(|w| w.deprioritized).count() >= 2);
        // Pass 4 slimming: the act board left the box for its own
        // section; dropped rows never return via code.
        for gone in ["nw-weekly", "clan", "conclave", "teshin-rot", "teshin-cap", "helminth", "nw-shop", "baro"] {
            assert!(
                t.weeklies.iter().all(|w| w.id != gone),
                "cut weekly still tracked: {gone}"
            );
        }
        assert!((t.missions.base_minutes("capture") - 3.0).abs() < 1e-9);
        assert!(t.missions.is_endless("survival"));
        assert!(!t.missions.is_endless("capture"));
        assert_eq!(t.refinement.trace_cost(Refinement::Radiant), 100);
        assert_eq!(t.planner.focus_max_parts, 3);
        // Pass 4 daily bars: one pool per standing family, PvP dropped
        // with the Conclave cut (main faction pool is separate).
        assert_eq!(t.planner.daily_caps.families.len(), 12);
        assert!(!t.planner.daily_caps.families.iter().any(|f| f.eq_ignore_ascii_case("pvp")));
        assert_eq!(t.planner.nightwave.name, "Nightwave");
        assert!(!t.planner.alert_sections.event_header.is_empty());
        assert!(!t.planner.alert_sections.lotus_header.is_empty());
    }

    #[test]
    fn every_family_has_a_player_facing_label() {
        // §4(b) audit: the profile emits raw `DailyAffiliation*` suffixes;
        // every one the renderer can show must have a data-owned label, or
        // it leaks the internal name ("Library" -> "Simaris"). The main
        // shared pool (`DailyAffiliation`) is keyed `main`.
        let t = Tuning::for_tests();
        let caps = &t.planner.daily_caps;
        assert_eq!(caps.label_for("main"), "Syndicates");
        assert_eq!(caps.label_for("Library"), "Simaris");
        for fam in &caps.families {
            // Every suffix the renderer can show must be an explicit entry
            // (an unmapped suffix would still render raw — the audit point).
            assert!(
                caps.labels.contains_key(fam),
                "family {fam:?} has no [daily_caps.labels] entry — it would render its raw internal suffix"
            );
            assert!(
                !caps.label_for(fam).is_empty(),
                "family {fam:?} maps to an empty label"
            );
        }
        // An unmapped suffix renders raw (visible), never blank.
        assert_eq!(caps.label_for("SomethingNew"), "SomethingNew");
    }

    #[test]
    fn refinement_bands_and_ladder() {
        let t = Tuning::for_tests();
        assert_eq!(t.refinement.rarity_of(25.33), Rarity::Common);
        assert_eq!(t.refinement.rarity_of(11.0), Rarity::Uncommon);
        assert_eq!(t.refinement.rarity_of(2.0), Rarity::Rare);
        assert_eq!(
            t.refinement.fallback_chance(2.0, Refinement::Radiant),
            10.0
        );
        assert_eq!(
            t.refinement.fallback_chance(25.33, Refinement::Radiant),
            16.67
        );
        assert_eq!(
            t.refinement.fallback_chance(2.0, Refinement::Intact),
            2.0
        );
    }

    /// Proof obligation (addendum #20): NO content string from the data
    /// files may appear in non-test Rust source. Non-test source ends at
    /// the `#[cfg(test)]` marker in each file (test modules never ship
    /// in the binary, so content edits never recompile them either —
    /// integration tests under `tests/` are fixtures by nature and out
    /// of scope for this scan).
    #[test]
    fn no_content_strings_in_rust_source() {
        let t = Tuning::for_tests();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut names: Vec<String> = t.weeklies.iter().map(|w| w.name.clone()).collect();
        // Multi-word names must not appear even split across lines; single
        // short names only match as whole words to avoid false positives.
        names.retain(|n| !n.trim().is_empty());
        let mut violations: Vec<String> = Vec::new();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "rs").unwrap_or(false))
            .collect();
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let live = text.split("#[cfg(test)]").next().unwrap_or("");
            // Skip this very assertion's own module (past the marker).
            for name in &names {
                if name.contains(' ') {
                    if live.contains(name.as_str()) {
                        violations.push(format!(
                            "{} contains weekly {:?}",
                            path.display(),
                            name
                        ));
                    }
                } else if name.len() > 4 {
                    let hit = live.split(|c: char| !c.is_alphanumeric()).any(|w| w == name);
                    if hit {
                        violations.push(format!(
                            "{} contains weekly {:?}",
                            path.display(),
                            name
                        ));
                    }
                }
            }
        }
        assert!(
            violations.is_empty(),
            "content strings in Rust source:\n{}",
            violations.join("\n")
        );
    }
}
