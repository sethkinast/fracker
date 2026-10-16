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

use fracker::{config, fetch, model, ocr, pack, plan, render, tuning};
use model::{OwnedEquipment, RelicStock};
use tuning::Tuning;

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
    /// OCR bootstrap: ingest a folder of inventory-grid screenshots and exit.
    #[arg(long)]
    ocr_ingest: Option<PathBuf>,
    /// Where to write the OCR stock state file (JSON).
    #[arg(long, default_value = "data/stock.json")]
    stock_out: PathBuf,
    /// Grid shape for OCR segmentation, COLSxROWS.
    #[arg(long, default_value = "5x4")]
    ocr_grid: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if let Some(dir) = args.ocr_ingest.clone() {
        if let Err(e) = run_ocr_ingest(&args, &dir) {
            eprintln!("fracker --ocr-ingest: {e}");
            std::process::exit(1);
        }
        return;
    }
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

    // 0. Runtime tuning + companion exclusions (no content in Rust).
    let tuning = Tuning::load_defaults()?;
    let companions_text = std::fs::read_to_string("data/companions.toml")
        .map_err(|e| format!("companions data/companions.toml: {e}"))?;
    let companions = model::CompanionsManifest::from_toml(&companions_text)?;

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

    // 2. Reconciled backlog (profile-derived primes), exclusions applied.
    let backlog_text = std::fs::read_to_string("data/backlog.json")
        .map_err(|e| format!("backlog data/backlog.json: {e}"))?;
    let backlog = model::Backlog::from_json(&backlog_text)?;
    let wants = companions.filter_wants(&backlog.wants());

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
                let (o, d) = profile_context(&tuning, &snap);
                (o, snap.mastery_rank, d, fetched.status.label())
            }
            Err(e) => {
                endpoint_failures.push(format!("profile: {e}"));
                let fixture = args.fixtures.join(format!("profile-{PROFILE_ACCOUNT_ID}.json"));
                let text = std::fs::read_to_string(&fixture)
                    .map_err(|e| format!("fixture profile: {e}"))?;
                let snap = fetch::parse_profile(&text)?;
                let (o, d) = profile_context(&tuning, &snap);
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
        expires_label: taubound
            .pack
            .expires
            .parse::<chrono::DateTime<chrono::Utc>>()
            .map(|t| t.format("%b %d").to_string())
            .unwrap_or_else(|_| "soon".to_owned()),
        contegorus_health: world
            .events
            .iter()
            .filter(|e| e.is_community && e.matches_pack_event(&taubound.event.match_text))
            .filter_map(|e| e.health)
            .next(),
        arcane_line: taubound.arcane_line(),
        pack_name: taubound.pack.name.clone(),
        event_label: taubound.event.match_text.clone(),
        payout_label: format!(
            "{} {} / {} SP",
            taubound.event.resource,
            taubound.event.alert_payout,
            taubound.event.alert_payout_steel_path,
        ),
        gated_label: format!("{} gated", taubound.community_goal.gated_set),
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
        tuning: &tuning,
    };
    let plan = plan::plan(&input);

    let weekly_done: HashMap<String, String> = HashMap::new();
    let proven_done: HashSet<String> = HashSet::new();
    // Demo export is self-contained (addendum #23): parent icons embed
    // as ≤128px data-URI thumbnails; the review surface fetches no
    // hotlinks. The product page keeps hotlinks (embedded_icons: None).
    let (embedded, embedded_count, hotlink_count) =
        embed_demo_icons(&client, &plan, &icons).await;
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
        embedded_icons: Some(&embedded),
        backlog_categories: &backlog.unmastered_by_category,
        daily,
        pack: Some(pack_render),
        tuning: &tuning,
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
    // Pass-4 bars: one per standing pool (PvP excluded) + focus.
    let bars = if let Some(d) = &ctx.daily {
        2 + d.families.len()
    } else {
        0
    };
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
    println!(
        "demo: self-contained icons: {} embedded thumbnails, {} hotlink fallbacks",
        embedded_count, hotlink_count,
    );
    Ok(())
}

/// Demo icon embedding: download each goal's parent icon, shrink to a
/// ≤128px thumbnail (python3 + PIL when available), and return the
/// parent-name -> data-URI map plus embedded/fallback counts. Any
/// failure keeps the CDN hotlink for that icon (counted, never fatal).
async fn embed_demo_icons(
    client: &reqwest::Client,
    plan: &plan::Plan,
    icons: &HashMap<String, String>,
) -> (HashMap<String, String>, usize, usize) {
    let mut embedded: HashMap<String, String> = HashMap::new();
    let mut fallback = 0usize;
    let mut gears: Vec<&str> = plan.goals.iter().map(|g| g.gear.as_str()).collect();
    gears.sort();
    gears.dedup();
    for gear in gears {
        let image = match icons.get(gear) {
            Some(i) => i,
            None => continue,
        };
        let url = format!("https://cdn.warframestat.us/img/{image}");
        let bytes = match client.get(&url).send().await {
            Ok(resp) => match resp.bytes().await {
                Ok(b) => b.to_vec(),
                Err(_) => {
                    fallback += 1;
                    continue;
                }
            },
            Err(_) => {
                fallback += 1;
                continue;
            }
        };
        // Thumbnail when possible; otherwise embed the original bytes
        // (still hotlink-free) — counts stay honest either way.
        let thumb = thumbnail_png(&bytes, 128).unwrap_or(bytes);
        embedded.insert(
            gear.to_owned(),
            format!("data:image/png;base64,{}", render::base64_encode(&thumb)),
        );
    }
    let done = embedded.len();
    (embedded, done, fallback)
}

