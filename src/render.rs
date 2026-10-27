//! HTML checklist renderer (twelfth pass §1: strict port of Seth's IA
//! reference).
//!
//! Seth authored the target IA by hand at
//! `/opt/opencode/ia-reference-seth.html`; this pass adopts that file's
//! stylesheet and markup patterns directly — multicolumn `.g2`/`.g2.b`/
//! `.g3` grids, the `.bg` backlog cells with `.segs` pips, the Hunt
//! `.tool` filter tabs, and the compact First-Crack phrasing. The CSS
//! below is the reference's, extended only where the hand-authored mock
//! could not express a live state (tick/struck rows, server-applied
//! ticks).
//!
//! All content (weekly board, caps, labels) still arrives via the render
//! context from the runtime tuning tables; this module holds no game
//! content. Icon delivery is a mode: the product page hotlinks the CDN
//! per BRIEF §4.2, while `--demo` embeds ≤128px data-URI thumbnails.

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
    /// Item 52: a token unique to THIS regeneration. The loopback server
    /// scopes the remembered tab to it, so the 60s refresh keeps the tab
    /// while a new report resets to First Crack.
    pub generation: String,
    /// e.g. "backlog 63 primes · profile live (MR30)".
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

/// The four plan tabs as `(radio id, stable slug, label)`. The slug is what
/// the loopback server records and what the real tab links carry, so the
/// persisted selection survives regenerations and app restarts (thirteenth
/// pass §2, addendum 46). Slugs are semantic so a future reordering of the
/// tabs does not change what a remembered selection means.
pub const TABS: [(&str, &str, &str); 4] = [
    ("t1", "first-crack", "First Crack"),
    ("t2", "hunt", "Hunt"),
    ("t3", "backlog", "Backlog"),
    ("t4", "routine", "Routine"),
];

/// The radio id for a persisted tab slug (`None` for an unknown slug).
pub fn tab_id_for_slug(slug: &str) -> Option<&'static str> {
    TABS.iter().find(|(_, s, _)| *s == slug).map(|(id, _, _)| *id)
}

/// Profile-backed daily bars: REMAINING caps from the live profile
/// (addendum #2) — never estimated. A profile fetched before the most
/// recent 00:00 UTC reset renders as reset (twelfth pass §3).
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
/// Copy stays terse: labels, limits, times.
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
// Small formatting helpers
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

/// "Ivara Prime" -> "Ivara": the mock's `.bn`/chip names drop the
/// "Prime" suffix (hand-trimmed IA, twelfth pass §1).
fn display_gear(gear: &str) -> String {
    let s = gear.trim();
    s.strip_suffix(" Prime").unwrap_or(s).to_owned()
}

/// `days_until` values carry an "in " prefix for the header pills
/// ("2 primes · in 6d"); the "… left" sites want the bare duration
/// (addendum #73: the pill must not read "IN 6D LEFT").
fn bare_duration(s: &str) -> &str {
    s.strip_prefix("in ").unwrap_or(s)
}

/// Rarity as the mock's palette class (`common` / `uncommon` / `rare`).
fn rarity_css(r: crate::plan::Rarity) -> &'static str {
    match r {
        crate::plan::Rarity::Common => "common",
        crate::plan::Rarity::Uncommon => "uncommon",
        crate::plan::Rarity::Rare => "rare",
    }
}

/// "Intact" -> "Int", "Radiant" -> "Rad" (the mock's terse refinement).
fn refinement_abbrev(r: &str) -> String {
    r.chars().take(3).collect()
}

