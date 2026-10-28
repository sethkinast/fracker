//! Golden tests: DE's recorded world state (`fixtures/worldstate.json`,
//! captured 2026-10-19T05:16:44Z from
//! https://api.warframe.com/cdn/worldState.php) parsed with the baked
//! decode tables + the reconciled backlog (`data/backlog.json`: 63
//! unmastered primes, profile-derived, NOT raw Xuerian wants) + the live
//! profile fixture (`fixtures/profile-5420fba23846322173707618.json`) +
//! UNKNOWN stock.
//!
//! Set FRACKER_REFRESH_FIXTURES=1 to re-record the DE world-state fixture
//! from the live feed before asserting (network-optional; default runs
//! offline).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use fracker::model::{
    Backlog, Decodes, OwnedEquipment, RelicStock, RelicTables, Roster, WorldState,
};
use fracker::plan::{self, ActionKind, PlanInput};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn world_from_fixture() -> WorldState {
    let world_text =
        std::fs::read_to_string(root().join("fixtures/worldstate.json")).unwrap();
    let decodes = Decodes::load(&root().join("data")).unwrap();
    let world = WorldState::from_de_json(&world_text, &decodes).unwrap();
    assert_golden_clock_matches_fixture(&world);
    world
}

/// The planner clock is tied to the committed fixture's own `Time`
/// (2026-10-19T05:16:44Z). Regenerating `fixtures/worldstate.json`
/// (`FRACKER_REFRESH_FIXTURES=1`) must not silently drift every golden
/// timestamp: [`assert_golden_clock_matches_fixture`] fails loudly when
/// the fixture's `Time` no longer matches this constant.
const GOLDEN_FIXTURE_TIME: &str = "2026-10-19T05:16:44Z";

fn golden_now() -> DateTime<Utc> {
    GOLDEN_FIXTURE_TIME.parse().unwrap()
}

/// Sixteenth pass §4: the hardcoded golden clock and the committed
/// fixture must agree. A refreshed fixture carries a new `Time`; without
/// this assertion the suite would keep asserting the old expectations
/// against new data and drift silently.
fn assert_golden_clock_matches_fixture(world: &WorldState) {
    assert_eq!(
        world.timestamp, GOLDEN_FIXTURE_TIME,
        "fixtures/worldstate.json Time drifted from the golden clock; \
         regenerate the golden expectations and update GOLDEN_FIXTURE_TIME \
         deliberately (FRACKER_REFRESH_FIXTURES=1 rewrites the fixture)"
    );
    assert_eq!(
        golden_now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        world.timestamp,
        "golden_now() must be the committed fixture's Time"
    );
}

struct Golden {
    plan: fracker::plan::Plan,
    backlog: Backlog,
}

fn golden_plan_recompute() -> Golden {
    let world = world_from_fixture();
    let backlog_text =
        std::fs::read_to_string(root().join("data/backlog.json")).unwrap();
    let backlog = Backlog::from_json(&backlog_text).unwrap();
    let profile_text = std::fs::read_to_string(
        root().join("fixtures/profile-5420fba23846322173707618.json"),
    )
    .unwrap();
    let snap = fracker::fetch::parse_profile(&profile_text).unwrap();
    let names_text =
        std::fs::read_to_string(root().join("data/cache/item-names.json")).unwrap();
    let names: HashMap<String, String> = serde_json::from_str(&names_text).unwrap();
    let owned = fracker::fetch::owned_names_from_profile(&snap, &names);
    let wants = backlog.wants();
    let rel = std::fs::read_to_string(root().join("data/cache/relictables.json")).unwrap();
    let relics = RelicTables::from_json(&rel).unwrap();
    let wish = std::fs::read_to_string(root().join("data/cache/wishlist.json")).unwrap();
    let roster = Roster::from_json(&wish).unwrap();
    // Leak to 'static so the borrowed input is valid for the call.
    let world: &'static WorldState = Box::leak(Box::new(world));
    let wants: &'static Vec<String> = Box::leak(Box::new(wants));
    let relics: &'static RelicTables = Box::leak(Box::new(relics));
    let roster: &'static Roster = Box::leak(Box::new(roster));
    let owned: &'static OwnedEquipment = Box::leak(Box::new(owned));
    let stock: &'static RelicStock = Box::leak(Box::new(RelicStock::Unknown));
    let part_stock: &'static std::collections::HashMap<String, u32> =
        Box::leak(Box::new(std::collections::HashMap::new()));
    let unverified_parts: &'static std::collections::HashMap<String, u32> =
        Box::leak(Box::new(std::collections::HashMap::new()));
    let pins: &'static Vec<String> = Box::leak(Box::new(Vec::new()));
    let recent: &'static HashSet<String> = Box::leak(Box::new(HashSet::new()));
    let item_names: &'static std::collections::HashMap<String, String> =
        Box::leak(Box::new(fracker::fetch::load_item_names(&root().join("data"))));
    let tuning: &'static fracker::tuning::Tuning =
        Box::leak(Box::new(fracker::tuning::Tuning::load(&root().join("data")).unwrap()));
    let input = PlanInput {
        now: golden_now(),
        world,
        owned,
        wants,
        roster,
        relics,
        stock,
        part_stock,
        unverified_parts,
        pins,
        recent_gear: recent,
        steel_path_tiebreak: true,
        completed_missions: &[],
        tuning,
        item_names,
    };
    let plan = plan::plan(&input);
    // Re-read the backlog for assertions (the leaked copies are 'static).
    let backlog_text =
        std::fs::read_to_string(root().join("data/backlog.json")).unwrap();
    let backlog = Backlog::from_json(&backlog_text).unwrap();
    Golden { plan, backlog }
}

