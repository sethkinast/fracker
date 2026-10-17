//! Fracker — Warframe session planner.
//!
//! Two entry points:
//!
//! - `fracker --demo`: render the check-listing page from live inputs
//!   (fixtures as fallback) as a single self-contained HTML file.
//! - `fracker` (default): the resident runtime — the §4 EE.log watch and
//!   the report pipeline run in one process behind a KDE tray icon
//!   (seventh pass). `--headless`/`--watch` run the identical loop with
//!   no tray.
//!
//! `--ocr-ingest` is the one-shot inventory bootstrap (pass 5).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap::Parser;
use tokio::sync::mpsc;

use fracker::{capture, config, instance, ocr, report, resident, tray, tuning};

/// Repo/install data file path. `$FRACKER_DATA_DIR`-aware (see
/// `tuning::data_dir`), so the Flatpak install resolves data from
/// `/app/share/fracker/data` via the launcher's env.
fn data_file(rel: &str) -> PathBuf {
    tuning::data_dir().join(rel)
}

#[derive(Parser, Debug)]
#[command(name = "fracker", about = "Warframe session planner (resident tray runtime)")]
struct Args {
    /// Render the checklist page from fixture/live inputs and exit.
    #[arg(long)]
    demo: bool,
    /// Where to write the page. Default: `docs/demo-plan.html` for
    /// `--demo`, the app data dir's `plan.html` for the resident.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Fixture directory (worldstate.json, profile fallback).
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
    /// Capture layout table for OCR (`[table]` key in --ocr-layouts).
    #[arg(long, default_value = "relic")]
    ocr_layout: String,
    /// Capture layout file defining per-screen OCR geometry.
    /// Default: `<data>/capture/layouts.toml`.
    #[arg(long)]
    ocr_layouts: Option<PathBuf>,
    /// Resident headless mode: identical watch + report loop, no tray.
    #[arg(long)]
    headless: bool,
    /// Legacy alias for `--headless`.
    #[arg(long)]
    watch: bool,
    /// Explicit EE.log path (overrides config + auto-detect).
    #[arg(long)]
    log: Option<PathBuf>,
    /// Watch wait tick in milliseconds; doubles as the poll fallback.
    #[arg(long, default_value_t = 1000)]
    watch_interval_ms: i32,
    /// Do not hit the network; read the recorded fixtures.
    #[arg(long)]
    offline: bool,
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
    if let Err(e) = run_resident(&args).await {
        eprintln!("fracker: {e}");
        std::process::exit(1);
    }
}

/// `--demo`: self-contained review export (embedded ≤128px icons,
/// addendum #23) written to `docs/demo-plan.html` by default.
async fn run_demo(args: &Args) -> Result<(), String> {
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("docs/demo-plan.html"));
    let cfg_path = args.config.clone().unwrap_or_else(config::config_file);
    let cfg = report::ReportConfig {
        data_dir: tuning::data_dir(),
        fixtures: args.fixtures.clone(),
        cache_dir: config::cache_dir(),
        out,
        stock_path: tuning::data_dir().join("stock.json"),
        config_path: cfg_path,
        offline: args.offline,
        embed_icons: true,
    };
    let outcome = report::generate(&cfg).await?;
    println!("demo: {} -> {}", outcome.summary(), outcome.out.display());
    println!(
        "demo: icons {}/{} · profile MR{} · endpoints: {}",
        outcome.icons_resolved,
        outcome.backlog_primes,
        outcome.mastery_rank,
        outcome.endpoints_label(),
    );
    println!(
        "demo: self-contained icons: {} embedded thumbnails, {} hotlink fallbacks",
        outcome.embedded, outcome.hotlinks,
    );
    Ok(())
}