fn part_state_class(state: &crate::plan::PartState) -> &'static str {
    match state {
        crate::plan::PartState::Have => "have",
        crate::plan::PartState::HaveUnverified => "uvhave",
        crate::plan::PartState::Need => "need",
        crate::plan::PartState::Unknown => "unk",
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

/// Compact count for the mock's caps/points: `349.6k`, `4.1k`, `31k`.
pub fn fmt_compact(n: u32) -> String {
    if n >= 1_000_000 {
        let v = n as f64 / 1_000_000.0;
        if (v - v.round()).abs() < 1e-9 {
            format!("{v:.0}M")
        } else {
            format!("{v:.1}M")
        }
    } else if n >= 1_000 {
        let v = n as f64 / 1_000.0;
        if (v - v.round()).abs() < 1e-9 {
            format!("{v:.0}k")
        } else {
            format!("{v:.1}k")
        }
    } else {
        n.to_string()
    }
}

/// "25m" / "1h05m" / "in 2d" -> minutes, for the event-alert heat class.
fn parse_left_minutes(left: &str) -> Option<u32> {
    let s = left.trim().trim_start_matches("in ").trim();
    if let Some((h, rest)) = s.split_once('h') {
        let hh: u32 = h.trim().parse().ok()?;
        let mm: u32 = rest.trim().trim_end_matches('m').trim().parse().unwrap_or(0);
        return Some(hh * 60 + mm);
    }
    s.strip_suffix('m')?.trim().parse().ok()
}

/// "Xini (Eris)" -> ("Xini", "Eris"); a bare node keeps an empty planet.
fn split_node(node: &str) -> (String, String) {
    match node.rfind(" (") {
        Some(i) => (
            node[..i].trim().to_owned(),
            node[i + 2..].trim_end_matches(')').trim().to_owned(),
        ),
        None => (node.trim().to_owned(), String::new()),
    }
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

/// Icon `src` for one parent gear: the embedded data-URI thumbnail
/// when demo mode supplied one, else the CDN hotlink (product page).
/// A prime part always shows its PARENT item's icon, never a generic
/// part icon. Unresolved items keep the monogram (None).
fn gear_icon_src(gear: &str, ctx: &RenderContext<'_>) -> Option<String> {
    if let Some(embedded) = ctx.embedded_icons {
        return embedded.get(gear).cloned();
    }
    crate::model::icon_url(gear, ctx.icons)
}

/// The mock's `.mi` icon chip: the parent-item image when resolved,
/// else the monogram. Demo strictness carries over through
/// [`gear_icon_src`] — a gear missing from the embed map keeps its
/// monogram, never a hotlink.
fn mi_span(gear: &str, ctx: &RenderContext<'_>, class: &str) -> String {
    let initial = gear.chars().next().unwrap_or('?');
    match gear_icon_src(gear, ctx) {
        Some(url) => format!(
            "<span class=\"{class}\"><img src=\"{}\" alt=\"\" loading=\"lazy\" onerror=\"this.remove()\">{initial}</span>",
            esc(&url),
            class = class,
            initial = initial,
        ),
        None => format!(
            "<span class=\"{class}\" aria-hidden=\"true\">{initial}</span>",
            class = class,
            initial = initial,
        ),
    }
}

/// The mock's `.segs` component-progress pips plus its `have/total`
/// count (the backlog-pips gap Seth named).
fn seg_html(g: &crate::plan::GoalCard) -> String {
    let mut h = String::new();
    h.push_str("<span class=\"segs\">");
    for p in &g.parts {
        h.push_str(&format!(
            "<i class=\"seg {}\"></i>",
            part_state_class(&p.state)
        ));
    }
    h.push_str("</span>");
    // Thirtieth pass (#93): a card holding any part only via the
    // unverified assertion says so beside the count, on Hunt and
    // Backlog alike — unverified progress never reads as verified.
    let uv_mark = if g
        .parts
        .iter()
        .any(|p| p.state == crate::plan::PartState::HaveUnverified)
    {
        " ⍻"
    } else {
        ""
    };
    h.push_str(&format!(
        "<i>{}/{}{}</i>",
        g.parts_have, g.parts_total, uv_mark
    ));
    h
}

// ---------------------------------------------------------------------------
// Page shell
// ---------------------------------------------------------------------------

pub fn render_plan(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    h.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    h.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    // No meta-refresh (forty-first pass): the loopback server pushes a
    // new generation over WebSocket when a regeneration lands and the
    // page swaps the changed fragments in place (see push.rs and the
    // fragment markers below); a slow fragment poll converges a
    // dropped socket. A file:// demo export has neither and is simply
    // static. The refresh-flash rule (§19) keeps every repaint dark
    // (root background + color-scheme: dark).
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
    h.push_str(&format!(
        "<body data-day=\"{}\" data-generation=\"{}\">\n<div class=\"wrap\">\n",
        esc(day),
        esc(&ctx.generation)
    ));

    // The mock's tab mechanism: radio inputs precede the header so the
    // reference CSS's `#t1:checked~main #p1` rules apply (twelfth pass §1).
    // Each tab is ALSO a real link (`?tab=<slug>`) the loopback server
    // records and renders checked on the next serve, so a fresh load
    // lands back on the active tab (thirteenth pass §2, item 46). The
    // default radio stays checked for a `file://` demo export.
    for (i, (id, _, _)) in TABS.iter().enumerate() {
        let checked = if i == 0 { " checked" } else { "" };
        h.push_str(&format!(
            "<input type=\"radio\" name=\"tab\" id=\"{id}\"{checked}>\n"
        ));
    }

    h.push_str(&header_html(plan, ctx));

    h.push_str("<nav class=\"tabs\">\n");
    for (id, slug, label) in TABS {
        h.push_str(&format!(
            "<a class=\"tab\" data-tab=\"{id}\" href=\"?tab={slug}\">{label}</a>\n"
        ));
    }
    h.push_str("</nav>\n<main>\n");

    h.push_str(&first_crack_html(plan, ctx));
    h.push_str(&hunt_html(plan, ctx));
    h.push_str(&backlog_html(plan, ctx));
    h.push_str(&routine_html(ctx));

    h.push_str("</main>\n");

    // checkbox persistence: the resident's loopback server is the store
    // of record; localStorage remains a progressive cache for file://.
    h.push_str("<script>\n");
    h.push_str(JS);
    h.push('\n');
    // The generation-sync policy (push/poll, fragment swap, last-resort
    // reload failsafe) lives in one testable module so the page script
    // cannot drift from it (nineteenth pass §1/§2; forty-first pass).
    h.push_str(&crate::push::client_script());
    h.push_str("</script>\n</div>\n</body>\n</html>\n");
    h
}

// ---------------------------------------------------------------------------
// Fragments (forty-first pass): the served page's swap units
// ---------------------------------------------------------------------------

/// The page's fragment ids, in document order. The loopback server
/// diffs these between generations and the page script swaps only
/// the changed ones in place (`innerHTML` on the fragment's
/// container) — no navigation, so scroll, focus, the active tab,
/// and every unchanged panel's `<img>` elements survive a
/// regeneration. `header` is the `<header class="top">` block; the
/// rest are the four tab pages.
pub const FRAGMENT_IDS: [&str; 5] = ["header", "p1", "p2", "p3", "p4"];

/// The opening marker a fragment's inner HTML starts after. The
/// markers are inert HTML comments in the served page; they exist
/// only so the server can split the rendered file back into the
/// sections the renderer composed it from.
fn frag_open(id: &str) -> String {
    format!("<!--@frag:{id}-->\n")
}

/// The closing marker a fragment's inner HTML ends before.
const FRAG_CLOSE: &str = "<!--/@frag-->\n";

/// Split a rendered page into its fragments: `(id, inner HTML)` in
/// document order. The inner HTML is exactly what the page script
/// assigns to the fragment container's `innerHTML` — the container
/// tags themselves are part of the static shell and are never
/// swapped. A page without markers (the placeholder, a demo export
/// from an older build) yields an empty vec.
pub fn extract_fragments(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(open) = rest.find("<!--@frag:") {
        let after = &rest[open + "<!--@frag:".len()..];
        let Some(id_end) = after.find("-->") else {
            break;
        };
        let id = &after[..id_end];
        let content_start = open + "<!--@frag:".len() + id_end + "-->".len();
        let Some(close_rel) = rest[content_start..].find("<!--/@frag-->") else {
            break;
        };
        let content = &rest[content_start..content_start + close_rel];
        // The opening marker sits on its own line; the payload is
        // the content after that line, byte-exact.
        let content = content.strip_prefix('\n').unwrap_or(content);
        out.push((id.to_owned(), content.to_owned()));
        rest = &rest[content_start + close_rel + "<!--/@frag-->".len()..];
    }
    out
}

/// A fragment's content hash, for the server's per-generation
/// change detection. Only ever compared within one server process
/// (the generation a page shows vs the generation on disk), so
/// the std hasher's per-process seed is fine.
pub fn fragment_hash(html: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    html.hash(&mut hasher);
    hasher.finish()
}

/// The mock's slim header: brand + three status pills + one-line stamp.
fn header_html(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    let vendor = esc(&ctx.tuning.planner.vendor.name.to_uppercase());
    let contact = esc(&ctx.tuning.planner.resurgence.contact.to_uppercase());
    let baro = match &plan.baro {
        Some(b) => format!("{} &middot; {}", esc(&b.location), esc(&b.ends_in)),
        None => match &ctx.world.baro {
            Some(vt) => relative_countdown(ctx, &vt.expiry),
            None => "—".to_owned(),
        },
    };
    let varzia = match &plan.resurgence {
        Some(r) => format!("{} primes &middot; {}", r.gears.len(), esc(&r.ends_in)),
        None => "—".to_owned(),
    };
    let parts: usize = plan.goals.iter().map(|g| g.parts.len()).sum();
    let mr = ctx
        .mastery_rank
        .map(|m| m.to_string())
        .unwrap_or_else(|| "—".to_owned());
    let mut h = String::new();
    h.push_str("<header class=\"top\">\n");
    h.push_str(&frag_open("header"));
    h.push_str("<div class=\"brand\">FRACKER</div>\n");
    h.push_str("<div class=\"pills\">\n");
    // Addendum #85: while Baro is live his pill pulses.
    let baro_class = if plan.baro.is_some() {
        " class=\"baro-live\""
    } else {
        ""
    };
    h.push_str(&format!("<span{baro_class}><b>{vendor}</b>{baro}</span>\n"));
    h.push_str(&format!("<span><b>{contact}</b>{varzia}</span>\n"));
    h.push_str(&format!(
        "<span><b>TARGETS</b>{} goals &middot; {} parts</span>\n",
        plan.goals.len(),
        parts,
    ));
    h.push_str("</div>\n");
    h.push_str(&format!(
        "<div class=\"stamp\">MR{mr} &middot; generated {} &middot; {} &middot; {}</div>\n",
        esc(&ctx.generated_label),
        esc(&ctx.world_status),
        esc(&ctx.targets_status),
    ));
    h.push_str(FRAG_CLOSE);
    h.push_str("</header>\n");
    h
}

/// One-line status countdown for a world-state expiry, relative to the
/// plan's own timestamp ("in 2d", "in 5h", "in 25m").
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

// ---------------------------------------------------------------------------
// First Crack (p1)
// ---------------------------------------------------------------------------

fn first_crack_html(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    h.push_str("<div class=\"page\" id=\"p1\">\n");
    h.push_str(&frag_open("p1"));
    h.push_str("<div class=\"g2\">\n");
    h.push_str(&event_alerts_panel(plan, ctx));
    h.push_str(&lotus_panel(plan, ctx));
    h.push_str(&crack_queue_panel(plan, ctx));
    h.push_str("</div>\n");

    if let Some(r) = &plan.resurgence {
        h.push_str("<div class=\"g2 b\">\n");
        h.push_str(&resurgence_panel(r, ctx));
        h.push_str(&pack_panel(ctx));
        h.push_str("</div>\n");
    } else if ctx.pack.as_ref().map(|p| p.live).unwrap_or(false) {
        h.push_str("<div class=\"g2 b\">\n");
        h.push_str(&pack_panel(ctx));
        h.push_str("</div>\n");
    }
    if !plan.notes.is_empty() {
        h.push_str(&format!(
            "<p class=\"mut sm\">{}</p>\n",
            esc(&plan.notes.join(" ")),
        ));
    }
    h.push_str(FRAG_CLOSE);
    h.push_str("</div>\n");
    h
}

/// "Event alerts" panel: one compact `.row.ev` per live alert — countdown,
/// node + planet, mission type, run estimate (the mock's terse phrasing).
fn event_alerts_panel(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    if plan.event_alerts.is_empty() {
        return String::new();
    }
    let payout = plan
        .event_alerts
        .first()
        .map(|a| esc(&a.payout))
        .unwrap_or_default();
    let small = if payout.is_empty() {
        String::new()
    } else {
        format!("<small>{payout} each</small>")
    };
    let mut h = String::new();
    h.push_str(&format!(
        "<section class=\"panel\">\n<h2>{}{small}</h2>\n",
        esc(&ctx.tuning.planner.alert_sections.event_header),
    ));
    h.push_str(&alert_rows(&plan.event_alerts, ctx));
    h.push_str("</section>\n");
    h
}

/// Gift-of-the-Lotus panel (hides when empty; the mock carried none).
fn lotus_panel(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    if plan.lotus_alerts.is_empty() {
        return String::new();
    }
    let mut h = String::new();
    h.push_str(&format!(
        "<section class=\"panel\">\n<h2>{}</h2>\n",
        esc(&ctx.tuning.planner.alert_sections.lotus_header),
    ));
    h.push_str(&alert_rows(&plan.lotus_alerts, ctx));
    h.push_str("</section>\n");
    h
}

/// The mock's compact `.row.ev` alert line.
fn alert_rows(alerts: &[crate::plan::AlertView], ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    for a in alerts {
        let (name, planet) = split_node(&a.node);
        let est = ctx.tuning.missions.base_minutes(&a.mission_type) as u32
            + ctx.tuning.missions.overhead as u32;
        let heat = match parse_left_minutes(&a.left) {
            Some(m) if m < 30 => " hot",
            Some(m) if m < 60 => " warm",
            _ => "",
        };
        let done = if a.done { " done" } else { "" };
        let proof = if a.done { " · log &check;" } else { "" };
        let planet = if planet.is_empty() {
            String::new()
        } else {
            format!(" <span class=\"mut\">{}</span>", esc(&planet))
        };
        h.push_str(&format!(
            "<div class=\"row ev{done}\">\n<b class=\"tm{heat}\">{left}</b>\n<div><b>{name}</b>{planet}<br><span class=\"mut sm\">{kind}</span></div>\n<span class=\"mut\">~{est}m{proof}</span>\n</div>\n",
            done = done,
            heat = heat,
            left = esc(&a.left),
            name = esc(&name),
            planet = planet,
            kind = esc(&a.mission_type),
            est = est,
            proof = proof,
        ));
    }
    h
}

/// The crack queue: the mock's `.row.ch` cards — NEXT badge, parent icon,
/// part/short-name mid-column, relic + refinement/traces right, the run
/// time last. Tickable (label + checkbox) so a crack can be marked done;
/// done rows strike, as do rows whose only relic source is unobtainable.
fn crack_queue_panel(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    let cracks: Vec<&crate::plan::Chunk> =
        plan.chunks.iter().filter(|c| c.crack.is_some()).collect();
    let mut h = String::new();
    h.push_str("<section class=\"panel cq\">\n");
    let small = match cracks.first().and_then(|c| c.crack.as_ref()) {
        Some(ci) => {
            let steel = if ci.steel_path {
                esc(&ctx.tuning.planner.steel_path_label)
            } else {
                String::new()
            };
            format!("{}{steel}", esc(&ci.fissure_node))
        }
        None => "—".to_owned(),
    };
    h.push_str(&format!("<h2>Crack queue<small>{small}</small></h2>\n"));
    if cracks.is_empty() {
        h.push_str("<div class=\"row ch\"><span class=\"mut\">No live cracks.</span></div>\n");
    } else {
        for (i, c) in cracks.iter().enumerate() {
            let ci = c.crack.as_ref().expect("filtered to crack chunks");
            let proven = ctx.proven_done.contains(&c.id);
            let next = if i == 0 { "<em class=nx>NEXT</em>" } else { "" };
            let extra = if ci.covers > 0 {
                format!(" &middot; +{} also", ci.covers)
            } else {
                String::new()
            };
            // Pass 38: the radshare marker rides the relic column —
            // the same literal the Hunt crack line renders, present
            // only when the verdict recommends sharing.
            let rad = if ci.radshare.is_some() {
                " <em class=rad>radshare</em>"
            } else {
                ""
            };
            let mut class = String::from("row ch tick");
            if i == 0 {
                class.push_str(" next");
            }
            if proven {
                class.push_str(" done");
            }
            if ci.unobtainable {
                class.push_str(" dead");
            }
            h.push_str(&format!(
                "<label class=\"{class}\">\n<input type=\"checkbox\" data-id=\"{id}\"{checked}>\n{icon}\n<div><b>{gear}</b>{next}<br><span class=\"sm {rarity}\">{part}{extra}</span></div>\n<div class=\"rl\"><b class=\"cy\">{relic}</b><br><span class=\"mut sm\">{refine}</span>{rad}</div>\n</label>\n",
                class = class,
                id = esc(&c.id),
                checked = if proven { " checked" } else { "" },
                icon = mi_span(&ci.gear, ctx, "mi lg"),
                gear = esc(&ci.gear),
                next = next,
                part = esc(&short_part(&ci.part, &ci.gear)),
                extra = extra,
                rarity = rarity_css(ci.rarity),
                relic = esc(&ci.relic),
                refine = esc(&ci.refinement),
                rad = rad,
            ));
        }
    }
    // Farm hints are carried on the plan but not rendered here:
    // the appended Farm rows were removed 2026-10-23 (Seth) — with
    // real stock they dumped every farmable relic into the panel.
    h.push_str("</section>\n");
    h
}

/// The Varzia strip: the rotation's covered gear as `.chip`s.
fn resurgence_panel(r: &crate::plan::ResurgenceView, ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    h.push_str(&format!(
        "<section class=\"panel\">\n<h2>{}<small>{}</small><em class=\"pill\">{} left</em></h2>\n<div class=\"chips\">\n",
        esc(&r.contact),
        esc(&r.location),
        esc(bare_duration(&r.ends_in)),
    ));
    for gear in &r.gears {
        h.push_str(&format!(
            "<span class=\"chip\">{}{}</span>\n",
            mi_span(gear, ctx, "mi"),
            esc(&display_gear(gear)),
        ));
    }
    h.push_str("</div>\n</section>\n");
    h
}

/// The Taubound pack strip: payout progress, the community gate, and the
/// remaining arcanes. All content from the pack TOML.
fn pack_panel(ctx: &RenderContext<'_>) -> String {
    let Some(pack) = &ctx.pack else {
        return String::new();
    };
    if !pack.live {
        return String::new();
    }
    let pct = match pack.contegorus_health {
        Some(v) => (v * 100.0).clamp(0.0, 100.0).round() as u32,
        None => 0,
    };
    let health = match pack.contegorus_health {
        Some(v) => format!("{} {:.2}", esc(&pack.event_label), v),
        None => format!("{} —", esc(&pack.event_label)),
    };
    format!(
        "<section class=\"panel\">\n<h2>{}<small>ends {}</small></h2>\n<div class=\"pb\"><span>{payout}</span><div class=\"bar\"><i style=\"width:{pct}%\"></i></div><b>{pct}%</b></div>\n<div class=\"kv\"><span class=\"mut\">{gated}</span><span>{health}</span></div>\n<div class=\"kv\"><span class=\"mut\">Arcanes</span><span class=\"chips\">{arcanes}</span></div>\n</section>\n",
        esc(&titlecase(&pack.pack_name)),
        esc(&pack.expires_label),
        payout = esc(&pack.payout_label),
        pct = pct,
        gated = esc(&pack.gated_label),
        health = health,
        arcanes = pack
            .arcane_line
            .split(',')
            .map(|a| format!("<span class=\"chip\">{}</span>", esc(a.trim())))
            .collect::<Vec<_>>()
            .join(""),
    )
}

// ---------------------------------------------------------------------------
// Hunt (p2)
// ---------------------------------------------------------------------------

/// The mock's article filter keys: `current`, `vaulted`, and the additive
/// `varzia` tag (the reference uses `data-f="vaulted varzia"`).
fn data_f(g: &crate::plan::GoalCard) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if g.vaulted {
        parts.push("vaulted");
    } else {
        parts.push("current");
    }
    if g.resurgence_live {
        parts.push("varzia");
    }
    parts.join(" ")
}

fn hunt_html(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    let varzia = plan.goals.iter().filter(|g| g.resurgence_live).count();
    let current = plan.goals.iter().filter(|g| !g.vaulted).count();
    let vaulted = plan.goals.iter().filter(|g| g.vaulted).count();
    // gear -> the crack chunk that plans it, for the `.gf` recommendation.
    let mut cracks: HashMap<&str, &crate::plan::CrackInfo> = HashMap::new();
    for c in &plan.chunks {
        if let Some(ci) = &c.crack {
            cracks.entry(ci.gear.as_str()).or_insert(ci);
        }
    }
    let varzia_end = plan.resurgence.as_ref().map(|r| r.ends_in.as_str());
    let legend = match plan.chunks.iter().find_map(|c| c.crack.as_ref()) {
        Some(ci) => {
            let steel = if ci.steel_path {
                esc(&ctx.tuning.planner.steel_path_label)
            } else {
                String::new()
            };
            format!(
                "Row: part &middot; relic &middot; refinement &middot; all cracks: {} ({}){steel}",
                esc(&ci.fissure_node),
                esc(&ci.fissure_mission),
            )
        }
        None => "Row: part &middot; relic &middot; refinement".to_owned(),
    };
    let mut h = String::new();
    h.push_str("<div class=\"page\" id=\"p2\">\n");
    h.push_str(&frag_open("p2"));
    h.push_str("<div class=\"tool\">\n");
    h.push_str(&format!(
        "<button class=\"f on\" data-f=\"all\">All {}</button>\n",
        plan.goals.len()
    ));
    h.push_str(&format!(
        "<button class=\"f\" data-f=\"varzia\">Varzia {varzia}</button>\n"
    ));
    h.push_str(&format!(
        "<button class=\"f\" data-f=\"current\">Unvaulted {current}</button>\n"
    ));
    h.push_str(&format!(
        "<button class=\"f\" data-f=\"vaulted\">Vaulted {vaulted}</button>\n"
    ));
    h.push_str(&format!("<span class=\"mut sm lg-note\">{legend}</span>\n"));
    h.push_str("</div>\n<div class=\"gg\">\n");
    for g in &plan.goals {
        h.push_str(&hunt_card_html(
            g,
            ctx,
            cracks.get(g.gear.as_str()).copied(),
            varzia_end,
        ));
    }
    h.push_str("</div>\n");
    h.push_str(FRAG_CLOSE);
    h.push_str("</div>\n");
    h
}

/// One Hunt card in the reference's exact `.g` pattern: header (icon,
/// name, V/VARZIA badges, pips), one `.r` per wanted part, one `.gf`
/// footer recommending the relic batch (or the Varzia window).
fn hunt_card_html(
    g: &crate::plan::GoalCard,
    ctx: &RenderContext<'_>,
    crack: Option<&crate::plan::CrackInfo>,
    varzia_end: Option<&str>,
) -> String {
    let mut h = String::new();
    h.push_str(&format!(
        "<article class=\"g\" data-f=\"{}\">\n<div class=\"gh\">\n{}\n<b>{}</b>\n",
        data_f(g),
        mi_span(&g.gear, ctx, "mi lg"),
        esc(&g.gear),
    ));
    // Addendum #88: a Varzia-live goal shows the Varzia marker
    // INSTEAD of the vault V — never both.
    if g.vaulted && !g.resurgence_live {
        h.push_str("<em class=\"bd v\">V</em>\n");
    }
    if g.resurgence_live {
        h.push_str("<em class=\"bd z\">VARZIA</em>\n");
    }
    h.push_str(&format!("<span class=\"gc\">{}</span>\n</div>\n", seg_html(g)));
    for p in &g.parts {
        let part = short_part(&p.name, &g.gear);
        let rarity = rarity_css(p.rarity);
        let (relic, refine, tier, vaulted) = match &p.deal {
            Some(d) => (
                d.relic.clone(),
                refinement_abbrev(&d.refinement),
                Some(d.source_tier),
                d.vaulted,
            ),
            None => (
                p.relics
                    .first()
                    .map(|r| r.relic.clone())
                    .unwrap_or_default(),
                String::new(),
                p.relics.first().map(|r| r.source_tier),
                p.relics.first().map(|r| r.vaulted).unwrap_or(false),
            ),
        };
        let vault_class = if vaulted { " v" } else { "" };
        let dead_class = if tier.map(|t| t.is_unobtainable()).unwrap_or(false) {
            " dead"
        } else {
            ""
        };
        // Addendum #84: a part already in hand (captured stock) is
        // denoted on the row itself — gold name with a check mark,
        // matching the Backlog pips' Have treatment.
        // Thirtieth pass (#93): an unverified-held part (Seth's
        // Hunt-tab assertion, capture has not verified it) is
        // denoted the same way but with ⍻ (U+237B) and a dimmed
        // gold name — never the ✓ of verified stock.
        let have = p.state == crate::plan::PartState::Have;
        let uvhave = p.state == crate::plan::PartState::HaveUnverified;
        let have_class = if have {
            " have"
        } else if uvhave {
            " uvhave"
        } else {
            ""
        };
        let part_label = if have {
            format!("✓ {part}")
        } else if uvhave {
            format!("⍻ {part}")
        } else {
            part.clone()
        };
        // Mutation affordance (Hunt ONLY — this renderer is the
        // Hunt card; Backlog renders pips through seg_html and
        // never emits these attributes): every row that is not
        // verified-Have toggles the part's unverified membership on
        // click. The full roster part name rides in data-uv for the
        // POST /unverified endpoint; verified rows get no handler.
        let uv_class = if have { "" } else { " uv" };
        let uv_attrs = if have {
            String::new()
        } else {
            format!(
                " data-uv=\"{}\" title=\"{}\" role=\"button\"",
                esc(&p.name),
                if uvhave {
                    "Unverified stock (⍻) — click to withdraw the assertion"
                } else {
                    "Click to assert unverified stock (⍻): I hold this — capture has not verified it"
                },
            )
        };
        h.push_str(&format!(
            "<div class=\"r{dead_class}{have_class}{uv_class}\"{uv_attrs}>\n<span class=\"pn\">{part}</span>\n<span class=\"rc {rarity}{vault_class}\">{relic}</span>\n<span class=\"pc {rarity}\">{refine}</span>\n</div>\n",
            dead_class = dead_class,
            have_class = have_class,
            uv_class = uv_class,
            uv_attrs = uv_attrs,
            part = esc(&part_label),
            rarity = rarity,
            vault_class = vault_class,
            relic = esc(&relic),
            refine = esc(&refine),
        ));
    }
    // Footer: the recommendation line (mock `.gf`).
    if g.resurgence_live {
        let ends = varzia_end
            .map(|e| esc(bare_duration(e)))
            .unwrap_or_else(|| "now".to_owned());
        h.push_str(&format!(
            "<div class=\"gf\"><span class=\"fa vz\">◆ In Varzia · {ends} left</span></div>\n"
        ));
    } else if let Some(ci) = crack {
        // Pass 38: the same `radshare` marker the crack queue row
        // renders, appended to the crack line when the verdict
        // recommends sharing.
        let rad = if ci.radshare.is_some() {
            " · <em class=rad>radshare</em>"
        } else {
            ""
        };
        h.push_str(&format!(
            "<div class=\"gf\"><span class=\"fa\">▸ <b>{relic}</b> ×{batch} · {refine} → {part}{rad}</span></div>\n",
            relic = esc(&ci.relic),
            batch = ci.batch,
            refine = esc(&refinement_abbrev(&ci.refinement)),
            part = esc(&short_part(&ci.part, &ci.gear)),
            rad = rad,
        ));
    } else if let Some(d) = g
        .parts
        .iter()
        .filter_map(|p| p.deal.as_ref())
        .next()
    {
        h.push_str(&format!(
            "<div class=\"gf\"><span class=\"fa\">▸ <b>{relic}</b> · {refine}</span></div>\n",
            relic = esc(&d.relic),
            refine = esc(&d.refinement),
        ));
    }
    h.push_str("</article>\n");
    h
}

// ---------------------------------------------------------------------------
// Backlog (p3)
// ---------------------------------------------------------------------------

fn backlog_html(plan: &Plan, ctx: &RenderContext<'_>) -> String {
    let varzia: Vec<&crate::plan::GoalCard> =
        plan.goals.iter().filter(|g| g.resurgence_live).collect();
    // The mock splits the vaulted list so Varzia live gear is not counted
    // twice: 6 + 21 + 34 = 61 in the reference.
    let unvaulted: Vec<&crate::plan::GoalCard> =
        plan.goals.iter().filter(|g| !g.vaulted).collect();
    let vaulted: Vec<&crate::plan::GoalCard> =
        plan.goals.iter().filter(|g| g.vaulted && !g.resurgence_live).collect();
    let mut h = String::new();
    h.push_str("<div class=\"page\" id=\"p3\">\n");
    h.push_str(&frag_open("p3"));
    h.push_str(&backlog_section("Varzia live", &varzia, ctx));
    h.push_str(&backlog_section("Unvaulted", &unvaulted, ctx));
    h.push_str(&backlog_section("Vaulted", &vaulted, ctx));
    if !ctx.backlog_categories.is_empty() {
        h.push_str("<h3>Non-prime</h3>\n");
        let mut cats: Vec<(&String, &Vec<String>)> = ctx
            .backlog_categories
            .iter()
            .filter(|(_, items)| !items.is_empty())
            .collect();
        cats.sort_by(|a, b| a.0.cmp(b.0));
        for (cat, items) in cats {
            // The mock opens the small categories and keeps the long
            // lists collapsed; deterministic and data-sized, not authored.
            let open = if items.len() <= 10 { " open" } else { "" };
            h.push_str(&format!(
                "<details class=\"np\"{open}>\n<summary>{} <small>{}</small></summary>\n<div class=\"chips\">\n",
                esc(cat),
                items.len(),
            ));
            for item in items {
                h.push_str(&format!("<span class=\"chip\">{}</span>\n", esc(item)));
            }
            h.push_str("</div>\n</details>\n");
        }
    }
    h.push_str(FRAG_CLOSE);
    h.push_str("</div>\n");
    h
}

/// One `.bg` backlog section: icon, trimmed name, pips and count.
fn backlog_section(
    title: &str,
    goals: &[&crate::plan::GoalCard],
    ctx: &RenderContext<'_>,
) -> String {
    if goals.is_empty() {
        return String::new();
    }
    let mut h = String::new();
    h.push_str(&format!(
        "<h3>{} <small>{}</small></h3>\n<div class=\"bg\">\n",
        esc(title),
        goals.len()
    ));
    for g in goals {
        h.push_str(&format!(
            "<span class=\"bc\">{}{}<span class=\"bn\">{}</span>{}</span>\n",
            mi_span(&g.gear, ctx, "mi"),
            "",
            esc(&display_gear(&g.gear)),
            seg_html(g),
        ));
    }
    h.push_str("</div>\n");
    h
}

// ---------------------------------------------------------------------------
// Routine (p4)
// ---------------------------------------------------------------------------

fn routine_html(ctx: &RenderContext<'_>) -> String {
    let mut h = String::new();
    h.push_str("<div class=\"page\" id=\"p4\">\n");
    h.push_str(&frag_open("p4"));
    h.push_str("<div class=\"g3\">\n<div>\n");
    h.push_str("<h2>Today <small>standing / focus left</small></h2>\n");
    h.push_str("<div class=\"caps\">\n");
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
                "<div class=\"cap\"><span>Focus</span><div class=\"bar\"><i style=\"width:0%\"></i></div><b>{focus_cap}</b></div>\n<div class=\"cap\"><span>{}</span><div class=\"bar\"><i style=\"width:0%\"></i></div><b>{stand_cap}</b></div>\n",
                esc(caps.label_for("main")),
            ));
        }
    }
    h.push_str("</div>\n");
    // "This week": the cadence checklist. Item 50a: the Sortie row (its
    // own reset) belongs with the checklist items here, not in the Today
    // standing block above — it is a cadence item, not a daily cap.
    h.push_str("<h2>This week</h2>\n<div class=\"wkg\">\n");
    let sortie = &ctx.tuning.planner.sortie;
    let sortie_done = ctx.weekly_done.contains_key(&sortie.ledger_id);
    h.push_str(&format!(
        "<label class=\"cell {done}\"><input type=\"checkbox\" data-id=\"{id}\"{checked}>\n<span class=\"tx\"><b>{name}</b><span class=\"mut sm\">{reset}{proven}</span></span>\n</label>\n",
        done = if sortie_done { "done" } else { "" },
        id = esc(&sortie.ledger_id),
        checked = if sortie_done { " checked" } else { "" },
        name = esc(&sortie.name),
        reset = esc(&sortie.reset_label),
        proven = if sortie_done { " · log" } else { "" },
    ));
    for w in weekly_board(ctx.tuning) {
        let done_label = ctx.weekly_done.get(&w.id).cloned().unwrap_or_default();
        let done = !done_label.is_empty();
        let mut class = String::from("cell");
        if w.deprioritized {
            class.push_str(" dim");
        }
        if done {
            class.push_str(" done");
        }
        h.push_str(&format!(
            "<label class=\"{class}\"><input type=\"checkbox\" data-id=\"wk-{id}\"{checked}>\n<span class=\"tx\"><b>{name}</b><span class=\"mut sm\">{limit}{proven}</span></span>\n<span class=\"pts\">{time}</span></label>\n",
            class = class,
            id = w.id,
            checked = if done { " checked" } else { "" },
            name = esc(&w.name),
            limit = esc(&w.limit),
            time = esc(&w.time),
            proven = if done {
                format!(" &middot; proven {}", esc(&done_label))
            } else {
                String::new()
            },
        ));
    }
    h.push_str("</div>\n</div>\n<div>\n");
    h.push_str(&nightwave_html(ctx));
    h.push_str("</div>\n</div>\n");
    h.push_str(FRAG_CLOSE);
    h.push_str("</div>\n");
    h
}

