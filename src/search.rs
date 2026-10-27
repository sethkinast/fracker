//! Item search (forty-seventh pass): the header search box's
//! server side.
//!
//! Seth's ask (2026-10-26): "a little search bar where I can
//! quickly search for an item to see its status." The page's box
//! (in the header fragment, every tab) sends `GET /search?q=…`;
//! this module renders the results fragment the page swaps in.
//! The server stays the only renderer (pass 41): the client
//! script ferries the query and paints returned HTML, nothing
//! plan-shaped is computed or rendered client-side.
//!
//! Status is a VIEW over the already-computed plan, never a
//! second computation of it: the report pipeline publishes each
//! generation's [`Plan`] (plus the relic tables and the captured
//! per-tier relic stock the same run consumed) into
//! [`SearchStore`] at the exact instant it publishes the page,
//! so a search answer and the page on screen are the same
//! generation's numbers by construction — the pass-44 lesson
//! (display and pricing agreeing because they are one figure)
//! applied to a new surface.
//!
//! Matching is deliberately plain: case-insensitive substring
//! over gear names, part names and relic names, ranked exact,
//! then prefix, then substring. No fuzzy rescue — a query that
//! matches nothing says so (the Meso L1/C11 garble class is a
//! capture-side concern, and search must not paper over it).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use crate::model::{RelicEntry, RelicTables};
use crate::plan::{CrackInfo, GoalCard, PartRow, PartState, Plan};
use crate::render::{card_footer_html, esc, refinement_abbrev, short_part};

/// Results are capped: past this many hits the fragment asks for
/// more of the name instead of walling the page with rows.
pub const MAX_RESULTS: usize = 8;

/// Display order of hit kinds within one rank: gear, part, relic.
const KIND_GEAR: u8 = 0;
const KIND_PART: u8 = 1;
const KIND_RELIC: u8 = 2;

/// One generation's searchable truth: the plan as rendered, the
/// relic tables it was computed against, and the captured relic
/// stock in per-tier form (the planner's `RelicStock` collapses
/// tiers to totals + the Radiant split; the relic status line
/// wants the tiers themselves, so they are distilled from the
/// same `StockState` the run read).
pub struct SearchSnapshot {
    plan: Plan,
    relics: RelicTables,
    /// Relic name -> (tier label, copies) in canonical tier
    /// order, held tiers only. Absent = none held. Meaningless
    /// when `stock_known` is false.
    relic_tiers: HashMap<String, Vec<(String, u32)>>,
    /// False when the run had no readable captured stock (the
    /// planner's `RelicStock::Unknown`): relic results then say
    /// "stock unknown" rather than inventing a zero.
    stock_known: bool,
}

impl SearchSnapshot {
    /// Build from one report run's inputs. `stock` is the parsed
    /// capture state the run consumed, or `None` when it fell
    /// back to unknown stock.
    pub fn new(plan: Plan, relics: RelicTables, stock: Option<&crate::ocr::StockState>) -> Self {
        let (relic_tiers, stock_known) = match stock {
            Some(s) => {
                let mut map: HashMap<String, Vec<(String, u32)>> = HashMap::new();
                for (name, stacks) in &s.relics {
                    let tiers: Vec<(String, u32)> = stacks
                        .tiers
                        .iter()
                        .filter(|(_, n)| **n > 0)
                        .map(|(t, n)| (tier_label(*t), *n))
                        .collect();
                    if !tiers.is_empty() {
                        map.insert(name.clone(), tiers);
                    }
                }
                (map, true)
            }
            None => (HashMap::new(), false),
        };
        Self {
            plan,
            relics,
            relic_tiers,
            stock_known,
        }
    }
}

