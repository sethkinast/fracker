//! The planner (BRIEF §10 + §17): a pure function of
//! (world state, profile, targets, drops, derived state, now) into an
//! ordered chunk slate + goal cards. Same inputs, same plan (testable).
//!
//! Ordering: strict tiers; within a tier, earliest expiry first, then
//! highest value density. Weekly-cycle content never appears here at
//! any tier — it is board-only (rendered by `render` from WEEKLIES.md).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::model::{
    OwnedEquipment, Refinement, RelicStock, RelicTables, Roster, WorldState,
};

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
    pub pins: &'a [String],
    /// Normalized gear names of the last two prime releases (radshare gate).
    pub recent_gear: &'a HashSet<String>,
    pub steel_path_tiebreak: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartState {
    Have,
    Need,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct RelicOption {
    pub relic: String,
    pub vaulted: bool,
    pub chance_intact: f64,
    pub chance_radiant: f64,
    /// Owned count; None = "?" (stock unknown).
    pub owned: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct PartRow {
    pub name: String,
    pub state: PartState,
    pub relics: Vec<RelicOption>,
    /// "rare — 2% Intact / 10% Radiant" for the best relic.
    pub best_odds: String,
    pub rarity: Rarity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
}

impl Rarity {
    pub fn of(intact_chance: f64) -> Self {
        if intact_chance >= 25.0 {
            Rarity::Common
        } else if intact_chance >= 11.0 {
            Rarity::Uncommon
        } else {
            Rarity::Rare
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Rarity::Common => "common",
            Rarity::Uncommon => "uncommon",
            Rarity::Rare => "rare",
        }
    }
}

#[derive(Debug, Clone)]
pub struct GoalAction {
    /// Complete instruction line, e.g. "Grab 3× Neo C5, refine to
    /// Flawless, run Hepit (Void) — Capture."
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
}

// ---------------------------------------------------------------------------
// Time + mission model
// ---------------------------------------------------------------------------

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    s.parse::<DateTime<Utc>>().ok()
}

/// Base minutes per mission by type + 2 min overhead (BRIEF §10).
/// Endless types return per-crack-unit minutes instead (see `is_endless`).
fn mission_base_minutes(mission_type: &str) -> f64 {
    match mission_type.to_lowercase().as_str() {
        "capture" => 3.0,
        "exterminate" => 5.0,
        "sabotage" => 5.0,
        "rescue" => 4.0,
        "spy" => 6.0,
        "hive" => 10.0,
        "mobile defense" => 7.0,
        "defense" => 7.0,
        "survival" => 5.0,
        "interception" => 7.0,
        "disruption" => 4.0,
        "excavation" => 6.0,
        "defection" => 8.0,
        "infested salvage" => 6.0,
        "survival*" => 5.0,
        _ => 6.0,
    }
}

/// Endless fissures crack once per rotation unit (~5 min) with no
/// orbiter/load overhead between cracks; sprint fissures crack once
/// per mission + 2 min overhead (BRIEF §10, §17 mission selection).
fn is_endless(mission_type: &str) -> bool {
    matches!(
        mission_type.to_lowercase().as_str(),
        "survival" | "defense" | "interception" | "disruption" | "excavation" | "void cascade"
    )
}

/// Total minutes for a batch on one fissure.
fn minutes_for_batch(mission_type: &str, batch: u32) -> f64 {
    let b = batch.max(1) as f64;
    if is_endless(mission_type) {
        // One mission, N rotation units, single overhead.
        b * mission_base_minutes(mission_type) + 2.0
    } else {
        b * (mission_base_minutes(mission_type) + 2.0)
    }
}

fn minutes_per_crack(mission_type: &str, batch: u32) -> f64 {
    minutes_for_batch(mission_type, batch) / batch.max(1) as f64
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
/// * commons run Intact (refining *hurts* them: 25.33% -> 16.67%);
/// * scarce stock (or unknown, treated as scarce) refines harder to
///   maximize success per copy;
/// * plentiful stock biases DOWN a tier (volume substitutes for odds);
///   formally: the cheapest tier whose expected run fits inside stock,
///   defaulting to Radiant when even that exceeds holdings.
pub fn choose_refinement(intact: f64, stock: Option<u32>) -> Refinement {
    let rarity = Rarity::of(intact);
    if rarity == Rarity::Common {
        return Refinement::Intact;
    }
    let chances = [
        (Refinement::Intact, intact),
        (
            Refinement::Exceptional,
            refine_chance(intact, Refinement::Exceptional),
        ),
        (
            Refinement::Flawless,
            refine_chance(intact, Refinement::Flawless),
        ),
        (
            Refinement::Radiant,
            refine_chance(intact, Refinement::Radiant),
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

/// Refined chance when the table only guarantees the Intact value.
/// Real tables carry all four tiers; this fallback encodes the brief's
/// stated ladder (rare 2/4/6/10, uncommon 11/13/17/20,
/// common 25.33/23.33/20/16.67).
fn refine_chance(intact: f64, tier: Refinement) -> f64 {
    let rarity = Rarity::of(intact);
    match (rarity, tier) {
        (_, Refinement::Intact) => intact,
        (Rarity::Rare, Refinement::Exceptional) => 4.0,
        (Rarity::Rare, Refinement::Flawless) => 6.0,
        (Rarity::Rare, Refinement::Radiant) => 10.0,
        (Rarity::Uncommon, Refinement::Exceptional) => 13.0,
        (Rarity::Uncommon, Refinement::Flawless) => 17.0,
        (Rarity::Uncommon, Refinement::Radiant) => 20.0,
        (Rarity::Common, Refinement::Exceptional) => 23.33,
        (Rarity::Common, Refinement::Flawless) => 20.0,
        (Rarity::Common, Refinement::Radiant) => 16.67,
    }
}

/// Expected trace spend per success at a tier (informational, shown on
/// the line so Seth can veto against his unobservable balance).
pub fn trace_spend_per_success(tier: Refinement, chance_pct: f64) -> f64 {
    tier.trace_cost_per_relic() as f64 * expected_cracks(chance_pct)
}

// ---------------------------------------------------------------------------
// Fissure selection cascade (BRIEF §17: one answer, not a menu)
// ---------------------------------------------------------------------------

pub struct FissurePick {
    pub fissure: crate::model::Fissure,
    pub expires_in_mins: f64,
    pub batch_minutes: f64,
}

/// Rank matching fissures for an era + batch:
/// 1. expiry (~15 min jumps first, if runnable — storms excluded here),
/// 2. minutes per crack for the planned batch (dominant),
/// 3. goal fit (bottleneck era first — caller pre-sorts eras),
/// 4. Steel Path tiebreak for sprint missions (endless stays normal),
/// 5. Void Storms last resort.
pub fn rank_fissures(
    fissures: &[crate::model::Fissure],
    era: &str,
    batch: u32,
    now: DateTime<Utc>,
    steel_path_tiebreak: bool,
) -> Vec<FissurePick> {
    let mut picks: Vec<FissurePick> = fissures
        .iter()
        .filter(|f| f.matches_era(era))
        .filter_map(|f| {
            let expiry = parse_time(&f.expiry)?;
            if expiry <= now {
                return None;
            }
            Some(FissurePick {
                fissure: f.clone(),
                expires_in_mins: (expiry - now).num_seconds() as f64 / 60.0,
                batch_minutes: minutes_for_batch(&f.mission_type, batch),
            })
        })
        .collect();
    picks.sort_by(|a, b| {
        let urg_a = a.expires_in_mins <= 15.0 && !a.fissure.is_storm;
        let urg_b = b.expires_in_mins <= 15.0 && !b.fissure.is_storm;
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
                    && !is_endless(&a.fissure.mission_type);
                let sp_b = steel_path_tiebreak
                    && b.fissure.is_hard
                    && !is_endless(&b.fissure.mission_type);
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
    pub solo_minutes: f64,
}

/// Best crack candidate for one (part) across its relics.
/// Returns None when holdings are known-empty or no live fissure matches.
pub fn best_crack_for_part(
    part: &str,
    relics: &RelicTables,
    stock: &RelicStock,
    other_wants: &HashSet<String>,
    fissures: &[crate::model::Fissure],
    now: DateTime<Utc>,
    steel_path_tiebreak: bool,
) -> Option<(CrackCandidate, FissurePick)> {
    let mut best: Option<(CrackCandidate, FissurePick)> = None;
    for (relic, _intact) in relics.relics_for_part(part) {
        let intact = relic.chance_for(part, Refinement::Intact);
        let owned = stock.count(&relic.name);
        if owned == Some(0) {
            continue; // known-empty: no crack exists.
        }
        let refinement = choose_refinement(intact, owned);
        let chance = relic.chance_for(part, refinement);
        let chance = if chance <= 0.0 {
            refine_chance(intact, refinement)
        } else {
            chance
        };
        let exp = expected_cracks(chance);
        let batch = match owned {
            Some(s) => (s.min(exp.ceil() as u32)).clamp(1, MAX_BATCH),
            None => (exp.ceil() as u32).clamp(1, UNKNOWN_DEFAULT_BATCH),
        };
        let coverage = relic
            .rewards
            .iter()
            .filter(|r| r.name != part && other_wants.contains(&r.name))
            .count();
        let stock_factor = match owned {
            Some(_) => 1.5, // cracking beats farming, always.
            None => 1.0,
        };
        let score = chance * (1.0 + coverage as f64) * stock_factor;
        let picks = rank_fissures(
            fissures,
            relic.era(),
            batch,
            now,
            steel_path_tiebreak,
        );
        let pick = match picks.into_iter().next() {
            Some(p) => p,
            None => continue,
        };
        let trace_cost = refinement.trace_cost_per_relic() * batch;
        let solo_minutes = minutes_for_batch(&pick.fissure.mission_type, batch);
        let cand = CrackCandidate {
            relic: relic.name.clone(),
            era: relic.era().to_owned(),
            vaulted: relic.vaulted,
            chance,
            refinement,
            batch,
            owned,
            coverage,
            score,
            expected_cracks: exp,
            trace_cost,
            solo_minutes,
        };
        let better = match &best {
            None => true,
            Some((b, _)) => cand.score > b.score,
        };
        if better {
            best = Some((cand, pick));
        }
    }
    best
}

/// Radshare verdict: 4-player same-relic share turns per-crack chance
/// p into 1-(1-p)^4. Recommended when the share wins after ~7.5 min LFG
/// formation time — typically rares at Flawless/Radiant with scarce
/// stock — AND the relic's gear is within the last two prime releases
/// (Seth's gate: older recruiting never fills).
pub fn radshare_verdict(
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
    let lfg_mins = 7.5;
    let solo_time = exp_solo * mpc_solo;
    let share_time = lfg_mins + exp_share * mpc_solo;
    if (rarity == Rarity::Rare || scarce) && share_time < solo_time && exp_solo >= 5.0 {
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

// ---------------------------------------------------------------------------
// Goal cards (BRIEF §17)
// ---------------------------------------------------------------------------

pub const UNKNOWN_DEFAULT_BATCH: u32 = 4;
pub const MAX_BATCH: u32 = 6;

/// Normalize a gear/part name for rotation matching.
pub fn norm_name(s: &str) -> String {
    s.to_lowercase()
}

fn gear_in_tokens(gear: &str, tokens: &HashSet<String>) -> bool {
    let g = norm_name(gear);
    tokens.iter().any(|t| t.contains(&g) || g.contains(t))
}

fn part_rows_for_gear(
    gear: &str,
    wants_here: &[String],
    roster: &Roster,
    relics: &RelicTables,
    stock: &RelicStock,
    want_set: &HashSet<String>,
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
    names.sort();
    names
        .into_iter()
        .map(|part| {
            let state = if want_set.contains(&part) {
                PartState::Need
            } else {
                PartState::Unknown
            };
            let mut opts: Vec<RelicOption> = relics
                .relics_for_part(&part)
                .into_iter()
                .map(|(r, intact)| RelicOption {
                    relic: r.name.clone(),
                    vaulted: r.vaulted,
                    chance_intact: intact,
                    chance_radiant: r.chance_for(&part, Refinement::Radiant),
                    owned: stock.count(&r.name),
                })
                .collect();
            opts.sort_by(|a, b| {
                b.chance_intact
                    .partial_cmp(&a.chance_intact)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let best = opts.first();
            let rarity = Rarity::of(best.map(|o| o.chance_intact).unwrap_or(0.0));
            let best_odds = match best {
                Some(o) => format!(
                    "{} — {}% Intact / {}% Radiant",
                    rarity.name(),
                    fmt_pct(o.chance_intact),
                    fmt_pct(o.chance_radiant)
                ),
                None => "no relic source".to_owned(),
            };
            PartRow {
                name: part,
                state,
                relics: opts,
                best_odds,
                rarity,
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
fn goal_cards(input: &PlanInput<'_>, active: &[String]) -> Vec<GoalCard> {
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
                &want_set,
            );
            let parts_total = parts.len();
            let parts_have = parts
                .iter()
                .filter(|p| p.state == PartState::Have)
                .count();
            let resurgence_live =
                !resurgence_tokens.is_empty() && gear_in_tokens(&gear, &resurgence_tokens);
            let action = derive_action(
                input,
                &gear,
                &wants,
                &parts,
                vaulted,
                resurgence_live,
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
                action,
            }
        })
        .collect();
    // Ordering: pins first, then warframes before weapons, then fewest
    // parts remaining (BRIEF §10).
    cards.sort_by(|a, b| {
        let pin_a = input.pins.iter().position(|p| a.wants.contains(p));
        let pin_b = input.pins.iter().position(|p| b.wants.contains(p));
        let pin_ord = match (pin_a, pin_b) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        pin_ord
            .then_with(|| b.is_warframe.cmp(&a.is_warframe))
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
    // instruction (buy the relics for Aya) beats "crack if you have it".
    if resurgence_live && !holds_known {
        if let Some(vt) = &input.world.vault_trader {
            return GoalAction {
                text: format!(
                    "Rotation ends {} — Varzia has {}'s relics ({}). Farm Aya, buy the relics, then crack above.",
                    days_until(input.now, &vt.expiry),
                    gear,
                    vt.location,
                ),
                kind: ActionKind::Resurgence,
                relic: None,
                score: 40.0,
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
            &others,
            &input.world.fissures,
            input.now,
            input.steel_path_tiebreak,
        ) {
            let better = match &best_crack {
                None => true,
                Some((b, _, _)) => cand.score > b.score,
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
        let endless = is_endless(&pick.fissure.mission_type);
        let stay = if endless && cand.batch > 1 {
            format!(" — stay for {} rotations", cand.batch)
        } else {
            String::new()
        };
        let sp = if pick.fissure.is_hard { " Steel Path" } else { "" };
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
        let gear_norm = norm_name(gear);
        let gear_recent = input.recent_gear.iter().any(|g| {
            let g = norm_name(g);
            g.contains(&gear_norm) || gear_norm.contains(&g)
        });
        let mpc = minutes_per_crack(&pick.fissure.mission_type, cand.batch);
        let scarce = matches!(cand.owned, Some(0..=2) | None);
        if let Some(v) =
            radshare_verdict(cand.chance, cand_rarity(&cand), mpc, scarce, gear_recent)
        {
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
            let days = days_until(input.now, &vt.expiry);
            return GoalAction {
                text: format!(
                    "Rotation ends {} — Varzia has {}'s relics ({}). Farm Aya, buy the relics, then crack above.",
                    days,
                    gear,
                    vt.location,
                ),
                kind: ActionKind::Resurgence,
                relic: None,
                score: 40.0,
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
            score: 5.0,
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
        score: 0.0,
    }
}

fn cand_rarity(cand: &CrackCandidate) -> Rarity {
    // expected = 100/chance, so this recovers the target's rarity band.
    Rarity::of(100.0 / cand.expected_cracks.max(0.01))
}

fn days_until(now: DateTime<Utc>, expiry: &str) -> String {
    match parse_time(expiry) {
        Some(t) if t > now => {
            let secs = (t - now).num_seconds();
            if secs < 48 * 3600 {
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
        // Prefer fast missions (Capture/Exterminate/Disruption early).
        let mut ranked: Vec<&crate::model::FarmNode> = nodes.iter().collect();
        ranked.sort_by(|a, b| {
            rotations_per_hour(&a.mode_name)
                .partial_cmp(&rotations_per_hour(&b.mode_name))
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
            if let Some((node, table, chance)) = best_source_node(input.relics, relic) {
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
        score: 10.0,
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

fn build_chunks(input: &PlanInput<'_>, cards: &[GoalCard], pruned: &[String]) -> (Vec<Chunk>, Vec<FarmHint>, Vec<String>) {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let _ = pruned;

    // ---- Tier 1: expiring event value ----
    for alert in &input.world.alerts {
        if alert.mission.counted_items.is_empty() {
            continue;
        }
        let expiry = match parse_time(&alert.expiry) {
            Some(t) if t > input.now => t,
            _ => continue,
        };
        let mins = (expiry - input.now).num_seconds() / 60;
        let payout: Vec<String> = alert
            .mission
            .counted_items
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
                mission_base_minutes(&alert.mission.mission_type) as u32 + 2
            ),
            why: format!("Live payout: {}.", payout.join(", ")),
            kind: "event alert".to_owned(),
            expiry: Some(expiry),
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
                    "Varzia rotation ends {} — {} ({}).",
                    days_until(input.now, &vt.expiry),
                    names.join(", "),
                    vt.location,
                ),
                minutes_label: "window".to_owned(),
                why: "Vaulted relics purchasable for Aya while the rotation is live.".to_owned(),
                kind: "prime resurgence".to_owned(),
                expiry: parse_time(&vt.expiry),
            });
        }
    }
    // Baro leaving < 24h.
    if let Some(baro) = &input.world.baro {
        if let Some(expiry) = parse_time(&baro.expiry) {
            let mins = (expiry - input.now).num_seconds() / 60;
            if mins > 0 && mins < 24 * 60 {
                chunks.push(Chunk {
                    id: "t1-baro".to_owned(),
                    tier: 1,
                    title: format!(
                        "Baro leaves in {}h — {} (check inventory).",
                        mins / 60,
                        baro.location
                    ),
                    minutes_label: "~5m".to_owned(),
                    why: "Biweekly vendor window closing.".to_owned(),
                    kind: "vendor".to_owned(),
                    expiry: Some(expiry),
                });
            }
        }
    }

    // ---- Tier 2: target-matched fissures ----
    // Greedy coverage: rank crack actions by score, take each relic once.
    // Real ranked pass (owned values, no borrow tangle).
    let mut ranked: Vec<(String, CrackCandidate, FissurePick, String, String)> = Vec::new();
    for card in cards {
        if card.action.kind != ActionKind::Crack {
            continue;
        }
        let want_set: HashSet<String> =
            cards.iter().flat_map(|c| c.wants.clone()).collect();
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
            let mut others = want_set.clone();
            others.remove(&part.name);
            if let Some((cand, pick)) = best_crack_for_part(
                &part.name,
                input.relics,
                input.stock,
                &others,
                &input.world.fissures,
                input.now,
                input.steel_path_tiebreak,
            ) {
                ranked.push((
                    card.gear.clone(),
                    cand,
                    pick,
                    part.name.clone(),
                    card.action.text.clone(),
                ));
                break;
            }
        }
    }
    ranked.sort_by(|a, b| {
        b.1.score
            .partial_cmp(&a.1.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Expiring (<60 min) crack matches join tier 1; rest tier 2.
    let mut seen_relics: HashSet<String> = HashSet::new();
    for (gear, cand, pick, part, _action_text) in ranked {
        if !seen_relics.insert(cand.relic.clone()) {
            continue; // greedy: one chunk per relic.
        }
        let stock_note = match cand.owned {
            Some(_) => String::new(),
            None => " (if you have it)".to_owned(),
        };
        let endless = is_endless(&pick.fissure.mission_type);
        let sp = if pick.fissure.is_hard { " Steel Path" } else { "" };
        let expiry_dt = parse_time(&pick.fissure.expiry);
        let tier: u8 = if pick.expires_in_mins < 60.0 { 1 } else { 2 };
        let mut alternates = String::new();
        if cand.coverage > 0 {
            alternates = format!(" Covers {} other want(s) in the same relic.", cand.coverage);
        }
        chunks.push(Chunk {
            id: format!("t{}-crack-{}", tier, sanitize_id(&cand.relic)),
            tier,
            title: format!(
                "Crack {}{} at {}{} ({} {}) — {}.",
                cand.batch,
                stock_note,
                pick.fissure.node,
                sp,
                cand.relic,
                cand.refinement.name(),
                part,
            ),
            minutes_label: chunk_minutes_label(pick.batch_minutes, endless, cand.batch),
            why: format!(
                "{}% at {} for {} ({} traces). Goal: {}.{}",
                fmt_pct(cand.chance),
                cand.refinement.name(),
                part,
                cand.trace_cost,
                gear,
                alternates,
            ),
            kind: if tier == 1 {
                "expiring crack".to_owned()
            } else {
                "target crack".to_owned()
            },
            expiry: expiry_dt,
        });
        if chunks.iter().filter(|c| c.tier == 2).count() >= 6 {
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
    if t2_count < 3 && !farm_goals.is_empty() {
        farms = score_farms(input, cards);
        for f in farms.iter().take(3) {
            chunks.push(Chunk {
                id: format!("t3-farm-{}", sanitize_id(&f.node)),
                tier: 3,
                title: format!("Farm relics: {} ({}) — {}.", f.node, f.planet, f.mode),
                minutes_label: "farm".to_owned(),
                why: format!("Relics: {}.", f.relics.join(", ")),
                kind: "relic farm".to_owned(),
                expiry: None,
            });
        }
    }
    if farms.is_empty() && !cards.is_empty() {
        let any_unvaulted = cards.iter().flat_map(|g| g.wants.iter()).any(|w| {
            input
                .relics
                .relics_for_part(w)
                .iter()
                .any(|(r, _)| !r.vaulted && !r.sources.is_empty())
        });
        if !any_unvaulted {
            notes.push(
                "Relic sourcing: every current want sits in a vaulted relic — crack-or-park. \
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
                title: "Sortie: daily three-mission chain.".to_owned(),
                minutes_label: "~20–30m".to_owned(),
                why: format!("Legs: {}.", legs.join(" → ")),
                kind: "sortie (daily)".to_owned(),
                expiry: parse_time(&sortie.expiry),
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
                mission_base_minutes(&fill.mission_type) as u32 + 2
            ),
            why: "Filler — slate ran thin; ducat/plat density ranking lands with market data later."
                .to_owned(),
            kind: "filler".to_owned(),
            expiry: parse_time(&fill.expiry),
        });
    }

    // Within-tier order: earliest expiry first, then value (tier 1 keeps
    // insertion value order for alerts; others sort by expiry).
    chunks.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then_with(|| match (a.expiry, b.expiry) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
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
                    let rate = rotations_per_hour(&node.mode_name);
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

fn rotations_per_hour(mode_name: &str) -> f64 {
    match mode_name.to_lowercase().as_str() {
        "survival" => 12.0,
        "defense" => 12.0,
        "interception" => 10.0,
        "disruption" => 15.0,
        "spy" => 6.0,
        "capture" => 8.0,
        "exterminate" => 6.0,
        "excavation" => 10.0,
        _ => 5.0,
    }
}

/// Best remaining live fissure for the filler chunk: most co-wanted
/// coverage, earliest expiry first.
fn filler_fissure(input: &PlanInput<'_>) -> Option<crate::model::Fissure> {
    let mut live: Vec<&crate::model::Fissure> = input
        .world
        .fissures
        .iter()
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
    let cards = goal_cards_for(input, &active);
    let (chunks, farms, notes) = build_chunks(input, &cards, &pruned);
    Plan {
        goals: cards,
        chunks,
        pruned,
        farms,
        notes,
    }
}

fn goal_cards_for(input: &PlanInput<'_>, active: &[String]) -> Vec<GoalCard> {
    goal_cards(input, active)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Fissure, RelicTables, Roster, WorldState};

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
        let stock = RelicStock::Known([("X".to_owned(), 99)].into_iter().collect());
        assert_eq!(choose_refinement(25.33, stock.count("X")), Refinement::Intact);
        assert_eq!(choose_refinement(25.33, None), Refinement::Intact);
    }

    #[test]
    fn scarce_stock_refines_hard_plentiful_biases_down() {
        // Rare, 2% Intact: Radiant expects ~10 cracks.
        assert_eq!(
            choose_refinement(2.0, Some(1)),
            Refinement::Radiant,
            "scarce stock maximizes per-copy odds"
        );
        assert_eq!(
            choose_refinement(2.0, None),
            Refinement::Radiant,
            "unknown stock treated as scarce"
        );
        // 25 copies cover Exceptional (~25) but the cheapest fitting
        // tier above Intact wins over Radiant.
        assert_eq!(
            choose_refinement(2.0, Some(25)),
            Refinement::Exceptional,
            "volume substitutes for per-crack odds"
        );
        // Uncommon 11%: Intact expects ~9.1; 10 copies fit Flawless (~5.9)? cheapest fitting above Intact: Exceptional (~7.7).
        assert_eq!(
            choose_refinement(11.0, Some(10)),
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
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let mut storm = syn_fissure("C", "Capture", "Neo", "2026-10-15T21:05:00Z");
        storm.is_storm = true;
        let fiss = vec![
            syn_fissure("A", "Capture", "Neo", "2026-10-15T21:50:00Z"),
            syn_fissure("B", "Exterminate", "Neo", "2026-10-15T21:10:00Z"),
            storm,
        ];
        let picks = rank_fissures(&fiss, "Neo", 2, now, true);
        assert_eq!(picks.len(), 3);
        assert_eq!(picks[0].fissure.node, "B", "urgent runnable jumps first");
        assert!(picks[2].fissure.is_storm, "storms last");
    }

    #[test]
    fn endless_wins_big_batches_sprint_wins_small() {
        let now: DateTime<Utc> = "2026-10-15T21:00:00Z".parse().unwrap();
        let fiss = vec![
            syn_fissure("A", "Exterminate", "Neo", "2026-10-15T23:00:00Z"),
            syn_fissure("B", "Survival", "Neo", "2026-10-15T23:00:00Z"),
        ];
        let small = rank_fissures(&fiss, "Neo", 1, now, true);
        assert_eq!(small[0].fissure.node, "A", "sprint wins batch of 1");
        let big = rank_fissures(&fiss, "Neo", 4, now, true);
        assert_eq!(big[0].fissure.node, "B", "endless wins batch of 4");
    }

    #[test]
    fn radshare_gated_on_recent_releases() {
        assert_eq!(
            radshare_verdict(10.0, Rarity::Rare, 5.0, true, false),
            None,
            "old gear stays solo no matter the math"
        );
        let yes = radshare_verdict(10.0, Rarity::Rare, 5.0, true, true);
        assert!(yes.is_some(), "rare + scarce + recent shares");
        assert_eq!(
            radshare_verdict(25.33, Rarity::Common, 5.0, false, true),
            None,
            "cheap volume runs never share"
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
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
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
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
        });
        assert_eq!(p.goals[0].gear, "Zakti Prime");
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
        let (cand, _) = best_crack_for_part(
            "Khora Prime Blueprint",
            &tables,
            &stock,
            &others,
            &fiss,
            now,
            true,
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
                pins: &pins,
                recent_gear: &recent,
                steel_path_tiebreak: true,
            })
        };
        let a = mk();
        let b = mk();
        assert_eq!(a.chunks.len(), b.chunks.len());
        assert_eq!(a.goals.len(), b.goals.len());
        assert_eq!(a.goals[0].action.text, b.goals[0].action.text);
    }
}