/// The resident runtime (seventh pass): single-instance guard, the §4
/// watch in-process, the report pipeline, and (unless headless) the tray.
async fn run_resident(args: &Args) -> Result<(), String> {
    let headless = args.headless || args.watch;
    let cfg_path = args.config.clone().unwrap_or_else(config::config_file);
    let (cfg, _) = config::Config::load(&cfg_path)?;

    // Single instance: a second launch exits cleanly instead of
    // double-watching the EE.log.
    let _guard = match instance::InstanceGuard::acquire(&config::instance_lock_file()) {
        Ok(g) => g,
        Err(instance::InstanceError::AlreadyRunning) => {
            println!("fracker: already running (resident lock held); exiting cleanly");
            return Ok(());
        }
        Err(e) => return Err(format!("single-instance lock: {e}")),
    };

    let log_path = resolve_log_path(args, &cfg)?;
    let report_cfg = report::ReportConfig {
        data_dir: tuning::data_dir(),
        fixtures: args.fixtures.clone(),
        cache_dir: config::cache_dir(),
        out: args.out.clone().unwrap_or_else(config::report_file),
        stock_path: config::data_dir().join("stock.json"),
        config_path: cfg_path,
        offline: args.offline,
        embed_icons: false,
    };
    let opts = resident::ResidentOptions {
        report: report_cfg,
        log_path: log_path.clone(),
        state_dir: config::data_dir(),
        watch_interval_ms: args.watch_interval_ms,
    };
    let shutdown = resident::Shutdown::new();

    println!(
        "fracker resident: watching {} ({})",
        log_path.display(),
        if headless { "headless" } else { "tray" },
    );

    if headless {
        return resident::run_headless(opts, shutdown).await;
    }

    let status = Arc::new(Mutex::new(tray::TrayStatus::default()));
    let (cmds_tx, cmds_rx) = mpsc::unbounded_channel();
    match tray::spawn(status.clone(), cmds_tx).await {
        Ok(handle) => resident::run(opts, shutdown, Some(handle), status, Some(cmds_rx)).await,
        Err(e) => {
            // Tray absence must never change watch/report behavior
            // (brief §4): fall back to headless rather than dying.
            eprintln!("fracker: tray unavailable ({e}); running headless");
            resident::run_headless(opts, shutdown).await
        }
    }
}

/// Resolve the EE.log path: `--log` beats config `ee_log_path`, which
/// beats the data-owned Steam/Proton auto-detect. When no file exists
/// yet, the first candidate is returned so the watch can see a NEW
/// EE.log appear (addendum #37).
fn resolve_log_path(args: &Args, cfg: &config::Config) -> Result<PathBuf, String> {
    if let Some(p) = &args.log {
        return Ok(p.clone());
    }
    if !cfg.ee_log_path.trim().is_empty() {
        return Ok(PathBuf::from(cfg.ee_log_path.trim()));
    }
    let home = dirs::home_dir().ok_or("cannot find home dir for EE.log auto-detect")?;
    let paths = config::EeLogPaths::load_default()?;
    let candidates = config::ee_log_candidates(&home, &paths);
    if let Some(existing) = candidates.iter().find(|p| p.exists()) {
        return Ok(existing.clone());
    }
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| "no EE.log candidates; pass --log or set ee_log_path".to_owned())
}

/// OCR bootstrap ingestion: screenshots folder -> validated stock file.
fn run_ocr_ingest(args: &Args, dir: &std::path::Path) -> Result<(), String> {
    let tuning = tuning::Tuning::load_defaults()?;
    let _ = &tuning;

    // Capture geometry is content (addendum #20): load the layout table
    // and pick the screen class named by --ocr-layout.
    let layouts_path = args
        .ocr_layouts
        .clone()
        .unwrap_or_else(|| data_file("capture/layouts.toml"));
    let layouts = ocr::Layout::load_file(&layouts_path)?;
    let layout = layouts
        .get(&args.ocr_layout)
        .cloned()
        .ok_or_else(|| {
            format!(
                "ocr layout {:?} not in {}",
                args.ocr_layout,
                layouts_path.display()
            )
        })?;

    // Item universe: roster parts + gear + relic names + the arcane set,
    // scrubbed (the same names the planner reasons about). Shared with the
    // §2 Capture Inventory flow so the debug path and the tray flow agree.
    let (universe, relic_names, arcane_names) = capture::load_universe(&tuning::data_dir())?;

    // Engine: tesseract when the binary exists, else the stub (empty
    // stock, never guesses) — status printed honestly either way.
    let tesseract = ocr::TesseractCliEngine::probe(layout.clone());
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
        &layout,
        &universe,
        &relic_names,
        &arcane_names,
        0.5,
    )?;
    stock.write_to(&args.stock_out)?;
    let held = results.iter().filter(|r| r.held).count();
    let (relic_copies, part_copies, arcane_copies) = stock.totals();
    println!(
        "ocr: {} cells, {} held ({} relic stacks/{} copies, {} parts, {} arcane stacks/{} copies) -> {}",
        results.len(),
        held,
        stock.relic_stack_count(),
        relic_copies,
        part_copies,
        stock.arcane_stack_count(),
        arcane_copies,
        args.stock_out.display(),
    );
    Ok(())
}