/// "intact" -> "Intact" (the tier labels the page uses).
fn tier_label(t: crate::ocr::RefinementTier) -> String {
    let s = t.as_str();
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The published-snapshot holder the resident shares between the
/// report pipeline (publisher) and the loopback server (reader).
/// Swapped wholesale per generation, so a lookup never observes
/// a half-updated plan.
pub struct SearchStore {
    current: Mutex<Option<SearchSnapshot>>,
}

impl SearchStore {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }

    /// Publish one generation's snapshot (called at the page's
    /// own publication point in `report::generate`).
    pub fn publish(&self, snapshot: SearchSnapshot) {
        if let Ok(mut cur) = self.current.lock() {
            *cur = Some(snapshot);
        }
    }

    /// The rendered results fragment for `query`; `None` until
    /// the first generation publishes.
    pub fn lookup(&self, query: &str) -> Option<String> {
        let guard = self.current.lock().ok()?;
        let snap = guard.as_ref()?;
        Some(render_results(snap, query))
    }
}

impl Default for SearchStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SearchStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ready = self
            .current
            .lock()
            .map(|c| c.is_some())
            .unwrap_or(false);
        f.debug_struct("SearchStore")
            .field("ready", &ready)
            .finish_non_exhaustive()
    }
}

/// One matched entity: match rank (0 exact, 1 prefix, 2 substring),
/// kind order within the rank, and the name key the deterministic
/// tiebreak sorts on. The HTML is rendered eagerly — a plan is
/// small (dozens of cards, ~800 relics) and a lookup is a linear
/// scan, so there is no index to keep in step with the snapshot.
struct Hit {
    rank: u8,
    kind: u8,
    key: String,
    html: String,
}

/// Case-insensitive match rank of `name` against the (already
/// lowercased, non-empty) query.
fn match_rank(name: &str, q: &str) -> Option<u8> {
    let n = name.to_lowercase();
    if n == q {
        Some(0)
    } else if n.starts_with(q) {
        Some(1)
    } else if n.contains(q) {
        Some(2)
    } else {
        None
    }
}

/// Render the results fragment for one query against one
/// snapshot. An empty query renders an empty fragment (the page
/// clears its container); a query with no hits renders the plain
/// no-match line — ambiguity is shown, never silently resolved,
/// and nothing is fuzzy-rescued.
fn render_results(snap: &SearchSnapshot, query: &str) -> String {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return String::new();
    }
    let plan = &snap.plan;
    // gear -> its crack chunk: the same map Hunt builds for its
    // card footers, so a gear hit's crack line is Hunt's.
    let mut cracks: HashMap<&str, &CrackInfo> = HashMap::new();
    for c in &plan.chunks {
        if let Some(ci) = &c.crack {
            cracks.entry(ci.gear.as_str()).or_insert(ci);
        }
    }
    let varzia_end = plan.resurgence.as_ref().map(|r| r.ends_in.as_str());
    // The pass-44 Need universe: part names in a genuine Need
    // state anywhere in the plan. A relic's "needed rewards" are
    // read off this set, never off a want list.
    let need_set: HashSet<&str> = plan
        .goals
        .iter()
        .flat_map(|g| g.parts.iter())
        .filter(|p| p.state == PartState::Need)
        .map(|p| p.name.as_str())
        .collect();

    let mut hits: Vec<Hit> = Vec::new();
    for g in &plan.goals {
        if let Some(rank) = match_rank(&g.gear, &q) {
            hits.push(Hit {
                rank,
                kind: KIND_GEAR,
                key: g.gear.to_lowercase(),
                html: gear_html(g, cracks.get(g.gear.as_str()).copied(), varzia_end),
            });
        }
        for p in &g.parts {
            if let Some(rank) = match_rank(&p.name, &q) {
                hits.push(Hit {
                    rank,
                    kind: KIND_PART,
                    key: p.name.to_lowercase(),
                    html: part_html(g, p),
                });
            }
        }
    }
    for entry in snap.relics.relics.values() {
        if let Some(rank) = match_rank(&entry.name, &q) {
            hits.push(Hit {
                rank,
                kind: KIND_RELIC,
                key: entry.name.to_lowercase(),
                html: relic_html(snap, entry, &need_set),
            });
        }
    }

    if hits.is_empty() {
        return format!(
            "<div class=\"sr-none\">No match for &ldquo;{}&rdquo; in the current plan.</div>\n",
            esc(query.trim())
        );
    }
    // Stable sort: equal (rank, kind, key) hits — the same part
    // name on two cards — keep plan order.
    hits.sort_by(|a, b| (a.rank, a.kind, &a.key).cmp(&(b.rank, b.kind, &b.key)));
    let total = hits.len();
    let mut h = String::new();
    for hit in hits.iter().take(MAX_RESULTS) {
        h.push_str(&hit.html);
    }
    if total > MAX_RESULTS {
        h.push_str(&format!(
            "<div class=\"sr-more\">+{} more — keep typing</div>\n",
            total - MAX_RESULTS
        ));
    }
    h
}