#[test]
#[should_panic(expected = "Time drifted")]
fn golden_clock_guard_rejects_a_refreshed_fixture() {
    // A regeneration rewrites `Time`; the guard must fail loudly rather
    // than letting every golden assertion run against drifted data.
    let mut world = world_from_fixture();
    world.timestamp = "2026-10-20T00:00:00Z".to_owned();
    assert_golden_clock_matches_fixture(&world);
}

#[test]
fn golden_fixture_parses_with_live_content() {
    let world = world_from_fixture();
    assert!(!world.fissures.is_empty(), "recorded fissures");
    assert!(!world.alerts.is_empty(), "recorded alerts");
    assert!(world.vault_trader.is_some(), "recorded Varzia rotation");
    assert!(world.nightwave.is_some(), "recorded nightwave");
    assert!(world.sortie.is_some(), "recorded sortie");
    assert!(world.archon.is_some(), "recorded Archon Hunt");
    // Taubound tier-1 needs the nested reward counted items + events.
    assert!(
        world.alerts.iter().any(|a| !a.mission.counted_items().is_empty()),
        "alert payouts parse from reward.countedItems"
    );
    assert!(!world.events.is_empty(), "recorded goals (Contegorus gate)");
}

#[test]
fn golden_live_profile_prunes_newly_mastered_backlog() {
    // The reconciled backlog holds 63 unmastered primes (64 on Oct 15
    // minus the excluded collar, addendum #22); the live profile
    // fixture shows Shade Prime + Tipedo Prime (both in-progress then)
    // now built in XPInfo = mastered since. The planner prunes them —
    // profile-backed state working end to end.
    let g = golden_plan_recompute();
    assert_eq!(
        g.backlog.primes.unmastered.len(),
        63,
        "reconciled backlog size (collar excluded)"
    );
    assert!(
        !g.backlog
            .primes
            .unmastered
            .iter()
            .any(|e| e.name == "Kavasa Prime"),
        "excluded collar never regenerates into the backlog"
    );
    assert_eq!(g.plan.goals.len(), 61, "two gears mastered since");
    assert!(
        !g.plan.goals.iter().any(|c| c.gear == "Shade Prime"),
        "Shade Prime pruned (now mastered)"
    );
    assert!(
        !g.plan.goals.iter().any(|c| c.gear == "Tipedo Prime"),
        "Tipedo Prime pruned (now mastered)"
    );
    assert_eq!(g.plan.pruned.len(), 7, "4 Shade + 3 Tipedo parts");
}

#[test]
fn golden_current_ranks_before_vaulted() {
    let g = golden_plan_recompute();
    let first_vaulted = g
        .plan
        .goals
        .iter()
        .position(|c| c.vaulted && !c.resurgence_live);
    let last_current = g
        .plan
        .goals
        .iter()
        .rposition(|c| !c.vaulted || c.resurgence_live);
    match (first_vaulted, last_current) {
        (Some(fv), Some(lc)) => assert!(
            lc < fv,
            "all current (unvaulted/resurgence) goals precede parked vaulted ones"
        ),
        _ => panic!("expected both current and parked-vaulted goals"),
    }
    // Frames before weapons within the current tier.
    let current: Vec<&fracker::plan::GoalCard> = g
        .plan
        .goals
        .iter()
        .take_while(|c| !c.vaulted || c.resurgence_live)
        .collect();
    let first_weapon = current.iter().position(|c| !c.is_warframe);
    let last_frame = current.iter().rposition(|c| c.is_warframe);
    if let (Some(fw), Some(lf)) = (first_weapon, last_frame) {
        assert!(lf < fw, "frames before weapons within the current tier");
    }
}

