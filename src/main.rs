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

use fracker::{config, fetch, model, plan, render};
use model::{OwnedEquipment, RelicStock};

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

/// Demo: fixture world state + seed targets + EMPTY profile + UNKNOWN
/// stock -> plan -> single HTML file.
async fn run_demo(args: &Args) -> Result<(), String> {
    // Demo "now" is the fixture's own timestamp: the page shows the
    // recorded world state faithfully (a live clock would expire the
    // recorded fissures and misrepresent the fixture).
    let world_text = match std::fs::read_to_string(args.fixtures.join("worldstate.json")) {
        Ok(t) => t,
        Err(_) => {
            let client = fetch::client()?;
            let cache = config::cache_dir();
            let fetched = fetch::fetch_worldstate(&client, &cache).await?;
            let text = std::fs::read_to_string(fetch::cache_file(&cache, "worldstate.json"))
                .map_err(|e| format!("reading recorded worldstate: {e}"))?;
            let _ = fetched;
            std::fs::create_dir_all(&args.fixtures)
                .map_err(|e| format!("fixtures dir: {e}"))?;
            let fixture_path = args.fixtures.join("worldstate.json");
            std::fs::write(&fixture_path, &text).map_err(|e| format!("fixture: {e}"))?;
            text
        }
    };
    let world = model::WorldState::from_json(&world_text)?;
    let now = world
        .timestamp
        .parse::<chrono::DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());

    // 2. Seed targets (49 wants).
    let seed_text = std::fs::read_to_string("data/targets.json")
        .map_err(|e| format!("seed targets data/targets.json: {e}"))?;
    let targets = model::Targets::from_seed_json(&seed_text)?;

    // 3. Seed relic/roster data.
    let rel_text = std::fs::read_to_string("data/cache/relictables.json")
        .map_err(|e| format!("seed relictables: {e}"))?;
    let wish_text = std::fs::read_to_string("data/cache/wishlist.json")
        .map_err(|e| format!("seed wishlist: {e}"))?;
    let relics = model::RelicTables::from_json(&rel_text)?;
    let roster = model::Roster::from_json(&wish_text)?;

    // 4. EMPTY profile + UNKNOWN stock.
    let owned = OwnedEquipment::empty();
    let stock = RelicStock::Unknown;

    let cfg_path = args.config.clone().unwrap_or_else(config::config_file);
    let (cfg, _) = config::Config::load(&cfg_path)
        .unwrap_or((config::Config::default(), config::ConfigSource::Defaults));

    let input = plan::PlanInput {
        now,
        world: &world,
        owned: &owned,
        wants: &targets.wants,
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
        targets_status: "seed (Xuerian token not set — targets may go stale)".to_owned(),
        world_status: "fixture".to_owned(),
        weekly_done: &weekly_done,
        proven_done: &proven_done,
        mastery_rank: None,
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
    println!(
        "demo: {} goals, {} chunks -> {}",
        plan.goals.len(),
        plan.chunks.len(),
        args.out.display()
    );
    Ok(())
}