/// (marker glyph, state word, css class) for one part state — the
/// denotation every surface uses: ✓ is verified stock, ⍻ is
/// Seth's unverified assertion, never interchanged.
fn state_bits(state: &PartState) -> (&'static str, &'static str, &'static str) {
    match state {
        PartState::Have => ("✓ ", "held", "have"),
        PartState::HaveUnverified => ("⍻ ", "unverified", "uvhave"),
        PartState::Need => ("", "needed", "need"),
        PartState::Unknown => ("", "unknown", "unk"),
    }
}

/// A gear hit: the card's state at lookup size — progress, one
/// row per part (state, units, best relic), and the card's crack
/// line composed by the very helper Hunt's footer uses.
fn gear_html(g: &GoalCard, crack: Option<&CrackInfo>, varzia_end: Option<&str>) -> String {
    let mut h = String::new();
    h.push_str("<div class=\"sr-item\">\n<div class=\"sr-head\">");
    h.push_str(&format!("<b>{}</b>", esc(&g.gear)));
    h.push_str("<em class=\"sr-kind\">item</em>");
    if g.vaulted && !g.resurgence_live {
        h.push_str("<em class=\"bd v\">V</em>");
    }
    if g.resurgence_live {
        h.push_str("<em class=\"bd z\">VARZIA</em>");
    }
    if g.ready {
        h.push_str("<em class=\"bd z\">READY</em>");
    }
    h.push_str(&format!(
        "<span class=\"sr-count\">{} / {} parts</span>",
        g.parts_have, g.parts_total
    ));
    h.push_str("</div>\n");
    for p in &g.parts {
        h.push_str(&gear_part_row(g, p));
    }
    h.push_str(&card_footer_html(g, crack, varzia_end));
    h.push_str("</div>\n");
    h
}

/// One part row inside a gear hit: short name, state, unit
/// progress for a multi-unit part, and the part's best relic at
/// its advised refinement (the Hunt row's own choice: the deal
/// when computed, else the first ranked relic option).
fn gear_part_row(g: &GoalCard, p: &PartRow) -> String {
    let (marker, word, cls) = state_bits(&p.state);
    let cnt = if p.required > 1 {
        format!(
            "<span class=\"sr-cnt\">{}/{}</span>",
            p.held.min(p.required),
            p.required
        )
    } else {
        String::new()
    };
    let (relic, refine) = match &p.deal {
        Some(d) => (d.relic.clone(), refinement_abbrev(&d.refinement)),
        None => (
            p.relics
                .first()
                .map(|r| r.relic.clone())
                .unwrap_or_default(),
            String::new(),
        ),
    };
    let rc = if relic.is_empty() {
        String::new()
    } else if refine.is_empty() {
        format!("<span class=\"sr-rc\">{}</span>", esc(&relic))
    } else {
        format!(
            "<span class=\"sr-rc\">{} · {}</span>",
            esc(&relic),
            esc(&refine)
        )
    };
    format!(
        "<div class=\"sr-row\"><span class=\"sr-pn\">{}{}</span><span class=\"sr-st {cls}\">{word}</span>{cnt}{rc}</div>\n",
        marker,
        esc(&short_part(&p.name, &g.gear)),
        cls = cls,
        word = word,
        cnt = cnt,
        rc = rc,
    )
}