#[test]
fn golden_ivara_unparked_by_live_resurgence() {
    let g = golden_plan_recompute();
    let ivara = g
        .plan
        .goals
        .iter()
        .find(|g| g.gear == "Ivara Prime")
        .expect("Ivara Prime goal card");
    assert!(ivara.vaulted, "Ivara is vaulted in the roster");
    assert!(
        ivara.resurgence_live,
        "Oct 1–29 rotation covers Ivara: goal un-parks"
    );
    assert_eq!(ivara.action.kind, ActionKind::Resurgence);
    assert!(
        ivara.action.text.contains("Varzia"),
        "action names the channel: {}",
        ivara.action.text
    );
}

#[test]
fn golden_source_priority_prefers_unvaulted_caliban_relic() {
    // Addendum #68: Caliban Prime Blueprint drops from the tier-4
    // "Axi Vanguard M1" at 25.33% Intact, but unvaulted Lith V11/Meso
    // K9/Meso V13 are only 11%. Source tier outranks raw chance, so the
    // deal must be an unvaulted relic.
    let g = golden_plan_recompute();
    let caliban = g
        .plan
        .goals
        .iter()
        .find(|c| c.gear == "Caliban Prime")
        .expect("Caliban Prime goal card");
    let bp = caliban
        .parts
        .iter()
        .find(|p| p.name == "Caliban Prime Blueprint")
        .expect("Caliban Prime Blueprint");
    let deal = bp.deal.as_ref().expect("deal for Caliban Blueprint");
    eprintln!(
        "Caliban Blueprint deal: {} ({:?}, {:.2}%)",
        deal.relic, deal.source_tier, deal.chance
    );
    assert_ne!(deal.relic, "Axi Vanguard M1", "tier 4 never wins on chance");
    assert_eq!(
        deal.source_tier,
        fracker::plan::SourceTier::Unvaulted,
        "unvaulted lith/meso source wins: {}",
        deal.relic
    );
    assert!(
        caliban.partially_unobtainable,
        "Chassis has tier-4 sources, so the card is partially unobtainable"
    );
    let chassis = caliban
        .parts
        .iter()
        .find(|p| p.name == "Caliban Prime Chassis Blueprint")
        .expect("Caliban Prime Chassis Blueprint");
    eprintln!(
        "Caliban Chassis deal: {:?}",
        chassis.deal.as_ref().map(|d| (&d.relic, d.source_tier))
    );
    assert!(caliban.parts.iter().any(|p| p.relics.iter().any(|r| {
        r.source_tier == fracker::plan::SourceTier::Unobtainable
    })));
    assert!(caliban.parts.iter().any(|p| {
        p.name == "Caliban Prime Chassis Blueprint"
            && p.relics.iter().any(|r| {
                r.source_tier == fracker::plan::SourceTier::Unobtainable
            })
    }));
}

#[test]
fn golden_taubound_alerts_enter_tier_1() {
    let g = golden_plan_recompute();
    assert!(
        g.plan
            .chunks
            .iter()
            .any(|c| c.tier == 1 && c.kind == "event alert"),
        "live Moonflesh alerts are tier-1 chunks: {:?}",
        g.plan.chunks.iter().map(|c| &c.kind).collect::<Vec<_>>(),
    );
}

#[test]
fn golden_vaulted_wants_yield_no_farm_chunks() {
    let g = golden_plan_recompute();
    // Backlog wants still sit in vaulted relics except current-rotation
    // gear; farms stay empty or current-only.
    assert!(
        g.plan.farms.is_empty(),
        "no farmable backlog relics in this rotation"
    );
    assert!(
        g.plan.notes.iter().any(|n| n.contains("crack-or-park")),
        "planner says crack-or-park: {:?}",
        g.plan.notes
    );
}

#[test]
fn golden_slate_has_cracks_and_labels_filler() {
    let g = golden_plan_recompute();
    // All target cracks expire <60 min in this fixture, so they join
    // tier 1 as expiring value; the slate holds cracks either way.
    let cracks: Vec<_> = g
        .plan
        .chunks
        .iter()
        .filter(|c| c.kind.contains("crack"))
        .collect();
    assert!(!cracks.is_empty(), "target-matched fissures exist");
    for c in &g.plan.chunks {
        if c.tier >= 4 {
            assert!(
                c.kind.contains("sortie") || c.kind.contains("filler"),
                "tier 4/5 skeletal + labeled: {}",
                c.title
            );
        }
    }
}

