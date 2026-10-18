//! The planner (BRIEF §10 + §17): a pure function of
//! (world state, profile, targets, drops, derived state, now) into an
//! ordered chunk slate + goal cards. Same inputs, same plan (testable).
//!
//! Ordering: strict tiers; within a tier, earliest expiry first, then
//! highest value density. Weekly-cycle content never appears here at
//! any tier — it is board-only.
//!
//! All estimates, thresholds, and windows come from the runtime tuning
//! tables (`crate::tuning`); this module holds no game content.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::model::{
    OwnedEquipment, Refinement, RelicStock, RelicTables, Roster, WorldState,
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
}

/// Best-relic deal for `part`: highest-Intact relic, stock-aware
/// refinement tier, table chance at that tier (ladder fallback).
pub fn best_deal_for_part(
    part: &str,
    relics: &RelicTables,
    stock: &RelicStock,
    tuning: &Tuning,
) -> Option<PartDeal> {
    let (relic, intact) = relics.relics_for_part(part).into_iter().next()?;
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
            if let Some((relic, _)) = relics.relics_for_part(&part.name).into_iter().next() {
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
    tuning: &Tuning,
) -> Option<(CrackCandidate, FissurePick)> {
    let mut best: Option<(CrackCandidate, FissurePick)> = None;
    for (relic, _intact) in relics.relics_for_part(part) {
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
        let picks = rank_fissures(
            fissures,
            &[relic.era()],
            batch,
            now,
            steel_path_tiebreak,
            tuning,
        );
        let pick = match picks.into_iter().next() {
            Some(p) => p,
            None => continue,
        };
        let trace_cost = tuning.refinement.trace_cost(refinement) * batch;
        let solo_minutes = tuning
            .missions
            .minutes_for_batch(&pick.fissure.mission_type, batch);
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
            let deal = best_deal_for_part(&part, relics, stock, tuning);
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
                input.tuning,
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
    // Ordering: pins first, then CURRENT (unvaulted, or vaulted but
    // un-parked by the live Resurgence rotation) before vaulted
    // (addendum #10), frames before weapons within a tier (BRIEF §10),
    // then fewest parts remaining.
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
            &others,
            &input.world.fissures,
            input.now,
            input.steel_path_tiebreak,
            input.tuning,
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
        if let Some(v) = radshare_verdict(
            &input.tuning.planner,
            cand.chance,
            cand_rarity(input.tuning, &cand),
            mpc,
            scarce,
            gear_recent,
        ) {
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

fn build_chunks(input: &PlanInput<'_>, cards: &[GoalCard], pruned: &[String], focus: &[FocusPart]) -> (Vec<Chunk>, Vec<FarmHint>, Vec<String>) {
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
                });
            }
        }
    }

    // ---- Tier 2: target-matched fissures ----
    // Greedy coverage: rank crack actions by score, take each relic once.
    // Do-Now rule: crack chunks pursue ONLY the ≤3 focus parts.
    let focus_set: HashSet<String> =
        focus.iter().map(|f| f.part.clone()).collect();
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
            .filter(|p| p.state == PartState::Need && focus_set.contains(&p.name))
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
                input.tuning,
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
        let endless = input.tuning.missions.is_endless(&pick.fissure.mission_type);
        let sp = if pick.fissure.is_hard { input.tuning.planner.steel_path_label.as_str() } else { "" };
        let expiry_dt = parse_time(&pick.fissure.expiry);
        let tier: u8 = if pick.expires_in_mins < input.tuning.planner.tier1_crack_mins {
            1
        } else {
            2
        };
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
            gear: Some(gear.clone()),
        });
        if chunks
            .iter()
            .filter(|c| c.kind.contains("crack"))
            .count()
            >= input.tuning.planner.max_crack_chunks
        {
            // Useful slate, not exhaustive (BRIEF §10). Expiring
            // matches join tier 1 but share the same cap.
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
        farms = score_farms(input, cards);
        for f in farms.iter().take(input.tuning.planner.farm_chunks) {
            chunks.push(Chunk {
                id: format!("t3-farm-{}", sanitize_id(&f.node)),
                tier: 3,
                title: format!("Farm relics: {} ({}) — {}.", f.node, f.planet, f.mode),
                minutes_label: "farm".to_owned(),
                why: format!("Relics: {}.", f.relics.join(", ")),
                kind: "relic farm".to_owned(),
                expiry: None,
                gear: None,
            });
        }
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
    let focus = pick_focus(&cards, input.relics, input.tuning);
    let (chunks, farms, notes) = build_chunks(input, &cards, &pruned, &focus);
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
    Some(BaroView {
        vendor: input.tuning.planner.vendor.name.clone(),
        location: live.location.clone(),
        ends_in: days_until(input.now, &live.expiry, input.tuning),
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

fn goal_cards_for(input: &PlanInput<'_>, active: &[String]) -> Vec<GoalCard> {
    goal_cards(input, active)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elog;
    use crate::model::{Fissure, RelicTables, Roster, WorldState};

    fn tuning() -> Tuning {
        Tuning::for_tests()
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
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
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
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
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
            pins: &[],
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
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
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            tuning: &tuning(),
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
                pins: &pins,
                recent_gear: &recent,
                steel_path_tiebreak: true,
                completed_missions: &[],
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
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
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
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            tuning: &t,
        });
        assert!(q.event_alerts.is_empty() && q.lotus_alerts.is_empty());
        let b = q.baro.expect("trader live mid-window");
        assert_eq!(b.location, "Orcus Relay (Pluto)");
        assert!(!b.ends_in.is_empty());
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
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &completions,
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
            pins: &pins,
            recent_gear: &recent,
            steel_path_tiebreak: true,
            completed_missions: &[],
            tuning: &t,
        });
        assert!(q.event_alerts.iter().all(|a| !a.done));
    }
}
