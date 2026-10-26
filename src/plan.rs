//! The planner (BRIEF §10 + §17): a pure function of
//! (world state, profile, targets, drops, derived state, now) into an
//! ordered chunk slate + goal cards. Same inputs, same plan (testable).
//!
//! Ordering: strict tiers across chunk kinds; within a tier, event
//! chunks keep earliest-expiry-first. Crack chunks are the exception
//! since pass 40: their order is intrinsic (source tier, then score
//! components) and stable across fissure rotations — mission quality
//! may reorder only chunks tied on every score component (see
//! `in_tie_band`), and expiry never promotes a crack. Weekly-cycle
//! content never appears here at any tier — it is board-only.
//!
//! All estimates, thresholds, and windows come from the runtime tuning
//! tables (`crate::tuning`); this module holds no game content.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::model::{
    OwnedEquipment, Refinement, RelicEntry, RelicStock, RelicTables, Roster, WorldState,
};
use crate::tuning::{MissionTuning, PlannerTuning, Tuning};

pub use crate::tuning::Rarity;

// ---------------------------------------------------------------------------
// Inputs / outputs
// ---------------------------------------------------------------------------

pub struct PlanInput<'a> {
    pub now: DateTime<Utc>,
    pub world: &'a WorldState,
    pub owned: &'a OwnedEquipment,
    /// Raw wants (pre-prune).
    pub wants: &'a [String],
    pub roster: &'a Roster,
    pub relics: &'a RelicTables,
    pub stock: &'a RelicStock,
    /// Captured part stock, canonicalised per roster part (built and
    /// blueprint forms summed — addendum #84/#89). Drives
    /// PartState::Have in part rows and the Backlog pips.
    pub part_stock: &'a HashMap<String, u32>,
    /// Seth-asserted part holdings capture has not verified
    /// (thirtieth pass, addendum #93), keyed by roster part name
    /// (already canonical — the toggle validates against the
    /// roster). Drives PartState::HaveUnverified where the part is
    /// not also verified-held; verified stock takes precedence.
    pub unverified_parts: &'a HashMap<String, u32>,
    pub pins: &'a [String],
    /// Normalized gear names of the last two prime releases (radshare gate).
    pub recent_gear: &'a HashSet<String>,
    pub steel_path_tiebreak: bool,
    /// Log-proven mission completions (ninth pass §4): the resident's
    /// accumulated, today-scoped ledger. Used to strike live alerts that
    /// the log positively matched (never inferred).
    pub completed_missions: &'a [crate::elog::MissionCompletion],
    /// Runtime estimates/thresholds (loaded from `data/`).
    pub tuning: &'a Tuning,
    /// uniqueName -> display name map (the items dataset). Resolves
    /// the Varzia rotation's Void Projection items to relic names
    /// (addendum #76).
    pub item_names: &'a HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartState {
    Have,
    /// Asserted held by Seth from the Hunt tab but NOT verified by
    /// capture (thirtieth pass, addendum #93). Progress-identical
    /// to `Have` — the part is not a farming target and lights its
    /// pip — but every surface renders it ⍻ (U+237B), never ✓, so
    /// unverified progress is never indistinguishable from
    /// verified progress.
    HaveUnverified,
    Need,
    Unknown,
}

impl PartState {
    /// Held in hand, by either evidence class (verified capture or
    /// Seth's unverified assertion). Progress counts, pip lighting
    /// and farming-target exclusion use this; rendering picks its
    /// marker from the variant itself.
    pub fn is_held(&self) -> bool {
        matches!(self, PartState::Have | PartState::HaveUnverified)
    }
}

/// Where a relic's copies can come from, in the order the planner
/// prefers them (addendum #76): held stock first, then relics still in
/// the drop tables, then vaulted relics carried by the live Varzia
/// rotation, and finally relics with no current source at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceTier {
    Owned = 0,
    Unvaulted = 1,
    Varzia = 2,
    Unobtainable = 3,
}

impl SourceTier {
    pub fn is_unobtainable(self) -> bool {
        self == Self::Unobtainable
    }
}

/// Live Varzia availability (addendum #76), resolved exactly from
/// the world state's `PrimeVaultTraders` manifest: the rotation
/// sells a FIXED set of relics, published as Void Projection items,
/// and the items dataset names each projection (the manifest's
/// `.../Projections/T2VoidProjectionProteaIvaraVaultBBronze` is the
/// dataset's "Meso V16 Intact"). Availability is a lookup, never an
/// inference.
///
/// Addendum #68's rarest-reward rule inferred availability from
/// content overlap; that is false — Axi V12 contains a Velox Prime
/// Barrel and Velox is in the current loop, but the loop's Velox
/// relic is Meso V16 and Axi V12 is not sold (Seth, 2026-10-22:
/// "the data model is important here").
#[derive(Debug, Clone, Default)]
pub struct VarziaAvailability {
    available: HashSet<String>,
}

impl VarziaAvailability {
    pub fn from_world(world: &WorldState, item_names: &HashMap<String, String>) -> Self {
        let Some(vt) = world.vault_trader.as_ref() else {
            return Self::default();
        };
        let mut available = HashSet::new();
        for item in &vt.inventory {
            // Manifest paths are StoreItems variants of the dataset's
            // /Lotus/Types keys; match on the projection tail.
            let Some(tail) = item.unique_name.split("/Projections/").nth(1) else {
                continue;
            };
            let key = format!("/Lotus/Types/Game/Projections/{tail}");
            let Some(display) = item_names.get(&key) else {
                continue;
            };
            // "Meso V16 Intact" -> "Meso V16": drop the refinement word.
            let relic = display
                .rsplit_once(' ')
                .map(|(name, _)| name)
                .unwrap_or(display.as_str());
            if !relic.is_empty() {
                available.insert(relic.to_owned());
            }
        }
        Self { available }
    }

    pub fn is_available(&self, relic: &str) -> bool {
        self.available.contains(relic)
    }

    /// Source tier for a relic with the captured stock, if any.
    pub fn source_tier(&self, relic: &RelicEntry, stock: &RelicStock) -> SourceTier {
        if stock.count(&relic.name).unwrap_or(0) > 0 {
            SourceTier::Owned
        } else if !relic.vaulted {
            SourceTier::Unvaulted
        } else if self.is_available(&relic.name) {
            SourceTier::Varzia
        } else {
            SourceTier::Unobtainable
        }
    }