/// Nightwave: the act board grouped Daily / Weekly / Elite exactly as the
/// mock lays it out (name + description everywhere acts render).
fn nightwave_html(ctx: &RenderContext<'_>) -> String {
    let title = ctx.tuning.planner.nightwave.name.as_str();
    let Some(nw) = &ctx.world.nightwave else {
        return format!(
            "<h2>{}</h2>\n<div class=\"cell dim\"><span class=\"tx\"><b>No season live</b><span class=\"mut sm\">world state</span></span></div>\n",
            esc(title)
        );
    };
    let mut h = String::new();
    h.push_str(&format!(
        "<h2>{} <small>Season {}</small></h2>\n",
        esc(title),
        nw.season,
    ));
    let groups: [(&str, fn(&crate::model::NightWaveChallenge) -> bool); 3] = [
        ("Daily", |a| a.is_daily),
        ("Weekly", |a| !a.is_daily && !a.is_elite),
        ("Elite", |a| a.is_elite),
    ];
    for (name, pred) in groups {
        let acts: Vec<&crate::model::NightWaveChallenge> =
            nw.active_challenges.iter().filter(|a| pred(a)).collect();
        if acts.is_empty() {
            continue;
        }
        let total: u32 = acts.iter().map(|a| a.standing).sum();
        h.push_str(&format!(
            "<h3>{} <small>{} · {}</small></h3>\n",
            name,
            acts.len(),
            fmt_compact(total),
        ));
        for a in acts {
            let done = ctx.proven_done.contains(&a.id);
            h.push_str(&format!(
                "<label class=\"cell {done}\"><input type=\"checkbox\" data-id=\"nw-{id}\"{checked}>\n<span class=\"tx\"><b>{title}</b><span class=\"mut sm\">{desc}</span></span>\n<span class=\"pts\">{standing}</span></label>\n",
                done = if done { "done" } else { "" },
                id = esc(&a.id),
                checked = if done { " checked" } else { "" },
                title = esc(&a.title),
                desc = esc(&a.desc),
                standing = fmt_compact(a.standing),
            ));
        }
    }
    h
}