/// Shrink PNG bytes to a ≤`max_px` thumbnail via python3 + PIL.
/// Returns None when the toolchain is unavailable (caller falls back).
fn thumbnail_png(png_bytes: &[u8], max_px: u32) -> Option<Vec<u8>> {
    use std::io::Write;
    let script = format!(
        "import sys\n\
         from PIL import Image\n\
         im = Image.open(sys.stdin.buffer).convert('RGBA')\n\
         im.thumbnail(({m}, {m}))\n\
         im.save(sys.stdout.buffer, 'PNG')\n",
        m = max_px,
    );
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(&script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    child
        .stdin
        .as_mut()?
        .write_all(png_bytes)
        .ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    Some(out.stdout)
}

/// Owned equipment + profile-backed daily bars from one snapshot.
/// Name resolution uses `data/cache/item-names.json` (uniqueName ->
/// display name, built from the WFCD items dataset); unresolvable
/// entries are skipped, never guessed. Caps scale with mastery rank
/// per the runtime tuning tables (profile values are REMAINING).
fn profile_context(
    tuning: &Tuning,
    snap: &fetch::ProfileSnapshot,
) -> (OwnedEquipment, Option<render::DailyRender>) {
    let names: std::collections::HashMap<String, String> =
        std::fs::read_to_string("data/cache/item-names.json")
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    let owned = fetch::owned_names_from_profile(snap, &names);
    let mr = snap.mastery_rank;
    let caps = &tuning.planner.daily_caps;
    let daily = render::DailyRender {
        affiliation_remaining: snap.daily.affiliation_remaining,
        affiliation_cap: caps.standing_base + caps.standing_per_mr * mr,
        focus_remaining: snap.daily.focus_remaining,
        focus_cap: caps.focus_base + caps.focus_per_mr * mr,
        families: {
            // Pass 4 §28: PvP leaves with the Conclave cut — never a bar.
            let mut f: Vec<(String, u32)> = snap
                .daily
                .per_family
                .iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("pvp"))
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            f.sort_by(|a, b| a.0.cmp(&b.0));
            f
        },
    };
    (owned, Some(daily))
}

/// OCR bootstrap ingestion: screenshots folder -> validated stock file.
fn run_ocr_ingest(args: &Args, dir: &std::path::Path) -> Result<(), String> {
    let tuning = Tuning::load_defaults()?;
    let _ = &tuning;
    let grid = ocr::GridSpec::parse(&args.ocr_grid)?;

    // Item universe: roster parts + gear + relic names from the seed
    // datasets (the same names the planner reasons about).
    let rel_text = std::fs::read_to_string("data/cache/relictables.json")
        .map_err(|e| format!("seed relictables: {e}"))?;
    let wish_text = std::fs::read_to_string("data/cache/wishlist.json")
        .map_err(|e| format!("seed wishlist: {e}"))?;
    let relics = model::RelicTables::from_json(&rel_text)?;
    let roster = model::Roster::from_json(&wish_text)?;
    let mut names: Vec<String> = Vec::new();
    for item in &roster.items {
        names.push(item.name.clone());
        for p in &item.parts {
            names.push(p.name.clone());
        }
    }
    for name in relics.relics.keys() {
        names.push(name.clone());
    }
    names.sort();
    names.dedup();
    let universe = ocr::Universe::from_names(&names);
    let relic_names: HashSet<String> = relics.relics.keys().cloned().collect();

    // Engine: tesseract when the binary exists, else the stub (empty
    // stock, never guesses) — status printed honestly either way.
    let tesseract = ocr::TesseractCliEngine::probe();
    let stub;
    let engine: &dyn ocr::OcrEngine = match &tesseract {
        Some(e) => e,
        None => {
            stub = ocr::StubEngine;
            &stub
        }
    };
    println!("ocr: engine {} ({})", engine.name(), ocr::engine_status());

    let (stock, results) = ocr::ingest_folder(
        dir,
        engine,
        grid,
        &universe,
        &relic_names,
        0.5,
        0.8,
    )?;
    stock.write_to(&args.stock_out)?;
    let held = results.iter().filter(|r| r.held).count();
    println!(
        "ocr: {} cells, {} held ({} relics, {} parts) -> {}",
        results.len(),
        held,
        stock.relics.values().sum::<u32>(),
        stock.parts.values().sum::<u32>(),
        args.stock_out.display(),
    );
    Ok(())
}
