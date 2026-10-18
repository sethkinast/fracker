//! HTML checklist renderer (BRIEF §4.2, §17 settled sections, §19 art).
//! Single self-contained file: inline CSS, no external assets except an
//! optional Google Fonts link (clean offline fallback) and WFCD CDN
//! icons that degrade to monograms when offline.
//!
//! All content (weekly board, caps, labels) arrives via the render
//! context from the runtime tuning tables; this module holds no game
//! content. Icon delivery is a mode: the product page hotlinks the CDN
//! per BRIEF §4.2, while `--demo` embeds ≤128px data-URI thumbnails
//! (see [`IconMode`]).

use std::collections::{HashMap, HashSet};

use crate::model::WorldState;
use crate::plan::Plan;
use crate::tuning::Tuning;

// ---------------------------------------------------------------------------
// Render context
// ---------------------------------------------------------------------------

pub struct RenderContext<'a> {
    pub world: &'a WorldState,
    pub generated_label: String,
    /// e.g. "backlog 64 primes · live profile".
    pub targets_status: String,
    pub world_status: String,
    /// Weekly item id -> completion label proven by the log.
    pub weekly_done: &'a HashMap<String, String>,
    /// Plan/challenge ids the log proves complete (struck-through).
    pub proven_done: &'a HashSet<String>,
    pub mastery_rank: Option<u32>,
    /// Parent gear name -> WFCD `imageName` (addendum #14).
    pub icons: &'a HashMap<String, String>,
    /// Parent gear name -> embedded data-URI thumbnail (demo mode).
    /// When present for a gear, it replaces the CDN hotlink; the
    /// product page passes None and keeps hotlinks per BRIEF §4.2.
    pub embedded_icons: Option<&'a HashMap<String, String>>,
    /// Non-prime unmastered gear by category (list only, no routing).
    pub backlog_categories: &'a HashMap<String, Vec<String>>,
    pub daily: Option<DailyRender>,
    pub pack: Option<PackRender>,
    /// Runtime tuning (weekly board, caps labels, sortie ledger).
    pub tuning: &'a Tuning,
}

/// How item icons are delivered on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconMode {
    /// Product page: CDN hotlinks (BRIEF §4.2).
    Hotlink,
    /// Demo export: embedded ≤128px data-URI thumbnails.
    Embedded,
}

/// Profile-backed daily bars: REMAINING caps from the live profile
/// (addendum #2) — never estimated.
pub struct DailyRender {
    pub affiliation_remaining: u32,
    pub affiliation_cap: u32,
    pub focus_remaining: u32,
    pub focus_cap: u32,
    /// Per-family `DailyAffiliation*` remaining, sorted by family.
    pub families: Vec<(String, u32)>,
}

/// Taubound pack strip for the page (pack v1, expires 2026-11-17).
pub struct PackRender {
    pub live: bool,
    pub expires_label: String,
    /// Community-goal `health` on the pack's event entry.
    pub contegorus_health: Option<f64>,
    pub arcane_line: String,
    /// All display content below comes from the pack TOML, never Rust.
    pub pack_name: String,
    pub event_label: String,
    pub payout_label: String,
    pub gated_label: String,
}

#[derive(Debug, Clone)]
pub struct WeeklyItem {
    pub id: String,
    pub name: String,
    pub limit: String,
    pub time: String,
    pub obs: String,
    pub deprioritized: bool,
}

/// Do Now alert subsection (pass 4 §§a–b): one card per live alert —
/// node, type, payout, countdown. The whole subsection hides when
/// empty; headers arrive from the tuning tables, never Rust.
fn donow_alert_section(
    h: &mut String,
    header: &str,
    alerts: &[crate::plan::AlertView],
    tuning: &Tuning,
    cls: &str,
) {
    if alerts.is_empty() {
        return;
    }
    h.push_str(&format!("<h3 class=\"donow-sub\">{}</h3>\n<div class=\"expiring\">\n", esc(header)));
    for a in alerts {
        let est = tuning.missions.base_minutes(&a.mission_type) as u32
            + tuning.missions.overhead as u32;
        let done_cls = if a.done { " done" } else { "" };
        let done_note = if a.done { " · log &check;" } else { "" };
        h.push_str(&format!(
            "<div class=\"card exp {cls}{done_cls}\"><span class=\"exp-title\">{} ({}) — {}</span>\n<span class=\"exp-sub\">{} left · ~{}m{}</span></div>\n",
            esc(&a.node),
            esc(&a.mission_type),
            esc(&a.payout),
            esc(&a.left),
            est,
            done_note,
        ));
    }
    h.push_str("</div>\n");
}