// ---------------------------------------------------------------------------
// Shared: short part name
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Daily caps ("Today" strip)
// ---------------------------------------------------------------------------

/// The Routine tab's "Today" cap grid: Focus + the bare shared
/// `DailyAffiliation` pool, then one compact cap per family. Remaining
/// values are the profile's own REMAINING amounts. Each `.cap` also
/// carries the exact `data-left`/`data-cap` (the §3 audit parses them;
/// the visible number is compact like the mock).
pub fn pool_bars_html(d: &DailyRender, caps: &crate::tuning::DailyCapsTuning) -> String {
    let mut h = String::new();
    h.push_str(&cap_html("Focus", d.focus_remaining, d.focus_cap));
    h.push_str(&cap_html(
        caps.label_for("main"),
        d.affiliation_remaining,
        d.affiliation_cap,
    ));
    let have: HashMap<&str, u32> = d.families.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    for fam in &caps.families {
        if fam.eq_ignore_ascii_case("pvp") {
            continue;
        }
        if let Some(remaining) = have.get(fam.as_str()).copied() {
            h.push_str(&cap_html(caps.label_for(fam), remaining, d.affiliation_cap));
        }
    }
    h
}

/// One `.cap`: label, remaining-fraction bar, compact number. `data-left`
/// / `data-cap` carry the raw values for the audit.
fn cap_html(name: &str, remaining: u32, cap: u32) -> String {
    let pct = if cap == 0 {
        0.0
    } else {
        (100.0 * remaining as f64 / cap as f64).clamp(0.0, 100.0)
    };
    format!(
        "<div class=\"cap\" data-left=\"{remaining}\" data-cap=\"{cap}\" title=\"{} / {}\"><span>{name}</span><div class=\"bar\"><i style=\"width:{pct:.0}%\"></i></div><b>{compact}</b></div>\n",
        fmt_int(remaining),
        fmt_int(cap),
        name = esc(name),
        remaining = remaining,
        cap = cap,
        pct = pct,
        compact = fmt_compact(remaining),
    )
}

// ---------------------------------------------------------------------------
// Art direction (BRIEF §19, twelfth pass §1)
// ---------------------------------------------------------------------------
//
// The first block is Seth's reference stylesheet, adopted verbatim. The
// second block extends it ONLY for live states the hand-authored mock
// could not show: struck/ticked rows, the tick checkbox column, and the
// compact filter/legend glue. Nothing in the reference block is edited.

