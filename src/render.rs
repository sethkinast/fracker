//! HTML checklist renderer (BRIEF §4.2, §17 settled sections, §19 art).
//! Single self-contained file: inline CSS, no external assets except an
//! optional Google Fonts link (clean offline fallback) and WFCD CDN
//! icons that degrade to monograms when offline.

use std::collections::{HashMap, HashSet};

use crate::model::WorldState;
use crate::plan::Plan;

// ---------------------------------------------------------------------------
// Render context
// ---------------------------------------------------------------------------

pub struct RenderContext<'a> {
    pub world: &'a WorldState,
    pub generated_label: String,
    /// e.g. "seed (Xuerian token not set — targets may go stale)".
    pub targets_status: String,
    pub world_status: String,
    /// Weekly item id -> completion label proven by the log.
    pub weekly_done: &'a HashMap<String, String>,
    /// Plan/challenge ids the log proves complete (struck-through).
    pub proven_done: &'a HashSet<String>,
    pub mastery_rank: Option<u32>,
}

pub struct WeeklyItem {
    pub id: &'static str,
    pub name: &'static str,
    pub limit: &'static str,
    pub time: &'static str,
    pub obs: &'static str,
}

/// All 24 tracked weeklies (WEEKLIES.md). Clem and Maroo are LAST —
/// least important, "just there if you're passing by" (BRIEF §4.2).
pub fn weekly_board() -> Vec<WeeklyItem> {
    vec![
        WeeklyItem { id: "archon", name: "Archon Hunt", limit: "1 clear/wk", time: "30–45m", obs: "missions in world state; completion from log" },
        WeeklyItem { id: "netra", name: "Netracells", limit: "5 Search Pulses/wk (shared)", time: "10–15m each", obs: "pulses tracked by ledger; rest manual" },
        WeeklyItem { id: "deep", name: "Deep Archimedea (+Elite)", limit: "rewards 1×/wk, 2 pulses", time: "30–45m", obs: "missions in world state; completion manual" },
        WeeklyItem { id: "temp", name: "Temporal Archimedea (+Elite)", limit: "rewards 1×/wk, 2 pulses", time: "30–45m", obs: "missions in world state; completion manual" },
        WeeklyItem { id: "circuit", name: "The Circuit — Normal", limit: "1 reward track/wk", time: "1.5–2h to Tier 10", obs: "manual" },
        WeeklyItem { id: "circuit-sp", name: "The Circuit — Steel Path", limit: "2 Incarnon picks/wk", time: "1.5–2.5h", obs: "manual" },
        WeeklyItem { id: "descendia", name: "Descendia (Normal + SP)", limit: "10 rewards per difficulty/wk", time: "45–90m each", obs: "seed in world state; completion manual" },
        WeeklyItem { id: "kahl", name: "Break Narmer (Kahl)", limit: "1 mission set/wk, ≤115 Stock", time: "15–25m", obs: "manual" },
        WeeklyItem { id: "nw-weekly", name: "Nightwave weekly + elite acts", limit: "5 weekly + 2 elite/wk", time: "mostly passive", obs: "act list in world state; completion manual" },
        WeeklyItem { id: "cal1999", name: "1999 Calendar weekly page", limit: "1 page/wk", time: "passive", obs: "page in world state; completion manual" },
        WeeklyItem { id: "clan", name: "Clan Weekly Initiative", limit: "4 tiers/wk (clan-wide)", time: "passive", obs: "region in world state; claims manual" },
        WeeklyItem { id: "conclave", name: "Conclave weekly challenges", limit: "3 + bonus/wk — Fri reset", time: "varies", obs: "list in world state; completion manual" },
        WeeklyItem { id: "teshin-rot", name: "Teshin — featured item", limit: "1/wk", time: "2m", obs: "rotation in world state; purchase manual" },
        WeeklyItem { id: "teshin-cap", name: "Teshin — evergreen caps", limit: "cipher 1/wk; Kuva/Relic ×25", time: "2m", obs: "stock in world state; purchases manual" },
        WeeklyItem { id: "palladino", name: "Palladino (Iron Wake)", limit: "each ware 1/wk", time: "2m", obs: "manual" },
        WeeklyItem { id: "acrithis", name: "Acrithis weekly wares", limit: "5 wares, 1 each/wk — Sun reset", time: "2m", obs: "manual" },
        WeeklyItem { id: "yonta", name: "Archimedean Yonta — Forma", limit: "3/wk", time: "2m", obs: "manual" },
        WeeklyItem { id: "bird3", name: "Bird 3 — Archon Shard", limit: "1/wk", time: "2m", obs: "manual" },
        WeeklyItem { id: "chipper", name: "Chipper — Archon Shard", limit: "1/wk (Rank 5)", time: "2m", obs: "manual" },
        WeeklyItem { id: "helminth", name: "Helminth Invigoration", limit: "3 frames/wk", time: "2m", obs: "manual" },
        WeeklyItem { id: "nw-shop", name: "Nightwave Cred Offerings", limit: "rotation/wk", time: "2m", obs: "manual" },
        WeeklyItem { id: "baro", name: "Baro Ki'Teer", limit: "biweekly Fri–Sun", time: "5m", obs: "schedule + inventory in world state" },
        WeeklyItem { id: "clem", name: "Help Clem (Darvo)", limit: "1/wk", time: "~10m", obs: "manual — passing by only" },
        WeeklyItem { id: "maroo", name: "Ayatan Treasure Hunt (Maroo)", limit: "1 sculpture/wk", time: "~5m", obs: "manual — passing by only" },
    ]
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

    // ---- 1. do now ----
    // Exactly ONE loud element (the current instruction); the rest of
    // the expiring band renders quieter underneath (§19).
    h.push_str("<section><h2>Do now</h2>\n");
    let top: Vec<&crate::plan::Chunk> =
        plan.chunks.iter().filter(|c| c.tier == 1).collect();
    if top.is_empty() {
        h.push_str("<div class=\"card quiet\">Nothing expiring right now. Work the hunt below.</div>\n");
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
    h.push_str("</section>\n");

    // ---- 2. the hunt ----
    h.push_str("<section><h2>The hunt</h2>\n");
    let slate: Vec<&crate::plan::Chunk> =
        plan.chunks.iter().filter(|c| c.tier == 2 || c.tier == 3).collect();
    if slate.is_empty() {
        h.push_str("<div class=\"card quiet\">No live crack matches. Cards below show farm or parked state.</div>\n");
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
    // goal cards
    h.push_str("<div class=\"goals\">\n");
    for g in &plan.goals {
        h.push_str(&goal_card_html(g, ctx));
    }
    h.push_str("</div>\n</section>\n");

    // ---- 3. farms ----
    h.push_str("<section><h2>Farms</h2>\n");
    if plan.farms.is_empty() {
        h.push_str("<div class=\"card quiet\">No relic farms targeted — every current want sits in vaulted relics (crack-or-park). Sourcing activates for current-rotation goals.</div>\n");
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

    // ---- 4. nightwave ----
    h.push_str("<section><h2>Nightwave</h2>\n");
    match &ctx.world.nightwave {
        Some(nw) if !nw.active_challenges.is_empty() => {
            h.push_str(&format!("<div class=\"nw-season\">Season {} — acts below, never a standing total (unobservable).</div>\n", nw.season));
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
            h.push_str("<div class=\"caveat\">World-state acts are a floor: Recovered backfill can exceed this list. Unmapped acts render display-only.</div>\n");
        }
        _ => {
            h.push_str("<div class=\"card quiet\">No Nightwave season live in world state.</div>\n");
        }
    }
    h.push_str("</section>\n");

    // ---- 5. this week ----
    h.push_str("<section><h2>This week</h2>\n<div class=\"wk-grid\">\n");
    for w in weekly_board() {
        let done_label = ctx.weekly_done.get(w.id).cloned().unwrap_or_default();
        let done = !done_label.is_empty();
        let small = (w.id == "clem" || w.id == "maroo") as u8;
        let _ = small;
        h.push_str(&format!(
            "<label class=\"wk{item_class}{small_class}\"><input type=\"checkbox\" data-id=\"wk-{id}\"{checked}>\n<span class=\"wk-name\">{name}</span>\n<span class=\"wk-meta\">{limit} &middot; {time}</span>\n<span class=\"wk-obs\">{obs}{proven}</span></label>\n",
            item_class = if done { " done" } else { "" },
            small_class = if w.id == "clem" || w.id == "maroo" { " wk-small" } else { "" },
            id = w.id,
            checked = if done { " checked" } else { "" },
            name = esc(w.name),
            limit = esc(w.limit),
            time = esc(w.time),
            obs = esc(w.obs),
            proven = if done { format!(" &middot; proven {}", esc(&done_label)) } else { String::new() },
        ));
    }
    h.push_str("</div>\n</section>\n");

    // ---- 6. today's bars ----
    h.push_str("<section><h2>Today&apos;s bars</h2>\n<div class=\"bars\">\n");
    let (focus_cap, stand_cap) = match ctx.mastery_rank {
        Some(mr) => (
            format!("{:}", 250_000 + 5_000 * mr),
            format!("{:}", 16_000 + 500 * mr),
        ),
        None => ("250,000 + 5,000 × MR".to_owned(), "16,000 + 500 × MR".to_owned()),
    };
    h.push_str(&format!(
        "<div class=\"card bar\"><div class=\"bar-name\">Focus cap <span class=\"est\">estimate</span></div>\n<div class=\"bar-track\"><div class=\"bar-fill\" style=\"width:0%\"></div></div>\n<div class=\"bar-meta\">daily cap {focus_cap} — progress unobservable from clean sources; best farm route lands with the focus spike</div></div>\n"
    ));
    h.push_str(&format!(
        "<div class=\"card bar\"><div class=\"bar-name\">Syndicate standing <span class=\"est\">estimate</span></div>\n<div class=\"bar-track\"><div class=\"bar-fill\" style=\"width:0%\"></div></div>\n<div class=\"bar-meta\">daily cap {stand_cap} — measured vs estimated depends on the §7.5 fixture check; shown as estimate until then</div></div>\n"
    ));
    let sortie_done = ctx.weekly_done.contains_key("sortie-daily");
    h.push_str(&format!(
        "<label class=\"card bar\"><input type=\"checkbox\" data-id=\"sortie-daily\"{}>\n<span class=\"bar-name\">Sortie done/not</span>\n<span class=\"bar-meta\">daily reset 17:00 UTC{}</span></label>\n",
        if sortie_done { " checked" } else { "" },
        if sortie_done { " &middot; proven by log" } else { "" },
    ));
    h.push_str("</div>\n</section>\n");

    // ---- footer: targets snapshot ----
    let total_wants: usize = plan.goals.iter().map(|g| g.wants.len()).sum();
    h.push_str("<footer><section><h2>Targets snapshot</h2>\n");
    h.push_str(&format!(
        "<div class=\"card snap\">{} wants remaining across {} goals. Completed this week: shown via log proofs as they land.</div>\n",
        total_wants,
        plan.goals.len()
    ));
    h.push_str("</section></footer>\n");

    // checkbox persistence: localStorage keyed by item id + in-game date.
    h.push_str("<script>\n");
    h.push_str(JS);
    h.push_str("\n</script>\n</div>\n</body>\n</html>\n");
    h
}

fn goal_card_html(g: &crate::plan::GoalCard, ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    let initial = g.gear.chars().next().unwrap_or('?');
    h.push_str("<article class=\"card goal\">\n");
    h.push_str(&format!(
        "<div class=\"goal-head\"><span class=\"portrait\" aria-hidden=\"true\">{initial}</span>\n<span class=\"goal-titles\"><span class=\"goal-name\">{name}</span>\n<span class=\"goal-sub\">",
        initial = initial,
        name = esc(&g.gear),
    ));
    if g.vaulted {
        h.push_str("<span class=\"vaulted\">VAULTED</span> ");
    }
    if g.resurgence_live {
        h.push_str("<span class=\"resurg\">VARZIA LIVE</span> ");
    }
    h.push_str(&format!(
        "parts {have}/{total}</span></span>\n<span class=\"pips\" aria-hidden=\"true\">",
        have = g.parts_have,
        total = g.parts_total,
    ));
    for p in &g.parts {
        let cls = match p.state {
            crate::plan::PartState::Have => "pip have",
            crate::plan::PartState::Need => "pip need",
            crate::plan::PartState::Unknown => "pip unknown",
        };
        h.push_str(&format!("<span class=\"{cls}\"></span>"));
    }
    h.push_str("</span></div>\n");
    // Part rows.
    for p in &g.parts {
        let state = match p.state {
            crate::plan::PartState::Have => "have",
            crate::plan::PartState::Need => "need",
            crate::plan::PartState::Unknown => "unknown",
        };
        h.push_str(&format!(
            "<div class=\"part\"><span class=\"part-name\">{name}</span>\n<span class=\"part-state {state}\">{state}</span>\n<span class=\"{rclass}\">{odds}</span>\n",
            name = esc(&p.name),
            state = state,
            rclass = rarity_class(p.rarity),
            odds = esc(&p.best_odds),
        ));
        if !p.relics.is_empty() {
            h.push_str("<span class=\"relic-list\">");
            let mut shown = 0;
            for r in &p.relics {
                if shown >= 5 {
                    break;
                }
                shown += 1;
                let held = match r.owned {
                    Some(n) => format!("×{n}"),
                    None => "?".to_owned(),
                };
                h.push_str(&format!(
                    "<span class=\"relic{vaulted}\">{relic} {held}</span> ",
                    vaulted = if r.vaulted { " is-vaulted" } else { "" },
                    relic = esc(&r.relic),
                    held = held,
                ));
            }
            h.push_str("</span>");
        }
        h.push_str("</div>\n");
    }
    // Action line: the ONE current instruction (pale cyan).
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
    let _ = ctx;
    h.push_str("</article>\n");
    h
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
        let world = WorldState::default();
        let plan = Plan::default();
        // Lifetimes: build ctx inline to avoid borrow tangle.
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let ctx = RenderContext {
            world: &world,
            generated_label: "x".to_owned(),
            targets_status: "seed".to_owned(),
            world_status: "fixture".to_owned(),
            weekly_done: &weekly,
            proven_done: &proven,
            mastery_rank: None,
        };
        let html = render_plan(&plan, &ctx);
        for section in [
            "Do now", "The hunt", "Farms", "Nightwave", "This week",
            "Today", "Targets snapshot",
        ] {
            assert!(html.contains(section), "missing section: {section}");
        }
        assert!(html.contains("color-scheme: dark"), "refresh-flash rule");
        assert!(html.contains("http-equiv=\"refresh\""), "meta refresh");
        assert!(html.contains("localStorage"), "checkbox persistence");
        assert!(html.contains("Rajdhani"), "game-adjacent type");
        assert!(html.contains("clip-path"), "chamfered cards");
    }

    #[test]
    fn clem_and_maroo_render_last() {
        let board = weekly_board();
        let tail: Vec<&str> = board.iter().rev().take(2).map(|w| w.id).collect();
        assert!(tail.contains(&"clem") && tail.contains(&"maroo"));
        assert_eq!(board.len(), 24);
    }
}