    /// A part's relics in source-priority order, rarity/chance as the
    /// tiebreak inside a tier (addendum #68).
    pub fn relics_for_part_ranked<'a>(
        &self,
        relics: &'a RelicTables,
        part: &str,
        stock: &RelicStock,
    ) -> Vec<(&'a RelicEntry, f64)> {
        let mut out = relics.relics_for_part(part);
        out.sort_by(|a, b| {
            self.source_tier(a.0, stock)
                .cmp(&self.source_tier(b.0, stock))
                .then_with(|| {
                    b.1.partial_cmp(&a.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.0.name.cmp(&b.0.name))
        });
        out
    }
}

#[derive(Debug, Clone)]
pub struct RelicOption {
    pub relic: String,
    pub vaulted: bool,
    pub chance_intact: f64,
    pub chance_radiant: f64,
    /// Owned count; None = "?" (stock unknown).
    pub owned: Option<u32>,
    pub source_tier: SourceTier,
}

#[derive(Debug, Clone)]
pub struct PartRow {
    pub name: String,
    pub state: PartState,
    pub relics: Vec<RelicOption>,
    /// "rare — 2% Intact / 10% Radiant" for the best relic.
    pub best_odds: String,
    pub rarity: Rarity,
    /// Terse backlog columns: best relic at the advised refinement.
    pub deal: Option<PartDeal>,
}

/// Best-relic deal for one part at the stock-aware refinement tier:
/// relic, its chance at that tier, expected cracks, advised refinement,
/// and held stock (None = unknown).
#[derive(Debug, Clone)]
pub struct PartDeal {
    pub relic: String,
    pub vaulted: bool,
    pub refinement: String,
    pub chance: f64,
    pub expected: f64,
    pub owned: Option<u32>,
    /// Copies already at Radiant (eighth pass §1: crack these first).
    pub radiant: Option<u32>,
    /// Source priority of the chosen relic (addendum #68).
    pub source_tier: SourceTier,
}

/// Best-relic deal for `part`: source tier first, then highest-Intact
/// relic within it, stock-aware refinement tier, table chance at that
/// tier (ladder fallback).
pub fn best_deal_for_part(
    part: &str,
    relics: &RelicTables,
    stock: &RelicStock,
    varzia: &VarziaAvailability,
    tuning: &Tuning,
) -> Option<PartDeal> {
    let (relic, intact) = varzia
        .relics_for_part_ranked(relics, part, stock)
        .into_iter()
        .next()?;
    let owned = stock.count(&relic.name);
    let radiant = stock.radiant_count(&relic.name);
    // Radiant stock is cracked as-is: no traces spent, best odds. Only
    // when there is none do we fall back to the stock-aware ladder.
    let refinement = match radiant {
        Some(r) if r > 0 => Refinement::Radiant,
        _ => choose_refinement(tuning, intact, owned),
    };
    let mut chance = relic.chance_for(part, refinement);
    if chance <= 0.0 {
        chance = tuning.refinement.fallback_chance(intact, refinement);
    }
    Some(PartDeal {
        relic: relic.name.clone(),
        vaulted: relic.vaulted,
        refinement: refinement.name().to_owned(),
        chance,
        expected: expected_cracks(chance),
        owned,
        radiant,
        source_tier: varzia.source_tier(relic, stock),
    })
}

#[derive(Debug, Clone)]
pub struct GoalAction {
    /// Complete instruction line, e.g. "Grab 3× <relic>, refine to
    /// <tier>, run <node> (<type>) — cracks <part>."
    pub text: String,
    pub kind: ActionKind,
    /// The relic/fissure backing it, for slate ranking.
    pub relic: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Crack,
    Resurgence,
    WaitFissure,
    Farm,
    Parked,
}

#[derive(Debug, Clone)]
pub struct GoalCard {
    pub gear: String,
    pub is_warframe: bool,
    pub vaulted: bool,
    pub resurgence_live: bool,
    /// Wanted parts of this gear.
    pub wants: Vec<String>,
    pub parts_have: usize,
    pub parts_total: usize,
    pub parts: Vec<PartRow>,
    /// At least one needed part has a tier-4 relic among its viable
    /// sources (addendum #68): the card is downranked in target and
    /// backlog ordering.
    pub partially_unobtainable: bool,
    pub action: GoalAction,
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: String,
    pub tier: u8,
    pub title: String,
    pub minutes_label: String,
    pub why: String,
    pub kind: String,
    pub expiry: Option<DateTime<Utc>>,
    /// Parent gear for the First Crack icon (sixth pass §2). Crack
    /// chunks carry their goal's gear; alert/rotation/vendor/farm/
    /// sortie/filler chunks have no single parent (None).
    pub gear: Option<String>,
    /// Structured crack facts for the twelfth-pass mock's compact crack
    /// rows (`Systems · +2 also`, `Meso C11`, `Radiant · 10% · 400 tr`).
    /// None for every non-crack chunk; the human strings (`title`,
    /// `why`) stay for other consumers.
    pub crack: Option<CrackInfo>,
}

/// The compact facts one First-Crack crack row renders (twelfth pass
/// §1): the target part, the chosen relic at its refinement, the batch,
/// the per-crack chance, the trace spend and the same-relic coverage.
#[derive(Debug, Clone, Default)]
pub struct CrackInfo {
    pub gear: String,
    pub part: String,
    pub relic: String,
    pub refinement: String,
    pub chance: f64,
    /// The target part's rarity band (queue part-name color, #72c).
    pub rarity: Rarity,
    /// Relic copies in the crack batch (`×4`).
    pub batch: u32,
    /// Total void-trace cost for the batch (`400 tr`).
    pub traces: u32,
    /// Other wanted parts the same relic drops (`+2 also`).
    pub covers: usize,
    /// The radshare verdict for this crack when sharing beats solo
    /// (pass 38, addendum #103): the full verdict line, carried so
    /// the queue row and the Hunt crack line can render their
    /// `radshare` marker. `None` whenever the verdict does not
    /// recommend sharing. Computed by the same helper that feeds
    /// the goal action text, so the surfaces cannot disagree.
    pub radshare: Option<String>,
    pub fissure_node: String,
    pub fissure_mission: String,
    pub steel_path: bool,
    pub vaulted: bool,
    /// The chosen relic has no current source (tier 4): the row strikes.
    pub unobtainable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FarmHint {
    pub node: String,
    pub planet: String,
    pub mode: String,
    pub relics: Vec<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub goals: Vec<GoalCard>,
    pub chunks: Vec<Chunk>,
    /// Wants pruned against the profile.
    pub pruned: Vec<String>,
    pub farms: Vec<FarmHint>,
    pub notes: Vec<String>,
    /// Do-Now focus: AT MOST 3 parts (Seth's rule), era-spread
    /// preferred. The slate's crack chunks pursue only these parts;
    /// everything else waits in Hunt/Backlog.
    pub focus: Vec<FocusPart>,
    /// Do Now §a: live time-limited event alerts (counted payouts).
    pub event_alerts: Vec<AlertView>,
    /// Do Now §b: separate gift-alert section (hides when empty).
    pub lotus_alerts: Vec<AlertView>,
    /// Do Now §d: first-class rotation element — the live rotation
    /// intersected with his unmastered vaulted items. None when the
    /// rotation covers nothing of his.
    pub resurgence: Option<ResurgenceView>,
    /// Pass-4 trader section: live only while a trader window covers
    /// now; gone when he leaves.
    pub baro: Option<BaroView>,
}

/// One alert row for the Do Now band: node, mission type, payout,
/// minutes left. Terse labels, numbers, countdowns — not sentences.
#[derive(Debug, Clone)]
pub struct AlertView {
    pub node: String,
    pub mission_type: String,
    pub payout: String,
    pub left: String,
    /// Proven complete in the EE.log (ninth pass §4): the row renders
    /// struck-through. Only ever true on a positive identity match.
    pub done: bool,
}

/// Live rotation ∩ unmastered vaulted items: the tool's showcase
/// element. Gears are covered goal names, ends_in is "in Xh/d".
#[derive(Debug, Clone)]
pub struct ResurgenceView {
    pub contact: String,
    pub location: String,
    pub ends_in: String,
    pub gears: Vec<String>,
    pub currency: String,
}

/// Live trader window: vendor + location + closing countdown.
#[derive(Debug, Clone)]
pub struct BaroView {
    pub vendor: String,
    pub location: String,
    pub ends_in: String,
}

#[derive(Debug, Clone)]
pub struct FocusPart {
    pub part: String,
    pub gear: String,
    pub era: String,
}

/// Do-Now focus picker (brief #2 item 3): at most `focus_max` parts by
/// card priority, preferring a spread across relic eras — a live
/// fissure almost always serves one of them, and an Omnia fissure can
/// serve all of them at once.
pub fn pick_focus(
    cards: &[GoalCard],
    relics: &RelicTables,
    stock: &RelicStock,
    varzia: &VarziaAvailability,
    tuning: &Tuning,
) -> Vec<FocusPart> {
    // Bottleneck (rarest) needed part per card, in card priority order.
    let mut cands: Vec<(String, String, String)> = Vec::new();
    for card in cards {
        let mut needed: Vec<&PartRow> = card
            .parts
            .iter()
            .filter(|p| p.state == PartState::Need)
            .collect();
        needed.sort_by(|a, b| {
            (a.rarity as u8)
                .cmp(&(b.rarity as u8))
                .reverse()
                .then_with(|| a.name.cmp(&b.name))
        });
        for part in needed {
            if let Some((relic, _)) = varzia
                .relics_for_part_ranked(relics, &part.name, stock)
                .into_iter()
                .next()
            {
                cands.push((
                    part.name.clone(),
                    card.gear.clone(),
                    relic.era().to_owned(),
                ));
                break;
            }
        }
    }
    // Greedy pass 1: priority order, new eras first (spread).
    let mut out: Vec<FocusPart> = Vec::new();
    let mut eras: HashSet<String> = HashSet::new();
    for (part, gear, era) in &cands {
        if out.len() >= tuning.planner.focus_max_parts {
            break;
        }
        if eras.insert(era.clone()) {
            out.push(FocusPart {
                part: part.clone(),
                gear: gear.clone(),
                era: era.clone(),
            });
        }
    }
    // Pass 2: fill remaining slots by priority regardless of era.
    for (part, gear, era) in &cands {
        if out.len() >= tuning.planner.focus_max_parts {
            break;
        }
        if !out.iter().any(|f| f.part == *part) {
            out.push(FocusPart {
                part: part.clone(),
                gear: gear.clone(),
                era: era.clone(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Time + mission model
// ---------------------------------------------------------------------------

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    s.parse::<DateTime<Utc>>().ok()
}

// ---------------------------------------------------------------------------
// Refinement advice (BRIEF §17 North Star)
// ---------------------------------------------------------------------------

pub fn expected_cracks(chance_pct: f64) -> f64 {
    if chance_pct <= 0.0 {
        f64::INFINITY
    } else {
        100.0 / chance_pct
    }
}

/// Stock-aware refinement tier.
/// * commons run Intact (refining *hurts* them);
/// * scarce stock (or unknown, treated as scarce) refines harder to
///   maximize success per copy;
/// * plentiful stock biases DOWN a tier (volume substitutes for odds);
///   formally: the cheapest tier whose expected run fits inside stock,
///   defaulting to Radiant when even that exceeds holdings.
/// Bands, ladder, and costs come from the runtime tuning tables.
pub fn choose_refinement(
    tuning: &Tuning,
    intact: f64,
    stock: Option<u32>,
) -> Refinement {
    let rarity = tuning.refinement.rarity_of(intact);
    if rarity == Rarity::Common {
        return Refinement::Intact;
    }
    let chances = [
        (Refinement::Intact, intact),
        (
            Refinement::Exceptional,
            tuning.refinement.fallback_chance(intact, Refinement::Exceptional),
        ),
        (
            Refinement::Flawless,
            tuning.refinement.fallback_chance(intact, Refinement::Flawless),
        ),
        (
            Refinement::Radiant,
            tuning.refinement.fallback_chance(intact, Refinement::Radiant),
        ),
    ];
    match stock {
        None => Refinement::Radiant,
        Some(s) => {
            for (tier, p) in chances {
                if tier == Refinement::Intact {
                    continue;
                }
                if (s as f64) >= expected_cracks(p).ceil() {
                    return tier;
                }
            }
            Refinement::Radiant
        }
    }
}

/// Expected trace spend per success at a tier (informational, shown on
/// the line so the unobservable balance can be vetoed against).
pub fn trace_spend_per_success(
    tuning: &Tuning,
    tier: Refinement,
    chance_pct: f64,
) -> f64 {
    tuning.refinement.trace_cost(tier) as f64 * expected_cracks(chance_pct)
}

// ---------------------------------------------------------------------------
// Fissure selection cascade (BRIEF §17: one answer, not a menu)
// ---------------------------------------------------------------------------

pub struct FissurePick {
    pub fissure: crate::model::Fissure,
    pub expires_in_mins: f64,
    pub batch_minutes: f64,
}

/// Rank matching fissures for a relic-era BATCH + planned batch size:
/// 1. expiry (rush window jumps first, if runnable — storms excluded),
/// 2. minutes per crack for the planned batch (dominant),
/// 3. goal fit (bottleneck era first — caller pre-sorts eras),
/// 4. Steel Path tiebreak for sprint missions (endless stays normal),
/// 5. Void Storms last resort.
///
/// HARD gate first (brief #2 §0): only fissures accepting the ENTIRE
/// batch's eras are candidates at all — mixed-era batches see solely
/// live Omnia fissures.
pub fn rank_fissures(
    fissures: &[crate::model::Fissure],
    eras: &[&str],
    batch: u32,
    now: DateTime<Utc>,
    steel_path_tiebreak: bool,
    tuning: &Tuning,
) -> Vec<FissurePick> {
    let mut picks: Vec<FissurePick> = fissures
        .iter()
        .filter(|f| f.accepts_eras(eras))
        .filter_map(|f| {
            // An unreadable expiry cannot rank. Parsing already recorded
            // the loss in `WorldState::decode_gaps`
            // ("fissure:<node>.Expiry unreadable"), so a drop here is
            // visible and never reads as "DE has no fissures".
            let expiry = parse_time(&f.expiry)?;
            if expiry <= now {
                return None;
            }
            Some(FissurePick {
                fissure: f.clone(),
                expires_in_mins: (expiry - now).num_seconds() as f64 / 60.0,
                batch_minutes: tuning.missions.minutes_for_batch(&f.mission_type, batch),
            })
        })
        .collect();
    picks.sort_by(|a, b| {
        let urg_a = a.expires_in_mins <= tuning.planner.expiry_jump_mins && !a.fissure.is_storm;
        let urg_b = b.expires_in_mins <= tuning.planner.expiry_jump_mins && !b.fissure.is_storm;
        urg_b
            .cmp(&urg_a)
            .then_with(|| {
                a.batch_minutes
                    .partial_cmp(&b.batch_minutes)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                let sp_a = steel_path_tiebreak
                    && a.fissure.is_hard
                    && !tuning.missions.is_endless(&a.fissure.mission_type);
                let sp_b = steel_path_tiebreak
                    && b.fissure.is_hard
                    && !tuning.missions.is_endless(&b.fissure.mission_type);
                sp_b.cmp(&sp_a)
            })
            .then_with(|| a.fissure.is_storm.cmp(&b.fissure.is_storm))
            .then_with(|| {
                a.expires_in_mins
                    .partial_cmp(&b.expires_in_mins)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    picks
}

// ---------------------------------------------------------------------------
// Crack candidates + radshare verdict
// ---------------------------------------------------------------------------

pub struct CrackCandidate {
    pub relic: String,
    pub era: String,
    pub vaulted: bool,
    /// Source priority of this relic (addendum #68).
    pub source_tier: SourceTier,
    /// Chance (%) of the target part in THIS relic at chosen refinement.
    pub chance: f64,
    pub refinement: Refinement,
    pub batch: u32,
    pub owned: Option<u32>,
    /// Other wanted parts the same relic can drop (coverage bonus).
    pub coverage: usize,
    /// Score: chance x coverage, stock-weighted.
    pub score: f64,
    pub expected_cracks: f64,
    pub trace_cost: u32,
}

/// Crack selection order, shared by every "best crack" pick:
/// lower source tier wins; within a tier, higher score wins.
/// Written as two explicit legs because the chain mixes
/// directions — the chained-cmp `.is_gt()` form this replaces
/// got the score leg right and the tier leg exactly backwards,
/// so a worse-tier candidate replaced a better-tier incumbent
/// whenever both reached the comparison (pass 40, addendum
/// #105). The per-relic fissure gate masked it only when the
/// worse relic's era had no live fissure; with an Omnia fissure
/// live — the feed's normal state — nothing was filtered and
/// the inversion fired (probe-verified on the v0.5.33 tree).
fn crack_outranks(
    tier: SourceTier,
    score: f64,
    best_tier: SourceTier,
    best_score: f64,
) -> bool {
    tier < best_tier
        || (tier == best_tier
            && score
                .partial_cmp(&best_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                == std::cmp::Ordering::Greater)
}

/// Best crack candidate for one part across its relics, INTRINSIC
/// (pass 40): a pure function of the part, the relic tables, stock,
/// and tuning — no fissure input at all, so the answer cannot
/// change as fissures rotate. Best = lowest source tier, then
/// highest score, then relic name (the iteration order below is
/// already tier/chance ranked, so the name falls out of it).
/// Returns None only when the part has no relics or every relic
/// for it is known-empty.
pub fn best_crack_candidate(
    part: &str,
    relics: &RelicTables,
    stock: &RelicStock,
    varzia: &VarziaAvailability,
    other_wants: &HashSet<String>,
    tuning: &Tuning,
) -> Option<CrackCandidate> {
    let mut best: Option<CrackCandidate> = None;
    for (relic, _intact) in varzia.relics_for_part_ranked(relics, part, stock) {
        let source_tier = varzia.source_tier(relic, stock);
        let intact = relic.chance_for(part, Refinement::Intact);
        let owned = stock.count(&relic.name);
        if owned == Some(0) {
            continue; // known-empty: no crack exists.
        }
        let refinement = choose_refinement(tuning, intact, owned);
        let chance = relic.chance_for(part, refinement);
        let chance = if chance <= 0.0 {
            tuning.refinement.fallback_chance(intact, refinement)
        } else {
            chance
        };
        let exp = expected_cracks(chance);
        let batch = match owned {
            Some(s) => (s.min(exp.ceil() as u32)).clamp(1, tuning.planner.max_batch),
            None => (exp.ceil() as u32).clamp(1, tuning.planner.unknown_default_batch),
        };
        let coverage = relic
            .rewards
            .iter()
            .filter(|r| r.name != part && other_wants.contains(&r.name))
            .count();
        let stock_factor = match owned {
            Some(_) => tuning.planner.known_stock_factor,
            None => tuning.planner.unknown_stock_factor,
        };
        let score = chance * (1.0 + coverage as f64) * stock_factor;
        let trace_cost = tuning.refinement.trace_cost(refinement) * batch;
        let cand = CrackCandidate {
            relic: relic.name.clone(),
            era: relic.era().to_owned(),
            vaulted: relic.vaulted,
            source_tier,
            chance,
            refinement,
            batch,
            owned,
            coverage,
            score,
            expected_cracks: exp,
            trace_cost,
        };
        let better = match &best {
            None => true,
            Some(b) => crack_outranks(source_tier, cand.score, b.source_tier, b.score),
        };
        if better {
            best = Some(cand);
        }
    }
    best
}

/// Best crack for one part WITH its mission: the intrinsic
/// candidate plus its best live fissure from the feed. None when
/// there is no candidate, or when no live fissure in the feed
/// serves the candidate's era. An Omnia fissure serves every era,
/// so with a complete feed the second case is a feed gap, not a
/// normal state — callers that must not depend on the feed (the
/// chunk slate) use `best_crack_candidate` and attach the pick
/// separately.
pub fn best_crack_for_part(
    part: &str,
    relics: &RelicTables,
    stock: &RelicStock,
    varzia: &VarziaAvailability,
    other_wants: &HashSet<String>,
    fissures: &[crate::model::Fissure],
    now: DateTime<Utc>,
    steel_path_tiebreak: bool,
    tuning: &Tuning,
) -> Option<(CrackCandidate, FissurePick)> {
    let cand = best_crack_candidate(part, relics, stock, varzia, other_wants, tuning)?;
    let pick = rank_fissures(
        fissures,
        &[cand.era.as_str()],
        cand.batch,
        now,
        steel_path_tiebreak,
        tuning,
    )
    .into_iter()
    .next()?;
    Some((cand, pick))
}

// ---------------------------------------------------------------------------
// Sort stability (pass 40): the tie band
// ---------------------------------------------------------------------------

/// The stock factor a candidate's score was built with, recovered
/// from its stock knowledge (the factor itself is not stored on
/// the candidate).
fn stock_factor_of(cand: &CrackCandidate, tuning: &Tuning) -> f64 {
    match cand.owned {
        Some(_) => tuning.planner.known_stock_factor,
        None => tuning.planner.unknown_stock_factor,
    }
}

/// "Pretty much tied on every metric" (Seth, pass 40): two crack
/// candidates in the SAME source tier (callers group by tier) whose
/// score components are each within the tuned tie band — chance
/// within `tie_chance_rel_epsilon` (relative), coverage within
/// `tie_coverage_epsilon` (0 in the shipped tuning: a coverage
/// step multiplies the score by at least 1.25x, never a tie), and
/// the stock factor exactly equal (it is a step function of stock
/// knowledge, not a continuum). Composite-score closeness alone
/// is NOT a tie: a high-chance / low-coverage candidate and a
/// low-chance / high-coverage one with similar composites differ
/// in kind, and mission quality must not reorder them.
fn in_tie_band(a: &CrackCandidate, b: &CrackCandidate, tuning: &Tuning) -> bool {
    let chance_close = (a.chance - b.chance).abs()
        <= a.chance.max(b.chance) * tuning.planner.tie_chance_rel_epsilon;
    let coverage_close =
        a.coverage.abs_diff(b.coverage) <= tuning.planner.tie_coverage_epsilon;
    let factor_close = stock_factor_of(a, tuning) == stock_factor_of(b, tuning);
    chance_close && coverage_close && factor_close
}

/// A chunk's mission quality for tie reordering: the batch minutes
/// at its best live fissure (the existing mission-time model). A
/// chunk with no live pick has no mission at all — worst quality,
/// so it never wins a tie against a chunk that has one.
fn mission_minutes(pick: Option<&FissurePick>) -> f64 {
    pick.map(|p| p.batch_minutes).unwrap_or(f64::INFINITY)
}

/// Radshare verdict: 4-player same-relic share turns per-crack chance
/// p into 1-(1-p)^4. Recommended when the share wins after LFG
/// formation time — typically bottleneck bands with scarce
/// stock — AND the relic's gear is within the last two prime releases
/// (older recruiting never fills).
pub fn radshare_verdict(
    tuning: &PlannerTuning,
    chance_pct: f64,
    rarity: Rarity,
    mpc_solo: f64,
    scarce: bool,
    gear_recent: bool,
) -> Option<String> {
    if !gear_recent {
        return None;
    }
    let p = chance_pct / 100.0;
    if p <= 0.0 {
        return None;
    }
    let p4 = 1.0 - (1.0 - p).powi(4);
    let exp_solo = 1.0 / p;
    let exp_share = 1.0 / p4;
    // Share stretches expensive relics: worth it when the solo run is
    // long (rare part) or stock is scarce, and the time math repays LFG.
    let solo_time = exp_solo * mpc_solo;
    let share_time = tuning.lfg_minutes + exp_share * mpc_solo;
    if (rarity == Rarity::Rare || scarce)
        && share_time < solo_time
        && exp_solo >= tuning.radshare_min_solo_cracks
    {
        Some(format!(
            "Best as a radshare (solo ~{:.0} cracks vs ~{:.0} shared).",
            exp_solo, exp_share
        ))
    } else {
        None
    }
}

/// "LF <relic> rad" recruiting line.
pub fn radshare_lfg(relic: &str) -> String {
    format!("LF {relic} rad")
}

/// The radshare verdict for one chosen crack (pass 38): the single
/// computation behind both the goal action text and the chunk's
/// [`CrackInfo`], so the rendered surfaces can never disagree with
/// the prose about the same crack. `gear_recent` is the last-two-
/// releases gate (older recruiting never fills); scarcity is the
/// chosen relic's known holdings against the tuned cap, with
/// unknown stock counting as scarce.
fn radshare_for(
    input: &PlanInput<'_>,
    gear: &str,
    cand: &CrackCandidate,
    pick: &FissurePick,
) -> Option<String> {
    let gear_norm = norm_name(gear);
    let gear_recent = input.recent_gear.iter().any(|g| {
        let g = norm_name(g);
        g.contains(&gear_norm) || gear_norm.contains(&g)
    });
    let mpc = input
        .tuning
        .missions
        .minutes_per_crack(&pick.fissure.mission_type, cand.batch);
    let scarce = cand
        .owned
        .map(|s| s <= input.tuning.planner.scarce_max_held)
        .unwrap_or(true);
    radshare_verdict(
        &input.tuning.planner,
        cand.chance,
        cand_rarity(input.tuning, cand),
        mpc,
        scarce,
        gear_recent,
    )
}

// ---------------------------------------------------------------------------
// Goal cards (BRIEF §17)
// ---------------------------------------------------------------------------

/// Normalize a gear/part name for rotation matching.
pub fn norm_name(s: &str) -> String {
    s.to_lowercase()
}

fn gear_in_tokens(gear: &str, tokens: &HashSet<String>) -> bool {
    let g = norm_name(gear);
    let g_words: HashSet<&str> = g.split_whitespace().collect();
    tokens.iter().any(|t| {
        t.contains(&g)
            || g.contains(t)
            || {
                let t_words: HashSet<&str> = t.split_whitespace().collect();
                // Word-order independent: "baza prime" matches the
                // rotation's "prime baza gun" ("Prime Baza Gun").
                g_words.is_subset(&t_words) || t_words.is_subset(&g_words)
            }
    })
}

fn part_rows_for_gear(
    gear: &str,
    wants_here: &[String],
    roster: &Roster,
    relics: &RelicTables,
    stock: &RelicStock,
    part_stock: &HashMap<String, u32>,
    unverified_parts: &HashMap<String, u32>,
    varzia: &VarziaAvailability,
    want_set: &HashSet<String>,
    tuning: &Tuning,
) -> Vec<PartRow> {
    let item = roster.by_name(gear);
    let mut names: Vec<String> = wants_here.to_owned();
    if let Some(item) = item {
        for p in &item.parts {
            if !names.contains(&p.name) {
                names.push(p.name.clone());
            }
        }
    }
    // Addendum #74: the item's own blueprint heads the list — exactly
    // "<Gear> Blueprint", not component blueprints like "<Gear>
    // Chassis Blueprint". The rest stay alphabetical.
    let own_blueprint = format!("{gear} Blueprint");
    names.sort_by(|a, b| {
        (a != &own_blueprint)
            .cmp(&(b != &own_blueprint))
            .then_with(|| a.cmp(b))
    });
    names
        .into_iter()
        .map(|part| {
            // Addendum #84: captured part stock reaches the plan. A
            // canonical count >= 1 (blueprint or built form, #89)
            // means the part is in hand; the roster carries no
            // required-quantity data, so the threshold is 1.
            // Thirtieth pass (#93): below verified stock, Seth's
            // unverified assertion (Hunt-tab toggle) also holds the
            // part — precedence verified > unverified > want/unknown.
            let state = if part_stock.get(&part).copied().unwrap_or(0) >= 1 {
                PartState::Have
            } else if unverified_parts.get(&part).copied().unwrap_or(0) >= 1 {
                PartState::HaveUnverified
            } else if want_set.contains(&part) {
                PartState::Need
            } else {
                PartState::Unknown
            };
            let opts: Vec<RelicOption> = varzia
                .relics_for_part_ranked(relics, &part, stock)
                .into_iter()
                .map(|(r, intact)| RelicOption {
                    relic: r.name.clone(),
                    vaulted: r.vaulted,
                    chance_intact: intact,
                    chance_radiant: r.chance_for(&part, Refinement::Radiant),
                    owned: stock.count(&r.name),
                    source_tier: varzia.source_tier(r, stock),
                })
                .collect();
            let best = opts.first();
            let rarity = tuning
                .refinement
                .rarity_of(best.map(|o| o.chance_intact).unwrap_or(0.0));
            let best_odds = match best {
                Some(o) => format!(
                    "{} — {}% Intact / {}% Radiant",
                    rarity.name(),
                    fmt_pct(o.chance_intact),
                    fmt_pct(o.chance_radiant)
                ),
                None => "no relic source".to_owned(),
            };
            let deal = best_deal_for_part(&part, relics, stock, varzia, tuning);
            PartRow {
                name: part,
                state,
                relics: opts,
                best_odds,
                rarity,
                deal,
            }
        })
        .collect()
}

fn fmt_pct(p: f64) -> String {
    if (p - p.round()).abs() < 1e-9 {
        format!("{}", p.round() as u32)
    } else {
        format!("{p}")
    }
}

/// Prune wants against profile XPInfo: a want is done when its parent
/// equipment is owned (BRIEF §10). Returns (active, pruned).
pub fn prune_wants(
    owned: &OwnedEquipment,
    wants: &[String],
    roster: &Roster,
) -> (Vec<String>, Vec<String>) {
    let mut active = Vec::new();
    let mut pruned = Vec::new();
    for w in wants {
        let parents = roster.parents_of(w);
        if parents.iter().any(|p| owned.names.contains(p)) {
            pruned.push(w.clone());
        } else {
            active.push(w.clone());
        }
    }
    (active, pruned)
}

/// Rank-ordered goal cards for the surviving wants.
fn goal_cards(
    input: &PlanInput<'_>,
    active: &[String],
    varzia: &VarziaAvailability,
) -> Vec<GoalCard> {
    let want_set: HashSet<String> = active.iter().cloned().collect();
    // Group wants by parent gear (first parent, deterministic).
    let mut by_gear: HashMap<String, Vec<String>> = HashMap::new();
    for w in active {
        let parents = input.roster.parents_of(w);
        let gear = parents.into_iter().next().unwrap_or_else(|| w.clone());
        by_gear.entry(gear).or_default().push(w.clone());
    }
    let resurgence_tokens: HashSet<String> = input
        .world
        .vault_trader
        .as_ref()
        .map(|v| v.gear_tokens())
        .unwrap_or_default();
    let mut cards: Vec<GoalCard> = by_gear
        .into_iter()
        .map(|(gear, mut wants)| {
            wants.sort();
            let item = input.roster.by_name(&gear);
            let is_warframe = item.map(|i| i.is_warframe()).unwrap_or(false);
            let vaulted = item.map(|i| i.vaulted).unwrap_or(false);
            let parts = part_rows_for_gear(
                &gear,
                &wants,
                input.roster,
                input.relics,
                input.stock,
                input.part_stock,
                input.unverified_parts,
                varzia,
                &want_set,
                input.tuning,
            );
            let parts_total = parts.len();
            // Held = verified Have + unverified-held (thirtieth
            // pass): progress counts both; the card stays
            // distinguishable through the rows'/pips' ⍻ rendering.
            let parts_have = parts.iter().filter(|p| p.state.is_held()).count();
            let partially_unobtainable = parts.iter().any(|p| {
                p.state == PartState::Need
                    && p.relics
                        .iter()
                        .any(|r| r.source_tier == SourceTier::Unobtainable)
            });
            let resurgence_live =
                !resurgence_tokens.is_empty() && gear_in_tokens(&gear, &resurgence_tokens);
            let action = derive_action(
                input,
                &gear,
                &wants,
                &parts,
                vaulted,
                resurgence_live,
                varzia,
                &want_set,
            );
            GoalCard {
                gear,
                is_warframe,
                vaulted,
                resurgence_live,
                wants,
                parts_have,
                parts_total,
                parts,
                partially_unobtainable,
                action,
            }
        })
        .collect();
    // Ordering: pins first, then CURRENT (unvaulted, or vaulted but
    // un-parked by the live Resurgence rotation) before vaulted
    // (addendum #10), frames before weapons within a tier (BRIEF §10),
    // then fully-obtainable before partially-unobtainable cards
    // (addendum #68), then fewest parts remaining.
    cards.sort_by(|a, b| {
        let pin_a = input.pins.iter().position(|p| a.wants.contains(p));
        let pin_b = input.pins.iter().position(|p| b.wants.contains(p));
        let pin_ord = match (pin_a, pin_b) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        let current_a = !a.vaulted || a.resurgence_live;
        let current_b = !b.vaulted || b.resurgence_live;
        pin_ord
            .then_with(|| current_b.cmp(&current_a))
            .then_with(|| b.is_warframe.cmp(&a.is_warframe))
            .then_with(|| {
                a.partially_unobtainable
                    .cmp(&b.partially_unobtainable)
            })
            .then_with(|| {
                (a.parts_total - a.parts_have).cmp(&(b.parts_total - b.parts_have))
            })
            .then_with(|| a.gear.cmp(&b.gear))
    });
    cards
}

/// The card's verb-first ACTION line, derived in §17 order:
/// certain instruction first — a live Resurgence rotation beats a
/// conditional crack when no copy is known-held — then crack (owns
/// relic or stock unknown + live fissure), wait-fissure, farm, parked.
#[allow(clippy::too_many_arguments)]
fn derive_action(
    input: &PlanInput<'_>,
    gear: &str,
    wants: &[String],
    parts: &[PartRow],
    vaulted: bool,
    resurgence_live: bool,
    varzia: &VarziaAvailability,
    want_set: &HashSet<String>,
) -> GoalAction {
    // Needed parts, bottleneck (rarest) first.
    let mut needed: Vec<&PartRow> = parts
        .iter()
        .filter(|p| p.state == PartState::Need)
        .collect();
    needed.sort_by(|a, b| {
        (a.rarity as u8)
            .cmp(&(b.rarity as u8))
            .reverse()
            .then_with(|| a.name.cmp(&b.name))
    });
    // Known holdings across the card's needed relics.
    let holds_known = needed.iter().flat_map(|p| input.relics.relics_for_part(&p.name)).any(
        |(r, _)| input.stock.count(&r.name).unwrap_or(0) > 0,
    );
    // No live crack. Resurgence un-parks vaulted goals in rotation.
    // When the rotation is live and no copy is known-held, the certain
    // instruction (buy the relics for the event currency) beats "crack if you have it".
    if resurgence_live && !holds_known {
        if let Some(vt) = &input.world.vault_trader {
            return GoalAction {
                text: format!(
                    "Rotation ends {} — {contact} has {}'s relics ({}). Farm {currency}, buy the relics, then crack above.",
                    days_until(input.now, &vt.expiry, input.tuning),
                    gear,
                    vt.location,
                    contact = input.tuning.planner.resurgence.contact.as_str(),
                    currency = input.tuning.planner.resurgence.currency.as_str(),
                ),
                kind: ActionKind::Resurgence,
                relic: None,
                score: input.tuning.planner.score_resurgence,
            };
        }
    }
    let mut best_crack: Option<(CrackCandidate, FissurePick, String)> = None;
    for part in &needed {
        let mut others = want_set.clone();
        others.remove(&part.name);
        // Other wants on THIS card get coverage weight too.
        if let Some((cand, pick)) = best_crack_for_part(
            &part.name,
            input.relics,
            input.stock,
            varzia,
            &others,
            &input.world.fissures,
            input.now,
            input.steel_path_tiebreak,
            input.tuning,
        ) {
            let better = match &best_crack {
                None => true,
                Some((b, _, _)) => {
                    crack_outranks(cand.source_tier, cand.score, b.source_tier, b.score)
                }
            };
            if better {
                best_crack = Some((cand, pick, part.name.clone()));
            }
        }
    }

    if let Some((cand, pick, part)) = best_crack {
        let stock_note = match cand.owned {
            Some(_) => String::new(),
            None => " (if you have it)".to_owned(),
        };
        let endless = input.tuning.missions.is_endless(&pick.fissure.mission_type);
        let stay = if endless && cand.batch > 1 {
            format!(" — stay for {} rotations", cand.batch)
        } else {
            String::new()
        };
        let sp = if pick.fissure.is_hard { input.tuning.planner.steel_path_label.as_str() } else { "" };
        let mut text = format!(
            "Grab {}\u{00d7} {}{}, refine to {}, run {} ({}){}{} — cracks {}.",
            cand.batch,
            cand.relic,
            stock_note,
            cand.refinement.name(),
            pick.fissure.node,
            pick.fissure.mission_type,
            sp,
            stay,
            part,
        );
        if cand.trace_cost > 0 {
            text.push_str(&format!(" ({} traces)", cand.trace_cost));
        }
        // Squad verdict (pre-decided line, never a choice).
        if let Some(v) = radshare_for(input, gear, &cand, &pick) {
            text.push_str(&format!(" {} {}", v, radshare_lfg(&cand.relic)));
        }
        return GoalAction {
            text,
            kind: ActionKind::Crack,
            relic: Some(cand.relic.clone()),
            score: cand.score,
        };
    }

    // Fallback: rotation live but holdings known (crack path found
    // nothing) — still surface the time-boxed buy window.
    if resurgence_live {
        if let Some(vt) = &input.world.vault_trader {
            let days = days_until(input.now, &vt.expiry, input.tuning);
            return GoalAction {
                text: format!(
                    "Rotation ends {} — {contact} has {}'s relics ({}). Farm {currency}, buy the relics, then crack above.",
                    days,
                    gear,
                    vt.location,
                    contact = input.tuning.planner.resurgence.contact.as_str(),
                    currency = input.tuning.planner.resurgence.currency.as_str(),
                ),
                kind: ActionKind::Resurgence,
                relic: None,
                score: input.tuning.planner.score_resurgence,
            };
        }
    }

    // Owns a relic but no matching fissure live -> next-fissure ETA.
    // (No forecast feed exists; say plainly when none is live.)
    let held_unvaulted_era: Option<String> = needed
        .iter()
        .flat_map(|p| input.relics.relics_for_part(&p.name))
        .filter(|(r, _)| !r.vaulted)
        .filter(|(r, _)| input.stock.count(&r.name).unwrap_or(0) > 0)
        .map(|(r, _)| r.era().to_owned())
        .next();
    if let Some(era) = held_unvaulted_era {
        return GoalAction {
            text: format!(
                "Holding a {} relic but no {} fissure is live — next {} window TBA (no forecast feed).",
                era, era, era
            ),
            kind: ActionKind::WaitFissure,
            relic: None,
            score: input.tuning.planner.score_wait_fissure,
        };
    }

    // Farm action for unvaulted relics from `sources`.
    if let Some(farm) = farm_action_for(input, wants) {
        return farm;
    }

    // Nothing held, relics vaulted, no rotation: parked, plainly.
    GoalAction {
        text: format!(
            "{}vaulted — crack-only, none held{}.",
            if vaulted { "" } else { "Relics " },
            if resurgence_live { "" } else { "" }
        ),
        kind: ActionKind::Parked,
        relic: None,
        score: input.tuning.planner.score_parked,
    }
}

fn cand_rarity(tuning: &Tuning, cand: &CrackCandidate) -> Rarity {
    // expected = 100/chance, so this recovers the target's rarity band.
    tuning
        .refinement
        .rarity_of(100.0 / cand.expected_cracks.max(0.01))
}

fn days_until(now: DateTime<Utc>, expiry: &str, tuning: &Tuning) -> String {
    match parse_time(expiry) {
        Some(t) if t > now => {
            let secs = (t - now).num_seconds();
            if secs < tuning.planner.days_until_hour_boundary * 3600 {
                format!("in {}h", secs / 3600)
            } else {
                format!("in {}d", secs / 86400)
            }
        }
        _ => "soon".to_owned(),
    }
}

/// Best farm node label + table + rotation chance for one relic.
fn best_source_node(
    missions: &MissionTuning,
    relics: &RelicTables,
    relic: &crate::model::RelicEntry,
) -> Option<(String, String, f64)> {
    let mut best: Option<(String, String, f64)> = None;
    for (table, src) in &relic.sources {
        let chance = src.rotations.values().copied().fold(0.0f64, f64::max);
        if chance <= 0.0 {
            continue;
        }
        let nodes = relics.sources.get(table)?;
        // Prefer fast missions (early-rotation farming first).
        let mut ranked: Vec<&crate::model::FarmNode> = nodes.iter().collect();
        ranked.sort_by(|a, b| {
            missions
                .rotations_per_hour(&a.mode_name)
                .partial_cmp(&missions.rotations_per_hour(&b.mode_name))
                .unwrap_or(std::cmp::Ordering::Equal)
                .reverse()
        });
        if let Some(n) = ranked.into_iter().next() {
            let label = format!("{} ({}, {})", n.name, n.planet, n.mode_name);
            let better = best.as_ref().map(|b| chance > b.2).unwrap_or(true);
            if better {
                best = Some((label, table.clone(), chance));
            }
        }
    }
    best
}

/// Farm action (§17 rule 3): best node/rotation for an unvaulted needed relic.
fn farm_action_for(input: &PlanInput<'_>, wants: &[String]) -> Option<GoalAction> {
    let mut scored: Vec<(f64, String, String)> = Vec::new();
    for w in wants {
        for (relic, _p) in input.relics.relics_for_part(w) {
            if relic.vaulted || relic.sources.is_empty() {
                continue;
            }
            if let Some((node, table, chance)) =
                best_source_node(&input.tuning.missions, input.relics, relic)
            {
                let _ = table;
                scored.push((chance, node, relic.name.clone()));
            }
        }
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.into_iter().next().map(|(chance, node, relic)| GoalAction {
        text: format!("Farm: {relic} — {node} (best rotation {chance}%)."),
        kind: ActionKind::Farm,
        relic: Some(relic),
        score: input.tuning.planner.score_farm,
    })
}

// ---------------------------------------------------------------------------
// Chunk slate (BRIEF §10 tiers)
// ---------------------------------------------------------------------------

fn chunk_minutes_label(minutes: f64, endless: bool, batch: u32) -> String {
    if endless {
        format!("~{}m, stay for {}", minutes.round() as u32, batch)
    } else if batch > 1 {
        format!("~{}m for {}", minutes.round() as u32, batch)
    } else {
        format!("~{}m", minutes.round() as u32)
    }
}

fn build_chunks(
    input: &PlanInput<'_>,
    cards: &[GoalCard],
    pruned: &[String],
    focus: &[FocusPart],
    varzia: &VarziaAvailability,
) -> (Vec<Chunk>, Vec<FarmHint>, Vec<String>) {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let _ = pruned;

    // ---- Tier 1: expiring event value ----
    for alert in &input.world.alerts {
        if alert.mission.counted_items().is_empty() {
            continue;
        }
        let expiry = match parse_time(&alert.expiry) {
            Some(t) if t > input.now => t,
            _ => continue,
        };
        let mins = (expiry - input.now).num_seconds() / 60;
        let payout: Vec<String> = alert
            .mission
            .counted_items()
            .iter()
            .map(|c| format!("{}× {}", c.count, c.kind))
            .collect();
        chunks.push(Chunk {
            id: format!("t1-alert-{}", alert.id),
            tier: 1,
            title: format!(
                "Run the event alert now: {} ({}) — {} left",
                alert.mission.node,
                alert.mission.mission_type,
                if mins >= 60 {
                    format!("{}h{}m", mins / 60, mins % 60)
                } else {
                    format!("{mins}m")
                },
            ),
            minutes_label: format!(
                "~{}m",
                input.tuning.missions.base_minutes(&alert.mission.mission_type) as u32
                    + input.tuning.missions.overhead as u32
            ),
            why: format!("Live payout: {}.", payout.join(", ")),
            kind: "event alert".to_owned(),
            expiry: Some(expiry),
            gear: None,
            crack: None,
        });
    }
    // Prime Resurgence rotation ends: time-boxed value for covered goals.
    let resurgence_goals: Vec<&GoalCard> = cards
        .iter()
        .filter(|g| g.resurgence_live)
        .collect();
    if let Some(vt) = &input.world.vault_trader {
        if !resurgence_goals.is_empty() {
            let names: Vec<String> =
                resurgence_goals.iter().map(|g| g.gear.clone()).collect();
            chunks.push(Chunk {
                id: "t1-resurgence".to_owned(),
                tier: 1,
                title: format!(
                    "{contact} rotation ends {} — {} ({}).",
                    days_until(input.now, &vt.expiry, input.tuning),
                    names.join(", "),
                    vt.location,
                    contact = input.tuning.planner.resurgence.contact.as_str(),
                ),
                minutes_label: "window".to_owned(),
                why: format!("Vaulted relics purchasable for {} while the rotation is live.", input.tuning.planner.resurgence.currency),
                kind: "prime resurgence".to_owned(),
                expiry: parse_time(&vt.expiry),
                gear: None,
                crack: None,
            });
        }
    }
    // Vendor leaving inside the tuned window.
    if let Some(baro) = &input.world.baro {
        if let Some(expiry) = parse_time(&baro.expiry) {
            let mins = (expiry - input.now).num_seconds() / 60;
            if mins > 0 && mins < input.tuning.planner.vendor_window_hours * 60 {
                chunks.push(Chunk {
                    id: "t1-baro".to_owned(),
                    tier: 1,
                    title: format!(
                        "{vendor} leaves in {}h — {} (check inventory).",
                        mins / 60,
                        baro.location,
                        vendor = input.tuning.planner.vendor.name.as_str(),
                    ),
                    minutes_label: "~5m".to_owned(),
                    why: "Biweekly vendor window closing.".to_owned(),
                    kind: "vendor".to_owned(),
                    expiry: Some(expiry),
                    gear: None,
                    crack: None,
                });
            }
        }
    }

    // ---- Tier 2: crack chunks (intrinsic slate, pass 40) ----
    // Greedy coverage: rank crack candidates by intrinsic value,
    // take each relic once. Do-Now rule: crack chunks pursue ONLY
    // the ≤3 focus parts. A chunk's EXISTENCE is intrinsic too —
    // wants, stock, and relic data alone decide it; the card's
    // action kind (which prices in the live feed) no longer gates
    // the slate, and the fissure pick is attached afterwards as
    // the chunk's mission, never as a condition.
    let focus_set: HashSet<String> =
        focus.iter().map(|f| f.part.clone()).collect();
    // Real ranked pass (owned values, no borrow tangle).
    let mut ranked: Vec<(String, CrackCandidate, Option<FissurePick>, String, String)> =
        Vec::new();
    for card in cards {
        let want_set: HashSet<String> =
            cards.iter().flat_map(|c| c.wants.clone()).collect();
        let mut needed: Vec<&PartRow> = card
            .parts
            .iter()
            .filter(|p| p.state == PartState::Need && focus_set.contains(&p.name))
            .collect();
        needed.sort_by(|a, b| {
            (a.rarity as u8)
                .cmp(&(b.rarity as u8))
                .reverse()
                .then_with(|| a.name.cmp(&b.name))
        });
        // Addendum #72b: the representative crack must be obtainable.
        // A frame whose needed parts have no obtainable source at all
        // (relics vaulted, none owned, not in the Varzia loop) never
        // enters the crack queue — there is nothing to run for it.
        let mut representative = None;
        for part in needed {
            let mut others = want_set.clone();
            others.remove(&part.name);
            if let Some(cand) = best_crack_candidate(
                &part.name,
                input.relics,
                input.stock,
                varzia,
                &others,
                input.tuning,
            ) {
                if !cand.source_tier.is_unobtainable() {
                    // The mission: best live fissure for the
                    // candidate's era (an Omnia fissure — the
                    // Conjunction Survival entries — serves every
                    // era, so a complete feed always yields one).
                    // No pick is a feed gap, not a missing chunk.
                    let pick = rank_fissures(
                        &input.world.fissures,
                        &[cand.era.as_str()],
                        cand.batch,
                        input.now,
                        input.steel_path_tiebreak,
                        input.tuning,
                    )
                    .into_iter()
                    .next();
                    representative = Some((cand, pick, part.name.clone()));
                    break;
                }
            }
        }
        if let Some((cand, pick, part_name)) = representative {
            ranked.push((
                card.gear.clone(),
                cand,
                pick,
                part_name,
                card.action.text.clone(),
            ));
        }
    }
    // Intrinsic order (pass 40): source tier, then score, then the
    // deterministic (part, relic) name pair — no live-mission input,
    // no expiry input, so two regens over unchanged intrinsic state
    // produce this same order bit for bit.
    ranked.sort_by(|a, b| {
        a.1.source_tier
            .cmp(&b.1.source_tier)
            .then_with(|| {
                b.1.score
                    .partial_cmp(&a.1.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.3.cmp(&b.3))
            .then_with(|| a.1.relic.cmp(&b.1.relic))
    });
    // Mission quality may reorder ONLY chunks tied on every score
    // component (the tie band): bubble such pairs until no adjacent
    // tied pair is mission-inverted. Bands can chain transitively
    // (A~B and B~C reorder even if A≁C) — with ≤ `max_crack_chunks`
    // + also-rans in the list this is a handful of passes.
    let mut swapped = true;
    while swapped {
        swapped = false;
        for i in 0..ranked.len().saturating_sub(1) {
            let tied = ranked[i].1.source_tier == ranked[i + 1].1.source_tier
                && in_tie_band(&ranked[i].1, &ranked[i + 1].1, input.tuning);
            if tied
                && mission_minutes(ranked[i + 1].2.as_ref())
                    < mission_minutes(ranked[i].2.as_ref())
            {
                ranked.swap(i, i + 1);
                swapped = true;
            }
        }
    }
    // All crack chunks are tier 2 now (pass 40): an expiring fissure
    // keeps only its "expiring crack" label, never a tier jump.
    let mut seen_relics: HashSet<String> = HashSet::new();
    for (gear, cand, pick, part, _action_text) in ranked {
        if !seen_relics.insert(cand.relic.clone()) {
            continue; // greedy: one chunk per relic.
        }
        let stock_note = match cand.owned {
            Some(_) => String::new(),
            None => " (if you have it)".to_owned(),
        };
        // Pass 40: expiring survives ONLY as the kind label — the
        // annotation was the same mechanism as the tier jump; the
        // label stays, the jump is gone (tier is always 2 below).
        let expiring = pick
            .as_ref()
            .map(|p| p.expires_in_mins < input.tuning.planner.tier1_crack_mins)
            .unwrap_or(false);
        let mut alternates = String::new();
        if cand.coverage > 0 {
            alternates = format!(" Covers {} other want(s) in the same relic.", cand.coverage);
        }
        let (title, minutes_label, expiry_dt) = match &pick {
            Some(pick) => {
                let endless = input.tuning.missions.is_endless(&pick.fissure.mission_type);
                let sp = if pick.fissure.is_hard {
                    input.tuning.planner.steel_path_label.as_str()
                } else {
                    ""
                };
                (
                    format!(
                        "Crack {}{} at {}{} ({} {}) — {}.",
                        cand.batch,
                        stock_note,
                        pick.fissure.node,
                        sp,
                        cand.relic,
                        cand.refinement.name(),
                        part,
                    ),
                    chunk_minutes_label(pick.batch_minutes, endless, cand.batch),
                    parse_time(&pick.fissure.expiry),
                )
            }
            // Feed gap (no live fissure serves the era — with the
            // permanent Omnia Conjunction Survival entries this is
            // an incomplete feed, not a normal state): the chunk
            // still exists, and its mission is the standing
            // fallback, labelled as such. No node, no expiry —
            // nothing the feed did not say.
            None => (
                format!(
                    "Crack {}{} ({} {}) — {}. Feed shows no {} fissure — Conjunction Survival (Lua) serves every era.",
                    cand.batch,
                    stock_note,
                    cand.relic,
                    cand.refinement.name(),
                    part,
                    cand.era,
                ),
                chunk_minutes_label(
                    input
                        .tuning
                        .missions
                        .minutes_for_batch("Survival", cand.batch),
                    true,
                    cand.batch,
                ),
                None,
            ),
        };
        chunks.push(Chunk {
            id: format!("t2-crack-{}", sanitize_id(&cand.relic)),
            tier: 2,
            title,
            minutes_label,
            why: format!(
                "{}% at {} for {} ({} traces). Goal: {}.{}",
                fmt_pct(cand.chance),
                cand.refinement.name(),
                part,
                cand.trace_cost,
                gear,
                alternates,
            ),
            kind: if expiring {
                "expiring crack".to_owned()
            } else {
                "target crack".to_owned()
            },
            expiry: expiry_dt,
            gear: Some(gear.clone()),
            crack: Some(CrackInfo {
                gear: gear.clone(),
                part: part.clone(),
                relic: cand.relic.clone(),
                refinement: cand.refinement.name().to_owned(),
                chance: cand.chance,
                rarity: cand_rarity(input.tuning, &cand),
                batch: cand.batch,
                traces: cand.trace_cost,
                covers: cand.coverage,
                radshare: pick
                    .as_ref()
                    .and_then(|pick| radshare_for(input, &gear, &cand, pick)),
                fissure_node: pick
                    .as_ref()
                    .map(|p| p.fissure.node.clone())
                    .unwrap_or_default(),
                fissure_mission: pick
                    .as_ref()
                    .map(|p| p.fissure.mission_type.clone())
                    .unwrap_or_default(),
                steel_path: pick.as_ref().map(|p| p.fissure.is_hard).unwrap_or(false),
                vaulted: cand.vaulted,
                unobtainable: cand.source_tier.is_unobtainable(),
            }),
        });
        if chunks
            .iter()
            .filter(|c| c.kind.contains("crack"))
            .count()
            >= input.tuning.planner.max_crack_chunks
        {
            // Useful slate, not exhaustive (BRIEF §10).
            break;
        }
    }

    // ---- Tier 3: relic acquisition (only when tier 2 is thin) ----
    let t2_count = chunks.iter().filter(|c| c.tier == 2).count();
    let mut farms: Vec<FarmHint> = Vec::new();
    let farm_goals: Vec<&GoalCard> = cards
        .iter()
        .filter(|g| g.action.kind == ActionKind::Farm)
        .collect();
    if t2_count < input.tuning.planner.thin_tier2_below && !farm_goals.is_empty() {
        // The farm data is still computed and carried on the plan,
        // but it is NOT chunked into the crack queue: with real stock
        // the rows listed every farmable relic across all goals —
        // removed 2026-10-23 (Seth) until a presentation that earns
        // its space exists.
        farms = score_farms(input, cards);
    }
    if farms.is_empty() && !cards.is_empty() {
        let any_farm_action = cards
            .iter()
            .any(|g| g.action.kind == ActionKind::Farm);
        if !any_farm_action {
            notes.push(
                "Relic sourcing: no farmable relic sources for current wants — crack-or-park. \
                 Sourcing activates for current-rotation goals and general relic farming."
                    .to_owned(),
            );
        }
    }

    // ---- Tier 4/5 (skeletal this pass): sortie + labeled filler ----
    if let Some(sortie) = &input.world.sortie {
        if !sortie.variants.is_empty() {
            let legs: Vec<String> = sortie
                .variants
                .iter()
                .map(|v| format!("{} ({})", v.node, v.mission_type))
                .collect();
            chunks.push(Chunk {
                id: "t4-sortie".to_owned(),
                tier: 4,
                title: format!("{}: daily three-mission chain.", input.tuning.planner.sortie.name),
                minutes_label: "~20–30m".to_owned(),
                why: format!("Legs: {}.", legs.join(" → ")),
                kind: "sortie (daily)".to_owned(),
                expiry: parse_time(&sortie.expiry),
                gear: None,
                crack: None,
            });
        }
    }
    // Filler: best remaining live fissure by coverage, labeled filler.
    if let Some(fill) = filler_fissure(input) {
        chunks.push(Chunk {
            id: "t5-filler".to_owned(),
            tier: 5,
            title: format!(
                "Filler: {} ({}) — highest-density live fissure.",
                fill.node, fill.mission_type
            ),
            minutes_label: format!(
                "~{}m",
                input.tuning.missions.base_minutes(&fill.mission_type) as u32
                    + input.tuning.missions.overhead as u32
            ),
            why: "Filler — slate ran thin; ducat/plat density ranking lands with market data later."
                .to_owned(),
            kind: "filler".to_owned(),
            expiry: parse_time(&fill.expiry),
            gear: None,
            crack: None,
        });
    }

    // Within-tier order: earliest expiry first for event chunks
    // (tier 1 keeps insertion value order for alerts; others sort
    // by expiry). Crack chunks are the pass-40 exception: they keep
    // the intrinsic order build_chunks established — expiry must
    // never reorder them, so crack pairs compare Equal and the
    // stable sort preserves insertion order.
    chunks.sort_by(|a, b| {
        a.tier.cmp(&b.tier).then_with(|| {
            if a.crack.is_some() && b.crack.is_some() {
                std::cmp::Ordering::Equal
            } else {
                match (a.expiry, b.expiry) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            }
        })
    });
    (chunks, farms, notes)
}

/// Co-yield farm scoring (§17): per node, expected wanted-relics/hour =
/// Σ over ALL wanted relics dropping there of rotation chance ×
/// rotations/hour for the mission type.
fn score_farms(input: &PlanInput<'_>, cards: &[GoalCard]) -> Vec<FarmHint> {
    let want_set: HashSet<String> =
        cards.iter().flat_map(|c| c.wants.clone()).collect();
    // node key -> (planet, mode, relics, score).
    let mut agg: HashMap<String, (String, String, Vec<String>, f64)> = HashMap::new();
    for w in &want_set {
        for (relic, _p) in input.relics.relics_for_part(w) {
            if relic.vaulted {
                continue;
            }
            for (table, src) in &relic.sources {
                let best_rot = src
                    .rotations
                    .values()
                    .copied()
                    .fold(0.0f64, f64::max);
                if best_rot <= 0.0 {
                    continue;
                }
                let nodes = match input.relics.sources.get(table) {
                    Some(n) => n,
                    None => continue,
                };
                for node in nodes {
                    let rate = input.tuning.missions.rotations_per_hour(&node.mode_name);
                    let contrib = best_rot / 100.0 * rate;
                    let key = format!("{}|{}", node.planet, node.name);
                    let entry = agg.entry(key).or_insert_with(|| {
                        (
                            node.planet.clone(),
                            node.mode_name.clone(),
                            Vec::new(),
                            0.0,
                        )
                    });
                    entry.3 += contrib;
                    if !entry.2.contains(&relic.name) {
                        entry.2.push(relic.name.clone());
                    }
                }
            }
        }
    }
    let mut out: Vec<FarmHint> = agg
        .into_iter()
        .map(|(key, (planet, mode, mut relics, score))| {
            relics.sort();
            FarmHint {
                node: key.split('|').nth(1).unwrap_or(&key).to_owned(),
                planet,
                mode,
                relics,
                score,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Best remaining live fissure for the filler chunk: most co-wanted
/// coverage, earliest expiry first.
fn filler_fissure(input: &PlanInput<'_>) -> Option<crate::model::Fissure> {
    let mut live: Vec<&crate::model::Fissure> = input
        .world
        .fissures
        .iter()
        // Same contract as `rank_fissures`: an unreadable expiry is
        // already a decode gap, so this filter cannot hide it.
        .filter(|f| parse_time(&f.expiry).map(|t| t > input.now).unwrap_or(false))
        .filter(|f| !f.is_storm)
        .collect();
    live.sort_by(|a, b| {
        a.expiry.cmp(&b.expiry).then_with(|| a.node.cmp(&b.node))
    });
    live.into_iter().next().cloned()
}

fn sanitize_id(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

/// The planner: pure function of (world, profile, targets, drops, now).
pub fn plan(input: &PlanInput<'_>) -> Plan {
    let (active, pruned) = prune_wants(input.owned, input.wants, input.roster);
    let varzia = VarziaAvailability::from_world(input.world, input.item_names);
    let cards = goal_cards_for(input, &active, &varzia);
    let focus = pick_focus(&cards, input.relics, input.stock, &varzia, input.tuning);
    let (chunks, farms, notes) =
        build_chunks(input, &cards, &pruned, &focus, &varzia);
    let (event_alerts, lotus_alerts) = donow_alerts(input);
    let resurgence = donow_resurgence(input, &cards);
    let baro = donow_baro(input);
    Plan {
        goals: cards,
        chunks,
        pruned,
        farms,
        notes,
        focus,
        event_alerts,
        lotus_alerts,
        resurgence,
        baro,
    }
}

/// Do Now §§a–b: split live alerts into time-limited event alerts
/// (counted payouts) and a separate gift section. Each hides when
/// empty; the countdowns (not prose) are the content.
fn donow_alerts(input: &PlanInput<'_>) -> (Vec<AlertView>, Vec<AlertView>) {
    let mut events = Vec::new();
    let mut lotus = Vec::new();
    for alert in &input.world.alerts {
        let expiry = match parse_time(&alert.expiry) {
            Some(t) if t > input.now => t,
            _ => continue,
        };
        let payout: Vec<String> = alert
            .mission
            .counted_items()
            .iter()
            .map(|c| format!("{}× {}", c.count, c.kind))
            .collect();
        // Ninth pass §4: strike only on a positive log match.
        let done = input
            .completed_missions
            .iter()
            .any(|c| c.matches_alert(alert));
        let view = AlertView {
            node: alert.mission.node.clone(),
            mission_type: alert.mission.mission_type.clone(),
            payout: payout.join(", "),
            left: mins_left_label(input.now, expiry),
            done,
        };
        if alert.is_lotus() {
            lotus.push(view);
        } else if alert.is_event() {
            events.push(view);
        }
    }
    (events, lotus)
}

/// Do Now §d: first-class rotation element — live rotation window
/// intersected with his unmastered vaulted items. None when the
/// rotation covers nothing of his (the showcase hides, not an empty
/// box).
fn donow_resurgence(input: &PlanInput<'_>, cards: &[GoalCard]) -> Option<ResurgenceView> {
    let vt = input.world.vault_trader.as_ref()?;
    let mut gears: Vec<String> = cards
        .iter()
        .filter(|g| g.resurgence_live)
        .map(|g| g.gear.clone())
        .collect();
    gears.sort();
    if gears.is_empty() {
        return None;
    }
    Some(ResurgenceView {
        contact: input.tuning.planner.resurgence.contact.clone(),
        location: vt.location.clone(),
        ends_in: days_until(input.now, &vt.expiry, input.tuning),
        gears,
        currency: input.tuning.planner.resurgence.currency.clone(),
    })
}

/// Pass-4 trader section: the live trader window, if any.
fn donow_baro(input: &PlanInput<'_>) -> Option<BaroView> {
    let live = input.world.active_trader(&input.now)?;
    // Addendum #85: while the window is live the header pill reads
    // "Leaves in HhMMm" at minute granularity — days_until's
    // day-rounded "in 0d" told Seth nothing on the last day.
    let ends_in = match parse_time(&live.expiry) {
        Some(expiry) => format!("Leaves in {}", mins_left_label(input.now, expiry)),
        None => "soon".to_owned(),
    };
    Some(BaroView {
        vendor: input.tuning.planner.vendor.name.clone(),
        location: live.location.clone(),
        ends_in,
    })
}

/// "25m" / "1h05m" countdown label for a live expiry.
fn mins_left_label(now: DateTime<Utc>, expiry: DateTime<Utc>) -> String {
    let mins = (expiry - now).num_seconds() / 60;
    if mins >= 60 {
        format!("{}h{:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

fn goal_cards_for(
    input: &PlanInput<'_>,
    active: &[String],
    varzia: &VarziaAvailability,
) -> Vec<GoalCard> {
    goal_cards(input, active, varzia)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elog;
    use crate::model::{Fissure, RelicTables, Roster, WorldState};

    fn test_item_names() -> &'static HashMap<String, String> {
        static NAMES: std::sync::OnceLock<HashMap<String, String>> =
            std::sync::OnceLock::new();
        NAMES.get_or_init(HashMap::new)
    }

    fn tuning() -> Tuning {
        Tuning::for_tests()
    }

    /// Shared empty part-stock map for PlanInput literals that do not
    /// exercise addendum #84.
    fn no_part_stock() -> &'static HashMap<String, u32> {
        static EMPTY: std::sync::OnceLock<HashMap<String, u32>> =
            std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    /// Shared empty unverified-parts map for PlanInput literals that
    /// do not exercise addendum #93.
    fn no_unverified() -> &'static HashMap<String, u32> {
        static EMPTY: std::sync::OnceLock<HashMap<String, u32>> =
            std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn syn_fissure(node: &str, mtype: &str, tier: &str, expiry: &str) -> Fissure {
        Fissure {
            id: node.to_owned(),
            node: node.to_owned(),
            mission_type: mtype.to_owned(),
            tier: tier.to_owned(),
            expiry: expiry.to_owned(),
            is_storm: false,
            is_hard: false,
        }
    }

    fn mini_tables() -> RelicTables {
        let text = r#"{
            "relics": {
                "Neo C5": {"name": "Neo C5",
                    "rewards": [
                        {"name": "Khora Prime Blueprint", "chance": {"Intact": 11, "Exceptional": 13, "Flawless": 17, "Radiant": 20}},
                        {"name": "Forma Blueprint", "chance": {"Intact": 25.33, "Exceptional": 23.33, "Flawless": 20, "Radiant": 16.67}}
                    ],
                    "vaulted": false,
                    "sources": {"Survival* 10-17": {"totalRotations": 3, "rotations": {"Rotation B": 7.69}}}},
                "Lith K9": {"name": "Lith K9",
                    "rewards": [
                        {"name": "Khora Prime Blueprint", "chance": {"Intact": 2, "Exceptional": 4, "Flawless": 6, "Radiant": 10}}
                    ],
                    "vaulted": true, "sources": {}}
            },
            "sources": {"Survival* 10-17": [
                {"planet": "Venus", "name": "Malva", "codeName": "ClanNode1", "type": 4,
                 "mode": "MT_SURVIVAL", "modeName": "Survival",
                 "faction": "FC_INFESTATION", "factionName": "Infested",
                 "minEnemyLevel": 8, "maxEnemyLevel": 18}]}
        }"#;
        RelicTables::from_json(text).unwrap()
    }

    fn mini_roster() -> Roster {
        Roster::from_json(
            r#"[{"name": "Khora Prime", "vaulted": true, "type": "warframe",
                  "parts": [{"name": "Khora Prime Blueprint"}, {"name": "Khora Prime Chassis Blueprint"}]}]"#,
        )
        .unwrap()
    }

    #[test]
    fn commons_run_intact_even_with_deep_stock() {
        let t = tuning();
        let stock = RelicStock::Known(crate::model::KnownStock::from_totals(
            [("X".to_owned(), 99)].into_iter().collect(),
        ));
        assert_eq!(choose_refinement(&t, 25.33, stock.count("X")), Refinement::Intact);
        assert_eq!(choose_refinement(&t, 25.33, None), Refinement::Intact);
    }

    #[test]
    fn scarce_stock_refines_hard_plentiful_biases_down() {
        let t = tuning();
        // Rare, 2% Intact: Radiant expects ~10 cracks.
        assert_eq!(
            choose_refinement(&t, 2.0, Some(1)),
            Refinement::Radiant,
            "scarce stock maximizes per-copy odds"
        );
        assert_eq!(
            choose_refinement(&t, 2.0, None),
            Refinement::Radiant,
            "unknown stock treated as scarce"
        );
        // 25 copies cover Exceptional (~25) but the cheapest fitting
        // tier above Intact wins over Radiant.
        assert_eq!(
            choose_refinement(&t, 2.0, Some(25)),
            Refinement::Exceptional,
            "volume substitutes for per-crack odds"
        );
        // Uncommon 11%: Intact expects ~9.1; 10 copies fit Flawless (~5.9)? cheapest fitting above Intact: Exceptional (~7.7).
        assert_eq!(
            choose_refinement(&t, 11.0, Some(10)),
            Refinement::Exceptional
        );
    }

    #[test]
    fn expected_cracks_honest_math() {
        assert!((expected_cracks(2.0) - 50.0).abs() < 1e-9);
        assert!((expected_cracks(10.0) - 10.0).abs() < 1e-9);
        // 4-share on a Radiant rare: 10% -> ~34%, ~10 -> ~2.9 cracks.
        let p4 = 1.0 - (1.0 - 0.10_f64).powi(4);
        assert!((p4 - 0.3439).abs() < 0.001);
        assert!((1.0 / p4 - 2.9).abs() < 0.1);
    }

    #[test]
    fn expiry_cascade_prefers_15min_runnable() {
        let t = tuning();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let mut storm = syn_fissure("C", "Capture", "Neo", "2026-10-15T21:05:00Z");
        storm.is_storm = true;
        let fiss = vec![
            syn_fissure("A", "Capture", "Neo", "2026-10-15T21:50:00Z"),
            syn_fissure("B", "Exterminate", "Neo", "2026-10-15T21:10:00Z"),
            storm,
        ];
        let picks = rank_fissures(&fiss, &["Neo"], 2, now, true, &t);
        assert_eq!(picks.len(), 3);
        assert_eq!(picks[0].fissure.node, "B", "urgent runnable jumps first");
        assert!(picks[2].fissure.is_storm, "storms last");
    }

    #[test]
    fn endless_wins_big_batches_sprint_wins_small() {
        let t = tuning();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let fiss = vec![
            syn_fissure("A", "Exterminate", "Neo", "2026-10-15T23:00:00Z"),
            syn_fissure("B", "Survival", "Neo", "2026-10-15T23:00:00Z"),
        ];
        let small = rank_fissures(&fiss, &["Neo"], 1, now, true, &t);
        assert_eq!(small[0].fissure.node, "A", "sprint wins batch of 1");
        let big = rank_fissures(&fiss, &["Neo"], 4, now, true, &t);
        assert_eq!(big[0].fissure.node, "B", "endless wins batch of 4");
    }

    #[test]
    fn radshare_gated_on_recent_releases() {
        let t = tuning();
        assert_eq!(
            radshare_verdict(&t.planner, 10.0, Rarity::Rare, 5.0, true, false),
            None,
            "old gear stays solo no matter the math"
        );
        let yes = radshare_verdict(&t.planner, 10.0, Rarity::Rare, 5.0, true, true);
        assert!(yes.is_some(), "rare + scarce + recent shares");
        assert_eq!(
            radshare_verdict(&t.planner, 25.33, Rarity::Common, 5.0, false, true),
            None,
            "cheap volume runs never share"
        );
    }

    #[test]
    fn radshare_verdict_reaches_crack_info() {
        // Pass 38 (addendum #103): the verdict used to die in the
        // goal action text, which no surface renders. It must now
        // land on the chunk's CrackInfo — the structure both the
        // queue row and the Hunt crack line render from — whenever
        // it recommends sharing, and stay None when it does not.
        let roster = Roster::from_json(
            r#"[{"name": "A Prime", "vaulted": false, "type": "warframe",
                  "parts": [{"name": "A Prime Blueprint"}]}]"#,
        )
        .unwrap();
        let text = r#"{
            "relics": {
                "Lith A1": {"name": "Lith A1",
                    "rewards": [{"name": "A Prime Blueprint", "chance": {"Intact": 2, "Radiant": 10}}],
                    "vaulted": false, "sources": {}}
            },
            "sources": {}
        }"#;
        let tables = RelicTables::from_json(text).unwrap();
        let mut world = WorldState::default();
        world.fissures = vec![syn_fissure(
            "Mantle (Earth)",
            "Exterminate",
            "Lith",
            "2026-10-15T23:00:00Z",
        )];
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec!["A Prime Blueprint".to_owned()];
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let mk = |recent: &HashSet<String>| {
            plan(&PlanInput {
                now,
                world: &world,
                owned: &owned,
                wants: &wants,
                roster: &roster,
                relics: &tables,
                stock: &stock,
                part_stock: no_part_stock(),
                unverified_parts: no_unverified(),
                pins: &[],
                recent_gear: recent,
                steel_path_tiebreak: true,
                completed_missions: &[],
                item_names: test_item_names(),
                tuning: &tuning(),
            })
        };
        let recent: HashSet<String> = ["A Prime".to_owned()].into_iter().collect();
        let p = mk(&recent);
        let chunk = p
            .chunks
            .iter()
            .find(|c| c.crack.is_some())
            .expect("a live crack chunk exists");
        let ci = chunk.crack.as_ref().unwrap();
        let verdict = ci
            .radshare
            .as_ref()
            .expect("recent + rare + scarce crack carries the verdict");
        assert!(
            verdict.contains("radshare"),
            "verdict text reaches CrackInfo: {verdict}"
        );
        assert!(
            p.goals[0].action.text.contains("Best as a radshare"),
            "action text and CrackInfo agree: {}",
            p.goals[0].action.text
        );
        let stale: HashSet<String> = HashSet::new();
        let p2 = mk(&stale);
        let chunk2 = p2
            .chunks
            .iter()
            .find(|c| c.crack.is_some())
            .expect("the crack itself still exists for old gear");
        assert!(
            chunk2.crack.as_ref().unwrap().radshare.is_none(),
            "no verdict for gear outside the recent releases"
        );
    }

    #[test]
    fn pruning_is_parentage_based() {
        let roster = mini_roster();
        let owned = OwnedEquipment {
            names: ["Khora Prime".to_owned()].into_iter().collect(),
        };
        let (active, pruned) =
            prune_wants(&owned, &["Khora Prime Blueprint".to_owned()], &roster);
        assert!(active.is_empty());
        assert_eq!(pruned.len(), 1);
    }

    #[test]
    fn focus_caps_at_three_with_era_spread() {
        // Brief #2 item 3: Do Now targets at most 3 parts, preferring
        // a spread across relic eras.
        let roster = Roster::from_json(
            r#"[{"name": "A Prime", "vaulted": false, "type": "warframe",
                  "parts": [{"name": "A Prime Blueprint"}]},
                 {"name": "B Prime", "vaulted": false, "type": "warframe",
                  "parts": [{"name": "B Prime Blueprint"}]},
                 {"name": "C Prime", "vaulted": false, "type": "warframe",
                  "parts": [{"name": "C Prime Blueprint"}]},
                 {"name": "D Prime", "vaulted": false, "type": "warframe",
                  "parts": [{"name": "D Prime Blueprint"}]}]"#,
        )
        .unwrap();
        let text = r#"{
            "relics": {
                "Lith A1": {"name": "Lith A1",
                    "rewards": [{"name": "A Prime Blueprint", "chance": {"Intact": 2, "Radiant": 10}}],
                    "vaulted": false, "sources": {}},
                "Meso B2": {"name": "Meso B2",
                    "rewards": [{"name": "B Prime Blueprint", "chance": {"Intact": 2, "Radiant": 10}}],
                    "vaulted": false, "sources": {}},
                "Neo C3": {"name": "Neo C3",
                    "rewards": [{"name": "C Prime Blueprint", "chance": {"Intact": 2, "Radiant": 10}}],
                    "vaulted": false, "sources": {}},
                "Axi D4": {"name": "Axi D4",
                    "rewards": [{"name": "D Prime Blueprint", "chance": {"Intact": 2, "Radiant": 10}}],
                    "vaulted": false, "sources": {}}
            },
            "sources": {}
        }"#;
        let tables = RelicTables::from_json(text).unwrap();
        let world = WorldState::default();
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec![
            "A Prime Blueprint".to_owned(),
            "B Prime Blueprint".to_owned(),
            "C Prime Blueprint".to_owned(),
            "D Prime Blueprint".to_owned(),
        ];
        let recent: HashSet<String> = HashSet::new();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let p = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &tuning(),
        });
        assert_eq!(p.focus.len(), 3, "at most 3 parts");
        let eras: HashSet<&str> = p.focus.iter().map(|f| f.era.as_str()).collect();
        assert_eq!(eras.len(), 3, "era spread: {eras:?}");
    }

    #[test]
    fn persto_regression_era_gate_is_hard() {
        // Brief #2 §0: the pass-1 demo paired Axi + Meso relics with
        // Persto (Deimos) while its fissure was era-locked. Era
        // compatibility gates BEFORE the cascade: an era-locked
        // fissure never receives another era, and a mixed-era batch
        // routes solely to live Omnia fissures.
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let mut persto = syn_fissure(
            "Persto (Deimos)",
            "Survival",
            "Meso",
            "2026-10-15T23:00:00Z",
        );
        let _ = &mut persto;
        let omnia = syn_fissure(
            "Circulus (Lua)",
            "Survival",
            "Omnia",
            "2026-10-15T23:00:00Z",
        );
        let fiss = vec![persto.clone(), omnia.clone()];
        // Axi batch: the Meso-locked Persto is not a candidate.
        let axi = rank_fissures(&fiss, &["Axi"], 4, now, true, &tuning());
        assert!(
            !axi.is_empty() && axi.iter().all(|p| p.fissure.tier == "Omnia"),
            "Axi batch sees only Omnia: {:?}",
            axi.iter().map(|p| &p.fissure.node).collect::<Vec<_>>(),
        );
        // Mixed Axi+Meso batch: Omnia only.
        let mixed = rank_fissures(&fiss, &["Axi", "Meso"], 4, now, true, &tuning());
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].fissure.node, "Circulus (Lua)");
        // Compatible Meso batch still reaches the locked fissure.
        let meso = rank_fissures(&fiss, &["Meso"], 4, now, true, &tuning());
        assert!(meso.iter().any(|p| p.fissure.node == "Persto (Deimos)"));
    }

    #[test]
    fn resurgence_matches_regardless_of_word_order() {
        // Live rotation lists "Prime Baza Gun" (token "prime baza gun");
        // the goal is "Baza Prime". Word order must not matter.
        let tokens: HashSet<String> =
            ["prime baza gun".to_owned(), "ivara prime".to_owned()]
                .into_iter()
                .collect();
        assert!(gear_in_tokens("Baza Prime", &tokens));
        assert!(gear_in_tokens("Ivara Prime", &tokens));
        assert!(!gear_in_tokens("Caliban Prime", &tokens));
    }

    #[test]
    fn current_before_vaulted_frames_before_weapons() {
        // Addendum #10: unvaulted/current ranks before vaulted; frames
        // before weapons within a tier. Pins still override everything.
        let roster = Roster::from_json(
            r#"[{"name": "Vaulted Frame", "vaulted": true, "type": "warframe",
                  "parts": [{"name": "Vaulted Frame Blueprint"}]},
                 {"name": "Current Gun", "vaulted": false, "type": "weapon",
                  "parts": [{"name": "Current Gun Barrel"}]},
                 {"name": "Current Frame", "vaulted": false, "type": "warframe",
                  "parts": [{"name": "Current Frame Blueprint"}]}]"#,
        )
        .unwrap();
        let tables = mini_tables();
        let world = WorldState::default();
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec![
            "Vaulted Frame Blueprint".to_owned(),
            "Current Gun Barrel".to_owned(),
            "Current Frame Blueprint".to_owned(),
        ];
        let recent: HashSet<String> = HashSet::new();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let p = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &tuning(),
        });
        let order: Vec<&str> = p.goals.iter().map(|g| g.gear.as_str()).collect();
        assert_eq!(
            order,
            vec!["Current Frame", "Current Gun", "Vaulted Frame"],
            "current first, frames before weapons: {order:?}"
        );
    }

    #[test]
    fn pins_override_all_ordering() {
        // Warframes normally rank before weapons; pinning a weapon's
        // want must surface it first regardless.
        let roster = Roster::from_json(
            r#"[{"name": "Zakti Prime", "vaulted": true, "type": "weapon",
                  "parts": [{"name": "Zakti Prime Blueprint"}]},
                 {"name": "Khora Prime", "vaulted": true, "type": "warframe",
                  "parts": [{"name": "Khora Prime Blueprint"}]}]"#,
        )
        .unwrap();
        let tables = mini_tables();
        let world = WorldState::default();
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec![
            "Zakti Prime Blueprint".to_owned(),
            "Khora Prime Blueprint".to_owned(),
        ];
        let recent: HashSet<String> = HashSet::new();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let unpinned = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &tuning(),
        });
        assert_eq!(unpinned.goals[0].gear, "Khora Prime");
        let pins = vec!["Zakti Prime Blueprint".to_owned()];
        let p = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &tuning(),
        });
        assert_eq!(p.goals[0].gear, "Zakti Prime");
    }

    fn varzia_world(unique_name: &str) -> WorldState {
        WorldState {
            vault_trader: Some(crate::model::VaultTrader {
                inventory: vec![crate::model::VaultTraderItem {
                    unique_name: unique_name.to_owned(),
                    item: String::new(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn varzia_resolves_the_manifests_projection_set() {
        // Addendum #76: the rotation sells a fixed relic set, published
        // as Void Projection items and named by the items dataset.
        // Content overlap is NOT availability: Axi V12 contains a
        // Velox Prime Barrel and Velox gear is in this loop, but the
        // loop's Velox relic is Meso V16 — Axi V12 stays unavailable.
        let mut names: HashMap<String, String> = HashMap::new();
        names.insert(
            "/Lotus/Types/Game/Projections/T2VoidProjectionProteaIvaraVaultBBronze"
                .to_owned(),
            "Meso V16 Intact".to_owned(),
        );
        names.insert(
            "/Lotus/Types/Game/Projections/T4VoidProjectionProteaIvaraVaultABronze"
                .to_owned(),
            "Axi A23 Radiant".to_owned(),
        );
        let world = WorldState {
            vault_trader: Some(crate::model::VaultTrader {
                inventory: vec![
                    crate::model::VaultTraderItem {
                        unique_name: "/Lotus/StoreItems/Weapons/Tenno/Pistols/PrimeVelox/PrimeVeloxPistol"
                            .to_owned(),
                        item: String::new(),
                    },
                    crate::model::VaultTraderItem {
                        unique_name: "/Lotus/StoreItems/Types/Game/Projections/T2VoidProjectionProteaIvaraVaultBBronze"
                            .to_owned(),
                        item: String::new(),
                    },
                    crate::model::VaultTraderItem {
                        unique_name: "/Lotus/StoreItems/Types/Game/Projections/T4VoidProjectionProteaIvaraVaultABronze"
                            .to_owned(),
                        item: String::new(),
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        };
        let varzia = VarziaAvailability::from_world(&world, &names);
        assert!(varzia.is_available("Meso V16"), "the Vault B Meso projection");
        assert!(varzia.is_available("Axi A23"), "any refinement variant names the relic");
        assert!(
            !varzia.is_available("Axi V12"),
            "contains a featured part, but is not in the sold set"
        );
        assert!(!varzia.is_available("Lith B12"), "not in this manifest");
        // No trader -> nothing is Varzia-available.
        assert!(
            !VarziaAvailability::from_world(&WorldState::default(), &names)
                .is_available("Meso V16")
        );
        // A projection the dataset cannot name resolves to nothing,
        // never a guess.
        let unnamed = varzia_world(
            "/Lotus/StoreItems/Types/Game/Projections/T1VoidProjectionNowhereVaultABronze",
        );
        assert!(
            !VarziaAvailability::from_world(&unnamed, &names).is_available("Lith B12")
        );
    }

    #[test]
    fn source_tier_orders_before_chance() {
        // Addendum #68: owned > unvaulted > Varzia > unobtainable, with
        // chance as the within-tier tiebreak. The 25.33% vaulted relic
        // must not beat the 11% unvaulted one on raw odds.
        let text = r#"{
            "relics": {
                "Lith U1": {"name": "Lith U1", "vaulted": false, "sources": {},
                    "rewards": [{"name": "Caliban Prime Blueprint", "chance": {"Intact": 11}}]},
                "Axi V1": {"name": "Axi V1", "vaulted": true, "sources": {},
                    "rewards": [{"name": "Caliban Prime Blueprint", "chance": {"Intact": 25.33}}]}
            },
            "sources": {}
        }"#;
        let relics = RelicTables::from_json(text).unwrap();
        let t = tuning();
        let varzia = VarziaAvailability::default();
        let unknown = RelicStock::Unknown;
        let deal = best_deal_for_part(
            "Caliban Prime Blueprint",
            &relics,
            &unknown,
            &varzia,
            &t,
        )
        .unwrap();
        assert_eq!(deal.relic, "Lith U1");
        assert_eq!(deal.source_tier, SourceTier::Unvaulted);

        let known = RelicStock::Known(crate::model::KnownStock::from_totals(
            [("Axi V1".to_owned(), 5)].into_iter().collect(),
        ));
        let dealt = best_deal_for_part(
            "Caliban Prime Blueprint",
            &relics,
            &known,
            &varzia,
            &t,
        )
        .unwrap();
        assert_eq!(dealt.relic, "Axi V1", "captured stock outranks drop status");
        assert_eq!(dealt.source_tier, SourceTier::Owned);

        let ranked = varzia.relics_for_part_ranked(
            &relics,
            "Caliban Prime Blueprint",
            &unknown,
        );
        assert_eq!(ranked[0].0.name, "Lith U1");
        assert_eq!(ranked[1].0.name, "Axi V1");
    }

    #[test]
    fn partially_unobtainable_cards_downrank() {
        // Addendum #68: a card with any needed tier-4 source sorts after
        // an otherwise-equal fully-obtainable card.
        let roster = Roster::from_json(
            r#"[{"name": "A Prime", "vaulted": false, "type": "warframe",
                  "parts": [{"name": "A Prime Blueprint"}]},
                 {"name": "Z Prime", "vaulted": false, "type": "warframe",
                  "parts": [{"name": "Z Prime Blueprint"}]}]"#,
        )
        .unwrap();
        let text = r#"{
            "relics": {
                "Axi D1": {"name": "Axi D1", "vaulted": true, "sources": {},
                    "rewards": [{"name": "A Prime Blueprint", "chance": {"Intact": 25.33}}]},
                "Lith Z1": {"name": "Lith Z1", "vaulted": false, "sources": {},
                    "rewards": [{"name": "Z Prime Blueprint", "chance": {"Intact": 11}}]}
            },
            "sources": {}
        }"#;
        let relics = RelicTables::from_json(text).unwrap();
        let world = WorldState::default();
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec![
            "A Prime Blueprint".to_owned(),
            "Z Prime Blueprint".to_owned(),
        ];
        let recent: HashSet<String> = HashSet::new();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let varzia = VarziaAvailability::from_world(&world, &HashMap::new());
        let cards = goal_cards(
            &PlanInput {
                now,
                world: &world,
                owned: &owned,
                wants: &wants,
                roster: &roster,
                relics: &relics,
                stock: &stock,
                part_stock: no_part_stock(),
                unverified_parts: no_unverified(),
                pins: &[],
                recent_gear: &recent,
                steel_path_tiebreak: true,
                completed_missions: &[],
                item_names: test_item_names(),
                tuning: &tuning(),
            },
            &wants,
            &varzia,
        );
        let order: Vec<&str> = cards.iter().map(|c| c.gear.as_str()).collect();
        assert_eq!(order, vec!["Z Prime", "A Prime"]);
        assert!(!cards[0].partially_unobtainable);
        assert!(cards[1].partially_unobtainable);
    }

    #[test]
    fn scored_matching_beats_blind_names() {
        // Same part, two relics: 11% vs 2% Intact. The 11% relic must win.
        let tables = mini_tables();
        let stock = RelicStock::Unknown;
        let fiss = vec![syn_fissure(
            "Hepit (Void)",
            "Capture",
            "Neo",
            "2026-10-15T23:00:00Z",
        )];
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let others: HashSet<String> = HashSet::new();
        let varzia = VarziaAvailability::default();
        let (cand, _) = best_crack_for_part(
            "Khora Prime Blueprint",
            &tables,
            &stock,
            &varzia,
            &others,
            &fiss,
            now,
            true,
            &tuning(),
        )
        .unwrap();
        assert_eq!(cand.relic, "Neo C5");
        assert!((cand.chance - 20.0).abs() < 1e-9, "Radiant pick: {}", cand.chance);
    }

    #[test]
    fn plan_is_pure_and_weekly_free() {
        let tables = mini_tables();
        let roster = mini_roster();
        let world = WorldState {
            fissures: vec![syn_fissure(
                "Hepit (Void)",
                "Capture",
                "Neo",
                "2026-10-15T23:00:00Z",
            )],
            ..Default::default()
        };
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec!["Khora Prime Blueprint".to_owned()];
        let pins: Vec<String> = Vec::new();
        let recent: HashSet<String> = HashSet::new();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let mk = || {
            plan(&PlanInput {
                now,
                world: &world,
                owned: &owned,
                wants: &wants,
                roster: &roster,
                relics: &tables,
                stock: &stock,
                part_stock: no_part_stock(),
                unverified_parts: no_unverified(),
                pins: &pins,
                recent_gear: &recent,
                steel_path_tiebreak: true,
                completed_missions: &[],
                item_names: test_item_names(),
                tuning: &tuning(),
            })
        };
        let a = mk();
        let b = mk();
        assert_eq!(a.chunks.len(), b.chunks.len());
        assert_eq!(a.goals.len(), b.goals.len());
        assert_eq!(a.goals[0].action.text, b.goals[0].action.text);
    }

    fn syn_alert(
        id: &str,
        tag: &str,
        desc: &str,
        expiry: &str,
        counted: bool,
    ) -> crate::model::Alert {
        use crate::model::{Alert, AlertMission, AlertReward, CountedItem};
        Alert {
            id: id.to_owned(),
            tag: tag.to_owned(),
            activation: String::new(),
            activation_degraded: false,
            expiry: expiry.to_owned(),
            mission: AlertMission {
                description: desc.to_owned(),
                node: "Nimus (Eris)".to_owned(),
                mission_type: "Survival".to_owned(),
                reward: AlertReward {
                    counted_items: if counted {
                        vec![CountedItem {
                            unique_name: "u".to_owned(),
                            kind: "Event Resource".to_owned(),
                            count: 50,
                        }]
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }

    #[test]
    fn donow_views_split_alerts_and_gate_trader() {
        // Pass 4: event vs gift sections split on the feed; the trader
        // view follows the live window (list shape included).
        let tables = mini_tables();
        let roster = mini_roster();
        let now: DateTime<Utc> = "2026-10-16T02:55:00Z".parse().unwrap();
        let mut world = WorldState {
            alerts: vec![
                syn_alert("e1", "RoadToTauEvent", "Event Alert Title", "2026-10-16T03:00:00Z", true),
                syn_alert("g1", "GiftOfTheLotus", "Gift of the Lotus", "2026-10-16T05:00:00Z", false),
                syn_alert("x1", "Expired", "Old Alert", "2026-10-15T00:00:00Z", true),
            ],
            baro: None,
            void_traders: vec![crate::model::VoidTrader {
                activation: "2026-10-16T13:00:00Z".to_owned(),
                expiry: "2026-10-18T13:00:00Z".to_owned(),
                location: "Orcus Relay (Pluto)".to_owned(),
                active: false,
            }],
            ..Default::default()
        };
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec!["Khora Prime Blueprint".to_owned()];
        let pins: Vec<String> = Vec::new();
        let recent: HashSet<String> = HashSet::new();
        let t = tuning();
        let p = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &t,
        });
        assert_eq!(p.event_alerts.len(), 1, "expired alert drops out");
        assert_eq!(p.event_alerts[0].left, "5m");
        assert!(p.event_alerts[0].payout.contains("50×"));
        assert_eq!(p.lotus_alerts.len(), 1);
        assert_eq!(p.lotus_alerts[0].node, "Nimus (Eris)");
        assert!(p.baro.is_none(), "window opens 13:00 — hidden at 02:55");
        // Mid-window the same world shows the trader (list shape).
        let noon: DateTime<Utc> = "2026-10-17T12:00:00Z".parse().unwrap();
        world.alerts.clear();
        let q = plan(&PlanInput {
            now: noon,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &t,
        });
        assert!(q.event_alerts.is_empty() && q.lotus_alerts.is_empty());
        let b = q.baro.expect("trader live mid-window");
        assert_eq!(b.location, "Orcus Relay (Pluto)");
        // Addendum #85: live window reads "Leaves in …" at hour/minute
        // granularity (noon Oct 17 -> expiry 13:00 Oct 18 = 25h00m).
        assert_eq!(b.ends_in, "Leaves in 25h00m");
    }

    #[test]
    fn captured_part_stock_marks_rows_have_and_lights_pips() {
        // Addendum #84 (Seth's case): "Dual Zoren Prime Handle" in the
        // captured stock -> the Dual Zoren goal card shows that part
        // as Have and counts it in parts_have (the Backlog pips).
        let tables = mini_tables();
        let roster = Roster::from_json(
            r#"[{"name": "Dual Zoren Prime", "vaulted": true, "type": "melee",
                 "parts": [{"name": "Dual Zoren Prime Blueprint"},
                           {"name": "Dual Zoren Prime Blade"},
                           {"name": "Dual Zoren Prime Handle"}]}]"#,
        )
        .unwrap();
        let now: DateTime<Utc> = "2026-10-16T02:55:00Z".parse().unwrap();
        let world = WorldState::default();
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        // The map report.rs builds from a capture holding the Handle.
        let captured = crate::ocr::StockState {
            parts: [("Dual Zoren Prime Handle".to_owned(), 1)]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let part_stock = captured.canonical_part_counts(&roster);
        let wants = vec!["Dual Zoren Prime Blade".to_owned()];
        let pins: Vec<String> = Vec::new();
        let recent: HashSet<String> = HashSet::new();
        let t = tuning();
        let p = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: &part_stock,
            unverified_parts: no_unverified(),
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &t,
        });
        let card = p
            .goals
            .iter()
            .find(|g| g.gear == "Dual Zoren Prime")
            .expect("goal card");
        let state_of = |name: &str| {
            card.parts
                .iter()
                .find(|r| r.name == name)
                .map(|r| &r.state)
        };
        assert_eq!(state_of("Dual Zoren Prime Handle"), Some(&PartState::Have));
        assert_eq!(state_of("Dual Zoren Prime Blade"), Some(&PartState::Need));
        assert_eq!(card.parts_have, 1, "the held Handle lights one pip");
    }

    #[test]
    fn unverified_stock_marks_rows_have_unverified_below_verified() {
        // Thirtieth pass (#93): Seth's unverified assertion holds a
        // part (PartState::HaveUnverified — progress counts it, it
        // is not a farming target), and verified capture stock
        // outranks the assertion where both exist.
        let tables = mini_tables();
        let roster = Roster::from_json(
            r#"[{"name": "Dual Zoren Prime", "vaulted": true, "type": "melee",
                 "parts": [{"name": "Dual Zoren Prime Blueprint"},
                           {"name": "Dual Zoren Prime Blade"},
                           {"name": "Dual Zoren Prime Handle"}]}]"#,
        )
        .unwrap();
        let now: DateTime<Utc> = "2026-10-16T02:55:00Z".parse().unwrap();
        let world = WorldState::default();
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec!["Dual Zoren Prime Blade".to_owned()];
        let pins: Vec<String> = Vec::new();
        let recent: HashSet<String> = HashSet::new();
        let t = tuning();
        let unverified: HashMap<String, u32> = [
            ("Dual Zoren Prime Handle".to_owned(), 1),
            ("Dual Zoren Prime Blade".to_owned(), 1),
        ]
        .into_iter()
        .collect();
        let build = |part_stock: &HashMap<String, u32>| {
            plan(&PlanInput {
                now,
                world: &world,
                owned: &owned,
                wants: &wants,
                roster: &roster,
                relics: &tables,
                stock: &stock,
                part_stock,
                unverified_parts: &unverified,
                pins: &pins,
                recent_gear: &recent,
                steel_path_tiebreak: true,
                completed_missions: &[],
                item_names: test_item_names(),
                tuning: &t,
            })
        };
        let state_of = |p: &crate::plan::Plan, name: &str| {
            p.goals
                .iter()
                .find(|g| g.gear == "Dual Zoren Prime")
                .and_then(|c| c.parts.iter().find(|r| r.name == name))
                .map(|r| r.state.clone())
        };

        // Assertion only: Handle is unverified-held; the Blade is
        // asserted too, so it leaves Need (not a farming target)
        // even though it is wanted.
        let p = build(no_part_stock());
        assert_eq!(
            state_of(&p, "Dual Zoren Prime Handle"),
            Some(PartState::HaveUnverified)
        );
        assert_eq!(
            state_of(&p, "Dual Zoren Prime Blade"),
            Some(PartState::HaveUnverified),
            "an asserted want is held, not a target"
        );
        let card = p
            .goals
            .iter()
            .find(|g| g.gear == "Dual Zoren Prime")
            .expect("goal card");
        assert_eq!(card.parts_have, 2, "unverified holds count toward progress");

        // Verified capture of the Handle outranks its assertion.
        let verified: HashMap<String, u32> =
            [("Dual Zoren Prime Handle".to_owned(), 1)].into_iter().collect();
        let p = build(&verified);
        assert_eq!(
            state_of(&p, "Dual Zoren Prime Handle"),
            Some(PartState::Have),
            "verified > unverified precedence"
        );
        assert_eq!(
            state_of(&p, "Dual Zoren Prime Blade"),
            Some(PartState::HaveUnverified)
        );
    }

    /// Ninth pass §4: a log-proven completion strikes exactly the alert
    /// it positively identifies, and nothing else.
    #[test]
    fn log_completed_alert_renders_struck_only_on_a_positive_match() {
        let tables = mini_tables();
        let roster = mini_roster();
        let now: DateTime<Utc> = "2026-10-16T02:55:00Z".parse().unwrap();
        let mut alerts = vec![
            syn_alert("e1", "RoadToTauEvent", "Event Alert Title", "2026-10-16T03:00:00Z", true),
            syn_alert("e2", "RoadToTauEvent", "Event Alert Title", "2026-10-16T03:10:00Z", true),
        ];
        // The sibling carries the same tag but a different mission type,
        // so only the positively identified alert may strike.
        alerts[1].mission.mission_type = "Extermination".to_owned();
        let world = WorldState {
            alerts,
            ..Default::default()
        };
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec!["Khora Prime Blueprint".to_owned()];
        let pins: Vec<String> = Vec::new();
        let recent: HashSet<String> = HashSet::new();
        let t = tuning();

        let completion = elog::MissionCompletion {
            info: elog::MissionInfo {
                alert_id: "e1".to_owned(),
                alert_tag: "RoadToTauEvent".to_owned(),
                mission_type: "MT_SURVIVAL".to_owned(),
                ..Default::default()
            },
            day: "2026-10-16".to_owned(),
            observed_at: "2026-10-16T02:56:00Z".to_owned(),
        };
        let completions = vec![completion];
        let p = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &completions,
            item_names: test_item_names(),
            tuning: &t,
        });
        assert_eq!(p.event_alerts.len(), 2);
        let e1 = p.event_alerts.iter().find(|a| a.left == "5m").unwrap();
        let e2 = p.event_alerts.iter().find(|a| a.left == "15m").unwrap();
        assert!(e1.done, "the positively identified alert is struck");
        assert!(!e2.done, "a sibling alert is never struck by inference");

        // With no log state, nothing is struck (auto/manual parity).
        let q = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &t,
        });
        assert!(q.event_alerts.iter().all(|a| !a.done));
    }

    // -----------------------------------------------------------------
    // Pass 40 (addendum #105): sort stability gates.
    //
    // Scenario shape: one gear per chunk, one relic per part, stock
    // Unknown unless a test says otherwise (so refinement is Radiant
    // and the candidate chance is the table's Radiant value; batches
    // land on the unknown-stock default of 4). Mission minutes for a
    // batch of 4 from the shipped tuning: Capture 20, Survival 22,
    // Exterminate 28, Mobile Defense 36.
    // -----------------------------------------------------------------

    /// (gear, relic, radiant chance) triples -> (roster, tables, wants).
    fn stability_fixture(
        spec: &[(&str, &str, f64)],
    ) -> (Roster, RelicTables, Vec<String>) {
        let roster_json: Vec<String> = spec
            .iter()
            .map(|(gear, _, _)| {
                format!(
                    r#"{{"name": "{gear}", "vaulted": false, "type": "warframe",
                         "parts": [{{"name": "{gear} Blueprint"}}]}}"#
                )
            })
            .collect();
        let roster =
            Roster::from_json(&format!("[{}]", roster_json.join(","))).unwrap();
        let relic_json: Vec<String> = spec
            .iter()
            .map(|(gear, relic, radiant)| {
                format!(
                    r#""{relic}": {{"name": "{relic}", "vaulted": false, "sources": {{}},
                        "rewards": [{{"name": "{gear} Blueprint",
                            "chance": {{"Intact": 11, "Exceptional": 13, "Flawless": 17, "Radiant": {radiant}}}}}]}}"#
                )
            })
            .collect();
        let tables = RelicTables::from_json(
            &format!(r#"{{"relics": {{{}}}, "sources": {{}}}}"#, relic_json.join(",")),
        )
        .unwrap();
        let wants: Vec<String> =
            spec.iter().map(|(gear, _, _)| format!("{gear} Blueprint")).collect();
        (roster, tables, wants)
    }

    fn stability_plan(
        roster: &Roster,
        tables: &RelicTables,
        wants: &[String],
        fissures: Vec<Fissure>,
    ) -> Plan {
        let world = WorldState {
            fissures,
            ..Default::default()
        };
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let recent: HashSet<String> = HashSet::new();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants,
            roster,
            relics: tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &tuning(),
        })
    }

    fn crack_relics(p: &Plan) -> Vec<String> {
        p.chunks
            .iter()
            .filter(|c| c.crack.is_some())
            .map(|c| c.crack.as_ref().unwrap().relic.clone())
            .collect()
    }

    #[test]
    fn stability_chunks_exist_without_any_fissure() {
        // Change 1: chunk existence is intrinsic. An empty fissure
        // feed yields the chunks anyway, carrying no invented node
        // or expiry — and an Omnia-only feed (the Conjunction
        // Survival shape) serves every era's chunk.
        let (roster, tables, wants) = stability_fixture(&[
            ("Alpha Prime", "Lith A1", 20.0),
            ("Beta Prime", "Meso B1", 19.0),
        ]);
        let p = stability_plan(&roster, &tables, &wants, vec![]);
        let cracks: Vec<&Chunk> =
            p.chunks.iter().filter(|c| c.crack.is_some()).collect();
        assert_eq!(cracks.len(), 2, "chunks exist on an empty feed");
        for c in &cracks {
            let ci = c.crack.as_ref().unwrap();
            assert_eq!(ci.fissure_node, "", "no invented node");
            assert!(c.expiry.is_none(), "no invented expiry");
            assert!(
                c.title.contains("Conjunction Survival"),
                "the standing fallback is labelled: {}",
                c.title
            );
        }
        let omnia = vec![syn_fissure(
            "Yuvarium (Lua)",
            "Survival",
            "Omnia",
            "2026-10-15T23:00:00Z",
        )];
        let p2 = stability_plan(&roster, &tables, &wants, omnia);
        for c in p2.chunks.iter().filter(|c| c.crack.is_some()) {
            assert_eq!(
                c.crack.as_ref().unwrap().fissure_node,
                "Yuvarium (Lua)",
                "Omnia serves every era"
            );
        }
    }

    #[test]
    fn stability_rotation_changes_order_only_within_tie_bands() {
        // Gate (a): a clearly separated slate (20 / 11 / 5 — no pair
        // inside the chance band) keeps its exact order across
        // rotations that change every chunk's mission and expiry.
        let (roster, tables, wants) = stability_fixture(&[
            ("Alpha Prime", "Lith A1", 20.0),
            ("Beta Prime", "Meso B1", 11.0),
            ("Gamma Prime", "Neo G1", 5.0),
        ]);
        let want_order = vec![
            "Lith A1".to_owned(),
            "Meso B1".to_owned(),
            "Neo G1".to_owned(),
        ];
        let feeds: Vec<Vec<Fissure>> = vec![
            // Rotation 1: everything slow.
            vec![
                syn_fissure("N1", "Mobile Defense", "Lith", "2026-10-15T22:00:00Z"),
                syn_fissure("N2", "Mobile Defense", "Meso", "2026-10-15T22:30:00Z"),
                syn_fissure("N3", "Mobile Defense", "Neo", "2026-10-15T23:00:00Z"),
            ],
            // Rotation 2: everything fast, expiries shuffled — the
            // old sort key (tier, then expiry) would reorder this.
            vec![
                syn_fissure("N4", "Capture", "Neo", "2026-10-15T21:20:00Z"),
                syn_fissure("N5", "Capture", "Lith", "2026-10-15T21:40:00Z"),
                syn_fissure("N6", "Capture", "Meso", "2026-10-15T21:50:00Z"),
            ],
            // Rotation 3: only the Omnia conjunction entry remains.
            vec![syn_fissure(
                "Yuvarium (Lua)",
                "Survival",
                "Omnia",
                "2026-10-15T23:00:00Z",
            )],
        ];
        for (i, feed) in feeds.into_iter().enumerate() {
            let p = stability_plan(&roster, &tables, &wants, feed);
            assert_eq!(
                crack_relics(&p),
                want_order,
                "rotation {} must not reorder a separated slate",
                i + 1
            );
        }
    }

    #[test]
    fn stability_near_tie_pair_flips_with_mission_quality_only() {
        // Gate (b): 20 vs 19 at Radiant — 5% apart, inside the 10%
        // chance band, same tier / coverage / stock factor, so the
        // pair is tied and mission quality alone decides. Feed 1
        // gives Beta the fast mission; feed 2 reverses it.
        let (roster, tables, wants) = stability_fixture(&[
            ("Alpha Prime", "Lith A1", 20.0),
            ("Beta Prime", "Meso B1", 19.0),
        ]);
        let feed1 = vec![
            syn_fissure("Slow", "Mobile Defense", "Lith", "2026-10-15T23:00:00Z"),
            syn_fissure("Fast", "Capture", "Meso", "2026-10-15T23:00:00Z"),
        ];
        let p1 = stability_plan(&roster, &tables, &wants, feed1);
        assert_eq!(
            crack_relics(&p1),
            vec!["Meso B1".to_owned(), "Lith A1".to_owned()],
            "tied pair: the better mission wins"
        );
        let feed2 = vec![
            syn_fissure("Fast", "Capture", "Lith", "2026-10-15T23:00:00Z"),
            syn_fissure("Slow", "Mobile Defense", "Meso", "2026-10-15T23:00:00Z"),
        ];
        let p2 = stability_plan(&roster, &tables, &wants, feed2);
        assert_eq!(
            crack_relics(&p2),
            vec!["Lith A1".to_owned(), "Meso B1".to_owned()],
            "and flips back when the feed reverses"
        );
    }

    #[test]
    fn stability_separated_pair_never_flips() {
        // Gate (c): 20 vs 10 is 50% apart — outside the band under
        // every feed, including the adversarial one (best chunk on
        // the slowest mission, worst chunk on the fastest, worst
        // chunk's fissure about to expire).
        let (roster, tables, wants) = stability_fixture(&[
            ("Alpha Prime", "Lith A1", 20.0),
            ("Beta Prime", "Meso B1", 10.0),
        ]);
        let want = vec!["Lith A1".to_owned(), "Meso B1".to_owned()];
        let feeds: Vec<Vec<Fissure>> = vec![
            vec![
                syn_fissure("Slow", "Mobile Defense", "Lith", "2026-10-15T23:00:00Z"),
                syn_fissure("Fast", "Capture", "Meso", "2026-10-15T23:00:00Z"),
            ],
            vec![
                syn_fissure("Fast", "Capture", "Lith", "2026-10-15T21:10:00Z"),
                syn_fissure("Slow", "Mobile Defense", "Meso", "2026-10-16T02:00:00Z"),
            ],
            vec![
                syn_fissure("Slow", "Mobile Defense", "Lith", "2026-10-16T02:00:00Z"),
                syn_fissure("Fast", "Capture", "Meso", "2026-10-15T21:10:00Z"),
            ],
        ];
        for (i, feed) in feeds.into_iter().enumerate() {
            let p = stability_plan(&roster, &tables, &wants, feed);
            assert_eq!(crack_relics(&p), want, "feed {} must not flip a separated pair", i + 1);
        }
    }

    #[test]
    fn stability_expiring_fissure_annotates_but_never_promotes() {
        // Gate (d): the worse chunk's fissure expires inside the
        // old tier-1 window. It keeps the "expiring crack" label
        // but stays tier 2, below the better chunk — and below the
        // tier-1 event alert, which still leads the slate.
        let (roster, tables, wants) = stability_fixture(&[
            ("Alpha Prime", "Lith A1", 20.0),
            ("Beta Prime", "Meso B1", 10.0),
        ]);
        let mut world = WorldState {
            fissures: vec![
                syn_fissure("Far", "Capture", "Lith", "2026-10-16T02:00:00Z"),
                syn_fissure("Near", "Capture", "Meso", "2026-10-15T21:30:00Z"),
            ],
            ..Default::default()
        };
        world
            .alerts
            .push(syn_alert("a1", "event_alert", "Event", "2026-10-15T21:20:00Z", true));
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let recent: HashSet<String> = HashSet::new();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let p = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &tuning(),
        });
        assert_eq!(p.chunks[0].tier, 1, "the event alert still leads");
        assert!(p.chunks[0].crack.is_none());
        assert_eq!(
            crack_relics(&p),
            vec!["Lith A1".to_owned(), "Meso B1".to_owned()],
            "expiry does not lift the worse chunk"
        );
        let beta = p
            .chunks
            .iter()
            .find(|c| c.crack.as_ref().map(|ci| ci.relic.as_str()) == Some("Meso B1"))
            .unwrap();
        assert_eq!(beta.tier, 2, "no tier jump for an expiring crack");
        assert_eq!(beta.kind, "expiring crack", "the annotation stays");
        let alpha = p
            .chunks
            .iter()
            .find(|c| c.crack.as_ref().map(|ci| ci.relic.as_str()) == Some("Lith A1"))
            .unwrap();
        assert_eq!(alpha.kind, "target crack");
    }

    #[test]
    fn stability_identical_inputs_identical_slate() {
        // Gate (e): bit-stability — two regens over identical
        // inputs produce identical chunk slates.
        let (roster, tables, wants) = stability_fixture(&[
            ("Alpha Prime", "Lith A1", 20.0),
            ("Beta Prime", "Meso B1", 19.0),
            ("Gamma Prime", "Neo G1", 5.0),
        ]);
        let feed = vec![
            syn_fissure("N1", "Mobile Defense", "Lith", "2026-10-15T22:00:00Z"),
            syn_fissure("N2", "Capture", "Meso", "2026-10-15T22:30:00Z"),
            syn_fissure("N3", "Capture", "Neo", "2026-10-15T23:00:00Z"),
        ];
        let sig = |p: &Plan| -> Vec<(String, u8, String, String)> {
            p.chunks
                .iter()
                .map(|c| (c.id.clone(), c.tier, c.title.clone(), c.kind.clone()))
                .collect()
        };
        let a = stability_plan(&roster, &tables, &wants, feed.clone());
        let b = stability_plan(&roster, &tables, &wants, feed);
        assert_eq!(sig(&a), sig(&b), "identical inputs, identical slate");
    }

    #[test]
    fn cross_tier_selection_keeps_the_lowest_tier() {
        // Regression (pass 40, addendum #105): the candidate
        // comparison's tier leg was inverted, so with an Omnia
        // fissure live (nothing filtered) a vaulted, unobtainable
        // relic replaced the unvaulted one as a part's best crack.
        // Probe-verified against the v0.5.33 tree, which chose the
        // Axi relic here.
        let text = r#"{
            "relics": {
                "Lith A1": {"name": "Lith A1", "vaulted": false, "sources": {},
                    "rewards": [{"name": "Probe Prime Blueprint", "chance": {"Intact": 11, "Radiant": 20}}]},
                "Axi B1": {"name": "Axi B1", "vaulted": true, "sources": {},
                    "rewards": [{"name": "Probe Prime Blueprint", "chance": {"Intact": 25.33, "Radiant": 25.33}}]}
            },
            "sources": {}
        }"#;
        let tables = RelicTables::from_json(text).unwrap();
        let stock = RelicStock::Unknown;
        let varzia = VarziaAvailability::default();
        let others: HashSet<String> = HashSet::new();
        let t = tuning();
        let cand = best_crack_candidate(
            "Probe Prime Blueprint",
            &tables,
            &stock,
            &varzia,
            &others,
            &t,
        )
        .unwrap();
        assert_eq!(cand.relic, "Lith A1", "lowest source tier wins");
        assert_eq!(cand.source_tier, SourceTier::Unvaulted);
        let fiss = vec![syn_fissure(
            "Yuvarium (Lua)",
            "Survival",
            "Omnia",
            "2026-10-15T23:00:00Z",
        )];
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let (cand2, _) = best_crack_for_part(
            "Probe Prime Blueprint",
            &tables,
            &stock,
            &varzia,
            &others,
            &fiss,
            now,
            true,
            &t,
        )
        .unwrap();
        assert_eq!(cand2.relic, "Lith A1");
    }

    #[test]
    fn card_action_picks_the_better_tier_across_parts() {
        // The same inverted comparison lived in derive_action's
        // cross-part choice: the rarest needed part is evaluated
        // first, and a later part's better-tier crack must replace
        // its worse-tier one. Systems (rare, vaulted-only relic)
        // evaluates first; Chassis (uncommon, unvaulted relic)
        // must win the action.
        let roster = Roster::from_json(
            r#"[{"name": "Probe Prime", "vaulted": false, "type": "warframe",
                  "parts": [{"name": "Probe Prime Systems Blueprint"},
                            {"name": "Probe Prime Chassis Blueprint"}]}]"#,
        )
        .unwrap();
        let text = r#"{
            "relics": {
                "Axi B1": {"name": "Axi B1", "vaulted": true, "sources": {},
                    "rewards": [{"name": "Probe Prime Systems Blueprint", "chance": {"Intact": 2, "Radiant": 10}}]},
                "Lith A1": {"name": "Lith A1", "vaulted": false, "sources": {},
                    "rewards": [{"name": "Probe Prime Chassis Blueprint", "chance": {"Intact": 11, "Radiant": 20}}]}
            },
            "sources": {}
        }"#;
        let tables = RelicTables::from_json(text).unwrap();
        let mut world = WorldState::default();
        world.fissures = vec![syn_fissure(
            "Yuvarium (Lua)",
            "Survival",
            "Omnia",
            "2026-10-15T23:00:00Z",
        )];
        let owned = OwnedEquipment::empty();
        let stock = RelicStock::Unknown;
        let wants = vec![
            "Probe Prime Systems Blueprint".to_owned(),
            "Probe Prime Chassis Blueprint".to_owned(),
        ];
        let recent: HashSet<String> = HashSet::new();
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let p = plan(&PlanInput {
            now,
            world: &world,
            owned: &owned,
            wants: &wants,
            roster: &roster,
            relics: &tables,
            stock: &stock,
            part_stock: no_part_stock(),
            unverified_parts: no_unverified(),
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            item_names: test_item_names(),
            tuning: &tuning(),
        });
        assert_eq!(p.goals.len(), 1);
        assert_eq!(p.goals[0].action.kind, ActionKind::Crack);
        assert_eq!(
            p.goals[0].action.relic.as_deref(),
            Some("Lith A1"),
            "the action names the better-tier crack: {}",
            p.goals[0].action.text
        );
    }

    #[test]
    fn tie_band_is_component_wise() {
        // The band's three components, exercised directly: chance
        // inside the relative epsilon ties; a coverage step never
        // does (the brief's example — high-chance/low-coverage vs
        // low-chance/high-coverage with near-equal composites); a
        // stock-knowledge step never does.
        let t = tuning();
        let varzia = VarziaAvailability::default();
        let others: HashSet<String> = ["B Prime Blueprint".to_owned()].into_iter().collect();
        let tables = RelicTables::from_json(
            r#"{"relics": {
                "Lith A1": {"name": "Lith A1", "vaulted": false, "sources": {},
                    "rewards": [{"name": "A Prime Blueprint",
                        "chance": {"Intact": 20, "Exceptional": 20, "Flawless": 20, "Radiant": 20}}]},
                "Meso B1": {"name": "Meso B1", "vaulted": false, "sources": {},
                    "rewards": [{"name": "B Prime Blueprint",
                        "chance": {"Intact": 20, "Exceptional": 20, "Flawless": 20, "Radiant": 20}},
                                {"name": "A Prime Blueprint",
                        "chance": {"Intact": 20, "Exceptional": 20, "Flawless": 20, "Radiant": 20}}]}
            }, "sources": {}}"#,
        )
        .unwrap();
        let unknown = RelicStock::Unknown;
        // Empty other-wants: Lith A1 is A's best candidate (with B
        // in the set, Meso B1's coverage would make IT A's best —
        // not the solo read this test wants).
        let none: HashSet<String> = HashSet::new();
        let a_solo = best_crack_candidate(
            "A Prime Blueprint",
            &tables,
            &unknown,
            &varzia,
            &none,
            &t,
        )
        .unwrap();
        let b_cover = best_crack_candidate(
            "B Prime Blueprint",
            &tables,
            &unknown,
            &varzia,
            &["A Prime Blueprint".to_owned()].into_iter().collect(),
            &t,
        )
        .unwrap();
        assert_eq!(a_solo.coverage, 0);
        assert_eq!(b_cover.coverage, 1, "Meso B1 also drops the other want");
        assert!(
            !in_tie_band(&a_solo, &b_cover, &t),
            "equal chance, different coverage: not a tie"
        );
        assert!(in_tie_band(&a_solo, &a_solo, &t), "a candidate ties itself");

        // Stock knowledge: same relic, same numbers, but one side
        // known-held (factor 1.5) and the other unknown (1.0).
        let known = RelicStock::Known(crate::model::KnownStock::from_totals(
            [("Lith A1".to_owned(), 3)].into_iter().collect(),
        ));
        let a_known = best_crack_candidate(
            "A Prime Blueprint",
            &tables,
            &known,
            &varzia,
            &none,
            &t,
        )
        .unwrap();
        assert_eq!(a_known.chance, a_solo.chance, "flat table: same chance");
        assert!(
            !in_tie_band(&a_solo, &a_known, &t),
            "known vs unknown stock: not a tie"
        );

        // Chance boundary: 20 vs 18 is exactly the 10% relative
        // band edge (tied); 20 vs 17 is outside it.
        let tables2 = RelicTables::from_json(
            r#"{"relics": {
                "Lith A1": {"name": "Lith A1", "vaulted": false, "sources": {},
                    "rewards": [{"name": "A Prime Blueprint", "chance": {"Intact": 11, "Radiant": 20}}]},
                "Lith C1": {"name": "Lith C1", "vaulted": false, "sources": {},
                    "rewards": [{"name": "C Prime Blueprint", "chance": {"Intact": 11, "Radiant": 18}}]},
                "Lith D1": {"name": "Lith D1", "vaulted": false, "sources": {},
                    "rewards": [{"name": "D Prime Blueprint", "chance": {"Intact": 11, "Radiant": 17}}]}
            }, "sources": {}}"#,
        )
        .unwrap();
        let none: HashSet<String> = HashSet::new();
        let c20 = best_crack_candidate("A Prime Blueprint", &tables2, &unknown, &varzia, &none, &t)
            .unwrap();
        let c18 = best_crack_candidate("C Prime Blueprint", &tables2, &unknown, &varzia, &none, &t)
            .unwrap();
        let c17 = best_crack_candidate("D Prime Blueprint", &tables2, &unknown, &varzia, &none, &t)
            .unwrap();
        assert!(in_tie_band(&c20, &c18, &t), "20 vs 18: at the band edge, tied");
        assert!(!in_tie_band(&c20, &c17, &t), "20 vs 17: outside the band");
    }
}
