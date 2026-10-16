//! Fracker — Warframe session planner (first pass).
//!
//! This pass builds the testable core (BRIEF `docs/first-pass.md`):
//! config, fetchers, planner, renderer, and `fracker --demo`, which
//! renders the checklist page from fixture inputs into a single HTML
//! file. The tray runtime, live log watch loop, OCR, Flatpak
//! packaging, and Xuerian write-back are explicitly out of scope.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::Utc;
use clap::Parser;

use fracker::{config, fetch, model, pack, plan, render};
use model::{OwnedEquipment, RelicStock};

/// Live profile account (addendum #1: resolved from a real fissure log's
/// reward relay and confirmed by the profile endpoint).
pub const PROFILE_ACCOUNT_ID: &str = "5420fba23846322173707618";

#[derive(Parser, Debug)]
#[command(name = "fracker", about = "Warframe session planner (first pass)")]
struct Args {
    /// Render the checklist page from fixture inputs and exit.
    #[arg(long)]
    demo: bool,
    /// Where to write the demo page.
    #[arg(long, default_value = "docs/demo-plan.html")]
    out: PathBuf,
    /// Fixture directory (worldstate.json).
    #[arg(long, default_value = "fixtures")]
    fixtures: PathBuf,
    /// Explicit config file (default: XDG config).
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if args.demo {
        if let Err(e) = run_demo(&args).await {
            eprintln!("fracker --demo: {e}");
            std::process::exit(1);
        }
        return;
    }
    // No live loop this pass (watch/tray are stubs): report config state.
    let cfg_path = args.config.unwrap_or_else(config::config_file);
    match config::Config::load(&cfg_path) {
        Ok((cfg, _)) => {
            println!("fracker first pass: live session loop not built yet (see --demo).");
            println!("config: {}", cfg_path.display());
            println!(
                "targets: {}",
                if cfg.use_seed_targets() {
                    "seed list (no Xuerian token)"
                } else {
                    "Xuerian"
                }
            );
        }
        Err(e) => {
            eprintln!("config: {e}");
            std::process::exit(1);
        }
    }
}