#[test]
fn golden_donow_views_match_live_feed() {
    // Pass 4 Do Now: the fixture's 3 alerts are all counted-payout
    // events (own section); no gift alerts live (section hides); the
    // rotation element covers Ivara; the trader window opens 13:00 —
    // hidden at the 02:55 golden now.
    let g = golden_plan_recompute();
    assert_eq!(g.plan.event_alerts.len(), 3);
    assert!(g.plan.lotus_alerts.is_empty());
    let r = g.plan.resurgence.expect("rotation covers backlog gear");
    assert!(r.gears.iter().any(|x| x == "Ivara Prime"), "{:?}", r.gears);
    assert!(g.plan.baro.is_none(), "trader window not open yet");
}

#[test]
fn golden_page_shows_pass4_sections_and_pool_bars() {
    use fracker::render::{DailyRender, RenderContext};
    let root = root();
    let world = world_from_fixture();
    let profile_text = std::fs::read_to_string(
        root.join("fixtures/profile-5420fba23846322173707618.json"),
    )
    .unwrap();
    let snap = fracker::fetch::parse_profile(&profile_text).unwrap();
    let tuning =
        fracker::tuning::Tuning::load(&root.join("data")).unwrap();
    let caps = &tuning.planner.daily_caps;
    let mut families: Vec<(String, u32)> = snap
        .daily
        .per_family
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("pvp"))
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    families.sort_by(|a, b| a.0.cmp(&b.0));
    let daily = DailyRender {
        affiliation_remaining: snap.daily.affiliation_remaining,
        affiliation_cap: caps.standing_base + caps.standing_per_mr * snap.mastery_rank,
        focus_remaining: snap.daily.focus_remaining,
        focus_cap: caps.focus_base + caps.focus_per_mr * snap.mastery_rank,
        families,
    };
    let g = golden_plan_recompute();
    let weekly_done: HashMap<String, String> = HashMap::new();
    let proven: HashSet<String> = HashSet::new();
    let icons: HashMap<String, String> = HashMap::new();
    let cats: HashMap<String, Vec<String>> = HashMap::new();
    let ctx = RenderContext {
        world: &world,
        generated_label: "golden".to_owned(),
        generation: "golden-generation".to_owned(),
        targets_status: "t".to_owned(),
        world_status: "w".to_owned(),
        de_lock: None,
        weekly_done: &weekly_done,
        proven_done: &proven,
        mastery_rank: Some(snap.mastery_rank),
        icons: &icons,
        embedded_icons: None,
        backlog_categories: &cats,
        daily: Some(daily),
        pack: None,
        tuning: &tuning,
        search: None,
        events: &[],
    };
    let html = fracker::render::render_plan(&g.plan, &ctx);
    // Event section live with countdowns; gift section hidden.
    assert!(html.contains(&tuning.planner.alert_sections.event_header));
    assert!(html.contains("<b>Naeglar</b>"));
    assert!(html.contains("Eris"));
    assert!(!html.contains(&tuning.planner.alert_sections.lotus_header));
    // Rotation element + act descriptions + trader hidden.
    assert!(html.contains("Ivara Prime"));
    assert!(html.contains("Kill 3 Drones or Ospreys with your Melee Weapon."));
    assert!(!html.contains("card vendor\">"));
    // Per-pool caps: Focus + the bare shared pool + 12 families, no
    // PvP; the visible number is compact, the raw value rides data-left
    // (twelfth pass §1 / §3).
    assert_eq!(html.matches("class=\"cap\"").count(), 14);
    assert!(!html.contains("Pvp"));
    assert!(html.contains("Focus"));
    assert!(html.contains("data-left=\""));
    // Slimmed box: kept rows render, cut rows never do.
    assert!(html.contains("Archon Hunt"));
    for cut in [
        "Clan Initiative",
        "Conclave",
        "Teshin item",
        "Teshin caps",
        "Helminth",
        "NW Creds",
    ] {
        assert!(!html.contains(cut), "cut weekly renders: {cut}");
    }
    assert!(!html.contains("wk-nw-weekly"), "acts own their section now");
    // Twelfth pass §1: four tabs in order; thirteenth pass §2 made each a
    // real `?tab=<slug>` link. The standing/weeklies surface sits under
    // Routine and NOT under the action-only First Crack tab.
    let mut last = 0usize;
    for label in ["First Crack", "Hunt", "Backlog", "Routine"] {
        let pos = html.find(&format!(">{label}</a>")).expect("tab link");
        assert!(pos >= last, "tab {label} renders out of order");
        last = pos;
    }
    let slice = |id: &str| -> &str {
        let needle = format!("<div class=\"page\" id=\"{id}\">");
        let start = html.find(&needle).expect("page");
        let rest = &html[start..];
        let end = rest[1..]
            .find("<div class=\"page\"")
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        &rest[..end]
    };
    let first_crack = slice("p1");
    let routine = slice("p4");
    assert!(routine.contains("Focus"), "focus renders under Routine");
    assert!(routine.contains("Syndicates"), "standing renders under Routine");
    assert!(routine.contains("Archon Hunt"), "weeklies render under Routine");
    assert!(routine.contains("wk-archon"), "weekly tick key lives under Routine");
    assert!(routine.contains("sortie-daily"), "sortie tick key lives under Routine");
    // Item 50a: the sortie row sits in the cadence checklist ("This week"
    // board), not the Today standing block.
    let board = &routine[routine.find("class=\"wkg\"").expect("weekly board")..];
    assert!(board.contains("sortie-daily"), "sortie row inside the cadence checklist");
    let today = &routine[..routine.find("<h2>This week</h2>").expect("week header")];
    assert!(!today.contains("sortie-daily"), "sortie row out of the Today standing block");
    assert!(!first_crack.contains("Focus"), "no standing/focus on First Crack");
    assert!(!first_crack.contains("Syndicates"), "no standing on First Crack");
    assert!(!first_crack.contains("Archon Hunt"), "no weeklies on First Crack");
    assert!(!first_crack.contains("wk-archon"), "no weekly tick keys on First Crack");
    assert!(!first_crack.contains("sortie-daily"), "no sortie on First Crack");
}