const CSS: &str = r#"
:root{--panel:#151a23;--cell:#131824;--line:#232b38;--mut:#7e8898;--tx:#c9d2de;--gold:#bfa05a;--cy:#aee3ef;--amb:#e3a85a;color-scheme:dark}
html{background:#0a0d12 radial-gradient(1200px 500px at 50% -10%,#141b28,transparent 70%)}
body{margin:0;color:var(--tx);font:14px/1.4 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif}
.wrap{max-width:1280px;margin:0 auto;padding:22px 24px 48px}
b,h1,h2,h3,.brand,.tab,.tm{font-family:Rajdhani,-apple-system,"Segoe UI",sans-serif}
.top{display:flex;align-items:center;gap:20px;flex-wrap:wrap;margin-bottom:14px}
.brand{font-size:26px;font-weight:700;letter-spacing:.3em;color:var(--gold)}
.pills{display:flex;gap:8px;flex-wrap:wrap;margin-left:auto}.pills span{background:var(--cell);border:1px solid var(--line);padding:4px 12px;font-size:13px}.pills b{color:var(--cy);margin-right:6px;letter-spacing:.08em}
.stamp{width:100%;color:var(--mut);font-size:12px;margin-top:-10px}
.mut{color:var(--mut)}.sm{font-size:12px}.cy{color:var(--cy)}
input[name=tab]{display:none}
.tabs{display:flex;gap:6px;margin-bottom:16px}
.tab{padding:7px 22px;cursor:pointer;font-weight:700;font-size:15px;letter-spacing:.14em;text-transform:uppercase;color:var(--mut);background:var(--cell);border:1px solid #1f2735;text-decoration:none;clip-path:polygon(7px 0,100% 0,100% calc(100% - 7px),calc(100% - 7px) 100%,0 100%,0 7px)}
.page{display:none}
#t1:checked~main #p1,#t2:checked~main #p2,#t3:checked~main #p3,#t4:checked~main #p4{display:block}
#t1:checked~.tabs .tab[data-tab=t1],#t2:checked~.tabs .tab[data-tab=t2],#t3:checked~.tabs .tab[data-tab=t3],#t4:checked~.tabs .tab[data-tab=t4]{color:var(--cy);border-color:var(--cy)}
h2{display:flex;align-items:baseline;gap:10px;font-size:12px;letter-spacing:.18em;text-transform:uppercase;color:var(--mut);margin:0 0 10px;font-weight:600}
h2 small,h3 small{font-size:11px;letter-spacing:.04em;text-transform:none;font-weight:500;color:#5d6676}
h3{font-size:12px;letter-spacing:.14em;text-transform:uppercase;color:var(--mut);margin:18px 0 8px}
.pill{margin-left:auto;font-style:normal;color:var(--gold);border:1px solid var(--gold);padding:0 8px;font-size:11px;letter-spacing:.08em}
.g2{display:grid;grid-template-columns:minmax(0,.85fr) minmax(0,1.15fr);gap:14px;margin-bottom:14px}.g2.b{grid-template-columns:minmax(0,1.15fr) minmax(0,.85fr)}
.panel{background:var(--panel);border:1px solid var(--line);padding:16px 18px;clip-path:polygon(10px 0,100% 0,100% calc(100% - 10px),calc(100% - 10px) 100%,0 100%,0 10px)}
.panel.cq{padding-left:0;padding-right:0}
.cq h2{padding-left:18px;padding-right:18px}
.cq .ch{padding-left:18px;padding-right:18px}
.row{display:grid;align-items:center;gap:12px;padding:9px 0;border-top:1px solid #1c2330}.row:first-of-type{border-top:0}
.ev{grid-template-columns:64px 1fr auto}.ch{grid-template-columns:40px 1fr auto auto}
.tm{font-size:21px;font-weight:700;color:var(--tx)}.tm.hot{color:var(--amb)}.tm.warm{color:var(--gold)}
.rl{text-align:right}.ch b{font-size:17px}.nx{font-style:normal;font-size:10px;letter-spacing:.12em;color:var(--cy);border:1px solid var(--cy);padding:0 5px;margin-left:8px}.rad{font-style:normal;font-size:10px;letter-spacing:.12em;color:var(--gold);border:1px solid var(--gold);padding:0 5px}
.ch.next{position:relative;background:linear-gradient(90deg,rgba(174,227,239,.07),transparent)}
.ch.next::before{content:"";position:absolute;left:0;top:0;bottom:0;width:3px;background:var(--cy)}
.mi{width:22px;height:22px;flex:none;position:relative;overflow:hidden;display:inline-flex;align-items:center;justify-content:center;font:700 12px Rajdhani,sans-serif;color:var(--gold);background:#0d1117;border:1px solid var(--line)}
.mi.lg{width:36px;height:36px;font-size:18px}.mi img{position:absolute;inset:0;width:100%;height:100%;object-fit:cover;background:#0d1117}
.chips{display:flex;flex-wrap:wrap;gap:6px}.chip{display:inline-flex;align-items:center;gap:7px;background:var(--cell);border:1px solid #1f2735;padding:3px 10px 3px 3px;font-size:13px}
.chips .chip:not(:has(.mi)){padding:3px 10px}.chip em{font-style:normal;color:var(--gold);font-size:11px}
.pb{display:grid;grid-template-columns:1fr 120px auto;gap:12px;align-items:center;padding-bottom:10px}.pb b{font-size:16px;color:var(--cy)}
.bar{height:5px;background:#0d1117}.bar i{display:block;height:100%;background:var(--gold)}
.kv{display:grid;grid-template-columns:90px 1fr;gap:10px;padding:8px 0;border-top:1px solid #1c2330;align-items:center}
.tool{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-bottom:12px}
.f{font:600 13px Rajdhani,sans-serif;letter-spacing:.08em;color:var(--mut);background:var(--cell);border:1px solid #1f2735;padding:5px 14px;cursor:pointer}.f.on{color:var(--cy);border-color:var(--cy)}.lg-note{margin-left:auto}
.gg{display:grid;grid-template-columns:repeat(auto-fill,minmax(290px,1fr));gap:12px}
.g{background:var(--panel);border:1px solid var(--line);padding:12px 14px;clip-path:polygon(8px 0,100% 0,100% calc(100% - 8px),calc(100% - 8px) 100%,0 100%,0 8px)}.g.hide{display:none}
.gh{display:flex;align-items:center;gap:9px;margin-bottom:8px}.gh>b{font-size:18px;white-space:nowrap}
.bd{font-style:normal;font-size:10px;letter-spacing:.1em;padding:0 5px;border:1px solid var(--mut);color:var(--mut)}.bd.z{color:var(--gold);border-color:var(--gold)}
.gc{margin-left:auto;display:flex;align-items:center;gap:8px;color:var(--mut);font-size:12px}.gc i,.bc i{font-style:normal}
.segs{display:inline-flex;gap:2px}.seg{width:13px;height:5px;background:#252e3d}.seg.have{background:var(--gold)}.seg.uvhave{background:var(--gold);opacity:.45}.seg.unk{height:3px;background:none;border:1px dashed var(--mut)}
.r{display:grid;grid-template-columns:1fr auto auto;gap:8px;align-items:center;padding:4px 0;border-top:1px solid #1c2330;font-size:13px}
.pn{font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.pn em{font-style:normal;color:var(--gold)}
.rc{border:1px solid;padding:0 7px;font-size:12px;white-space:nowrap}.rc.v{opacity:.7}.pc{font-size:12px;text-align:right;font-variant-numeric:tabular-nums}
.r.dead .pn,.r.dead .rc,.r.dead .pc{text-decoration:line-through;opacity:.55}
.common{color:#b08d4f}.uncommon{color:#c0c8d4}.rare{color:#e3c878}.rc.common,.rc.uncommon,.rc.rare{border-color:currentColor;background:#0d1117}
.gf{display:flex;justify-content:space-between;gap:8px;margin-top:8px;padding:6px 9px;background:#0d1420;border-left:3px solid var(--cy);font-size:12px}.gf b{color:var(--cy)}.fa{color:var(--tx)}.fa.vz{color:var(--gold)}.gf:has(.vz){border-color:var(--gold)}
.bg{display:grid;grid-template-columns:repeat(auto-fill,minmax(200px,1fr));gap:6px}.bc{display:flex;align-items:center;gap:8px;background:var(--cell);border:1px solid #1f2735;padding:4px 10px 4px 4px;font-size:13px}.bn{flex:1;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.bc i{color:var(--mut);font-size:11px}
details.np{background:var(--cell);border:1px solid #1f2735;padding:8px 12px;margin-bottom:6px}summary{cursor:pointer;color:var(--gold);font-weight:600}summary small{color:var(--mut)}details .chips{margin-top:8px}
.g3{display:grid;grid-template-columns:minmax(0,1.15fr) minmax(0,1fr);gap:24px}.g3 h2:not(:first-child){margin-top:22px}.g3 h3:first-of-type{margin-top:0}
.caps{display:grid;grid-template-columns:repeat(auto-fill,minmax(130px,1fr));gap:6px;margin-bottom:6px}
.cap{background:var(--cell);border:1px solid #1f2735;padding:7px 10px;display:grid;gap:5px;font-size:12px}.cap span{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.cap b{font-size:14px;color:var(--cy)}
.wkg{display:grid;grid-template-columns:repeat(auto-fill,minmax(230px,1fr));gap:6px}
.cell{display:flex;align-items:center;gap:10px;background:var(--cell);border:1px solid #1f2735;padding:7px 10px;cursor:pointer;margin-bottom:6px}.wkg .cell,.caps+.cell{margin-bottom:0}.cell:hover{border-color:#3a4354}.cell.dim{opacity:.7}
.tx{flex:1;display:flex;flex-direction:column;min-width:0}.tx b{font-size:15px}.pts{color:var(--gold);font-size:12px;white-space:nowrap;font-variant-numeric:tabular-nums}
.cell.done{opacity:.4}.cell.done .tx b{text-decoration:line-through}
input[type=checkbox]{appearance:none;-webkit-appearance:none;flex:none;width:18px;height:18px;margin:0;border:1.5px solid #3a4354;background:#0d1117;border-radius:3px;cursor:pointer;display:grid;place-content:center}
input[type=checkbox]:checked{background:var(--gold);border-color:var(--gold)}
input[type=checkbox]:checked::after{content:"";width:9px;height:5px;border:2px solid #0a0d12;border-top:0;border-right:0;transform:rotate(-45deg) translate(1px,-1px)}
@media(max-width:860px){.g2,.g2.b,.g3{grid-template-columns:1fr}.lg-note{margin-left:0}}

/* Fracker extensions: states the static mock cannot show. */
@keyframes baro-pulse{0%,100%{opacity:1}50%{opacity:.5}}
.pills span.baro-live{animation:baro-pulse 1.6s ease-in-out infinite}
.row.done{opacity:.45}.row.done b,.row.done .tm{text-decoration:line-through}
.row.dead .rl,.row.dead>div{opacity:.55;text-decoration:line-through}
.g .r.dead{opacity:.75}
.g .r.have .pn{color:var(--gold)}
.g .r.uvhave .pn{color:var(--gold);opacity:.75}
.g .r.uv{cursor:pointer}
.g .r.uv:hover .pn{text-decoration:underline dotted}
label.row{cursor:pointer}
.row.ch.tick{grid-template-columns:auto 40px 1fr auto auto}
.row.ch.tick input[type=checkbox]{align-self:center}
.g.done{opacity:.45}.g.done .gh>b{text-decoration:line-through}
.cap .bar{margin:0}
"#;

const JS: &str = r#"
// Dynamic page state, factored into re-runnable pieces (forty-first
// pass): checkbox ticks, the Hunt filter, and the unverified (⍻) row
// clicks are applied by functions the fragment-sync script re-runs
// (window.__frackerResync) after it swaps a panel's innerHTML, so a
// swapped panel behaves exactly like a freshly loaded one. Initial
// page load calls the same pieces in the same order.
(function () {
  var day = document.body.getAttribute('data-day') || '';
  var served = document.body.hasAttribute('data-fracker-ticks');
  var done = {};
  // Checkbox ticks: the resident's loopback HTTP server is the store
  // of record (POST /tick, keyed by item id + in-game day) and
  // re-applies the ticks it injects onto <body data-fracker-ticks>
  // (base64 JSON) on every serve — and in every fragment payload, so
  // a swap carries the same store. localStorage remains only a
  // progressive cache for a file:// page.
  function loadTicks() {
    day = document.body.getAttribute('data-day') || '';
    done = {};
    try {
      var raw = document.body.getAttribute('data-fracker-ticks');
      if (raw) {
        var store = JSON.parse(atob(raw)).days || {};
        (store[day] || []).forEach(function (i) { done[i] = true; });
      }
    } catch (e) {}
  }
  function key(id) { return 'fracker:' + day + ':' + id; }
  function host(box) { return box.closest('.cell,.row,.g'); }
  function paint(box, on) {
    var h = host(box);
    if (h) { h.classList.toggle('done', on); }
    if (box.checked !== on) { box.checked = on; }
  }
  function bindBox(box) {
    var id = box.getAttribute('data-id');
    var on = !!done[id];
    // localStorage is only a fallback for a file:// page: when the
    // resident serves the page, its tick store is authoritative.
    if (!served) {
      try { if (localStorage.getItem(key(id)) === '1') { on = true; } } catch (e) {}
    }
    if (on) { paint(box, true); }
    if (box.__frackerTick) { return; }
    box.__frackerTick = true;
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
  }
  function applyTicks() {
    loadTicks();
    document.querySelectorAll('input[type=checkbox][data-id]').forEach(bindBox);
  }
  applyTicks();
  // Tabs: the loopback server records the active tab and renders the
  // matching radio checked, so a fresh load — and an app restart —
  // lands back where he was (§2, item 46). (Fragment swaps never
  // touch the radios, so the viewed tab also survives every regen.)
  // The tab links are real (they work without JS: `?tab=<slug>` is
  // recorded on the next GET); when served, we record via a light
  // fetch and switch instantly instead of reloading. A file:// demo
  // export falls back to ?tab / sessionStorage.
  function showTab(id) {
    var r = document.getElementById(id);
    if (r) { r.checked = true; }
    try { sessionStorage.setItem('fracker:tab', id); } catch (e) {}
  }
  try {
    var q = new URLSearchParams(location.search).get('tab');
    var qa = q && document.querySelector('.tab[href="?tab=' + q + '"]');
    if (qa) { showTab(qa.getAttribute('data-tab')); }
    else if (!served) {
      var t = sessionStorage.getItem('fracker:tab');
      if (t) { showTab(t); }
    }
  } catch (e) {}
  document.querySelectorAll('.tabs .tab').forEach(function (a) {
    a.addEventListener('click', function (ev) {
      showTab(a.getAttribute('data-tab'));
      if (served) {
        ev.preventDefault();
        var slug = (a.getAttribute('href') || '').split('tab=')[1] || '';
        try { fetch('/tab/' + encodeURIComponent(slug)); } catch (e) {}
      }
    });
  });
  // Hunt's in-page filter controls (reference behaviour, CSS-driven).
  var curFilter = 'all';
  try { curFilter = sessionStorage.getItem('fracker:f') || 'all'; } catch (e) {}
  function filt(f) {
    curFilter = f;
    document.querySelectorAll('.f').forEach(function (x) {
      x.classList.toggle('on', x.dataset.f === f);
    });
    document.querySelectorAll('.g').forEach(function (g) {
      g.classList.toggle('hide', f !== 'all' && g.dataset.f.split(' ').indexOf(f) < 0);
    });
    try { sessionStorage.setItem('fracker:f', f); } catch (e) {}
  }
  function bindFilters() {
    document.querySelectorAll('.f').forEach(function (x) {
      if (x.__frackerFilter) { return; }
      x.__frackerFilter = true;
      x.addEventListener('click', function () { filt(x.dataset.f); });
    });
  }
  function applyFilter() { filt(curFilter); }
  bindFilters();
  applyFilter();
  // Unverified stock (thirtieth pass): Hunt part rows that are not
  // verified-Have carry data-uv with the full roster part name.
  // Clicking POSTs the toggle to the loopback server, which
  // persists stock.json and regenerates the plan — the new ⍻ state
  // lands through the same fragment swap as every other
  // regeneration. Only Hunt renders these rows, so only Hunt can
  // mutate the unverified set.
  function bindUv() {
    document.querySelectorAll('.r.uv[data-uv]').forEach(function (row) {
      if (row.__frackerUv) { return; }
      row.__frackerUv = true;
      row.addEventListener('click', function () {
        try {
          fetch('/unverified', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ name: row.getAttribute('data-uv') })
          });
        } catch (e) {}
      });
    });
  }
  bindUv();
  // The fragment-sync script's re-entry point after a swap: re-read
  // the (possibly updated) tick/day payload and re-apply every
  // dynamic behaviour to the swapped-in DOM. Bindings are
  // once-per-element, so panels that were not swapped are untouched.
  window.__frackerResync = function () {
    applyTicks();
    bindFilters();
    applyFilter();
    bindUv();
  };
})();
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        ActionKind, Chunk, CrackInfo, GoalAction, GoalCard, PartDeal, PartRow, PartState, Rarity,
    };

    fn empty_ctx<'a>(
        world: &'a WorldState,
        t: &'a Tuning,
        weekly: &'a HashMap<String, String>,
        proven: &'a HashSet<String>,
        icons: &'a HashMap<String, String>,
        cats: &'a HashMap<String, Vec<String>>,
        daily: Option<DailyRender>,
    ) -> RenderContext<'a> {
        RenderContext {
            world,
            generated_label: "2026-10-18 04:03 UTC".to_owned(),
            generation: "test-generation".to_owned(),
            targets_status: "backlog 63 primes · profile live (MR30)".to_owned(),
            world_status: "fixture".to_owned(),
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

    /// The HTML slice for one `<div class="page" id="pN">` block.
    fn page<'a>(html: &'a str, id: &str) -> &'a str {
        let needle = format!("<div class=\"page\" id=\"{id}\">");
        let start = html
            .find(&needle)
            .unwrap_or_else(|| panic!("missing {needle}"));
        let rest = &html[start..];
        let end = rest[1..]
            .find("<div class=\"page\"")
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        &rest[..end]
    }

    fn goal(gear: &str, part: &str, vaulted: bool, varzia: bool) -> GoalCard {
        GoalCard {
            gear: gear.to_owned(),
            is_warframe: true,
            vaulted,
            resurgence_live: varzia,
            wants: vec![part.to_owned()],
            parts_have: 0,
            parts_total: 4,
            partially_unobtainable: false,
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
        }
    }

    #[test]
    fn render_covers_reference_sections_and_dark_rules() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let html = render_plan(&Plan::default(), &ctx);
        for section in ["First Crack", "Hunt", "Backlog", "Routine", "Today", "This week", "Nightwave"] {
            assert!(html.contains(section), "missing section: {section}");
        }
        assert!(html.contains("color-scheme:dark"), "refresh-flash rule");
        assert!(
            !html.contains("http-equiv=\"refresh\""),
            "no meta refresh: updates swap fragments in place (pass 41)"
        );
        assert!(
            html.contains("/ws") && html.contains("WebSocket"),
            "the page subscribes to generation pushes (§2 item 58)"
        );
        assert!(
            html.contains("msg.generation !== gen"),
            "the predicate compares the push against the loaded page's token"
        );
        assert!(
            html.contains("/fragments?gen="),
            "a push fetches the fragment diff instead of reloading (pass 41)"
        );
        assert!(
            html.contains("__frackerResync"),
            "swapped panels re-apply ticks/filter/uv state (pass 41)"
        );
        assert!(
            html.contains("fracker:push-off") && html.contains("sessionStorage"),
            "the breaker is session-scoped (§2 hotfix 19)"
        );
        assert!(
            html.contains("location.reload()"),
            "only the guarded last-resort reload remains (pass 41)"
        );
        assert!(
            html.contains("var FAIL_MAX = 3;"),
            "the last-resort reload waits out its failure budget (pass 41)"
        );
        assert!(
            html.contains("var POLL_MS = 60000;"),
            "a slow fragment poll replaces the meta-refresh backstop (pass 41)"
        );
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
    }

    /// Forty-first pass: the rendered page splits back into exactly
    /// the five fragments, in document order, each byte-contained in
    /// the page — the property the /fragments diff is built on.
    #[test]
    fn rendered_page_extracts_all_fragments_in_order() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let plan = Plan {
            goals: vec![goal(
                "Caliban Prime",
                "Caliban Prime Chassis Blueprint",
                false,
                false,
            )],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        let frags = extract_fragments(&html);
        let ids: Vec<&str> = frags.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, FRAGMENT_IDS);
        for (id, frag) in &frags {
            assert!(!frag.is_empty(), "fragment {id} is empty");
            assert!(
                html.contains(frag.as_str()),
                "fragment {id} is byte-contained in the page"
            );
        }
        let by_id = |want: &str| {
            frags
                .iter()
                .find(|(id, _)| id == want)
                .map(|(_, h)| h.as_str())
                .unwrap()
        };
        assert!(by_id("header").contains("FRACKER"), "header fragment");
        assert!(by_id("p1").contains("Crack queue"), "p1 fragment");
        assert!(by_id("p2").contains("data-f=\"all\""), "p2 fragment");
        assert!(by_id("p3").contains("class=\"bg\""), "p3 fragment");
        assert!(by_id("p4").contains("Today"), "p4 fragment");
        // A markerless page yields nothing (placeholder, old exports).
        assert!(extract_fragments("<html><body>plain</body></html>").is_empty());
        // Hash identity is the diff primitive.
        assert_eq!(fragment_hash("x"), fragment_hash("x"));
        assert_ne!(fragment_hash("x"), fragment_hash("y"));
    }

    /// Twelfth pass §1: four tabs in the reference's order. Thirteenth
    /// pass §2: each is a real `?tab=<slug>` link for server persistence.
    #[test]
    fn four_tabs_render_in_order() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let html = render_plan(&Plan::default(), &ctx);
        let mut last = 0usize;
        for (id, slug, label) in TABS {
            let needle = format!("data-tab=\"{id}\" href=\"?tab={slug}\">{label}</a>");
            let pos = html
                .find(&needle)
                .unwrap_or_else(|| panic!("missing tab link for {label}: {needle}"));
            assert!(pos >= last, "tab {label} renders out of order");
            last = pos;
        }
        // The reference's radio-driven mechanism, not a JS tab switcher.
        for id in ["t1", "t2", "t3", "t4"] {
            assert!(html.contains(&format!("id=\"{id}\"")), "radio {id}");
        }
        // First Crack is the default; the server re-injects the remembered
        // tab (see server::inject_tab).
        assert!(html.contains("id=\"t1\" checked"), "First Crack default");
        assert!(html.contains(".tab[data-tab=t1]"), "active-tab CSS selector");
        assert!(!html.contains("[for=t1]"), "label-based tab CSS is gone");
    }

    #[test]
    fn tab_slug_mapping_round_trips() {
        for (id, slug, _) in TABS {
            assert_eq!(tab_id_for_slug(slug), Some(id));
        }
        assert_eq!(tab_id_for_slug("nope"), None);
    }

    /// Twelfth pass §1: the mock's grids land on the right pages.
    #[test]
    fn reference_grid_classes_on_expected_pages() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let plan = Plan {
            goals: vec![
                goal("Caliban Prime", "Caliban Prime Chassis Blueprint", false, false),
                goal("Ivara Prime", "Ivara Prime Chassis Blueprint", true, true),
            ],
            resurgence: Some(crate::plan::ResurgenceView {
                contact: "Varzia".to_owned(),
                location: "Maroo's Bazaar (Mars)".to_owned(),
                ends_in: "in 11d".to_owned(),
                gears: vec!["Ivara Prime".to_owned()],
                currency: "Aya".to_owned(),
            }),
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        let p1 = page(&html, "p1");
        assert!(p1.contains("class=\"g2\""), "First Crack g2 grid");
        assert!(p1.contains("class=\"g2 b\""), "First Crack second g2 grid");
        let p2 = page(&html, "p2");
        assert!(p2.contains("class=\"gg\""), "Hunt card grid");
        let p3 = page(&html, "p3");
        assert!(p3.contains("class=\"bg\""), "Backlog cell grid");
        let p4 = page(&html, "p4");
        assert!(p4.contains("class=\"g3\""), "Routine two-column grid");
        assert!(p4.contains("class=\"caps\""), "Routine caps grid");
        assert!(p4.contains("class=\"wkg\""), "Routine weekly grid");
    }

    /// Twelfth pass §1: the in-page Hunt filter controls, exactly the
    /// mock's four buttons with its data-f keys.
    #[test]
    fn hunt_has_reference_filter_controls() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let plan = Plan {
            goals: vec![
                goal("Caliban Prime", "Caliban Prime Chassis Blueprint", false, false),
                goal("Ivara Prime", "Ivara Prime Chassis Blueprint", true, true),
            ],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        let p2 = page(&html, "p2");
        assert!(p2.contains("<div class=\"tool\">"), "filter toolbar");
        for f in ["all", "varzia", "current", "vaulted"] {
            assert!(
                p2.contains(&format!("class=\"f\" data-f=\"{f}\""))
                    || p2.contains(&format!("class=\"f on\" data-f=\"{f}\"")),
                "filter button data-f={f}"
            );
        }
        assert!(p2.contains("data-f=\"current\""), "article current tag");
        assert!(p2.contains("data-f=\"vaulted varzia\""), "article varzia tag");
        assert!(html.contains("function filt("), "CSS-driven filter JS");
    }

    /// Addendum #88: a vaulted goal that is live at Varzia shows the
    /// VARZIA badge instead of the vault V — never both; a vaulted
    /// goal not at Varzia keeps its V.
    #[test]
    fn varzia_badge_replaces_vault_badge_never_both() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let plan = Plan {
            goals: vec![
                goal("Protea Prime", "Protea Prime Blueprint", true, true),
                goal("Vauban Prime", "Vauban Prime Blueprint", true, false),
            ],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        let p2 = page(&html, "p2");
        assert_eq!(
            p2.matches("<em class=\"bd z\">VARZIA</em>").count(),
            1,
            "the Varzia-live goal carries the VARZIA badge"
        );
        assert_eq!(
            p2.matches("<em class=\"bd v\">V</em>").count(),
            1,
            "only the non-Varzia vaulted goal carries the V"
        );
    }

    /// Addendum #85: the Baro pill pulses only while a trader window
    /// is live (plan.baro present); the pulse keyframes ship in the
    /// style block either way.
    #[test]
    fn baro_pill_pulses_only_while_the_trader_is_live() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let live = Plan {
            baro: Some(crate::plan::BaroView {
                vendor: "Baro Ki'Teer".to_owned(),
                location: "Larunda Relay (Mercury)".to_owned(),
                ends_in: "Leaves in 18h24m".to_owned(),
            }),
            ..Default::default()
        };
        let html = render_plan(&live, &ctx);
        assert!(html.contains("class=\"baro-live\""), "live pill carries the pulse class");
        assert!(html.contains("@keyframes baro-pulse"), "pulse keyframes in the style block");
        assert!(html.contains("Leaves in 18h24m"), "live pill shows the leave countdown");
        let idle = render_plan(&Plan::default(), &ctx);
        assert!(
            !idle.contains("class=\"baro-live\""),
            "no pulse class attribute without a live window"
        );
    }

    /// Addendum #84: a part row in state Have is denoted on the Hunt
    /// card (row class + check mark) and fills its Backlog pip.
    #[test]
    fn have_part_is_denoted_on_hunt_row_and_backlog_pip() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let mut card = goal("Dual Zoren Prime", "Dual Zoren Prime Handle", true, false);
        card.parts[0].state = crate::plan::PartState::Have;
        card.parts_have = 1;
        let plan = Plan {
            goals: vec![card],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        let p2 = page(&html, "p2");
        assert!(p2.contains("class=\"r have\""), "Hunt row carries the have class");
        assert!(p2.contains("✓ Handle"), "Hunt row shows the check + part name");
        let p3 = page(&html, "p3");
        assert!(p3.contains("<i class=\"seg have\"></i>"), "Backlog Have pip fills");
        assert!(p3.contains("<i>1/4</i>"), "have count beside the pips");
    }

    #[test]
    fn unverified_part_denoted_with_not_check_and_toggles_only_on_hunt() {
        // Thirtieth pass (#93): an unverified-held part renders ⍻
        // (U+237B, never ✓) on its Hunt row and as a dimmed pip +
        // ⍻ count on Backlog; the click-to-toggle attributes exist
        // ONLY on Hunt rows that are not verified-Have.
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let mut card = goal("Dual Zoren Prime", "Dual Zoren Prime Handle", true, false);
        card.parts[0].state = crate::plan::PartState::HaveUnverified;
        card.parts_have = 1;
        let row = |name: &str, state: PartState| PartRow {
            name: name.to_owned(),
            state,
            relics: vec![],
            best_odds: String::new(),
            rarity: Rarity::Rare,
            deal: None,
        };
        card.parts.push(row("Dual Zoren Prime Blade", PartState::Have));
        card.parts.push(row("Dual Zoren Prime Grip", PartState::Need));
        let plan = Plan {
            goals: vec![card],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        let p2 = page(&html, "p2");
        assert!(p2.contains("⍻ Handle"), "Hunt row shows ⍻ + part name");
        assert!(!p2.contains("✓ Handle"), "unverified never earns the ✓");
        assert!(
            p2.contains("data-uv=\"Dual Zoren Prime Handle\""),
            "unverified row carries the toggle (click withdraws it)"
        );
        assert!(p2.contains("✓ Blade"), "verified row keeps its ✓");
        assert!(
            !p2.contains("data-uv=\"Dual Zoren Prime Blade\""),
            "verified-Have rows are NOT clickable"
        );
        assert!(
            p2.contains("data-uv=\"Dual Zoren Prime Grip\""),
            "a Need row carries the toggle (click asserts the hold)"
        );
        assert!(
            html.contains("fetch('/unverified'"),
            "the page script posts toggles to the endpoint"
        );
        let p3 = page(&html, "p3");
        assert!(
            p3.contains("<i class=\"seg uvhave\"></i>"),
            "Backlog unverified pip lights, dimmed from a verified pip"
        );
        assert!(p3.contains("1/4 ⍻"), "Backlog count carries the ⍻ mark");
        // (Attribute form `data-uv="..."`: the page script itself
        // names the bare attribute in its selector, so only the
        // valued form pins an actual row handler.)
        assert!(!p3.contains("data-uv=\""), "Backlog emits NO toggle handlers");
        for id in ["p1", "p4"] {
            assert!(
                !page(&html, id).contains("data-uv=\""),
                "no toggle handlers off the Hunt page ({id})"
            );
        }
    }

    /// Twelfth pass §1: backlog component-progress pips sit beside the
    /// 0/4-style count.
    #[test]
    fn backlog_has_reference_pips() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let plan = Plan {
            goals: vec![goal("Caliban Prime", "Caliban Prime Chassis Blueprint", false, false)],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        let p3 = page(&html, "p3");
        assert!(p3.contains("<span class=\"segs\">"), "pips present");
        assert!(p3.contains("<i class=\"seg need\"></i>"), "need pip");
        assert!(p3.contains("<i>0/4</i>"), "have/total count beside pips");
    }

    /// Twelfth pass §1: standing/weeklies absent from First Crack and
    /// present in Routine.
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
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, Some(daily));
        let html = render_plan(&Plan::default(), &ctx);
        let p1 = page(&html, "p1");
        let p4 = page(&html, "p4");
        assert!(p4.contains(&t.planner.daily_caps.label_for("main")), "standing under Routine");
        assert!(p4.contains("Focus"), "focus under Routine");
        assert!(p4.contains("Ostron"), "family cap under Routine");
        assert!(p4.contains("sortie-daily"), "sortie tick key under Routine");
        // Item 50a: the sortie row is a cadence-checklist item, not a daily
        // cap — it lives inside the "This week" board, never the Today block.
        let board = &p4[p4.find("class=\"wkg\"").expect("weekly board")..];
        assert!(board.contains("sortie-daily"), "sortie row inside the cadence checklist");
        let today = &p4[..p4.find("<h2>This week</h2>").expect("week header")];
        assert!(!today.contains("sortie-daily"), "sortie row out of the Today standing block");
        assert!(p4.contains("Archon Hunt"), "weeklies under Routine");
        assert!(p4.contains("wk-archon"), "weekly tick key under Routine");
        assert!(!p1.contains(&t.planner.daily_caps.label_for("main")), "no standing on First Crack");
        assert!(!p1.contains(">Focus<"), "no focus on First Crack");
        assert!(!p1.contains("Archon Hunt"), "no weeklies on First Crack");
        assert!(!p1.contains("wk-archon"), "no weekly tick keys on First Crack");
        assert!(!p1.contains("sortie-daily"), "no sortie on First Crack");
        assert!(!p1.contains("This week"), "no weeklies header on First Crack");
    }

    /// Twelfth pass §1: First Crack is the mock's compact phrasing —
    /// event rows carry countdown + node + type + minutes, and nothing
    /// from the standing surface.
    #[test]
    fn first_crack_uses_compact_alert_rows() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let plan = Plan {
            event_alerts: vec![crate::plan::AlertView {
                node: "Xini (Eris)".to_owned(),
                mission_type: "Extermination".to_owned(),
                payout: "50× Road To Tau Event Resource".to_owned(),
                left: "26m".to_owned(),
                done: false,
            }],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        let p1 = page(&html, "p1");
        assert!(p1.contains("<div class=\"row ev\">"), "compact event row");
        assert!(p1.contains("<b class=\"tm hot\">26m</b>"), "hot countdown");
        assert!(p1.contains("<b>Xini</b> <span class=\"mut\">Eris</span>"), "node split");
        assert!(p1.contains("<span class=\"mut sm\">Extermination</span>"), "mission type");
    }

    #[test]
    fn backlog_group_shows_parent_icon_not_part_icon() {
        let t = crate::tuning::Tuning::for_tests();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> =
            [("Ivara Prime".to_owned(), "IvaraPrime.png".to_owned())]
                .into_iter()
                .collect();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let world = WorldState::default();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let plan = Plan {
            goals: vec![goal("Ivara Prime", "Ivara Prime Chassis Blueprint", true, false)],
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
        assert_eq!(
            base64_encode(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
            "iVBORw0KGgo="
        );
    }

    #[test]
    fn least_important_weeklies_render_last() {
        let t = crate::tuning::Tuning::for_tests();
        let board = weekly_board(&t);
        assert_eq!(board.len(), 14);
        let tail = &board[board.len() - 2..];
        assert!(tail.iter().all(|w| w.deprioritized));
        assert!(board[..board.len() - 2].iter().all(|w| !w.deprioritized));
    }

    #[test]
    fn embedded_icons_replace_hotlinks() {
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
        let plan = Plan {
            goals: vec![goal("Ivara Prime", "Ivara Prime Chassis Blueprint", true, false)],
            ..Default::default()
        };
        let hot = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let hotlinked = render_plan(&plan, &hot);
        assert!(
            hotlinked.contains("https://cdn.warframestat.us/img/IvaraPrime.png"),
            "product page keeps hotlinks"
        );
        let mut emb = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        emb.embedded_icons = Some(&embedded);
        let embedded_html = render_plan(&plan, &emb);
        assert!(
            embedded_html.contains("data:image/png;base64,"),
            "demo embeds data URIs"
        );
        assert!(
            !embedded_html.contains("https://cdn.warframestat.us/img/IvaraPrime.png"),
            "demo fetches no hotlinks for embedded gears"
        );
    }

    #[test]
    fn demo_export_carries_zero_external_references() {
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
        let mut ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        ctx.embedded_icons = Some(&embedded);
        let plan = Plan {
            goals: vec![
                goal("Ivara Prime", "Ivara Prime Chassis Blueprint", true, false),
                goal("Baza Prime", "Baza Prime Blueprint", true, false),
            ],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        assert!(!html.contains("https://"), "demo fetches nothing external");
        assert!(html.contains("data:image/png;base64,"), "embedded icons stay");
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
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let html = render_plan(&Plan::default(), &ctx);
        assert!(html.contains(&t.planner.nightwave.name));
        assert!(html.contains("Accelerator"));
        assert!(
            html.contains("Kill 10 Enemies while Sliding"),
            "description renders with the name"
        );
        assert!(html.contains(">Daily <small>1 · 1k</small>"), "nightwave grouping");
    }

    #[test]
    fn today_renders_one_compact_cap_per_pool() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
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
            focus_remaining: 349_600,
            focus_cap: 400_000,
            families,
        };
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, Some(daily));
        let html = render_plan(&Plan::default(), &ctx);
        let p4 = page(&html, "p4");
        // Focus + main + 12 families.
        assert_eq!(p4.matches("class=\"cap\"").count(), 14);
        assert!(p4.contains("<b>349.6k</b>"), "compact remaining like the mock");
        assert!(p4.contains("data-left=\"349600\""), "raw remaining for audit");
        assert!(p4.contains("data-cap=\"400000\""), "raw cap for audit");
        assert!(p4.contains(&t.planner.daily_caps.label_for("main")), "Syndicates label");
        assert!(p4.contains("Ostron"), "Cetus player-facing label");
        assert!(!p4.contains("Pvp"), "PvP leaves with the Conclave cut");
        assert!(p4.contains("width:87%"), "remaining-fraction bar (349.6k/400k)");
    }

    #[test]
    fn crack_queue_uses_structured_facts() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let plan = Plan {
            chunks: vec![Chunk {
                id: "t1-crack-axi".to_owned(),
                tier: 1,
                title: "long human title".to_owned(),
                minutes_label: "~22m".to_owned(),
                why: "why".to_owned(),
                kind: "expiring crack".to_owned(),
                expiry: None,
                gear: Some("Caliban Prime".to_owned()),
                crack: Some(CrackInfo {
                    gear: "Caliban Prime".to_owned(),
                    part: "Caliban Prime Systems Blueprint".to_owned(),
                    relic: "Neo C8".to_owned(),
                    refinement: "Radiant".to_owned(),
                    chance: 10.0,
                    rarity: crate::plan::Rarity::Uncommon,
                    batch: 4,
                    traces: 400,
                    covers: 2,
                    radshare: None,
                    fissure_node: "Circulus (Lua)".to_owned(),
                    fissure_mission: "Survival".to_owned(),
                    steel_path: true,
                    vaulted: false,
                    unobtainable: false,
                }),
            }],
            ..Default::default()
        };
        let html = render_plan(&plan, &ctx);
        let p1 = page(&html, "p1");
        assert!(p1.contains("class=\"row ch tick next\""), "first crack is NEXT + tickable");
        // Addendum #87: the NEXT accent is a ::before bar pinned to the
        // row's own box (left/top/bottom 0), not an inset box-shadow.
        assert!(
            html.contains(".ch.next::before{content:\"\";position:absolute;left:0;top:0;bottom:0;width:3px;background:var(--cy)}"),
            "NEXT accent bar is pinned to the row box"
        );
        assert!(!html.contains("inset 3px 0"), "the old inset-shadow accent is gone");
        // Addendum #87 (pass 31): the crack-queue container carries no
        // horizontal padding; each row item carries it instead, so the
        // bar at the row's left:0 sits in the row's own padding zone,
        // floated away from the icon. Scoped to the crack queue via
        // the `cq` modifier — the base .panel rule is untouched, so
        // every other section keeps its inset.
        assert!(
            p1.contains("<section class=\"panel cq\">"),
            "crack queue panel is the padding-restructured one"
        );
        assert!(
            html.contains(".panel.cq{padding-left:0;padding-right:0}"),
            "crack queue container sheds its horizontal padding"
        );
        assert!(
            html.contains(".cq .ch{padding-left:18px;padding-right:18px}"),
            "crack rows carry the horizontal padding"
        );
        assert!(
            html.contains(".cq h2{padding-left:18px;padding-right:18px}"),
            "crack queue header keeps its inset"
        );
        assert!(
            html.contains(".panel{background:var(--panel);border:1px solid var(--line);padding:16px 18px;"),
            "base panel padding is unchanged for other sections"
        );
        assert!(p1.contains("<em class=nx>NEXT</em>"));
        assert!(p1.contains("<b class=\"cy\">Neo C8</b>"));
        assert!(p1.contains("Radiant"));
        assert!(
            !p1.contains("400 tr"),
            "trace prep framing is gone from the queue (addendum #72)"
        );
        assert!(
            p1.contains("class=\"sm uncommon\">Systems"),
            "the part name carries its rarity color (addendum #72)"
        );
        assert!(
            !p1.contains("Radiant &middot; 10%"),
            "per-crack chance is dead weight in the queue (addendum #67)"
        );
        assert!(p1.contains("Systems &middot; +2 also"), "short part + coverage");
        assert!(
            !p1.contains("stay for"),
            "no batch framing in the queue header (addendum #72)"
        );
        assert!(!p1.contains("long human title"), "mock phrasing replaces prose");
    }

    /// Pass 38 (addendum #103): the radshare verdict carried on
    /// CrackInfo renders the same compact marker on the crack-queue
    /// row and the Hunt card's crack line — and only there: no
    /// verdict, no marker on either surface.
    #[test]
    fn radshare_marker_on_queue_row_and_hunt_line() {
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let mk = |radshare: Option<String>| Plan {
            goals: vec![goal(
                "Caliban Prime",
                "Caliban Prime Systems Blueprint",
                false,
                false,
            )],
            chunks: vec![Chunk {
                id: "t2-crack-neo-c8".to_owned(),
                tier: 2,
                title: "title".to_owned(),
                minutes_label: "~22m".to_owned(),
                why: "why".to_owned(),
                kind: "target crack".to_owned(),
                expiry: None,
                gear: Some("Caliban Prime".to_owned()),
                crack: Some(CrackInfo {
                    gear: "Caliban Prime".to_owned(),
                    part: "Caliban Prime Systems Blueprint".to_owned(),
                    relic: "Neo C8".to_owned(),
                    refinement: "Radiant".to_owned(),
                    chance: 10.0,
                    rarity: crate::plan::Rarity::Rare,
                    batch: 4,
                    traces: 400,
                    covers: 0,
                    radshare,
                    fissure_node: "Circulus (Lua)".to_owned(),
                    fissure_mission: "Survival".to_owned(),
                    steel_path: false,
                    vaulted: false,
                    unobtainable: false,
                }),
            }],
            ..Default::default()
        };
        let verdict =
            Some("Best as a radshare (solo ~10 cracks vs ~3 shared).".to_owned());
        let html = render_plan(&mk(verdict), &ctx);
        assert!(
            page(&html, "p1").contains("<em class=rad>radshare</em>"),
            "queue row carries the marker"
        );
        assert!(
            page(&html, "p2").contains("<em class=rad>radshare</em>"),
            "hunt crack line carries the marker"
        );
        let html = render_plan(&mk(None), &ctx);
        assert!(
            !page(&html, "p1").contains("radshare"),
            "no verdict, no marker (queue)"
        );
        assert!(
            !page(&html, "p2").contains("radshare"),
            "no verdict, no marker (hunt)"
        );
    }

    #[test]
    fn via_deal_part_row_renders_refinement_alone() {
        // The Hunt `.r` line keeps the refinement label only; rarity is
        // carried by color (addendum #67). Uses the stock-aware deal.
        let mut g = goal("Gyre Prime", "Gyre Prime Neuroptics Blueprint", false, false);
        g.parts_total = 4;
        g.parts[0].rarity = Rarity::Rare;
        g.parts[0].deal = Some(PartDeal {
            relic: "Lith G14".to_owned(),
            vaulted: false,
            refinement: "Radiant".to_owned(),
            chance: 10.0,
            expected: 10.0,
            owned: Some(2),
            radiant: None,
            source_tier: crate::plan::SourceTier::Unvaulted,
        });
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let html = render_plan(
            &Plan {
                goals: vec![g],
                ..Default::default()
            },
            &ctx,
        );
        assert!(html.contains("<span class=\"rc rare\">Lith G14</span>"));
        assert!(html.contains("<span class=\"pc rare\">Rad</span>"));
        assert!(!html.contains("<span class=\"cr\">"));
        assert!(
            !html.contains("Rad 10%"),
            "chance/expected figures leave the row (addendum #67)"
        );
    }

    #[test]
    fn unobtainable_relic_rows_strike() {
        // Addendum #68 tier 4: the chosen relic has no current source, so
        // the Hunt part row strikes rather than reading as actionable.
        let mut g = goal("Gyre Prime", "Gyre Prime Neuroptics Blueprint", true, false);
        g.parts[0].deal = Some(PartDeal {
            relic: "Axi Vanguard M1".to_owned(),
            vaulted: true,
            refinement: "Intact".to_owned(),
            chance: 25.33,
            expected: 3.95,
            owned: None,
            radiant: None,
            source_tier: crate::plan::SourceTier::Unobtainable,
        });
        let t = crate::tuning::Tuning::for_tests();
        let world = WorldState::default();
        let weekly: HashMap<String, String> = HashMap::new();
        let proven: HashSet<String> = HashSet::new();
        let icons: HashMap<String, String> = HashMap::new();
        let cats: HashMap<String, Vec<String>> = HashMap::new();
        let ctx = empty_ctx(&world, &t, &weekly, &proven, &icons, &cats, None);
        let html = render_plan(
            &Plan {
                goals: vec![g],
                ..Default::default()
            },
            &ctx,
        );
        // The class list continues past `dead` since the thirtieth
        // pass: a non-verified Hunt row also carries the unverified
        // toggle class/attributes (#93). The pin here is the strike.
        assert!(html.contains("<div class=\"r dead"));
        assert!(html.contains("Axi Vanguard M1"));
        assert!(!html.contains("25.33%"), "chance text stays out of the row");
    }
}