/// Demo: LIVE world state + reconciled backlog + fresh profile pull ->
/// plan -> single HTML file.
///
/// Fail-soft chain (brief: report endpoint failures, never die on them):
/// world state falls back to `fixtures/worldstate.json`, the profile to
/// `fixtures/profile-<id>.json`. Stock stays UNKNOWN (OCR bootstrap is
/// pass 3); refinement advice treats unknown stock as scarce.
async fn run_demo(args: &Args) -> Result<(), String> {
    let client = fetch::client()?;
    let cache = config::cache_dir();
    let mut endpoint_failures: Vec<String> = Vec::new();

    // 1. World state: live, else recorded fixture.
    let (world_text, world_status) = match fetch::fetch_worldstate(&client, &cache).await {
        Ok(fetched) => {
            let text = std::fs::read_to_string(fetch::cache_file(&cache, "worldstate.json"))
                .map_err(|e| format!("reading recorded worldstate: {e}"))?;
            (text, fetched.status.label())
        }
        Err(e) => {
            endpoint_failures.push(format!("worldstate: {e}"));
            let text = std::fs::read_to_string(args.fixtures.join("worldstate.json"))
                .map_err(|e| format!("fixture worldstate: {e}"))?;
            (text, "fixture".to_owned())
        }
    };
    let world = model::WorldState::from_json(&world_text)?;
    let now = world
        .timestamp
        .parse::<chrono::DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());

    // 2. Reconciled backlog (64 unmastered primes), not raw Xuerian wants.
    let backlog_text = std::fs::read_to_string("data/backlog.json")
        .map_err(|e| format!("backlog data/backlog.json: {e}"))?;
    let backlog = model::Backlog::from_json(&backlog_text)?;
    let wants = backlog.wants();

    // 3. Seed relic/roster data.
    let rel_text = std::fs::read_to_string("data/cache/relictables.json")
        .map_err(|e| format!("seed relictables: {e}"))?;
    let wish_text = std::fs::read_to_string("data/cache/wishlist.json")
        .map_err(|e| format!("seed wishlist: {e}"))?;
    let relics = model::RelicTables::from_json(&rel_text)?;
    let roster = model::Roster::from_json(&wish_text)?;

    // 4. Fresh profile pull -> owned equipment + daily REMAINING caps.
    let (owned, mastery_rank, daily, profile_status) =
        match fetch::fetch_profile(&client, &cache, PROFILE_ACCOUNT_ID).await {
            Ok(fetched) => {
                let snap = fetched.value;
                let (o, d) = profile_context(&snap);
                (o, snap.mastery_rank, d, fetched.status.label())
            }
            Err(e) => {
                endpoint_failures.push(format!("profile: {e}"));
                let fixture = args.fixtures.join(format!("profile-{PROFILE_ACCOUNT_ID}.json"));
                let text = std::fs::read_to_string(&fixture)
                    .map_err(|e| format!("fixture profile: {e}"))?;
                let snap = fetch::parse_profile(&text)?;
                let (o, d) = profile_context(&snap);
                (o, snap.mastery_rank, d, "fixture".to_owned())
            }
        };

    // 5. Parent-item icons (addendum #14).
    let icons_text =
        std::fs::read_to_string("data/icons.json").map_err(|e| format!("icons: {e}"))?;
    let icons = model::load_icons(&icons_text)?;
    let icon_resolved = backlog
        .gear_names()
        .iter()
        .filter(|g| icons.contains_key(*g))
        .count();

    // 6. Taubound pack strip.
    let pack_text = std::fs::read_to_string("data/packs/taubound.toml")
        .map_err(|e| format!("taubound pack: {e}"))?;
    let taubound = pack::TauboundPack::from_toml(&pack_text)?;
    let pack_render = render::PackRender {
        live: taubound.is_live(now),
        expires_label: "Nov 17".to_owned(),
        contegorus_health: world
            .events
            .iter()
            .filter(|e| e.is_community && e.is_road_to_tau())
            .filter_map(|e| e.health)
            .next(),
        arcane_line: taubound.arcane_line(),
    };

    let stock = RelicStock::Unknown;

    let cfg_path = args.config.clone().unwrap_or_else(config::config_file);
    let (cfg, _) = config::Config::load(&cfg_path)
        .unwrap_or((config::Config::default(), config::ConfigSource::Defaults));

    let input = plan::PlanInput {
        now,
        world: &world,
        owned: &owned,
        wants: &wants,
        roster: &roster,
        relics: &relics,
        stock: &stock,
        pins: &cfg.pins,
        recent_gear: &HashSet::new(),
        steel_path_tiebreak: cfg.steel_path_tiebreak,
    };
    let plan = plan::plan(&input);

    let weekly_done: HashMap<String, String> = HashMap::new();
    let proven_done: HashSet<String> = HashSet::new();
    let ctx = render::RenderContext {
        world: &world,
        generated_label: now.format("%Y-%m-%d %H:%M UTC").to_string(),
        targets_status: format!(
            "backlog {} primes · profile {} (MR{})",
            backlog.primes.unmastered.len(),
            profile_status,
            mastery_rank,
        ),
        world_status,
        weekly_done: &weekly_done,
        proven_done: &proven_done,
        mastery_rank: Some(mastery_rank),
        icons: &icons,
        backlog_categories: &backlog.unmastered_by_category,
        daily,
        pack: Some(pack_render),
    };
    let html = render::render_plan(&plan, &ctx);

    if let Some(parent) = args.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("out dir: {e}"))?;
        }
    }
    let tmp = args.out.with_extension("html.tmp");
    std::fs::write(&tmp, html).map_err(|e| format!("write demo: {e}"))?;
    std::fs::rename(&tmp, &args.out).map_err(|e| format!("publish demo: {e}"))?;
    let bars = if ctx.daily.is_some() { 2 } else { 0 };
    println!(
        "demo: {} goals, {} chunks, {} bars -> {}",
        plan.goals.len(),
        plan.chunks.len(),
        bars,
        args.out.display()
    );
    println!(
        "demo: icons {}/{} · profile MR{} · endpoints: {}",
        icon_resolved,
        backlog.primes.unmastered.len(),
        mastery_rank,
        if endpoint_failures.is_empty() {
            "all live".to_owned()
        } else {
            endpoint_failures.join("; ")
        },
    );
    Ok(())
}

/// Owned equipment + profile-backed daily bars from one snapshot.
/// Name resolution uses `data/cache/item-names.json` (uniqueName ->
/// display name, built from the WFCD items dataset); unresolvable
/// entries are skipped, never guessed.
fn profile_context(
    snap: &fetch::ProfileSnapshot,
) -> (OwnedEquipment, Option<render::DailyRender>) {
    let names: std::collections::HashMap<String, String> =
        std::fs::read_to_string("data/cache/item-names.json")
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    let owned = fetch::owned_names_from_profile(snap, &names);
    let mr = snap.mastery_rank;
    let daily = render::DailyRender {
        affiliation_remaining: snap.daily.affiliation_remaining,
        affiliation_cap: 16_000 + 500 * mr,
        focus_remaining: snap.daily.focus_remaining,
        focus_cap: 250_000 + 5_000 * mr,
        families: {
            let mut f: Vec<(String, u32)> =
                snap.daily.per_family.iter().map(|(k, v)| (k.clone(), *v)).collect();
            f.sort_by(|a, b| a.0.cmp(&b.0));
            f
        },
    };
    (owned, Some(daily))
}