/// A part hit: state and units, the gear(s) it belongs to (one
/// block per card carrying the part), its best crack where one
/// is computed (the part's deal), and the relics that drop it.
fn part_html(g: &GoalCard, p: &PartRow) -> String {
    let (marker, word, cls) = state_bits(&p.state);
    let mut h = String::new();
    h.push_str("<div class=\"sr-item\">\n<div class=\"sr-head\">");
    h.push_str(&format!("<b>{}</b>", esc(&p.name)));
    h.push_str("<em class=\"sr-kind\">part</em>");
    h.push_str(&format!(
        "<span class=\"sr-st {cls}\">{marker}{word}</span>"
    ));
    h.push_str(&format!(
        "<span class=\"sr-count\">{} / {} held</span>",
        p.held.min(p.required),
        p.required
    ));
    h.push_str("</div>\n");
    h.push_str(&format!(
        "<div class=\"sr-sub\">Part of <b>{}</b></div>\n",
        esc(&g.gear)
    ));
    if let Some(d) = &p.deal {
        let own = match d.owned {
            Some(n) => format!(" · own {n}"),
            None => String::new(),
        };
        let rad = match d.radiant {
            Some(r) if r > 0 => format!(" · {r} radiant"),
            _ => String::new(),
        };
        h.push_str(&format!(
            "<div class=\"sr-sub\">Best crack: <b>{}</b> · {} · {}% · ~{} cracks{}{}</div>\n",
            esc(&d.relic),
            esc(&d.refinement),
            fmt_num(d.chance),
            fmt_num(d.expected),
            own,
            rad,
        ));
    }
    if !p.relics.is_empty() {
        let names: Vec<String> = p
            .relics
            .iter()
            .take(6)
            .map(|r| {
                if r.vaulted {
                    format!("{} (vaulted)", esc(&r.relic))
                } else {
                    esc(&r.relic)
                }
            })
            .collect();
        let more = if p.relics.len() > 6 {
            format!(" +{} more", p.relics.len() - 6)
        } else {
            String::new()
        };
        h.push_str(&format!(
            "<div class=\"sr-sub\">Relics: {}{}</div>\n",
            names.join(", "),
            more
        ));
    }
    h.push_str("</div>\n");
    h
}

/// A relic hit: held stock by refinement tier (or the honest
/// unknown), and which of its rewards are in a genuine Need
/// state in the current plan.
fn relic_html(snap: &SearchSnapshot, entry: &RelicEntry, need: &HashSet<&str>) -> String {
    let mut h = String::new();
    h.push_str("<div class=\"sr-item\">\n<div class=\"sr-head\">");
    h.push_str(&format!("<b>{}</b>", esc(&entry.name)));
    h.push_str("<em class=\"sr-kind\">relic</em>");
    if entry.vaulted {
        h.push_str("<em class=\"bd v\">V</em>");
    }
    let stock_line = if !snap.stock_known {
        "stock unknown".to_owned()
    } else {
        match snap.relic_tiers.get(&entry.name) {
            Some(tiers) => {
                let total: u32 = tiers.iter().map(|(_, n)| n).sum();
                format!("×{total} held")
            }
            None => "none held".to_owned(),
        }
    };
    h.push_str(&format!(
        "<span class=\"sr-count\">{}</span>",
        esc(&stock_line)
    ));
    h.push_str("</div>\n");
    if snap.stock_known {
        if let Some(tiers) = snap.relic_tiers.get(&entry.name) {
            let bits: Vec<String> = tiers
                .iter()
                .map(|(t, n)| format!("{} {}", esc(t), n))
                .collect();
            h.push_str(&format!(
                "<div class=\"sr-sub\">{}</div>\n",
                bits.join(" · ")
            ));
        }
    }
    let needed: Vec<String> = entry
        .rewards
        .iter()
        .map(|r| r.name.as_str())
        .filter(|n| need.contains(n))
        .map(|n| esc(n))
        .collect();
    if needed.is_empty() {
        h.push_str(&format!(
            "<div class=\"sr-sub\">None of its {} rewards are currently needed.</div>\n",
            entry.rewards.len()
        ));
    } else {
        h.push_str(&format!(
            "<div class=\"sr-sub\">Needed: {}</div>\n",
            needed.join(", ")
        ));
    }
    h.push_str("</div>\n");
    h
}

