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

fn world_age(world: &WorldState) -> String {
    if world.timestamp.is_empty() {
        return "unknown age".to_owned();
    }
    format!("as of {}", esc(&world.timestamp))
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
    h.push_str("<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n");
    h.push_str("<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?family=Rajdhani:wght@500;600;700&display=swap\">\n");
    h.push_str("<style>\n");
    h.push_str(CSS);
    h.push_str("\n</style>\n</head>\n<body>\n<div class=\"wrap\">\n");

    // ---- header ----
    h.push_str("<header class=\"masthead\">\n");
    h.push_str("<div class=\"brand\">FRACKER</div>\n");
    h.push_str(&format!(
        "<div class=\"stamp\">plan generated {} &middot; world state {} &middot; targets: {}</div>\n",
        esc(&ctx.generated_label),
        esc(&world_age(ctx.world)),
        esc(&ctx.targets_status),
    ));
    h.push_str("</header>\n");

    // ---- tabs: Do Now (≤3-part slate) / Hunt (all primes) / Backlog (all gear) ----
    h.push_str("<div class=\"tabs\">\n");
    h.push_str("<input type=\"radio\" name=\"tab\" id=\"tab-donow\" checked><label class=\"tab\" for=\"tab-donow\">Do Now</label>\n");
    h.push_str("<input type=\"radio\" name=\"tab\" id=\"tab-hunt\"><label class=\"tab\" for=\"tab-hunt\">Hunt</label>\n");
    h.push_str("<input type=\"radio\" name=\"tab\" id=\"tab-backlog\"><label class=\"tab\" for=\"tab-backlog\">Backlog</label>\n");

    // ---- tab 1: do now ----
    // Exactly ONE loud element (the current instruction); everything
    // else on the page stays quiet. Crack chunks pursue only the
    // ≤3 focus parts.
    h.push_str("<div class=\"tabpage\" id=\"page-donow\">\n<section><h2>Do now</h2>\n");
    if !plan.focus.is_empty() {
        h.push_str("<div class=\"focus\">\n");
        for f in &plan.focus {
            h.push_str(&format!(
                "<span class=\"focus-part\">{} · {} · {}</span>\n",
                esc(&f.part),
                esc(&f.gear),
                esc(&f.era),
            ));
        }
        h.push_str("</div>\n");
    }
    let top: Vec<&crate::plan::Chunk> =
        plan.chunks.iter().filter(|c| c.tier == 1).collect();
    if top.is_empty() {
        h.push_str("<div class=\"card quiet\">Nothing expiring.</div>\n");
    } else {
        let first = top[0];
        h.push_str(&format!(
            "<div class=\"card donow\"><div class=\"do-text\">{}</div>\n<div class=\"do-sub\">{} &middot; {}</div></div>\n",
            esc(&first.title),
            esc(&first.minutes_label),
            esc(&first.why),
        ));
        if top.len() > 1 {
            h.push_str("<div class=\"expiring\">\n");
            for c in &top[1..] {
                h.push_str(&format!(
                    "<div class=\"card exp\"><span class=\"exp-title\">{}</span>\n<span class=\"exp-sub\">{} &middot; {}</span></div>\n",
                    esc(&c.title),
                    esc(&c.minutes_label),
                    esc(&c.why),
                ));
            }
            h.push_str("</div>\n");
        }
    }
    // slate chunks (quiet rows)
    let slate: Vec<&crate::plan::Chunk> =
        plan.chunks.iter().filter(|c| c.tier == 2 || c.tier == 3).collect();
    if slate.is_empty() {
        h.push_str("<div class=\"card quiet\">No live cracks.</div>\n");
    } else {
        h.push_str("<div class=\"slate\">\n");
        for c in &slate {
            let proven = ctx.proven_done.contains(&c.id);
            h.push_str(&format!(
                "<label class=\"card chunk{}\"><input type=\"checkbox\" data-id=\"{}\"{}>\n<span class=\"chunk-body\"><span class=\"chunk-title\">{}</span>\n<span class=\"chunk-meta\">{} &middot; {}</span>\n<span class=\"chunk-why\">{}</span></span></label>\n",
                if proven { " done" } else { "" },
                esc(&c.id),
                if proven { " checked" } else { "" },
                esc(&c.title),
                esc(&c.minutes_label),
                esc(&c.kind),
                esc(&c.why),
            ));
        }
        h.push_str("</div>\n");
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
    h.push_str("</section>\n</div>\n</div>\n");

    // ---- Event pack strip (content all from the pack TOML) ----
    if let Some(pack) = &ctx.pack {
        if pack.live {
            h.push_str(&format!("<section><h2>{}</h2>\n", esc(&titlecase(&pack.pack_name))));
            let health = match pack.contegorus_health {
                Some(v) => format!("{} {:.2}", pack.event_label, v),
                None => format!("{} —", pack.event_label),
            };
            h.push_str(&format!(
                "<div class=\"card tau\"><span class=\"tau-line\">{} · ends {}</span><span class=\"tau-line\">{} · {}</span><span class=\"tau-line\">{}</span></div>\n",
                esc(&pack.payout_label),
                esc(&pack.expires_label),
                esc(&pack.gated_label),
                esc(&health),
                esc(&pack.arcane_line),
            ));
            h.push_str("</section>\n");
        }
    }

    // ---- farms ----
    h.push_str("<section><h2>Farms</h2>\n");
    if plan.farms.is_empty() {
        h.push_str("<div class=\"card quiet\">No farms — vaulted wants are crack-or-park.</div>\n");
    } else {
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
    for n in &plan.notes {
        h.push_str(&format!("<div class=\"card quiet\">{}</div>\n", esc(n)));
    }
    h.push_str("</section>\n");

    // ---- 4. season acts (display name from the data pack) ----
    let season_title = ctx
        .tuning
        .weeklies
        .iter()
        .find(|w| w.id == "nw-weekly")
        .map(|w| w.name.as_str())
        .unwrap_or("Season acts");
    h.push_str(&format!("<section><h2>{}</h2>\n", esc(season_title)));
    match &ctx.world.nightwave {
        Some(nw) if !nw.active_challenges.is_empty() => {
            h.push_str(&format!("<div class=\"nw-season\">Season {} — acts, never a standing total.</div>\n", nw.season));
            h.push_str("<div class=\"nw-grid\">\n");
            for a in &nw.active_challenges {
                let tag = if a.is_daily { "daily" } else if a.is_elite { "elite" } else { "weekly" };
                let done = ctx.proven_done.contains(&a.id);
                h.push_str(&format!(
                    "<label class=\"nw{item_class}\"><input type=\"checkbox\" data-id=\"nw-{id}\"{checked}>\n<span class=\"nw-title\">{title}</span>\n<span class=\"nw-meta\">{tag} &middot; {standing} standing</span></label>\n",
                    item_class = if done { " done" } else { "" },
                    id = esc(&a.id),
                    checked = if done { " checked" } else { "" },
                    title = esc(&a.title),
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
    h.push_str("</section>\n");

    // ---- 5. this week ----
    h.push_str("<section><h2>This week</h2>\n<div class=\"wk-grid\">\n");
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
    h.push_str("</div>\n</section>\n");

    // ---- 6. today's bars (profile REMAINING values, never estimated) ----
    h.push_str("<section><h2>Today</h2>\n<div class=\"bars\">\n");
    match &ctx.daily {
        Some(d) => {
            let focus_pct = (100.0 * (1.0 - d.focus_remaining as f64 / d.focus_cap as f64))
                .clamp(0.0, 100.0);
            let aff_pct = (100.0
                * (1.0 - d.affiliation_remaining as f64 / d.affiliation_cap as f64))
            .clamp(0.0, 100.0);
            h.push_str(&format!(
                "<div class=\"card bar\"><div class=\"bar-name\">Focus · {} / {} left</div>\n<div class=\"bar-track\"><div class=\"bar-fill\" style=\"width:{:.0}%\"></div></div></div>\n",
                fmt_int(d.focus_remaining),
                fmt_int(d.focus_cap),
                focus_pct,
            ));
            h.push_str(&format!(
                "<div class=\"card bar\"><div class=\"bar-name\">Standing · {} / {} left</div>\n<div class=\"bar-track\"><div class=\"bar-fill\" style=\"width:{:.0}%\"></div></div></div>\n",
                fmt_int(d.affiliation_remaining),
                fmt_int(d.affiliation_cap),
                aff_pct,
            ));
            if !d.families.is_empty() {
                let fams: Vec<String> = d
                    .families
                    .iter()
                    .map(|(k, v)| format!("{} {}", short_family(k), fmt_int(*v)))
                    .collect();
                h.push_str(&format!(
                    "<div class=\"card quiet fam\">{}</div>\n",
                    esc(&fams.join(" · ")),
                ));
            }
        }
        None => {
            let caps = &ctx.tuning.planner.daily_caps;
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
    let sortie = &ctx.tuning.planner.sortie;
    let sortie_done = ctx.weekly_done.contains_key(&sortie.ledger_id);
    h.push_str(&format!(
        "<label class=\"card bar\"><input type=\"checkbox\" data-id=\"{id}\"{checked}>\n<span class=\"bar-name\">{name}</span>\n<span class=\"bar-meta\">{reset}{proven}</span></label>\n",
        id = esc(&sortie.ledger_id),
        checked = if sortie_done { " checked" } else { "" },
        name = esc(&sortie.name),
        reset = esc(&sortie.reset_label),
        proven = if sortie_done { " · log" } else { "" },
    ));
    h.push_str("</div>\n</section>\n");

    // ---- footer: targets snapshot ----
    let total_wants: usize = plan.goals.iter().map(|g| g.wants.len()).sum();
    let live_resurg = plan.goals.iter().filter(|g| g.resurgence_live).count();
    h.push_str("<footer><section><h2>Targets</h2>\n");
    h.push_str(&format!(
        "<div class=\"card snap\">{} goals · {} parts · {} {}-live</div>\n",
        plan.goals.len(),
        total_wants,
        live_resurg,
        ctx.tuning.planner.resurgence.contact,
    ));
    h.push_str("</section></footer>\n");

    // checkbox persistence: localStorage keyed by item id + in-game date.
    h.push_str("<script>\n");
    h.push_str(JS);
    h.push_str("\n</script>\n</div>\n</body>\n</html>\n");
    h
}

/// Icon `src` for one parent gear: the embedded data-URI thumbnail
/// when demo mode supplied one, else the CDN hotlink (product page).
/// A prime part always shows its PARENT item's icon, never a generic
/// part icon. Unresolved items keep the monogram (None).
fn gear_icon_src(gear: &str, ctx: &RenderContext<'_>) -> Option<String> {
    if let Some(embedded) = ctx.embedded_icons {
        if let Some(uri) = embedded.get(gear) {
            return Some(uri.clone());
        }
    }
    crate::model::icon_url(gear, ctx.icons)
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

/// "DailyAffiliationCavia" -> "Cavia".
fn short_family(key: &str) -> &str {
    key.strip_prefix("DailyAffiliation").unwrap_or(key)
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
  line-height: 1.45;
}
.wrap { max-width: 1080px; margin: 0 auto; padding: 28px 22px 60px; }
h1, h2, h3, .brand, .do-text, .chunk-title, .goal-name, .farm-title {
  font-family: Rajdhani, -apple-system, "Segoe UI", Roboto, sans-serif;
}
h2 {
  font-size: 12px; letter-spacing: .18em; text-transform: uppercase;
  color: #7e8898; margin: 34px 0 12px; font-weight: 600;
}
section { margin-bottom: 6px; }
.masthead { display: flex; align-items: baseline; gap: 16px; flex-wrap: wrap; }
.brand { font-size: 26px; font-weight: 700; letter-spacing: .3em; color: #bfa05a; }
.stamp { color: #7e8898; font-size: 13px; }
.card {
  background: #151a23; border: 1px solid #232b38; border-radius: 0;
  clip-path: polygon(10px 0, 100% 0, 100% calc(100% - 10px), calc(100% - 10px) 100%, 0 100%, 0 10px);
  padding: 20px 22px; margin: 10px 0;
}
.quiet { color: #7e8898; }
.donow { border-color: #bfa05a; }
.do-text { font-size: 31px; font-weight: 700; color: #aee3ef; line-height: 1.2; }
.do-sub { color: #7e8898; margin-top: 6px; }
.expiring { display: grid; gap: 2px; margin-top: 8px; }
.exp { padding: 12px 16px; }
.exp-title { font-family: Rajdhani, sans-serif; font-weight: 600; font-size: 16px; }
.exp-sub { color: #7e8898; font-size: 13px; display: block; margin-top: 2px; }
.slate { display: grid; gap: 2px; }
.chunk { display: flex; gap: 12px; align-items: flex-start; cursor: pointer; }
.chunk.done { opacity: .45; }
.chunk.done .chunk-title { text-decoration: line-through; }
.chunk input { margin-top: 5px; accent-color: #bfa05a; }
.chunk-body { display: flex; flex-direction: column; gap: 2px; }
.chunk-title { font-size: 19px; font-weight: 600; }
.chunk-meta { color: #7e8898; font-size: 13px; }
.chunk-why { font-size: 14px; }
.goals { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 12px; margin-top: 12px; }
.goal-head { display: flex; gap: 12px; align-items: center; }
.portrait {
  width: 46px; height: 46px; flex: none; display: flex; align-items: center; justify-content: center;
  font-family: Rajdhani, sans-serif; font-size: 24px; font-weight: 700; color: #bfa05a;
  background: #0d1117; border: 1px solid #232b38;
  clip-path: polygon(8px 0, 100% 0, 100% calc(100% - 8px), calc(100% - 8px) 100%, 0 100%, 0 8px);
}
.goal-titles { display: flex; flex-direction: column; }
.goal-name { font-size: 19px; font-weight: 700; }
.goal-sub { color: #7e8898; font-size: 12px; display: flex; gap: 8px; align-items: center; }
.vaulted { color: #7e8898; border: 1px solid #7e8898; padding: 0 6px; font-size: 11px; letter-spacing: .12em; }
.resurg { color: #bfa05a; border: 1px solid #bfa05a; padding: 0 6px; font-size: 11px; letter-spacing: .12em; }
.pips { display: inline-flex; gap: 5px; margin-left: auto; }
.pip { width: 11px; height: 11px; border-radius: 50%; border: 1px solid #7e8898; }
.pip.have { background: #bfa05a; border-color: #bfa05a; }
.pip.need { background: transparent; border-color: #c9d2de; }
.pip.unknown { background: transparent; border-style: dashed; }
.part { display: flex; gap: 8px; align-items: baseline; flex-wrap: wrap; padding: 7px 0; border-top: 1px solid #1c2330; font-size: 14px; }
.part-name { font-weight: 600; }
.part-state { font-size: 12px; color: #7e8898; text-transform: uppercase; letter-spacing: .1em; }
.part-state.need { color: #c9d2de; }
.relic-list { font-size: 12px; color: #7e8898; }
.relic.is-vaulted { opacity: .75; }
.action { margin-top: 10px; padding: 10px 12px; background: #0d1420; border-left: 3px solid #aee3ef; font-size: 15px; color: #aee3ef; }
.action-parked { border-color: #3a4354; color: #7e8898; }
.action-resurgence { border-color: #bfa05a; color: #e8d9ae; }
.action-farm, .action-wait { border-color: #7e8898; color: #c9d2de; }
.chip-common, .chip-uncommon, .chip-rare {
  font-size: 12px; padding: 1px 8px; border: 1px solid;
  clip-path: polygon(5px 0, 100% 0, 100% calc(100% - 5px), calc(100% - 5px) 100%, 0 100%, 0 5px);
}
.chip-common { color: #b08d4f; border-color: #b08d4f; }
.chip-uncommon { color: #c0c8d4; border-color: #c0c8d4; }
.chip-rare { color: #e3c878; border-color: #e3c878; }
.nw-grid, .wk-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 8px; }
.nw, .wk {
  display: flex; flex-direction: column; gap: 2px; background: #131824; border: 1px solid #1f2735;
  padding: 10px 12px; font-size: 13px; cursor: pointer;
  clip-path: polygon(7px 0, 100% 0, 100% calc(100% - 7px), calc(100% - 7px) 100%, 0 100%, 0 7px);
}
.nw.done, .wk.done { opacity: .45; }
.nw.done .nw-title, .wk.done .wk-name { text-decoration: line-through; }
.nw input, .wk input, .bar input { accent-color: #bfa05a; }
.nw-meta, .wk-meta, .wk-obs { color: #7e8898; font-size: 12px; }
.wk-small { opacity: .8; font-size: 12px; }
.nw-season { color: #7e8898; font-size: 13px; margin-bottom: 8px; }
.caveat { color: #7e8898; font-size: 12px; margin-top: 6px; }
.bars { display: grid; gap: 2px; }
.bar { cursor: pointer; }
.bar-name { font-family: Rajdhani, sans-serif; font-weight: 600; font-size: 16px; }
.bar-track { height: 6px; background: #0d1117; margin: 8px 0 4px; }
.bar-fill { height: 6px; background: #bfa05a; }
.bar-meta { color: #7e8898; font-size: 12px; }
.est { font-size: 11px; color: #7e8898; border: 1px solid #7e8898; padding: 0 5px; letter-spacing: .1em; }
.snap { color: #c9d2de; }
footer { margin-top: 10px; }
details.alt { margin-top: 6px; font-size: 13px; color: #7e8898; }
/* v2 tabs: Do Now vs Backlog (same palette, no new colors) */
.tabs { margin-top: 8px; }
.tabs input[name=tab] { display: none; }
.tab {
  display: inline-block; padding: 8px 22px; margin-right: 6px; cursor: pointer;
  font-family: Rajdhani, sans-serif; font-weight: 700; font-size: 15px;
  letter-spacing: .14em; text-transform: uppercase; color: #7e8898;
  background: #131824; border: 1px solid #1f2735;
  clip-path: polygon(7px 0, 100% 0, 100% calc(100% - 7px), calc(100% - 7px) 100%, 0 100%, 0 7px);
}
.tabpage { display: none; }
#tab-donow:checked ~ #page-donow { display: block; }
#tab-hunt:checked ~ #page-hunt { display: block; }
#tab-backlog:checked ~ #page-backlog { display: block; }
#tab-donow:checked ~ label[for=tab-donow] { color: #aee3ef; border-color: #aee3ef; }
#tab-hunt:checked ~ label[for=tab-hunt] { color: #aee3ef; border-color: #aee3ef; }
#tab-backlog:checked ~ label[for=tab-backlog] { color: #aee3ef; border-color: #aee3ef; }
/* do-now focus trio (≤3 parts, terse) */
.focus { display: flex; gap: 8px; flex-wrap: wrap; margin-bottom: 10px; }
.focus-part { font-size: 13px; color: #bfa05a; border: 1px solid #bfa05a; padding: 2px 10px; }
/* backlog compact lines */
.bline {
  display: flex; gap: 10px; align-items: center; font-size: 13px;
  background: #131824; border: 1px solid #1f2735; padding: 5px 10px;
}
.miniport {
  width: 24px; height: 24px; flex: none; display: inline-flex; align-items: center; justify-content: center;
  font-family: Rajdhani, sans-serif; font-size: 13px; font-weight: 700; color: #bfa05a;
  background: #0d1117; border: 1px solid #232b38; position: relative; overflow: hidden;
}
.miniport img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; background: #0d1117; }
.bkind { color: #7e8898; font-size: 11px; text-transform: uppercase; letter-spacing: .1em; margin-left: auto; }
/* backlog: parent groups with aligned part rows */
.backlog-head { color: #7e8898; font-size: 13px; margin: 2px 0 8px; }
.backlog { display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 12px; }
.bgroup { margin: 0; }
.bhead { display: flex; gap: 10px; align-items: center; }
.bname { font-family: Rajdhani, sans-serif; font-size: 18px; font-weight: 700; }
.bcount { color: #7e8898; font-size: 12px; margin-left: auto; }
.brow {
  display: grid; grid-template-columns: minmax(92px, 1.1fr) minmax(150px, 1.9fr) 44px 44px;
  gap: 8px; align-items: baseline; padding: 5px 0; border-top: 1px solid #1c2330;
  font-size: 13px;
}
.bpart { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.brelic { color: #7e8898; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.brelic.is-vaulted { opacity: .7; }
.bstock, .bcracks { text-align: right; color: #c9d2de; font-variant-numeric: tabular-nums; }
.portrait.has-icon { position: relative; overflow: hidden; }
.portrait.has-icon img {
  position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover;
  background: #0d1117;
}
.nonprime-h { font-size: 12px; letter-spacing: .18em; text-transform: uppercase; color: #7e8898; margin: 18px 0 8px; }
.nonprime { display: grid; gap: 6px; }
.npcat { display: flex; gap: 10px; font-size: 13px; background: #131824; border: 1px solid #1f2735; padding: 8px 12px; }
.npcat-name { color: #bfa05a; font-weight: 600; white-space: nowrap; }
.npcat-items { color: #7e8898; }
.tau { display: flex; flex-direction: column; gap: 2px; font-size: 14px; border-color: #bfa05a; }
.fam { font-size: 12px; }
"#;

const JS: &str = r#"
// Checkbox persistence: localStorage keyed by plan-item id + in-game date.
(function () {
  var date = document.querySelector('.stamp');
  var day = (date ? date.textContent : '').slice(0, 10).replace(/[^0-9-]/g, '');
  function key(id) { return 'fracker:' + day + ':' + id; }
  document.querySelectorAll('input[type=checkbox][data-id]').forEach(function (box) {
    var id = box.getAttribute('data-id');
    try {
      if (localStorage.getItem(key(id)) === '1' && !box.checked) { box.checked = true; }
    } catch (e) {}
    box.addEventListener('change', function () {
      var host = box.closest('.chunk,.nw,.wk,.bar,.card');
      if (host) { host.classList.toggle('done', box.checked); }
      try {
        if (box.checked) { localStorage.setItem(key(id), '1'); }
        else { localStorage.removeItem(key(id)); }
      } catch (e) {}
    });
    if (box.checked) {
      var host = box.closest('.chunk,.nw,.wk,.bar,.card');
      if (host) { host.classList.add('done'); }
    }
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
            "Do now", "Hunt", "Backlog", "Farms", "Nightwave", "This week",
            "Today", "Targets",
        ] {
            assert!(html.contains(section), "missing section: {section}");
        }
        assert!(html.contains("color-scheme: dark"), "refresh-flash rule");
        assert!(html.contains("http-equiv=\"refresh\""), "meta refresh");
        assert!(html.contains("localStorage"), "checkbox persistence");
        assert!(html.contains("Rajdhani"), "game-adjacent type");
        assert!(html.contains("clip-path"), "chamfered cards");
        // v2 tabs: empty plan has no loud line; all three tabs render.
        assert!(!html.contains("card donow"), "no loud line without tier-1 chunks");
        assert!(html.contains("tab-donow") && html.contains("tab-hunt") && html.contains("tab-backlog"), "three tabs");
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
        // deprioritized flag and the quiet style.
        let t = crate::tuning::Tuning::for_tests();
        let board = weekly_board(&t);
        assert_eq!(board.len(), 24);
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
}
