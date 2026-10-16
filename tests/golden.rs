//! Golden test: recorded live world state (`fixtures/worldstate.json`,
//! captured 2026-10-15T20:55:06Z from https://api.warframestat.us/pc/)
//! + the real seed targets (`data/targets.json`, 49 wants) + an EMPTY
//! profile + UNKNOWN stock.
//!
//! Set FRACKER_REFRESH_FIXTURES=1 to re-record the fixture from the live
//! API before asserting (network-optional; default runs offline).

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use fracker::model::{OwnedEquipment, RelicStock, RelicTables, Roster, Targets, WorldState};
use fracker::plan::{self, ActionKind, PlanInput};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Planner "now" just after the fixture capture: fissures expiring
/// imminently exercise the expiry cascade deterministically.
fn golden_now() -> DateTime<Utc> {
    "2026-10-15T21:00:00Z".parse().unwrap()
}

fn golden_plan_recompute() -> fracker::plan::Plan {
    let world_text =
        std::fs::read_to_string(root().join("fixtures/worldstate.json")).unwrap();
    let world = WorldState::from_json(&world_text).unwrap();
    let seed = std::fs::read_to_string(root().join("data/targets.json")).unwrap();
    let targets = Targets::from_seed_json(&seed).unwrap();
    let rel = std::fs::read_to_string(root().join("data/cache/relictables.json")).unwrap();
    let relics = RelicTables::from_json(&rel).unwrap();
    let wish = std::fs::read_to_string(root().join("data/cache/wishlist.json")).unwrap();
    let roster = Roster::from_json(&wish).unwrap();
    // Leak to 'static so the borrowed input is valid for the call.
    let world: &'static WorldState = Box::leak(Box::new(world));
    let targets: &'static Targets = Box::leak(Box::new(targets));
    let relics: &'static RelicTables = Box::leak(Box::new(relics));
    let roster: &'static Roster = Box::leak(Box::new(roster));
    let owned: &'static OwnedEquipment = Box::leak(Box::new(OwnedEquipment::empty()));
    let stock: &'static RelicStock = Box::leak(Box::new(RelicStock::Unknown));
    let pins: &'static Vec<String> = Box::leak(Box::new(Vec::new()));
    let recent: &'static HashSet<String> = Box::leak(Box::new(HashSet::new()));
    let input = PlanInput {
        now: golden_now(),
        world,
        owned,
        wants: &targets.wants,
        roster,
        relics,
        stock,
        pins,
        recent_gear: recent,
        steel_path_tiebreak: true,
    };
    plan::plan(&input)
}

#[test]
fn golden_fixture_parses_with_live_content() {
    let text =
        std::fs::read_to_string(root().join("fixtures/worldstate.json")).unwrap();
    let world = WorldState::from_json(&text).unwrap();
    assert!(!world.fissures.is_empty(), "recorded fissures");
    assert!(!world.alerts.is_empty(), "recorded alerts");
    assert!(world.vault_trader.is_some(), "recorded Varzia rotation");
    assert!(world.nightwave.is_some(), "recorded nightwave");
    assert!(world.sortie.is_some(), "recorded sortie");
}

#[test]
fn golden_all_49_wants_survive_empty_profile() {
    let plan = golden_plan_recompute();
    assert!(plan.pruned.is_empty(), "empty profile prunes nothing");
    let covered: usize = plan.goals.iter().map(|g| g.wants.len()).sum();
    assert_eq!(covered, 49, "all 49 seed wants appear on goal cards");
    assert!(!plan.goals.is_empty());
}

#[test]
fn golden_ivara_unparked_by_live_resurgence() {
    let plan = golden_plan_recompute();
    let ivara = plan
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
fn golden_vaulted_wants_yield_no_farm_chunks() {
    let plan = golden_plan_recompute();
    assert!(
        plan.farms.is_empty(),
        "every seed want sits in a vaulted relic: nothing to farm"
    );
    assert!(
        plan.notes.iter().any(|n| n.contains("vaulted")),
        "planner says crack-or-park: {:?}",
        plan.notes
    );
}

#[test]
fn golden_slate_has_cracks_and_labels_filler() {
    let plan = golden_plan_recompute();
    assert!(
        plan.chunks.iter().any(|c| c.tier == 2),
        "target-matched fissures exist in the fixture"
    );
    for c in &plan.chunks {
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
fn golden_weeklies_never_enter_chunks() {
    let plan = golden_plan_recompute();
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
            !plan.chunks.iter().any(|c| c.title.contains(banned)),
            "weekly content is board-only, found in chunk: {banned}"
        );
    }
}

#[test]
fn golden_unknown_stock_labels_if_you_have_it() {
    let plan = golden_plan_recompute();
    let cracks: Vec<_> = plan
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

/// Re-record the fixture from the live API. No-op unless
/// FRACKER_REFRESH_FIXTURES=1 (keeps the suite green offline).
#[tokio::test]
async fn refresh_worldstate_fixture() {
    if std::env::var("FRACKER_REFRESH_FIXTURES").ok().as_deref() != Some("1") {
        return;
    }
    let client = reqwest::Client::new();
    let text = client
        .get("https://api.warframestat.us/pc/")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(!v["fissures"].as_array().unwrap().is_empty());
    std::fs::write(root().join("fixtures/worldstate.json"), &text).unwrap();
}