/// Percent / expected-cracks figures without dead decimals:
/// 10 -> "10", 25.33 -> "25.33", 3.95 -> "3.95".
fn fmt_num(v: f64) -> String {
    let s = format!("{v:.2}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Chunk, CrackInfo, GoalAction, ActionKind, PartDeal, RelicOption, SourceTier};
    use crate::tuning::Rarity;

    /// A two-card plan: Citrine Prime (one held part, one
    /// multi-unit part at 1/2) with a crack chunk, and Baruuk
    /// Prime carrying a ⍻ part.
    fn fixture_plan() -> Plan {
        let citrine = GoalCard {
            gear: "Citrine Prime".to_owned(),
            is_warframe: true,
            vaulted: false,
            resurgence_live: false,
            wants: vec![
                "Citrine Prime Blueprint".to_owned(),
                "Citrine Prime Chassis Blueprint".to_owned(),
            ],
            parts_have: 1,
            parts_total: 3,
            parts: vec![
                PartRow {
                    name: "Citrine Prime Blueprint".to_owned(),
                    state: PartState::Need,
                    held: 0,
                    required: 1,
                    relics: vec![RelicOption {
                        relic: "Meso C11".to_owned(),
                        vaulted: false,
                        chance_intact: 2.0,
                        chance_radiant: 10.0,
                        owned: Some(14),
                        source_tier: SourceTier::Owned,
                    }],
                    best_odds: "rare — 2% Intact / 10% Radiant".to_owned(),
                    rarity: Rarity::Rare,
                    deal: Some(PartDeal {
                        relic: "Meso C11".to_owned(),
                        vaulted: false,
                        refinement: "Radiant".to_owned(),
                        chance: 10.0,
                        expected: 10.0,
                        owned: Some(14),
                        radiant: Some(3),
                        source_tier: SourceTier::Owned,
                    }),
                },
                PartRow {
                    name: "Citrine Prime Chassis Blueprint".to_owned(),
                    state: PartState::Need,
                    held: 1,
                    required: 2,
                    relics: vec![],
                    best_odds: String::new(),
                    rarity: Rarity::Common,
                    deal: None,
                },
            ],
            ready: false,
            partially_unobtainable: false,
            action: GoalAction {
                text: String::new(),
                kind: ActionKind::Crack,
                relic: Some("Meso C11".to_owned()),
                score: 1.0,
            },
        };
        let baruuk = GoalCard {
            gear: "Baruuk Prime".to_owned(),
            is_warframe: true,
            vaulted: true,
            resurgence_live: false,
            wants: vec!["Baruuk Prime Systems Blueprint".to_owned()],
            parts_have: 1,
            parts_total: 1,
            parts: vec![PartRow {
                name: "Baruuk Prime Systems Blueprint".to_owned(),
                state: PartState::HaveUnverified,
                held: 1,
                required: 1,
                relics: vec![],
                best_odds: String::new(),
                rarity: Rarity::Uncommon,
                deal: None,
            }],
            ready: true,
            partially_unobtainable: false,
            action: GoalAction {
                text: String::new(),
                kind: ActionKind::Parked,
                relic: None,
                score: 0.0,
            },
        };
        let chunk = Chunk {
            id: "c1".to_owned(),
            tier: 1,
            title: String::new(),
            minutes_label: String::new(),
            why: String::new(),
            kind: "crack".to_owned(),
            expiry: None,
            gear: Some("Citrine Prime".to_owned()),
            crack: Some(CrackInfo {
                gear: "Citrine Prime".to_owned(),
                part: "Citrine Prime Blueprint".to_owned(),
                relic: "Meso C11".to_owned(),
                refinement: "Radiant".to_owned(),
                chance: 10.0,
                rarity: Rarity::Rare,
                batch: 4,
                traces: 400,
                covers: 0,
                radshare: None,
                fissure_node: String::new(),
                fissure_mission: String::new(),
                steel_path: false,
                vaulted: false,
                unobtainable: false,
            }),
        };
        Plan {
            goals: vec![citrine, baruuk],
            chunks: vec![chunk],
            ..Default::default()
        }
    }

    fn fixture_tables() -> RelicTables {
        let mut relics = HashMap::new();
        // Ten Axi relics (the cap test's wall) plus Meso C11.
        for i in 1..=10 {
            let name = format!("Axi A{i}");
            relics.insert(
                name.clone(),
                RelicEntry {
                    name,
                    rewards: vec![],
                    vaulted: false,
                    sources: HashMap::new(),
                },
            );
        }
        relics.insert(
            "Meso C11".to_owned(),
            RelicEntry {
                name: "Meso C11".to_owned(),
                rewards: vec![
                    crate::model::RelicReward {
                        name: "Citrine Prime Blueprint".to_owned(),
                        chance: HashMap::from([("Intact".to_owned(), 2.0)]),
                    },
                    crate::model::RelicReward {
                        name: "Forma Blueprint".to_owned(),
                        chance: HashMap::from([("Intact".to_owned(), 25.33)]),
                    },
                ],
                vaulted: false,
                sources: HashMap::new(),
            },
        );
        RelicTables {
            relics,
            sources: HashMap::new(),
        }
    }

    fn fixture_stock() -> crate::ocr::StockState {
        crate::ocr::StockState::from_json(
            r#"{"relics":{"Meso C11":{"tiers":{"Intact":9,"Radiant":3}}},"parts":{},"arcanes":{},"unverified":{}}"#,
        )
        .unwrap()
    }

    fn store() -> SearchStore {
        let store = SearchStore::new();
        store.publish(SearchSnapshot::new(
            fixture_plan(),
            fixture_tables(),
            Some(&fixture_stock()),
        ));
        store
    }

    #[test]
    fn lookup_before_first_publish_is_none() {
        let store = SearchStore::new();
        assert!(store.lookup("citrine").is_none());
    }

    #[test]
    fn ranking_exact_then_prefix_then_substring_case_insensitive() {
        let store = store();
        // Exact (shouted) beats everything; the relic is the only
        // exact hit for its own name.
        let html = store.lookup("MESO C11").unwrap();
        let relic_at = html.find("Meso C11").unwrap();
        assert!(
            html[..relic_at].contains("sr-item"),
            "the exact relic hit renders: {html}"
        );
        // "citrine" is a prefix of the gear and of its parts:
        // the gear (kind order) precedes the parts.
        let html = store.lookup("citrine").unwrap();
        let gear_at = html.find("<b>Citrine Prime</b>").unwrap();
        let part_at = html.find("<b>Citrine Prime Blueprint</b>").unwrap();
        assert!(gear_at < part_at, "gear precedes its parts: {html}");
        // A pure substring query ranks below the prefix hits would,
        // and still finds the part.
        let html = store.lookup("chassis blueprint").unwrap();
        assert!(html.contains("Citrine Prime Chassis Blueprint"), "{html}");
    }

    #[test]
    fn results_are_capped_with_a_keep_typing_line() {
        let store = store();
        let html = store.lookup("axi").unwrap();
        assert_eq!(html.matches("sr-item").count(), MAX_RESULTS, "{html}");
        assert!(html.contains("+2 more — keep typing"), "{html}");
    }

    #[test]
    fn gear_status_shows_states_units_and_the_hunt_crack_line() {
        let store = store();
        let html = store.lookup("Citrine Prime").unwrap();
        // Card progress and per-part states.
        assert!(html.contains("1 / 3 parts"), "{html}");
        assert!(html.contains("needed"), "{html}");
        // The multi-unit part at partial progress reads 1/2.
        assert!(html.contains("Chassis"), "{html}");
        assert!(html.contains("1/2"), "{html}");
        // The crack line is the Hunt footer's composition: the
        // chunk's relic, batch and target part.
        assert!(html.contains("Meso C11"), "{html}");
        assert!(html.contains("×4"), "{html}");
        assert!(html.contains("Blueprint"), "{html}");
    }

    #[test]
    fn part_status_multi_unit_partial_progress() {
        let store = store();
        let html = store.lookup("Citrine Prime Chassis Blueprint").unwrap();
        assert!(html.contains("Part of <b>Citrine Prime</b>"), "{html}");
        assert!(html.contains("1 / 2 held"), "{html}");
        assert!(html.contains("needed"), "{html}");
    }

    #[test]
    fn part_status_unverified_renders_the_unverified_badge() {
        let store = store();
        let html = store.lookup("Baruuk Prime Systems Blueprint").unwrap();
        // The ⍻ state renders as itself — never the ✓ of verified
        // stock — and carries no toggle affordance (data-uv is
        // Hunt's mutation channel; search is read-only).
        assert!(html.contains("⍻ "), "{html}");
        assert!(html.contains("unverified"), "{html}");
        assert!(!html.contains("data-uv"), "search never mutates: {html}");
    }

    #[test]
    fn part_status_best_crack_and_relic_sources() {
        let store = store();
        let html = store.lookup("Citrine Prime Blueprint").unwrap();
        assert!(
            html.contains("Best crack: <b>Meso C11</b> · Radiant · 10% · ~10 cracks · own 14 · 3 radiant"),
            "{html}"
        );
        assert!(html.contains("Relics: Meso C11"), "{html}");
    }

    #[test]
    fn relic_status_tiers_and_needed_rewards() {
        let store = store();
        let html = store.lookup("Meso C11").unwrap();
        // Stock by tier from the captured state the run consumed.
        assert!(html.contains("×12 held"), "{html}");
        assert!(html.contains("Intact 9 · Radiant 3"), "{html}");
        // Only the genuine-Need reward is named; Forma is not a
        // plan part and never reads as needed.
        assert!(html.contains("Needed: Citrine Prime Blueprint"), "{html}");
        assert!(!html.contains("Forma Blueprint"), "{html}");
    }

    #[test]
    fn relic_status_unknown_stock_says_so() {
        let store = SearchStore::new();
        store.publish(SearchSnapshot::new(
            fixture_plan(),
            fixture_tables(),
            None,
        ));
        let html = store.lookup("Meso C11").unwrap();
        assert!(html.contains("stock unknown"), "{html}");
        assert!(!html.contains("none held"), "{html}");
    }

    #[test]
    fn nonsense_query_gets_a_plain_no_match() {
        let store = store();
        let html = store.lookup("zzzqqq").unwrap();
        assert!(html.contains("No match for &ldquo;zzzqqq&rdquo;"), "{html}");
        assert!(!html.contains("sr-item"), "{html}");
    }

    #[test]
    fn empty_query_renders_an_empty_fragment() {
        let store = store();
        assert_eq!(store.lookup("").unwrap(), "");
        assert_eq!(store.lookup("   ").unwrap(), "");
    }
}