/// The tracked weeklies, loaded from the data pack (file order =
/// render order; flagged items render last and visually quiet).
/// Copy stays terse: labels, limits, times, short sources.
pub fn weekly_board(tuning: &Tuning) -> Vec<WeeklyItem> {
    tuning
        .weeklies
        .iter()
        .map(|w| WeeklyItem {
            id: w.id.clone(),
            name: w.name.clone(),
            limit: w.limit.clone(),
            time: w.time.clone(),
            obs: w.obs.clone(),
            deprioritized: w.deprioritized,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Presentation-only capitalization for data-owned names.
fn titlecase(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn rarity_class(r: crate::plan::Rarity) -> &'static str {
    match r {
        crate::plan::Rarity::Common => "chip-common",
        crate::plan::Rarity::Uncommon => "chip-uncommon",
        crate::plan::Rarity::Rare => "chip-rare",
    }
}

pub fn render_plan(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    h.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    h.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    // Converge without a server; the refresh-flash rule (§19) keeps
    // every repaint dark (root background + color-scheme: dark).
    h.push_str("<meta http-equiv=\"refresh\" content=\"60\">\n");
    h.push_str("<title>Fracker — session plan</title>\n");
    // Demo exports are self-contained (addendum #23: zero CDN
    // references): no font links, no hotlinks — system type only.
    // The product page keeps the display type + CDN icons.
    if ctx.embedded_icons.is_none() {
        h.push_str("<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n");
        h.push_str("<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?family=Rajdhani:wght@500;600;700&display=swap\">\n");
    }
    h.push_str("<style>\n");
    h.push_str(CSS);
    h.push_str("\n</style>\n</head>\n");
    // The in-game day the tick store is keyed by (ninth pass §2): the plan
    // date, read by the client + the loopback server when applying ticks.
    let day = ctx.generated_label.get(..10).unwrap_or("");
    h.push_str(&format!("<body data-day=\"{}\">\n<div class=\"wrap\">\n", esc(day)));

    // ---- slim header: brand + three status pills + one-line stamp ----
    h.push_str(&header_html(plan, ctx));

    // ---- four tabs: First Crack / Hunt / Backlog / Routine ----
    // (Same CSS-radio mechanism as before; item ids — the tick keys —
    // are unchanged by the restructure, so moved rows keep identity.)
    h.push_str("<div class=\"tabs\">\n");
    h.push_str("<input type=\"radio\" name=\"tab\" id=\"tab-donow\" checked><label class=\"tab\" for=\"tab-donow\">First Crack</label>\n");
    h.push_str("<input type=\"radio\" name=\"tab\" id=\"tab-hunt\"><label class=\"tab\" for=\"tab-hunt\">Hunt</label>\n");
    h.push_str("<input type=\"radio\" name=\"tab\" id=\"tab-backlog\"><label class=\"tab\" for=\"tab-backlog\">Backlog</label>\n");
    h.push_str("<input type=\"radio\" name=\"tab\" id=\"tab-routine\"><label class=\"tab\" for=\"tab-routine\">Routine</label>\n");

    // ---- tab 1: First Crack — ACTION ONLY ----
    // Event alerts, the crack queue, the Varzia strip, the pack strip,
    // and a closing one-liner. NO standing bars, NO weeklies, nothing
    // routine (that lives under Routine now).
    h.push_str("<div class=\"tabpage\" id=\"page-donow\">\n<section><h2>First crack</h2>\n");
    donow_alert_section(&mut h, &ctx.tuning.planner.alert_sections.event_header, &plan.event_alerts, ctx.tuning, "evt");
    donow_alert_section(&mut h, &ctx.tuning.planner.alert_sections.lotus_header, &plan.lotus_alerts, ctx.tuning, "lotus");
    h.push_str(&crack_queue_html(plan, ctx));
    if let Some(r) = &plan.resurgence {
        h.push_str(&resurgence_strip_html(r));
    }
    h.push_str(&pack_strip_html(ctx));
    if !plan.notes.is_empty() {
        h.push_str(&format!(
            "<div class=\"closing\">{}</div>\n",
            esc(&plan.notes.join(" · ")),
        ));
    }
    h.push_str("</section>\n</div>\n");

    // ---- tab 2: hunt (fan-out over all prime goal cards) ----
    h.push_str("<div class=\"tabpage\" id=\"page-hunt\">\n<section><h2>Hunt</h2>\n");
    h.push_str(&format!(
        "<div class=\"backlog-head\">{} goals · {} parts</div>\n",
        plan.goals.len(),
        plan.goals.iter().map(|g| g.wants.len()).sum::<usize>(),
    ));
    h.push_str("<div class=\"backlog\">\n");
    for g in &plan.goals {
        h.push_str(&backlog_group_html(g, ctx));
    }
    h.push_str("</div>\n");
    h.push_str("</section>\n</div>\n");

    // ---- tab 3: backlog (everything unmastered, all gear) ----
    h.push_str("<div class=\"tabpage\" id=\"page-backlog\">\n<section><h2>Backlog</h2>\n");
    h.push_str("<div class=\"backlog\">\n");
    for g in &plan.goals {
        h.push_str(&backlog_compact_html(g, ctx));
    }
    h.push_str("</div>\n");
    // non-prime unmastered gear, grouped by category (list only —
    // no farm routing this pass).
    if !ctx.backlog_categories.is_empty() {
        h.push_str("<h3 class=\"nonprime-h\">Non-prime</h3>\n<div class=\"nonprime\">\n");
        let mut cats: Vec<(&String, &Vec<String>)> = ctx.backlog_categories.iter().collect();
        cats.sort_by(|a, b| a.0.cmp(b.0));
        for (cat, items) in cats {
            if cat == "Primes" || items.is_empty() {
                continue;
            }
            let mut sorted = items.clone();
            sorted.sort();
            h.push_str(&format!(
                "<div class=\"npcat\"><span class=\"npcat-name\">{} · {}</span><span class=\"npcat-items\">{}</span></div>\n",
                esc(cat),
                sorted.len(),
                sorted.iter().map(|i| esc(i)).collect::<Vec<_>>().join(", "),
            ));
        }
        h.push_str("</div>\n");
    }
    h.push_str("</section>\n</div>\n");

    // ---- tab 4: routine — the daily/weekly surface ----
    h.push_str(&routine_html(ctx));
    h.push_str("</div>\n");

    // checkbox persistence: the resident's loopback server is the store
    // of record; localStorage remains a progressive cache for file://.
    h.push_str("<script>\n");
    h.push_str(JS);
    h.push_str("\n</script>\n</div>\n</body>\n</html>\n");
    h
}

/// One-line status countdown for a world-state expiry, relative to the
/// plan's own timestamp ("in 2d", "in 5h", "in 25m"). Presentation only:
/// no plan data or scoring changes.
fn relative_countdown(ctx: &RenderContext<'_>, expiry: &str) -> String {
    let now = ctx
        .world
        .timestamp
        .parse::<chrono::DateTime<chrono::Utc>>()
        .ok();
    let then = expiry.parse::<chrono::DateTime<chrono::Utc>>().ok();
    match (now, then) {
        (Some(now), Some(then)) => {
            let mins = (then - now).num_minutes();
            if mins <= 0 {
                "now".to_owned()
            } else if mins < 60 {
                format!("in {mins}m")
            } else if mins < 48 * 60 {
                format!("in {}h", mins / 60)
            } else {
                format!("in {}d", mins / (24 * 60))
            }
        }
        _ => "—".to_owned(),
    }
}

/// The slim header: brand, three status pills (Baro countdown, Varzia
/// summary, Targets summary), and one one-line stamp (MR, generated
/// time, world state, backlog count, profile state). The old multi-block
/// header boilerplate is gone. Pill labels come from the tuning tables;
/// countdown values come from the plan/world state, never invented.
fn header_html(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    let vendor = esc(&ctx.tuning.planner.vendor.name);
    let contact = esc(&ctx.tuning.planner.resurgence.contact);
    let baro = match &plan.baro {
        Some(b) => format!("{} &middot; {}", esc(&b.ends_in), esc(&b.location)),
        None => match &ctx.world.baro {
            Some(vt) => relative_countdown(ctx, &vt.expiry),
            None => "—".to_owned(),
        },
    };
    let varzia = match &plan.resurgence {
        Some(r) => format!("{} gears &middot; {}", r.gears.len(), esc(&r.ends_in)),
        None => "—".to_owned(),
    };
    let mr = ctx
        .mastery_rank
        .map(|m| m.to_string())
        .unwrap_or_else(|| "—".to_owned());
    let mut h = String::new();
    h.push_str("<header class=\"masthead\">\n<div class=\"brand\">FRACKER</div>\n");
    h.push_str("<div class=\"pills\">\n");
    h.push_str(&format!(
        "<span class=\"pill pill-baro\"><b>{vendor}</b> {baro}</span>\n"
    ));
    h.push_str(&format!(
        "<span class=\"pill pill-varzia\"><b>{contact}</b> {varzia}</span>\n"
    ));
    h.push_str(&format!(
        "<span class=\"pill pill-targets\">{}</span>\n",
        esc(&ctx.targets_status),
    ));
    h.push_str("</div>\n");
    h.push_str(&format!(
        "<div class=\"stamp\">MR{mr} &middot; {} &middot; {} &middot; {}</div>\n",
        esc(&ctx.generated_label),
        esc(&ctx.world_status),
        esc(&ctx.targets_status),
    ));
    h.push_str("</header>\n");
    h
}

/// The crack queue is cracks + relic acquisition. The event alerts, the
/// rotation, and the trader have their own surfaces on First Crack (the
/// Events subsection, the Varzia strip, the Baro pill), so their tier-1
/// chunks never duplicate here. Unknown kinds still render — nothing is
/// deleted, only de-duplicated.
fn queue_chunk(c: &crate::plan::Chunk) -> bool {
    !matches!(c.kind.as_str(), "event alert" | "prime resurgence" | "vendor")
}

/// The crack queue: the ≤3 focus parts, the loud expiring chunk, the
/// rest of the expiring row, the tier 2/3 slate, then any farms (quiet,
/// still action). Mission / relic / refinement / target part / chance /
/// time all ride the existing chunk + deal strings.
fn crack_queue_html(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    h.push_str("<h3 class=\"blockhead\">Crack queue</h3>\n");
    if !plan.focus.is_empty() {
        h.push_str("<div class=\"focus\">\n");
        for f in &plan.focus {
            h.push_str(&format!(
                "<span class=\"focus-part\">{}&nbsp;{} · {} · {}</span>\n",
                icon_span(&f.gear, ctx, "focus-icon"),
                esc(&f.part),
                esc(&f.gear),
                esc(&f.era),
            ));
        }
        h.push_str("</div>\n");
    }
    let top: Vec<&crate::plan::Chunk> = plan
        .chunks
        .iter()
        .filter(|c| c.tier == 1 && queue_chunk(c))
        .collect();
    if top.is_empty() {
        h.push_str("<div class=\"card quiet\">Nothing expiring.</div>\n");
    } else {
        let first = top[0];
        h.push_str(&format!(
            "<div class=\"card donow\"><div class=\"do-text\">{}<span class=\"do-label\">{}</span></div>\n<div class=\"do-sub\">{} &middot; {}</div></div>\n",
            chunk_icon(first, ctx),
            esc(&first.title),
            esc(&first.minutes_label),
            esc(&first.why),
        ));
        if top.len() > 1 {
            h.push_str("<div class=\"expiring\">\n");
            for c in &top[1..] {
                h.push_str(&format!(
                    "<div class=\"card exp\">{}<span class=\"exp-title\">{}</span>\n<span class=\"exp-sub\">{} &middot; {}</span></div>\n",
                    chunk_icon(c, ctx),
                    esc(&c.title),
                    esc(&c.minutes_label),
                    esc(&c.why),
                ));
            }
            h.push_str("</div>\n");
        }
    }
    let slate: Vec<&crate::plan::Chunk> = plan
        .chunks
        .iter()
        .filter(|c| (c.tier == 2 || c.tier == 3) && queue_chunk(c))
        .collect();
    if slate.is_empty() {
        h.push_str("<div class=\"card quiet\">No live cracks.</div>\n");
    } else {
        h.push_str("<div class=\"slate\">\n");
        for c in &slate {
            let proven = ctx.proven_done.contains(&c.id);
            h.push_str(&format!(
                "<label class=\"card chunk{}\"><input type=\"checkbox\" data-id=\"{}\"{}>\n<span class=\"chunk-body\"><span class=\"chunk-head\">{}<span class=\"chunk-title\">{}</span></span>\n<span class=\"chunk-meta\">{} &middot; {}</span>\n<span class=\"chunk-why\">{}</span></span></label>\n",
                if proven { " done" } else { "" },
                esc(&c.id),
                if proven { " checked" } else { "" },
                chunk_icon(c, ctx),
                esc(&c.title),
                esc(&c.minutes_label),
                esc(&c.kind),
                esc(&c.why),
            ));
        }
        h.push_str("</div>\n");
    }
    if !plan.farms.is_empty() {
        h.push_str("<h3 class=\"blockhead\">Farms</h3>\n");
        for f in &plan.farms {
            h.push_str(&format!(
                "<div class=\"card farm\"><div class=\"farm-title\">Farm: {} ({}) — {}</div>\n<div class=\"farm-sub\">Relics: {} &middot; score {:.1}/h</div></div>\n",
                esc(&f.node),
                esc(&f.planet),
                esc(&f.mode),
                esc(&f.relics.iter().map(|r| esc(r)).collect::<Vec<_>>().join(", ")),
                f.score,
            ));
        }
    }
    h
}

/// The Varzia / Prime Resurgence strip: first-class on First Crack.
fn resurgence_strip_html(r: &crate::plan::ResurgenceView) -> String {
    format!(
        "<div class=\"card resurg-first\"><span class=\"resurg-line\">{} — {} ({}) · ends {}</span>\n<span class=\"resurg-sub\">Vaulted relics for {} while the rotation is live.</span></div>\n",
        esc(&r.contact),
        esc(&r.gears.join(", ")),
        esc(&r.location),
        esc(&r.ends_in),
        esc(&r.currency),
    )
}

/// The event pack strip (Taubound): payout, gate, community health, and
/// the remaining-arcanes line. Content all from the pack TOML.
fn pack_strip_html(ctx: &RenderContext<'_>) -> String {
    let Some(pack) = &ctx.pack else {
        return String::new();
    };
    if !pack.live {
        return String::new();
    }
    let health = match pack.contegorus_health {
        Some(v) => format!("{} {:.2}", pack.event_label, v),
        None => format!("{} —", pack.event_label),
    };
    format!(
        "<h3 class=\"blockhead\">{}</h3>\n<div class=\"card tau pack-strip\"><span class=\"tau-line\">{} · ends {}</span><span class=\"tau-line\">{} · {}</span><span class=\"tau-line\">{}</span></div>\n",
        esc(&titlecase(&pack.pack_name)),
        esc(&pack.payout_label),
        esc(&pack.expires_label),
        esc(&pack.gated_label),
        esc(&health),
        esc(&pack.arcane_line),
    )
}

/// Tab 4, Routine: the daily/weekly surface in one place.
/// - "Today": standing + focus as compact inline number chips with slim
///   bars, then the sortie line with its reset time.
/// - "This week": the weeklies board (server-side ticks as of pass 9)
///   as compact tick rows with time estimates, then the act board.
fn routine_html(ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    h.push_str("<div class=\"tabpage\" id=\"page-routine\">\n<section><h2>Routine</h2>\n");
    h.push_str("<h3 class=\"blockhead\">Today</h3>\n<div class=\"pools\">\n");
    let caps = &ctx.tuning.planner.daily_caps;
    match &ctx.daily {
        Some(d) => h.push_str(&pool_bars_html(d, caps)),
        None => {
            let (focus_cap, stand_cap) = match ctx.mastery_rank {
                Some(mr) => (
                    fmt_int(caps.focus_base + caps.focus_per_mr * mr),
                    fmt_int(caps.standing_base + caps.standing_per_mr * mr),
                ),
                None => (
                    caps.fallback_focus_label.clone(),
                    caps.fallback_standing_label.clone(),
                ),
            };
            h.push_str(&format!(
                "<div class=\"card quiet\">Focus cap {focus_cap} · Standing cap {stand_cap} — profile offline.</div>\n"
            ));
        }
    }
    h.push_str("</div>\n");
    // Sortie: a single tick row with its reset time (unchanged tick key).
    let sortie = &ctx.tuning.planner.sortie;
    let sortie_done = ctx.weekly_done.contains_key(&sortie.ledger_id);
    h.push_str(&format!(
        "<label class=\"card bar sortie\"><input type=\"checkbox\" data-id=\"{id}\"{checked}>\n<span class=\"bar-name\">{name}</span>\n<span class=\"bar-meta\">{reset}{proven}</span></label>\n",
        id = esc(&sortie.ledger_id),
        checked = if sortie_done { " checked" } else { "" },
        name = esc(&sortie.name),
        reset = esc(&sortie.reset_label),
        proven = if sortie_done { " · log" } else { "" },
    ));
    // "This week": the weeklies board + the act board.
    h.push_str("<h3 class=\"blockhead\">This week</h3>\n");
    h.push_str(&weeklies_html(ctx));
    h.push_str(&nightwave_html(ctx));
    h.push_str("</section>\n</div>\n");
    h
}

/// The weeklies board (existing WEEKLIES content set), file order =
/// render order; flagged items render last and visually quiet. Tick keys
/// are `wk-{id}` — the same ids as before the move into Routine.
fn weeklies_html(ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    h.push_str("<div class=\"wk-grid\">\n");
    for w in weekly_board(ctx.tuning) {
        let done_label = ctx.weekly_done.get(&w.id).cloned().unwrap_or_default();
        let done = !done_label.is_empty();
        h.push_str(&format!(
            "<label class=\"wk{item_class}{small_class}\"><input type=\"checkbox\" data-id=\"wk-{id}\"{checked}>\n<span class=\"wk-name\">{name}</span>\n<span class=\"wk-meta\">{limit} &middot; {time}</span>\n<span class=\"wk-obs\">{obs}{proven}</span></label>\n",
            item_class = if done { " done" } else { "" },
            small_class = if w.deprioritized { " wk-small" } else { "" },
            id = w.id,
            checked = if done { " checked" } else { "" },
            name = esc(&w.name),
            limit = esc(&w.limit),
            time = esc(&w.time),
            obs = esc(&w.obs),
            proven = if done { format!(" &middot; proven {}", esc(&done_label)) } else { String::new() },
        ));
    }
    h.push_str("</div>\n");
    h
}

/// The act board (Nightwave): name + DESCRIPTION everywhere acts render
/// — the name alone doesn't say what to do. Header from the tuning
/// tables, never Rust; tick keys are `nw-{id}`.
fn nightwave_html(ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    let season_title = ctx.tuning.planner.nightwave.name.as_str();
    h.push_str(&format!("<h3 class=\"blockhead\">{}</h3>\n", esc(season_title)));
    match &ctx.world.nightwave {
        Some(nw) if !nw.active_challenges.is_empty() => {
            h.push_str(&format!(
                "<div class=\"nw-season\">Season {} — acts, never a standing total.</div>\n",
                nw.season,
            ));
            h.push_str("<div class=\"nw-grid\">\n");
            for a in &nw.active_challenges {
                let tag = if a.is_daily {
                    "daily"
                } else if a.is_elite {
                    "elite"
                } else {
                    "weekly"
                };
                let done = ctx.proven_done.contains(&a.id);
                h.push_str(&format!(
                    "<label class=\"nw{item_class}\"><input type=\"checkbox\" data-id=\"nw-{id}\"{checked}>\n<span class=\"nw-title\">{title}</span>\n<span class=\"nw-desc\">{desc}</span>\n<span class=\"nw-meta\">{tag} &middot; {standing} standing</span></label>\n",
                    item_class = if done { " done" } else { "" },
                    id = esc(&a.id),
                    checked = if done { " checked" } else { "" },
                    title = esc(&a.title),
                    desc = esc(&a.desc),
                    tag = tag,
                    standing = a.standing,
                ));
            }
            h.push_str("</div>\n");
            h.push_str("<div class=\"caveat\">Acts are a floor — Recovered backfill can exceed this list.</div>\n");
        }
        _ => {
            h.push_str(&format!(
                "<div class=\"card quiet\">No {} season live in world state.</div>\n",
                esc(season_title),
            ));
        }
    }
    h
}

/// Icon `src` for one parent gear: the embedded data-URI thumbnail
/// when demo mode supplied one, else the CDN hotlink (product page).
/// A prime part always shows its PARENT item's icon, never a generic
/// part icon. Unresolved items keep the monogram (None).
///
/// Demo strictness (addendum #23: zero CDN references): when the
/// caller passes an embed map (demo export), gears missing from the
/// map fall back to the monogram — never to a hotlink — so the
/// exported file fetches nothing.
fn gear_icon_src(gear: &str, ctx: &RenderContext<'_>) -> Option<String> {
    if let Some(embedded) = ctx.embedded_icons {
        return embedded.get(gear).cloned();
    }
    crate::model::icon_url(gear, ctx.icons)
}

/// One icon chip for a parent gear (sixth pass §2): the parent-item
/// image when resolved, else the monogram. Demo strictness carries over
/// through [`gear_icon_src`] — a gear missing from the embed map keeps
/// its monogram, never a hotlink.
fn icon_span(gear: &str, ctx: &RenderContext<'_>, class: &str) -> String {
    let initial = gear.chars().next().unwrap_or('?');
    match gear_icon_src(gear, ctx) {
        Some(url) => format!(
            "<span class=\"{class}\"><img src=\"{}\" alt=\"\" loading=\"lazy\" onerror=\"this.remove()\">{initial}</span>",
            esc(&url),
            initial = initial,
        ),
        None => format!(
            "<span class=\"{class}\" aria-hidden=\"true\">{initial}</span>",
            initial = initial,
        ),
    }
}

/// First Crack chunk-row icon: the goal's parent gear when the chunk
/// carries one (crack chunks), else nothing.
fn chunk_icon(c: &crate::plan::Chunk, ctx: &RenderContext<'_>) -> String {
    match &c.gear {
        Some(gear) => icon_span(gear, ctx, "chunk-icon"),
        None => String::new(),
    }
}

/// One parent group: icon + gear + pips, then aligned part rows
/// (part | best relic + tier chance | stock | expected cracks).
fn backlog_group_html(g: &crate::plan::GoalCard, ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    let initial = g.gear.chars().next().unwrap_or('?');
    let icon = gear_icon_src(&g.gear, ctx);
    h.push_str("<article class=\"card bgroup\">\n");
    h.push_str("<div class=\"bhead\">");
    match &icon {
        Some(url) => h.push_str(&format!(
            "<span class=\"portrait has-icon\"><img src=\"{}\" alt=\"\" loading=\"lazy\" onerror=\"this.remove()\">{initial}</span>",
            esc(url),
            initial = initial,
        )),
        None => h.push_str(&format!(
            "<span class=\"portrait\" aria-hidden=\"true\">{initial}</span>",
            initial = initial,
        )),
    }
    h.push_str(&format!("<span class=\"bname\">{}</span>", esc(&g.gear)));
    if g.vaulted {
        h.push_str("<span class=\"vaulted\">V</span>");
    }
    if g.resurgence_live {
        h.push_str("<span class=\"resurg\">VARZIA</span>");
    }
    h.push_str("<span class=\"pips\" aria-hidden=\"true\">");
    for p in &g.parts {
        let cls = match p.state {
            crate::plan::PartState::Have => "pip have",
            crate::plan::PartState::Need => "pip need",
            crate::plan::PartState::Unknown => "pip unknown",
        };
        h.push_str(&format!("<span class=\"{cls}\"></span>"));
    }
    h.push_str(&format!(
        "</span><span class=\"bcount\">{}/{}</span></div>\n",
        g.parts_have, g.parts_total,
    ));
    for p in &g.parts {
        let (relic, tier, stock, cracks) = match &p.deal {
            Some(d) => (
                d.relic.clone(),
                format!("{} {}%", d.refinement, fmt_terse_pct(d.chance)),
                match d.owned {
                    Some(n) => format!("×{n}"),
                    None => "?".to_owned(),
                },
                format!("~{:.0}", d.expected),
            ),
            None => (
                "—".to_owned(),
                "—".to_owned(),
                "?".to_owned(),
                "—".to_owned(),
            ),
        };
        h.push_str(&format!(
            "<div class=\"brow\"><span class=\"bpart\">{}</span><span class=\"brelic{} {}\">{} · {}</span><span class=\"bstock\">{}</span><span class=\"bcracks\">{}</span></div>\n",
            esc(&short_part(&p.name, &g.gear)),
            if p.relics.first().map(|r| r.vaulted).unwrap_or(false) {
                " is-vaulted"
            } else {
                ""
            },
            rarity_class(p.rarity),
            esc(&relic),
            esc(&tier),
            esc(&stock),
            esc(&cracks),
        ));
    }
    let kind = match g.action.kind {
        crate::plan::ActionKind::Crack => "crack",
        crate::plan::ActionKind::Resurgence => "resurgence",
        crate::plan::ActionKind::WaitFissure => "wait",
        crate::plan::ActionKind::Farm => "farm",
        crate::plan::ActionKind::Parked => "parked",
    };
    h.push_str(&format!(
        "<div class=\"action action-{kind}\">{text}</div>\n",
        kind = kind,
        text = esc(&g.action.text),
    ));
    h.push_str("</article>\n");
    h
}

/// Compact one-line gear entry for the Backlog tab (everything
/// unmastered): icon, name, badges, pips, count, action kind.
fn backlog_compact_html(g: &crate::plan::GoalCard, ctx: &RenderContext<'_>) -> String {
    let initial = g.gear.chars().next().unwrap_or('?');
    let icon = gear_icon_src(&g.gear, ctx);
    let mut h = String::new();
    h.push_str("<div class=\"bline\">");
    match &icon {
        Some(url) => h.push_str(&format!(
            "<span class=\"miniport\"><img src=\"{}\" alt=\"\" loading=\"lazy\" onerror=\"this.remove()\">{initial}</span>",
            esc(url),
            initial = initial,
        )),
        None => h.push_str(&format!(
            "<span class=\"miniport\" aria-hidden=\"true\">{initial}</span>",
            initial = initial,
        )),
    }
    h.push_str(&format!("<span class=\"bname\">{}</span>", esc(&g.gear)));
    if g.vaulted {
        h.push_str("<span class=\"vaulted\">V</span>");
    }
    if g.resurgence_live {
        h.push_str("<span class=\"resurg\">VARZIA</span>");
    }
    h.push_str("<span class=\"pips\" aria-hidden=\"true\">");
    for p in &g.parts {
        let cls = match p.state {
            crate::plan::PartState::Have => "pip have",
            crate::plan::PartState::Need => "pip need",
            crate::plan::PartState::Unknown => "pip unknown",
        };
        h.push_str(&format!("<span class=\"{cls}\"></span>"));
    }
    h.push_str(&format!(
        "</span><span class=\"bcount\">{}/{}</span>",
        g.parts_have, g.parts_total,
    ));
    let kind = match g.action.kind {
        crate::plan::ActionKind::Crack => "crack",
        crate::plan::ActionKind::Resurgence => "resurgence",
        crate::plan::ActionKind::WaitFissure => "wait",
        crate::plan::ActionKind::Farm => "farm",
        crate::plan::ActionKind::Parked => "parked",
    };
    h.push_str(&format!("<span class=\"bkind\">{kind}</span>"));
    h.push_str("</div>\n");
    h
}

/// Minimal base64 encoder (demo data-URI embedding). Dependency-free
/// on purpose: the one external-asset path in the page needs no new
/// crates.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
/// "Ivara Prime Chassis Blueprint" -> "Chassis" inside its parent group.
fn short_part(part: &str, gear: &str) -> String {
    let s = part
        .strip_prefix(gear)
        .unwrap_or(part)
        .trim()
        .strip_suffix("Blueprint")
        .unwrap_or(
            part.strip_prefix(gear)
                .unwrap_or(part)
                .trim(),
        )
        .trim()
        .to_owned();
    if s.is_empty() {
        part.to_owned()
    } else {
        s
    }
}

fn fmt_terse_pct(p: f64) -> String {
    if (p - p.round()).abs() < 1e-9 {
        format!("{}", p.round() as u32)
    } else {
        format!("{p:.2}")
    }
}

fn fmt_int(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// The pass-4 daily pool bars — now the Routine tab's compact "Today"
/// strip: Focus + the bare shared `DailyAffiliation` pool (full size),
/// then one slim inline number chip per family. Remaining values are the
/// profile's own REMAINING amounts, printed verbatim (never estimated,
/// never arithmetic-transformed). Public so the ninth-pass §3 audit test
/// can assert rendered remaining == the profile's raw values for every
/// pool, including the bare one.
pub fn pool_bars_html(d: &DailyRender, caps: &crate::tuning::DailyCapsTuning) -> String {
    let mut h = String::new();
    h.push_str(&pool_bar("Focus", d.focus_remaining, d.focus_cap, false));
    h.push_str(&pool_bar(
        caps.label_for("main"),
        d.affiliation_remaining,
        d.affiliation_cap,
        false,
    ));
    let have: HashMap<&str, u32> = d.families.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    for fam in &caps.families {
        if fam.eq_ignore_ascii_case("pvp") {
            continue;
        }
        if let Some(remaining) = have.get(fam.as_str()).copied() {
            h.push_str(&pool_bar(
                caps.label_for(fam),
                remaining,
                d.affiliation_cap,
                true,
            ));
        }
    }
    h
}

/// One standing/focus pool chip: a compact inline label + value with a
/// slim 5px bar. Full pools dim (`bar-full`) — the pattern is the
/// information. The `bar-name` text stays `{label} · {remaining} /
/// {cap} left` (the §3 audit parses it back to the raw values).
fn pool_bar(name: &str, remaining: u32, cap: u32, slim: bool) -> String {
    let pct = if cap == 0 {
        0.0
    } else {
        (100.0 * (1.0 - remaining as f64 / cap as f64)).clamp(0.0, 100.0)
    };
    let full = cap > 0 && remaining >= cap;
    format!(
        "<div class=\"pool{slim}{full}\"><span class=\"bar-name\">{} · {} / {} left</span>\n<span class=\"bar-track\"><span class=\"bar-fill\" style=\"width:{:.0}%\"></span></span></div>\n",
        esc(name),
        fmt_int(remaining),
        fmt_int(cap),
        pct,
        slim = if slim { " slim" } else { "" },
        full = if full { " bar-full" } else { "" },
    )
}

// ---------------------------------------------------------------------------
// Art direction (BRIEF §19)
// ---------------------------------------------------------------------------

const CSS: &str = r#"
:root { color-scheme: dark; }
html {
  background: #0a0d12;
  background-image:
    radial-gradient(1200px 500px at 50% -10%, #141b28 0%, rgba(20,27,40,0) 70%),
    radial-gradient(1px 1px at 12% 22%, rgba(201,210,222,.14) 50%, transparent 51%),
    radial-gradient(1px 1px at 68% 8%, rgba(201,210,222,.10) 50%, transparent 51%),
    radial-gradient(1px 1px at 84% 34%, rgba(201,210,222,.12) 50%, transparent 51%),
    radial-gradient(1px 1px at 32% 12%, rgba(201,210,222,.09) 50%, transparent 51%),
    linear-gradient(#0a0d12, #11151d);
  color-scheme: dark;
}
body {
  margin: 0; background: transparent; color: #c9d2de;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  line-height: 1.4;
}
.wrap { max-width: 1080px; margin: 0 auto; padding: 18px 18px 48px; }
h1, h2, h3, .brand, .do-text, .chunk-title, .bname, .farm-title {
  font-family: Rajdhani, -apple-system, "Segoe UI", Roboto, sans-serif;
}
h2 {
  font-size: 11px; letter-spacing: .18em; text-transform: uppercase;
  color: #7e8898; margin: 20px 0 8px; font-weight: 600;
}
h2:first-child { margin-top: 4px; }
section { margin-bottom: 4px; }
/* slim header: one row — brand, pills, stamp */
.masthead { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.brand { font-size: 20px; font-weight: 700; letter-spacing: .28em; color: #bfa05a; }
.pills { display: flex; gap: 6px; flex-wrap: wrap; }
.pill {
  font-size: 11px; color: #c9d2de; background: #131824; border: 1px solid #1f2735;
  padding: 3px 9px; white-space: nowrap;
  clip-path: polygon(5px 0, 100% 0, 100% calc(100% - 5px), calc(100% - 5px) 100%, 0 100%, 0 5px);
}
.pill b { color: #bfa05a; font-weight: 600; }
.stamp { color: #7e8898; font-size: 11px; margin-left: auto; }
.card {
  background: #151a23; border: 1px solid #232b38; border-radius: 0;
  clip-path: polygon(10px 0, 100% 0, 100% calc(100% - 10px), calc(100% - 10px) 100%, 0 100%, 0 10px);
  padding: 10px 13px; margin: 6px 0;
}
.quiet { color: #7e8898; }
.blockhead {
  font-size: 11px; letter-spacing: .16em; text-transform: uppercase;
  color: #7e8898; margin: 16px 0 6px; font-weight: 600;
}
.closing { color: #7e8898; font-size: 12px; margin-top: 8px; }
.donow { border-color: #bfa05a; }
.do-text { font-size: 18px; font-weight: 700; color: #aee3ef; line-height: 1.2; display: flex; align-items: center; gap: 8px; }
.do-sub { color: #7e8898; margin-top: 3px; font-size: 12px; }
.expiring { display: grid; gap: 2px; margin-top: 4px; }
.exp { padding: 7px 11px; }
.exp.done { opacity: .5; }
.exp.done .exp-title { text-decoration: line-through; }
.exp-title { font-family: Rajdhani, sans-serif; font-weight: 600; font-size: 13px; }
.exp-sub { color: #7e8898; font-size: 11px; display: block; margin-top: 1px; }
.slate { display: grid; gap: 2px; }
.chunk { display: flex; gap: 10px; align-items: flex-start; cursor: pointer; }
.chunk.done { opacity: .45; }
.chunk.done .chunk-title { text-decoration: line-through; }
.chunk input { margin-top: 3px; accent-color: #bfa05a; }
.chunk-body { display: flex; flex-direction: column; gap: 1px; }
.chunk-title { font-size: 14px; font-weight: 600; }
.chunk-meta { color: #7e8898; font-size: 11px; }
.chunk-why { font-size: 12px; }
.portrait {
  width: 38px; height: 38px; flex: none; display: flex; align-items: center; justify-content: center;
  font-family: Rajdhani, sans-serif; font-size: 19px; font-weight: 700; color: #bfa05a;
  background: #0d1117; border: 1px solid #232b38;
  clip-path: polygon(8px 0, 100% 0, 100% calc(100% - 8px), calc(100% - 8px) 100%, 0 100%, 0 8px);
}
.portrait.has-icon { position: relative; overflow: hidden; }
.portrait.has-icon img {
  position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover;
  background: #0d1117;
}
.vaulted { color: #7e8898; border: 1px solid #7e8898; padding: 0 6px; font-size: 10px; letter-spacing: .12em; }
.resurg { color: #bfa05a; border: 1px solid #bfa05a; padding: 0 6px; font-size: 10px; letter-spacing: .12em; }
.pips { display: inline-flex; gap: 5px; margin-left: auto; }
.pip { width: 10px; height: 10px; border-radius: 50%; border: 1px solid #7e8898; }
.pip.have { background: #bfa05a; border-color: #bfa05a; }
.pip.need { background: transparent; border-color: #c9d2de; }
.pip.unknown { background: transparent; border-style: dashed; }
.action { margin-top: 8px; padding: 7px 10px; background: #0d1420; border-left: 3px solid #aee3ef; font-size: 13px; color: #aee3ef; }
.action-parked { border-color: #3a4354; color: #7e8898; }
.action-resurgence { border-color: #bfa05a; color: #e8d9ae; }
.action-farm, .action-wait { border-color: #7e8898; color: #c9d2de; }
.chip-common, .chip-uncommon, .chip-rare {
  font-size: 11px; padding: 1px 8px; border: 1px solid;
  clip-path: polygon(5px 0, 100% 0, 100% calc(100% - 5px), calc(100% - 5px) 100%, 0 100%, 0 5px);
}
.chip-common { color: #b08d4f; border-color: #b08d4f; }
.chip-uncommon { color: #c0c8d4; border-color: #c0c8d4; }
.chip-rare { color: #e3c878; border-color: #e3c878; }
.nw-grid, .wk-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 6px; }
.nw, .wk {
  display: flex; flex-direction: column; gap: 1px; background: #131824; border: 1px solid #1f2735;
  padding: 7px 10px; font-size: 12px; cursor: pointer;
  clip-path: polygon(7px 0, 100% 0, 100% calc(100% - 7px), calc(100% - 7px) 100%, 0 100%, 0 7px);
}
.nw.done, .wk.done { opacity: .45; }
.nw.done .nw-title, .wk.done .wk-name { text-decoration: line-through; }
.nw input, .wk input, .bar input { accent-color: #bfa05a; }
.nw-meta, .wk-meta, .wk-obs { color: #7e8898; font-size: 11px; }
.wk-small { opacity: .8; font-size: 11px; }
.nw-season { color: #7e8898; font-size: 12px; margin-bottom: 6px; }
.caveat { color: #7e8898; font-size: 11px; margin-top: 5px; }
/* Routine "Today": compact pool chips + one sortie line */
.pools { display: flex; flex-wrap: wrap; gap: 5px; }
.pool {
  display: inline-flex; align-items: center; gap: 8px;
  background: #131824; border: 1px solid #1f2735; padding: 4px 9px;
  clip-path: polygon(6px 0, 100% 0, 100% calc(100% - 6px), calc(100% - 6px) 100%, 0 100%, 0 6px);
}
.pool .bar-name { font-family: Rajdhani, sans-serif; font-weight: 600; font-size: 12px; white-space: nowrap; }
.pool .bar-track { height: 5px; width: 72px; background: #0d1117; margin: 0; }
.pool .bar-fill { display: block; height: 5px; background: #bfa05a; }
.pool.slim .bar-name { font-size: 11px; font-weight: 500; }
.pool.slim .bar-track { width: 54px; }
.pool.bar-full { opacity: .45; }
.bar { cursor: pointer; }
.bar-name { font-family: Rajdhani, sans-serif; font-weight: 600; font-size: 13px; }
.bar-track { height: 5px; background: #0d1117; margin: 6px 0 3px; }
.bar-fill { height: 5px; background: #bfa05a; }
.bar-meta { color: #7e8898; font-size: 11px; }
.snap { color: #c9d2de; }
/* First Crack focus trio (≤3 parts, terse) + parent icons */
.focus { display: flex; gap: 6px; flex-wrap: wrap; margin-bottom: 6px; }
.focus-part { font-size: 12px; color: #bfa05a; border: 1px solid #bfa05a; padding: 2px 9px; display: inline-flex; align-items: center; gap: 6px; }
.focus-icon, .chunk-icon {
  width: 20px; height: 20px; flex: none; position: relative; overflow: hidden;
  display: inline-flex; align-items: center; justify-content: center;
  font-family: Rajdhani, sans-serif; font-size: 11px; font-weight: 700; color: #bfa05a;
  background: #0d1117; border: 1px solid #232b38;
}
.focus-icon img, .chunk-icon img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; background: #0d1117; }
.do-text .chunk-icon { width: 26px; height: 26px; font-size: 15px; border-color: #bfa05a; }
.chunk-head { display: inline-flex; align-items: center; gap: 8px; }
/* backlog compact lines */
.bline {
  display: flex; gap: 8px; align-items: center; font-size: 12px;
  background: #131824; border: 1px solid #1f2735; padding: 4px 9px;
}
.miniport {
  width: 22px; height: 22px; flex: none; display: inline-flex; align-items: center; justify-content: center;
  font-family: Rajdhani, sans-serif; font-size: 12px; font-weight: 700; color: #bfa05a;
  background: #0d1117; border: 1px solid #232b38; position: relative; overflow: hidden;
}
.miniport img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; background: #0d1117; }
.bkind { color: #7e8898; font-size: 10px; text-transform: uppercase; letter-spacing: .1em; margin-left: auto; }
/* backlog: parent groups with aligned part rows */
.backlog-head { color: #7e8898; font-size: 12px; margin: 2px 0 6px; }
.backlog { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 8px; }
.bgroup { margin: 0; }
.bhead { display: flex; gap: 8px; align-items: center; }
.bname { font-family: Rajdhani, sans-serif; font-size: 15px; font-weight: 700; }
.bcount { color: #7e8898; font-size: 11px; margin-left: auto; }
.brow {
  display: grid; grid-template-columns: minmax(88px, 1.1fr) minmax(140px, 1.9fr) 42px 42px;
  gap: 8px; align-items: baseline; padding: 4px 0; border-top: 1px solid #1c2330;
  font-size: 12px;
}
.bpart { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.brelic { color: #7e8898; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.brelic.is-vaulted { opacity: .7; }
.bstock, .bcracks { text-align: right; color: #c9d2de; font-variant-numeric: tabular-nums; }
.nonprime-h { font-size: 11px; letter-spacing: .18em; text-transform: uppercase; color: #7e8898; margin: 14px 0 6px; }
.nonprime { display: grid; gap: 5px; }
.npcat { display: flex; gap: 10px; font-size: 12px; background: #131824; border: 1px solid #1f2735; padding: 6px 10px; }
.npcat-name { color: #bfa05a; font-weight: 600; white-space: nowrap; }
.npcat-items { color: #7e8898; }
/* First Crack strips: Varzia + the event pack */
.resurg-first { border-color: #bfa05a; display: flex; flex-direction: column; gap: 2px; }
.resurg-line { font-family: Rajdhani, sans-serif; font-weight: 700; font-size: 15px; color: #e8d9ae; }
.resurg-sub { color: #7e8898; font-size: 12px; }
.tau { display: flex; flex-direction: column; gap: 1px; font-size: 13px; border-color: #bfa05a; }
.donow-sub { font-size: 11px; letter-spacing: .18em; text-transform: uppercase; color: #7e8898; margin: 14px 0 5px; font-weight: 600; }
.nw-title { font-weight: 600; }
.nw-desc { font-size: 12px; }
.farm-title { font-family: Rajdhani, sans-serif; font-weight: 600; font-size: 14px; }
.farm-sub { color: #7e8898; font-size: 11px; }
/* v2 tabs: First Crack / Hunt / Backlog / Routine (same palette) */
.tabs { margin-top: 6px; }
.tabs input[name=tab] { display: none; }
.tab {
  display: inline-block; padding: 6px 16px; margin-right: 5px; cursor: pointer;
  font-family: Rajdhani, sans-serif; font-weight: 700; font-size: 13px;
  letter-spacing: .12em; text-transform: uppercase; color: #7e8898;
  background: #131824; border: 1px solid #1f2735;
  clip-path: polygon(7px 0, 100% 0, 100% calc(100% - 7px), calc(100% - 7px) 100%, 0 100%, 0 7px);
}
.tabpage { display: none; }
#tab-donow:checked ~ #page-donow { display: block; }
#tab-hunt:checked ~ #page-hunt { display: block; }
#tab-backlog:checked ~ #page-backlog { display: block; }
#tab-routine:checked ~ #page-routine { display: block; }
#tab-donow:checked ~ label[for=tab-donow] { color: #aee3ef; border-color: #aee3ef; }
#tab-hunt:checked ~ label[for=tab-hunt] { color: #aee3ef; border-color: #aee3ef; }
#tab-backlog:checked ~ label[for=tab-backlog] { color: #aee3ef; border-color: #aee3ef; }
#tab-routine:checked ~ label[for=tab-routine] { color: #aee3ef; border-color: #aee3ef; }
"#;

const JS: &str = r#"
// Checkbox ticks: the resident's loopback HTTP server is the store of
// record (POST /tick, keyed by item id + in-game day) and re-applies the
// ticks it injects onto <body data-fracker-ticks> (base64 JSON) on every
// serve. localStorage remains only a progressive cache for a file://
// page. The 60s meta-refresh reloads the served page, so ticks survive.
(function () {
  var day = document.body.getAttribute('data-day') || '';
  var served = document.body.hasAttribute('data-fracker-ticks');
  var done = {};
  try {
    var raw = document.body.getAttribute('data-fracker-ticks');
    if (raw) {
      var store = JSON.parse(atob(raw)).days || {};
      (store[day] || []).forEach(function (i) { done[i] = true; });
    }
  } catch (e) {}
  function key(id) { return 'fracker:' + day + ':' + id; }
  function host(box) { return box.closest('.chunk,.nw,.wk,.bar,.card'); }
  function paint(box, on) {
    var h = host(box);
    if (h) { h.classList.toggle('done', on); }
    if (box.checked !== on) { box.checked = on; }
  }
  document.querySelectorAll('input[type=checkbox][data-id]').forEach(function (box) {
    var id = box.getAttribute('data-id');
    var on = !!done[id];
    // localStorage is only a fallback for a file:// page: when the
    // resident serves the page, its tick store is authoritative.
    if (!served) {
      try { if (localStorage.getItem(key(id)) === '1') { on = true; } } catch (e) {}
    }
    if (on) { paint(box, true); }
    box.addEventListener('change', function () {
      var checked = box.checked;
      paint(box, checked);
      try {
        if (checked) { localStorage.setItem(key(id), '1'); }
        else { localStorage.removeItem(key(id)); }
      } catch (e) {}
      try {
        fetch('/tick', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ id: id, day: day, done: checked })
        });
      } catch (e) {}
    });
  });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_covers_settled_sections_and_dark_rules() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let plan = Plan::default();
        // Lifetimes: build ctx inline to avoid borrow tangle.
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = RenderContext {
            world: &world,
            generated_label: "x".to_owned(),
            targets_status: "seed".to_owned(),
            world_status: "fixture".to_owned(),
            weekly_done: &weekly,
            proven_done: &proven,
            mastery_rank: None,
            icons: &icons,
            embedded_icons: None,
            backlog_categories: &cats,
            daily: None,
            pack: None,
            tuning: &t,
        };
        let html = render_plan(&plan, &ctx);
        for section in [
            "First crack", "Hunt", "Backlog", "Routine", "Today", "This week",
            "Nightwave",
        ] {
            assert!(html.contains(section), "missing section: {section}");
        }
        // Pass 10 IA: the routine surface is NOT on the action-only tab.
        assert!(
            !html.contains("<h2>Today</h2>"),
            "standing/sortie are not their own always-on section"
        );
        assert!(!html.contains("Do now"), "tab renamed to First Crack (sixth pass §2)");
        assert!(!html.contains("Do Now"), "no user-visible Do Now copy");
        assert!(html.contains("color-scheme: dark"), "refresh-flash rule");
        assert!(html.contains("http-equiv=\"refresh\""), "meta refresh");
        assert!(html.contains("localStorage"), "checkbox persistence");
        assert!(
            html.contains("data-day=\""),
            "the in-game day is on the body for tick keying (§2)"
        );
        assert!(
            html.contains("fetch('/tick'"),
            "checkbox changes POST to the loopback server (§2)"
        );
        assert!(html.contains("Rajdhani"), "game-adjacent type");
        assert!(html.contains("clip-path"), "chamfered cards");
        // v2 tabs: empty plan has no loud line; all four tabs render.
        assert!(!html.contains("card donow"), "no loud line without tier-1 chunks");
        assert!(
            html.contains("tab-donow")
                && html.contains("tab-hunt")
                && html.contains("tab-backlog")
                && html.contains("tab-routine"),
            "four tabs"
        );
    }

    #[test]
    fn backlog_group_shows_parent_icon_not_part_icon() {
        use crate::plan::{GoalCard, GoalAction, ActionKind, PartRow, PartState, Rarity};
        let t = crate::tuning::Tuning::for_tests();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> =
            [("Ivara Prime".to_owned(), "IvaraPrime.png".to_owned())]
                .into_iter()
                .collect();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let world = WorldState::default();
        let ctx = RenderContext {
            world: &world,
            generated_label: "x".to_owned(),
            targets_status: "t".to_owned(),
            world_status: "w".to_owned(),
            weekly_done: &weekly,
            proven_done: &proven,
            mastery_rank: None,
            icons: &icons,
            embedded_icons: None,
            backlog_categories: &cats,
            daily: None,
            pack: None,
            tuning: &t,
        };
        let plan = Plan {
            goals: vec![GoalCard {
                gear: "Ivara Prime".to_owned(),
                is_warframe: true,
                vaulted: true,
                resurgence_live: true,
                wants: vec!["Ivara Prime Chassis Blueprint".to_owned()],
                parts_have: 0,
                parts_total: 4,
                parts: vec![PartRow {
                    name: "Ivara Prime Chassis Blueprint".to_owned(),
                    state: PartState::Need,
                    relics: vec![],
                    best_odds: String::new(),
                    rarity: Rarity::Rare,
                    deal: None,
                }],
                action: GoalAction {
                    text: "x".to_owned(),
                    kind: ActionKind::Resurgence,
                    relic: None,
                    score: 0.0,
                },
            }],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        assert!(
            html.contains("https://cdn.warframestat.us/img/IvaraPrime.png"),
            "parent icon URL"
        );
        assert!(!html.contains("IvaraPrimeChassis"), "no part icon");
    }

    #[test]
    fn first_crack_renders_parent_icons_for_parts_and_chunks() {
        // Sixth pass §2: the First Crack tab shows parent-item icons on
        // its focus parts AND its chunk rows (addendum #14/#31).
        use crate::plan::{Chunk, FocusPart};
        let t = crate::tuning::Tuning::for_tests();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> =
            [("Ivara Prime".to_owned(), "IvaraPrime.png".to_owned())]
                .into_iter()
                .collect();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let world = WorldState::default();
        let ctx = RenderContext {
            world: &world,
            generated_label: "x".to_owned(),
            targets_status: "t".to_owned(),
            world_status: "w".to_owned(),
            weekly_done: &weekly,
            proven_done: &proven,
            mastery_rank: None,
            icons: &icons,
            embedded_icons: None,
            backlog_categories: &cats,
            daily: None,
            pack: None,
            tuning: &t,
        };
        let plan = Plan {
            focus: vec![FocusPart {
                part: "Ivara Prime Chassis Blueprint".to_owned(),
                gear: "Ivara Prime".to_owned(),
                era: "Neo".to_owned(),
            }],
            chunks: vec![Chunk {
                id: "t1-crack-neo-c5".to_owned(),
                tier: 1,
                title: "Crack 4 Ivory Prime Chassis at Hepit (Void).".to_owned(),
                minutes_label: "~3m".to_owned(),
                why: "w".to_owned(),
                kind: "expiring crack".to_owned(),
                expiry: None,
                gear: Some("Ivara Prime".to_owned()),
            }],
            ..Default::default()
        };
        // A goal must exist or the focus/chunk lookups would be empty
        // strings anyway; the plan above is enough for rendering.
        let html = render_plan(&plan, &ctx);
        let url = "https://cdn.warframestat.us/img/IvaraPrime.png";
        assert!(
            html.contains(&format!("<span class=\"focus-icon\"><img src=\"{url}\"")),
            "focus part renders the parent icon"
        );
        assert!(
            html.contains(&format!("<span class=\"chunk-icon\"><img src=\"{url}\"")),
            "chunk row renders the parent icon"
        );
    }

    #[test]
    fn base64_encoder_matches_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        // Binary-transparent (PNG magic bytes).
        assert_eq!(
            base64_encode(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
            "iVBORw0KGgo="
        );
    }

    #[test]
    fn least_important_weeklies_render_last() {
        // Flagged in data, not code: the last board entries carry the
        // deprioritized flag and the quiet style. Pass-4 slimming: 16
        // tracked (acts own their section; dropped rows are gone).
        let t = crate::tuning::Tuning::for_tests();
        let board = weekly_board(&t);
        assert_eq!(board.len(), 16);
        let tail = &board[board.len() - 2..];
        assert!(tail.iter().all(|w| w.deprioritized));
        assert!(board[..board.len() - 2].iter().all(|w| !w.deprioritized));
    }

    #[test]
    fn embedded_icons_replace_hotlinks() {
        use crate::plan::{GoalCard, GoalAction, ActionKind, PartRow, PartState, Rarity};
        let t = crate::tuning::Tuning::for_tests();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> =
            [("Ivara Prime".to_owned(), "IvaraPrime.png".to_owned())]
                .into_iter()
                .collect();
        let embedded: HashMap<String, String> = [(
            "Ivara Prime".to_owned(),
            "data:image/png;base64,iVBORw0KGgo=".to_owned(),
        )]
        .into_iter()
        .collect();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let world = WorldState::default();
        fn mk<'a>(
            world: &'a WorldState,
            weekly: &'a HashMap<String, String>,
            proven: &'a HashSet<String>,
            icons: &'a HashMap<String, String>,
            embedded: Option<&'a HashMap<String, String>>,
            cats: &'a HashMap<String, Vec<String>>,
            t: &'a crate::tuning::Tuning,
        ) -> RenderContext<'a> {
            RenderContext {
                world,
                generated_label: "x".to_owned(),
                targets_status: "t".to_owned(),
                world_status: "w".to_owned(),
                weekly_done: weekly,
                proven_done: proven,
                mastery_rank: None,
                icons,
                embedded_icons: embedded,
                backlog_categories: cats,
                daily: None,
                pack: None,
                tuning: t,
            }
        }
        let plan = Plan {
            goals: vec![GoalCard {
                gear: "Ivara Prime".to_owned(),
                is_warframe: true,
                vaulted: true,
                resurgence_live: false,
                wants: vec![],
                parts_have: 0,
                parts_total: 0,
                parts: vec![PartRow {
                    name: "Ivara Prime Chassis Blueprint".to_owned(),
                    state: PartState::Need,
                    relics: vec![],
                    best_odds: String::new(),
                    rarity: Rarity::Rare,
                    deal: None,
                }],
                action: GoalAction {
                    text: "x".to_owned(),
                    kind: ActionKind::Parked,
                    relic: None,
                    score: 0.0,
                },
            }],
            ..Default::default()
        };
        let hotlinked = render_plan(&plan, &mk(&world, &weekly, &proven, &icons, None, &cats, &t));
        assert!(
            hotlinked.contains("https://cdn.warframestat.us/img/IvaraPrime.png"),
            "product page keeps hotlinks"
        );
        let embedded_html = render_plan(
            &plan,
            &mk(&world, &weekly, &proven, &icons, Some(&embedded), &cats, &t),
        );
        assert!(
            embedded_html.contains("data:image/png;base64,"),
            "demo embeds data URIs"
        );
        assert!(
            !embedded_html.contains("https://cdn.warframestat.us/img/IvaraPrime.png"),
            "demo fetches no hotlinks for embedded gears"
        );
    }

    /// Minimal context for section-visibility tests (world + plan
    /// supplied per case; everything else empty).
    fn section_ctx<'a>(
        world: &'a WorldState,
        t: &'a crate::tuning::Tuning,
        weekly: &'a HashMap<String, String>,
        proven: &'a HashSet<String>,
        icons: &'a HashMap<String, String>,
        cats: &'a HashMap<String, Vec<String>>,
        daily: Option<DailyRender>,
    ) -> RenderContext<'a> {
        RenderContext {
            world,
            generated_label: "x".to_owned(),
            targets_status: "t".to_owned(),
            world_status: "w".to_owned(),
            weekly_done: weekly,
            proven_done: proven,
            mastery_rank: Some(30),
            icons,
            embedded_icons: None,
            backlog_categories: cats,
            daily,
            pack: None,
            tuning: t,
        }
    }

    #[test]
    fn donow_alert_sections_hide_when_empty() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = section_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let html = render_plan(&Plan::default(), &ctx);
        assert!(
            !html.contains(&t.planner.alert_sections.event_header),
            "event section hides with no live alerts"
        );
        assert!(
            !html.contains(&t.planner.alert_sections.lotus_header),
            "gift section hides with no live alerts"
        );
        assert!(!html.contains("resurg-first\">"), "no rotation element without coverage");
        assert!(!html.contains("card vendor\">"), "no trader section without a live window");
    }

    #[test]
    fn donow_alert_sections_render_split_with_countdowns() {
        use crate::plan::{AlertView, BaroView, ResurgenceView};
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = section_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let plan = Plan {
            event_alerts: vec![AlertView {
                node: "Nimus (Eris)".to_owned(),
                mission_type: "Survival".to_owned(),
                payout: "50× Road To Tau Event Resource".to_owned(),
                left: "25m".to_owned(),
                done: true,
            }],
            lotus_alerts: vec![AlertView {
                node: "Larunda (Mercury)".to_owned(),
                mission_type: "Capture".to_owned(),
                payout: "Orokin Catalyst".to_owned(),
                left: "1h05m".to_owned(),
                done: false,
            }],
            resurgence: Some(ResurgenceView {
                contact: "Varzia".to_owned(),
                location: "Maroo's Bazaar".to_owned(),
                ends_in: "in 13d".to_owned(),
                gears: vec!["Ivara Prime".to_owned()],
                currency: "Aya".to_owned(),
            }),
            baro: Some(BaroView {
                vendor: "Baro".to_owned(),
                location: "Orcus Relay (Pluto)".to_owned(),
                ends_in: "in 20h".to_owned(),
            }),
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        // Separate sections, each with node + payout + countdown.
        assert!(html.contains(&t.planner.alert_sections.event_header));
        assert!(html.contains(&t.planner.alert_sections.lotus_header));
        assert!(html.contains("Nimus (Eris)"));
        assert!(html.contains("50× Road To Tau Event Resource"));
        assert!(html.contains("25m left"));
        assert!(html.contains("Larunda (Mercury)"));
        // Ninth pass §4: a log-proven alert renders struck with the proof.
        assert!(html.contains("class=\"card exp evt done\""));
        assert!(html.contains("log &check;"));
        let evt_pos = html.find("Nimus (Eris)").unwrap();
        let lotus_pos = html.find("Larunda (Mercury)").unwrap();
        assert!(evt_pos < lotus_pos, "events render before gifts");
        // First-class rotation element names covered gear + window.
        assert!(html.contains("resurg-first"));
        assert!(html.contains("Ivara Prime"));
        assert!(html.contains("in 13d"));
        // Pass 10 IA: the trader is a slim header status pill now (no
        // separate always-on section), still carrying location + countdown.
        assert!(html.contains("pill-baro"));
        assert!(html.contains("Orcus Relay (Pluto)"));
        assert!(html.contains("in 20h"));
    }

    #[test]
    fn nightwave_acts_render_name_plus_description() {
        use crate::model::{NightWave, NightWaveChallenge};
        let t = crate::tuning::Tuning::for_tests();
        let mut world = WorldState::default();
        world.nightwave = Some(NightWave {
            season: 18,
            active_challenges: vec![NightWaveChallenge {
                id: "a1".to_owned(),
                title: "Accelerator".to_owned(),
                desc: "Kill 10 Enemies while Sliding".to_owned(),
                standing: 1000,
                is_daily: true,
                is_elite: false,
            }],
        });
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = section_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let html = render_plan(&Plan::default(), &ctx);
        assert!(html.contains(&t.planner.nightwave.name));
        assert!(html.contains("Accelerator"));
        assert!(
            html.contains("Kill 10 Enemies while Sliding"),
            "description renders with the name"
        );
    }

    #[test]
    fn today_renders_one_slim_bar_per_pool() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        // Untouched pools read full caps; one pool partly used; a PvP
        // value must never become a bar.
        let families: Vec<(String, u32)> = t
            .planner
            .daily_caps
            .families
            .iter()
            .map(|f| {
                let left = if f == "Cetus" { 6_300 } else { 31_000 };
                (f.clone(), left)
            })
            .chain(std::iter::once(("Pvp".to_owned(), 0)))
            .collect();
        let daily = DailyRender {
            affiliation_remaining: 0,
            affiliation_cap: 31_000,
            focus_remaining: 214_545,
            focus_cap: 400_000,
            families,
        };
        let ctx = section_ctx(&world, &t, &weekly, &proven, &icons, &cats, Some(daily));
        let html = render_plan(&Plan::default(), &ctx);
        // Focus + main standing + one slim chip per family (12), all
        // under the Routine tab's compact "Today" strip.
        assert_eq!(html.matches("pool slim").count(), 12);
        assert!(html.contains("Focus · 214,545 / 400,000 left"));
        // Labels come from the data-owned map: main pool is "Syndicates",
        // not the raw bare "Standing"; Cetus renders as its player-facing
        // name "Ostron".
        assert!(html.contains("Syndicates · 0 / 31,000 left"));
        assert!(html.contains("Ostron · 6,300 / 31,000 left"));
        assert!(html.contains("Cavia · 31,000 / 31,000 left"));
        assert!(!html.contains("Pvp"), "PvP leaves with the Conclave cut");
        // Full pools dim; the used pool does not.
        assert!(html.contains("bar-full"), "untouched pools dim");
        let cetus_bar = html.find("Ostron · 6,300").unwrap();
        let window = &html[cetus_bar.saturating_sub(60)..cetus_bar];
        assert!(!window.contains("bar-full"), "used pool stays loud");
    }

    #[test]
    fn demo_export_carries_zero_external_references() {
        // Addendum #23: the review surface fetches nothing — no icon
        // hotlinks, no font links. Strict mode: gears missing from the
        // embed map keep the monogram, never a hotlink.
        use crate::plan::{GoalCard, GoalAction, ActionKind, PartRow, PartState, Rarity};
        let t = crate::tuning::Tuning::for_tests();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = [
            ("Ivara Prime".to_owned(), "IvaraPrime.png".to_owned()),
            ("Baza Prime".to_owned(), "BazaPrime.png".to_owned()),
        ]
        .into_iter()
        .collect();
        let embedded: HashMap<String, String> = [(
            "Ivara Prime".to_owned(),
            "data:image/png;base64,iVBORw0KGgo=".to_owned(),
        )]
        .into_iter()
        .collect();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let world = WorldState::default();
        let ctx = RenderContext {
            world: &world,
            generated_label: "x".to_owned(),
            targets_status: "t".to_owned(),
            world_status: "w".to_owned(),
            weekly_done: &weekly,
            proven_done: &proven,
            mastery_rank: None,
            icons: &icons,
            embedded_icons: Some(&embedded),
            backlog_categories: &cats,
            daily: None,
            pack: None,
            tuning: &t,
        };
        // Ivara embeds; Baza resolves in the icon map but missed the
        // embed download — strict mode keeps its monogram, never a
        // hotlink.
        let mk_goal = |gear: &str, part: &str| GoalCard {
            gear: gear.to_owned(),
            is_warframe: false,
            vaulted: true,
            resurgence_live: false,
            wants: vec![],
            parts_have: 0,
            parts_total: 0,
            parts: vec![PartRow {
                name: part.to_owned(),
                state: PartState::Need,
                relics: vec![],
                best_odds: String::new(),
                rarity: Rarity::Rare,
                deal: None,
            }],
            action: GoalAction {
                text: "x".to_owned(),
                kind: ActionKind::Parked,
                relic: None,
                score: 0.0,
            },
        };
        let plan = Plan {
            goals: vec![
                mk_goal("Ivara Prime", "Ivara Prime Chassis Blueprint"),
                mk_goal("Baza Prime", "Baza Prime Blueprint"),
            ],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        assert!(!html.contains("https://"), "demo fetches nothing external");
        assert!(html.contains("data:image/png;base64,"), "embedded icons stay");
    }

    /// The HTML slice for one `<div class="tabpage" id="page-X">` block
    /// (from the id to the next tabpage or, for the last tab, the script).
    fn tabpage<'a>(html: &'a str, id: &str) -> &'a str {
        let needle = format!("id=\"page-{id}\"");
        let start = html
            .find(&needle)
            .unwrap_or_else(|| panic!("missing {needle}"));
        let rest = &html[start..];
        let end = rest[1..]
            .find("class=\"tabpage\"")
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        &rest[..end]
    }

    /// Pass 10 IA: four tabs, in order — First Crack / Hunt / Backlog /
    /// Routine.
    #[test]
    fn four_tabs_render_in_order() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = section_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let html = render_plan(&Plan::default(), &ctx);
        let mut last = 0usize;
        for label in ["First Crack", "Hunt", "Backlog", "Routine"] {
            let needle = format!(">{label}</label>");
            let pos = html
                .find(&needle)
                .unwrap_or_else(|| panic!("missing tab {label}"));
            assert!(pos >= last, "tab {label} renders out of order");
            last = pos;
        }
    }

    /// Pass 10 IA: the standing/focus surface and the weeklies board live
    /// under Routine and NOT under First Crack (the action-only tab).
    #[test]
    fn routine_owns_standing_and_weeklies_not_first_crack() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let daily = DailyRender {
            affiliation_remaining: 0,
            affiliation_cap: 31_000,
            focus_remaining: 214_545,
            focus_cap: 400_000,
            families: vec![("Cetus".to_owned(), 6_300)],
        };
        let ctx = section_ctx(&world, &t, &weekly, &proven, &icons, &cats, Some(daily));
        let html = render_plan(&Plan::default(), &ctx);
        let donow = tabpage(&html, "donow");
        let routine = tabpage(&html, "routine");

        // Under Routine: standing chips, the sortie line with its reset,
        // and the weeklies board with stable `wk-*` tick keys.
        assert!(routine.contains("Syndicates · 0 / 31,000 left"), "standing under Routine");
        assert!(routine.contains("Focus · 214,545 / 400,000 left"), "focus under Routine");
        assert!(routine.contains("Ostron · 6,300 / 31,000 left"), "family chip under Routine");
        assert!(routine.contains("sortie-daily"), "sortie tick key under Routine");
        assert!(routine.contains(&t.planner.sortie.reset_label), "sortie reset time under Routine");
        assert!(routine.contains("this week") || routine.contains("This week"), "weeklies header under Routine");
        assert!(routine.contains("Archon Hunt"), "weeklies board under Routine");
        assert!(routine.contains("wk-archon"), "weekly tick key under Routine");

        // NOT under First Crack.
        assert!(!donow.contains("Syndicates ·"), "no standing on First Crack");
        assert!(!donow.contains("Focus ·"), "no focus bar on First Crack");
        assert!(!donow.contains("Archon Hunt"), "no weeklies on First Crack");
        assert!(!donow.contains("wk-archon"), "no weekly tick keys on First Crack");
        assert!(!donow.contains("sortie-daily"), "no sortie on First Crack");
        assert!(!donow.contains("This week"), "no weeklies header on First Crack");
    }
}