#[test]
fn golden_donow_targets_at_most_three_parts() {
    // Brief #2 item 3: the slate's crack chunks pursue only the ≤3
    // focus parts; everything else waits in Hunt/Backlog.
    let g = golden_plan_recompute();
    assert!(!g.plan.focus.is_empty(), "focus is never empty with live wants");
    assert!(
        g.plan.focus.len() <= 3,
        "at most 3 parts: {:?}",
        g.plan.focus.iter().map(|f| &f.part).collect::<Vec<_>>(),
    );
    let eras: HashSet<&str> = g.plan.focus.iter().map(|f| f.era.as_str()).collect();
    assert_eq!(eras.len(), g.plan.focus.len(), "era spread");
    for c in g.plan.chunks.iter().filter(|c| c.kind.contains("crack")) {
        let text = format!("{} {}", c.title, c.why);
        assert!(
            g.plan.focus.iter().any(|f| text.contains(&f.part)),
            "slate crack pursues a focus part: {}",
            c.title,
        );
    }
}

#[test]
fn golden_weeklies_never_enter_chunks() {
    let g = golden_plan_recompute();
    // Weekly-item phrases (board-only). Location names such as
    // "Maroo's Bazaar" are fine — they are not weekly content.
    // (The skeletal T4 sortie chunk is the deliberate tier-4 exception.)
    for banned in [
        "Archon Hunt",
        "Netracell",
        "Archimedea",
        "Circuit",
        "Break Narmer",
        "Ayatan Treasure Hunt",
        "Help Clem",
        "Descendia",
        "syndicate mission",
    ] {
        assert!(
            !g.plan.chunks.iter().any(|c| c.title.contains(banned)),
            "weekly content is board-only, found in chunk: {banned}"
        );
    }
}

#[test]
fn golden_unknown_stock_labels_if_you_have_it() {
    let g = golden_plan_recompute();
    let cracks: Vec<_> = g
        .plan
        .chunks
        .iter()
        .filter(|c| c.kind.contains("crack"))
        .collect();
    assert!(!cracks.is_empty());
    assert!(
        cracks.iter().all(|c| format!("{} {}", c.title, c.why).contains("if you have it")),
        "unknown stock is labeled honestly"
    );
}

/// Re-record the world-state fixture from DE's live feed. No-op unless
/// FRACKER_REFRESH_FIXTURES=1 (keeps the suite green offline).
#[tokio::test]
async fn refresh_worldstate_fixture() {
    if std::env::var("FRACKER_REFRESH_FIXTURES").ok().as_deref() != Some("1") {
        return;
    }
    let client = reqwest::Client::new();
    let text = client
        .get(fracker::fetch::WORLD_STATE_URL)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v.get("Time").is_some(), "DE payload carries Time");
    assert!(!v["Alerts"].as_array().unwrap().is_empty());
    std::fs::write(root().join("fixtures/worldstate.json"), &text).unwrap();
}
