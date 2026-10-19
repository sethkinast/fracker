//! Resident runtime (seventh pass §2/§4): the watch + report loop.
//!
//! Fracker stays running. Inside one process:
//!
//! - a dedicated OS thread runs the §4 EE.log watch ([`crate::watch`]:
//!   directory events, drain-on-rotate, persisted position, startup
//!   reconciliation) and pushes [`Trigger`]s into an async channel;
//! - the async loop turns a trigger into a fresh report via
//!   [`crate::report::generate`] and refreshes the tray status;
//! - tray verbs arrive on a second channel ("Regenerate now",
//!   "Open plan page", "Quit").
//!
//! Headless parity (brief §4): with no tray handle and no command
//! channel, `run` still watches and reports identically — the tray is a
//! pure presentation layer and never changes watch/report behavior.
//!
//! Triggers (addendum #37): a NEW EE.log appearing (or rotating) fires a
//! report automatically, as does a `Logged in` session start inside one
//! file and every mission-end event during a session. The manual verb is
//! just another trigger.
//!
//! Auto-open discipline (twelfth pass §2): only a settled `Logged in`
//! session start may open the plan, and only once per game session. File
//! churn and regenerations never open; startup replay never opens.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

/// Warframe recreates EE.log in bursts before login settles. A `Logged
/// in` line is only promoted to a session start once the log has been
/// quiet for this long (twelfth pass §2), so one start yields one open.
const SESSION_SETTLE: Duration = Duration::from_millis(1500);

/// A vanished EE.log means the game closed. Only after it stays gone for
/// this long does the next login count as a NEW session, so startup churn
/// (delete/recreate) cannot re-arm the auto-open.
const SESSION_END_QUIET: Duration = Duration::from_secs(60);

use crate::capture::{self, CaptureData, CaptureLog, CaptureSession, CaptureStage};
use crate::ocr::StockState as OcrStock;
use crate::report::{self, ReportConfig};
use crate::state::AppState;
use crate::tray::{FrackerTray, TrayCommand, TrayStatus};
use crate::watch::LogWatcher;
use crate::{elog, portal, screencast, server, tabs, ticks};

/// What asked for a report run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A fresh game session started: a `Logged in` line after the app
    /// was already watching (twelfth pass §2). This — and only this —
    /// may auto-open the plan.
    SessionStart,
    /// A new EE.log appeared / rotated (addendum #37): regenerate, but
    /// never auto-open (twelfth pass §2).
    FileChanged,
    /// A mission-end event during the session.
    MissionEnd,
    /// The tray's manual Regenerate verb.
    Manual,
}

/// Cooperative shutdown for both the async loop and the watch thread.
#[derive(Clone)]
pub struct Shutdown {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl Shutdown {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, rx }
    }

    /// Ask the resident to stop.
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }

    pub fn is_triggered(&self) -> bool {
        *self.rx.borrow()
    }

    /// A receiver for the watch thread (and the async loop).
    pub fn receiver(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything the resident loop needs that is not tray-specific.
#[derive(Debug, Clone)]
pub struct ResidentOptions {
    /// Report pipeline config (paths + offline + icon mode).
    pub report: ReportConfig,
    /// Resolved EE.log path (may not exist yet — the watch sees it land).
    pub log_path: PathBuf,
    /// App state dir (`state.json`, persisted tail position).
    pub state_dir: PathBuf,
    /// Watch wait tick in milliseconds (also the poll fallback).
    pub watch_interval_ms: i32,
    /// Loopback HTTP port to serve the plan on (ninth pass §2). Falls back
    /// to an ephemeral port when taken; 0 = always ephemeral.
    pub serve_port: u16,
    /// Open the plan page automatically on session start (item 40b).
    pub pop_on_session_start: bool,
}

/// A message from the Capture Inventory worker thread to the resident
/// loop (eighth pass §2, ninth pass §1).
enum CaptureEvent {
    /// The lifecycle reached a new stage (consent → connecting → …).
    Stage(CaptureStage),
    /// Live per-bucket stack counts while scrolling.
    Progress(capture::CaptureCounts),
    /// The capture ended cleanly; the session carries the union of stacks
    /// and which buckets were touched (for the partial-safe finalize).
    Finished(Box<CaptureSession>),
    /// The capture aborted; `stage` names the failing lifecycle stage and
    /// `message` is the verbatim D-Bus/PipeWire error. Never swallowed.
    Failed {
        stage: &'static str,
        message: String,
    },
}

/// A running Capture Inventory worker. Dropping/stopping it ends the
/// capture; the worker owns the PipeWire source and its thread.
struct CaptureRun {
    stop: Arc<AtomicBool>,
    events: std::sync::mpsc::Receiver<CaptureEvent>,
    join: Option<std::thread::JoinHandle<()>>,
}

/// Run the resident with no tray (brief §4 headless mode). Identical
/// watch + report behavior; the absence of a tray changes nothing.
pub async fn run_headless(opts: ResidentOptions, shutdown: Shutdown) -> Result<(), String> {
    let status = Arc::new(Mutex::new(TrayStatus::default()));
    run(opts, shutdown, None, status, None).await
}

/// Run the resident with an optional tray handle + command channel.
pub async fn run(
    opts: ResidentOptions,
    shutdown: Shutdown,
    tray: Option<ksni::Handle<FrackerTray>>,
    status: Arc<Mutex<TrayStatus>>,
    mut cmds: Option<mpsc::UnboundedReceiver<TrayCommand>>,
) -> Result<(), String> {
    let (trigger_tx, mut trigger_rx) = mpsc::unbounded_channel::<Trigger>();
    let capture_log = CaptureLog::open(&opts.state_dir);
    // Ninth pass §4: the log-proven mission ledger the watch thread fills
    // and every report reads. Shared so the report path always sees the
    // latest completions, and seeded from persisted state so a restart
    // mid-session keeps its strikes.
    let completions: Arc<Mutex<Vec<elog::MissionCompletion>>> = Arc::new(Mutex::new(Vec::new()));
    // Ninth pass §2: the resident serves the plan itself on loopback, so
    // ticks persist server-side and "Open plan page" is a plain browser
    // URL (no file handoff, no chooser). Best-effort: a bind failure
    // leaves page_url unset and Open falls back to the file path.
    let tick_path = ticks::TickStore::file(&opts.state_dir);
    let tick_store = Arc::new(Mutex::new(ticks::TickStore::load(&tick_path)));
    // Thirteenth pass §2: the served page remembers the active tab
    // server-side so the 60s meta-refresh (and a restart) lands back where
    // Seth was, not on First Crack.
    let tab_path = tabs::TabStore::file(&opts.state_dir);
    let plan_server = match server::serve(
        opts.serve_port,
        opts.report.out.clone(),
        tick_path.clone(),
        tab_path,
        tick_store.clone(),
    ) {
        Ok(s) => {
            println!("resident: serving the plan at {}", s.url());
            if let Ok(mut st) = status.lock() {
                st.page_url = Some(s.url().to_owned());
            }
            Some(s)
        }
        Err(e) => {
            eprintln!("resident: plan server unavailable ({e}); opening the file path instead");
            None
        }
    };
    let watch_thread = spawn_watch(
        opts.log_path.clone(),
        opts.watch_interval_ms,
        opts.state_dir.clone(),
        status.clone(),
        trigger_tx,
        shutdown.receiver(),
        completions.clone(),
    );

    let mut sd = shutdown.receiver();
    // The Capture Inventory worker is polled on a light tick: draining the
    // event channel here (rather than holding a future borrow across the
    // select) keeps the loop borrow-check-clean while live counts still
    // refresh a few times a second.
    let mut capture: Option<CaptureRun> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = loop {
        tokio::select! {
            _ = sd.changed() => {
                if *sd.borrow() {
                    break Ok(());
                }
            }
            Some(trigger) = trigger_rx.recv() => {
                // Coalesce a burst (session start + file churn + mission
                // end can all arrive together) into a single report run,
                // and open only when the burst contained the settled
                // session start — never because of file churn (§2).
                let mut burst = vec![trigger];
                while let Ok(t) = trigger_rx.try_recv() {
                    burst.push(t);
                }
                let session_started = burst.contains(&Trigger::SessionStart);
                do_report(&opts, &status, &tray, burst[0], &completions).await;
                // Ninth pass §2/item 40b + twelfth pass §2: the settled
                // login is the only auto-open trigger, at most once per
                // session. pop_on_session_start is the §9 switch.
                if session_started && opts.pop_on_session_start {
                    open_page(&opts, &status, &tray, &completions, &plan_server).await;
                }
            }
            cmd = recv_cmd(&mut cmds) => {
                match cmd {
                    Some(TrayCommand::Regenerate) => {
                        do_report(&opts, &status, &tray, Trigger::Manual, &completions).await;
                    }
                    Some(TrayCommand::OpenPage) => {
                        open_page(&opts, &status, &tray, &completions, &plan_server).await;
                    }
                    Some(TrayCommand::CaptureInventory) => {
                        toggle_capture(&opts, &status, &tray, &mut capture, &capture_log).await;
                    }
                    Some(TrayCommand::Quit) | None => break Ok(()),
                }
            }
            _ = tick.tick() => {
                pump_capture(&opts, &status, &tray, &mut capture, &capture_log, &completions)
                    .await;
            }
        }
    };

    // Cooperative shutdown order: release the watch first (it listens on
    // the shutdown receiver), then stop/reap any capture, then close the
    // tray. ksni's `Handle` has no Drop and its zbus service/socket tasks
    // otherwise keep the tokio runtime alive after the loop returns — the
    // reason a tray Quit used to leave the process resident (item 48).
    shutdown.trigger();
    stop_capture(&mut capture);
    let _ = watch_thread.join();
    if let Some(handle) = &tray {
        match tokio::time::timeout(Duration::from_secs(3), handle.shutdown()).await {
            Ok(()) => {}
            Err(_) => eprintln!("resident: tray shutdown timed out"),
        }
    }
    result
}

/// Move the shared capture lifecycle to `stage` and append the transition
/// to the capture log (ninth pass §1: every transition is instrumented).
fn set_capture_stage(status: &Arc<Mutex<TrayStatus>>, log: &CaptureLog, stage: CaptureStage) {
    if let Ok(mut s) = status.lock() {
        s.capture.set_stage(stage);
        let line = s.capture.status_line();
        log.record("stage", &line);
    }
}

/// Record a capture failure at `stage`: the verbatim message goes to the
/// log and the lifecycle lands in the visible Error state.
fn fail_capture(
    status: &Arc<Mutex<TrayStatus>>,
    log: &CaptureLog,
    stage: &'static str,
    message: &str,
) {
    log.record(stage, message);
    if let Ok(mut s) = status.lock() {
        s.capture.fail(stage, message);
        let line = s.capture.status_line();
        log.record("stage", &line);
    }
}

/// Toggle the Capture Inventory flow: start a ScreenCast session when
/// idle, ask the running one to stop when capturing.
async fn toggle_capture(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    tray: &Option<ksni::Handle<FrackerTray>>,
    capture: &mut Option<CaptureRun>,
    log: &CaptureLog,
) {
    if let Some(run) = capture.as_ref() {
        run.stop.store(true, Ordering::Relaxed);
        set_capture_stage(status, log, CaptureStage::Finalizing);
        log.record("finalizing", "stop requested by the tray toggle");
        refresh_tray(tray).await;
        return;
    }

    set_capture_stage(status, log, CaptureStage::ConsentPending);
    log.record("consent pending", "asking the ScreenCast portal for consent");
    refresh_tray(tray).await;

    match start_capture(opts, status, log).await {
        Ok(run) => {
            println!("resident: capture started");
            *capture = Some(run);
        }
        Err(e) => {
            eprintln!("resident: capture start failed ({}): {}", e.stage, e.message);
            // Name the stage that FAILED (eleventh pass §2, item 40a): a
            // post-consent remote-fd failure must not render as
            // "consent pending". `NegotiateError` carries the stage.
            fail_capture(status, log, e.stage, &e.message);
            refresh_tray(tray).await;
        }
    }
}

/// Open the ScreenCast portal session and spawn the capture worker thread.
async fn start_capture(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    log: &CaptureLog,
) -> Result<CaptureRun, screencast::NegotiateError> {
    let token = screencast::load_restore_token(&opts.state_dir);
    match token.as_ref() {
        Some(_) => log.record("consent pending", "using saved restore token"),
        None => log.record(
            "consent pending",
            "no restore token yet (first run) — full consent required",
        ),
    }
    let negotiated = screencast::negotiate(token).await?;
    log.record(
        "consent pending",
        &format!(
            "consent granted; remote fd ready (restore_token {})",
            if negotiated.restore_token.is_some() {
                "issued"
            } else {
                "unchanged"
            },
        ),
    );
    if let Some(tok) = negotiated.restore_token.clone() {
        if let Err(e) = screencast::save_restore_token(&opts.state_dir, &tok) {
            // A token we cannot persist costs Seth a consent dialog next
            // time; it is not fatal, but it is visible.
            eprintln!("resident: could not save ScreenCast token: {e}");
            log.record("consent pending", &format!("could not save token: {e}"));
        }
    }
    // Consent is done; the worker owns connecting → capturing.
    set_capture_stage(status, log, CaptureStage::Connecting);
    let (tx, events) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let data_dir = opts.report.data_dir.clone();
    let worker_log = log.clone();
    let join = std::thread::spawn(move || {
        capture_worker(data_dir, negotiated, stop_thread, tx, worker_log)
    });
    Ok(CaptureRun {
        stop,
        events,
        join: Some(join),
    })
}

/// The blocking capture worker: load content, connect PipeWire, and drain
/// frames through the §2 pipeline until stopped. Every failure is sent to
/// the loop as a visible [`CaptureEvent::Failed`] naming its stage.
fn capture_worker(
    data_dir: PathBuf,
    negotiated: screencast::Negotiated,
    stop: Arc<AtomicBool>,
    tx: std::sync::mpsc::Sender<CaptureEvent>,
    log: CaptureLog,
) {
    let data = match CaptureData::load(&data_dir) {
        Ok(d) => d,
        Err(e) => {
            return send_failed(&tx, "connecting", format!("capture data: {e}"));
        }
    };
    let Some(layout) = data.layouts.get("relic").cloned() else {
        return send_failed(
            &tx,
            "connecting",
            "capture: no relic layout in capture/layouts.toml".to_owned(),
        );
    };
    let Some(engine) = crate::ocr::TesseractCliEngine::probe(layout) else {
        return send_failed(&tx, "connecting", "capture: tesseract unavailable".to_owned());
    };
    let mut source = match screencast::ScreenCastSource::new(negotiated, stop.clone(), log.clone())
    {
        Ok(s) => s,
        Err(e) => {
            return send_failed(&tx, "connecting", format!("capture: {e}"));
        }
    };
    // The stream is connected; from here the lifecycle is Capturing until
    // the drain ends.
    let _ = tx.send(CaptureEvent::Stage(CaptureStage::Capturing));
    log.record("capturing", "draining frames (scroll your inventory)");
    let ctx = data.context();
    let idle = data.classifier.idle_stop_frames;
    let progress_tx = tx.clone();
    let result = capture::run_capture(
        &ctx,
        &engine,
        &mut source,
        &stop,
        idle,
        move |s| {
            let _ = progress_tx.send(CaptureEvent::Progress(s.counts()));
        },
    );
    match result {
        Ok(session) => {
            let _ = tx.send(CaptureEvent::Finished(Box::new(session)));
        }
        Err(e) => send_failed(&tx, "capturing", format!("capture: {e}")),
    }
}

/// Send a staged failure to the resident loop.
fn send_failed(tx: &std::sync::mpsc::Sender<CaptureEvent>, stage: &'static str, message: String) {
    let _ = tx.send(CaptureEvent::Failed { stage, message });
}

/// Drain the capture worker's events: apply lifecycle stages, refresh live
/// counts, and on completion finalize the stock + regenerate the plan.
async fn pump_capture(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    tray: &Option<ksni::Handle<FrackerTray>>,
    capture: &mut Option<CaptureRun>,
    log: &CaptureLog,
    completions: &Arc<Mutex<Vec<elog::MissionCompletion>>>,
) {
    let Some(run) = capture.as_ref() else { return };
    let mut stages: Vec<CaptureStage> = Vec::new();
    let mut latest: Option<capture::CaptureCounts> = None;
    let mut finished: Option<CaptureEvent> = None;
    loop {
        match run.events.try_recv() {
            Ok(CaptureEvent::Stage(s)) => stages.push(s),
            Ok(CaptureEvent::Progress(c)) => latest = Some(c),
            Ok(ev) => {
                finished = Some(ev);
                break;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                finished = Some(CaptureEvent::Failed {
                    stage: "capturing",
                    message: "capture worker stopped unexpectedly".to_owned(),
                });
                break;
            }
        }
    }

    let mut changed = false;
    for stage in stages {
        if let Ok(mut s) = status.lock() {
            s.capture.set_stage(stage);
            log.record("stage", &s.capture.status_line());
            changed = true;
        }
    }
    if let Some(c) = latest {
        if finished.is_none() {
            if let Ok(mut s) = status.lock() {
                s.capture.set_counts(c);
                changed = true;
            }
        }
    }
    if changed {
        refresh_tray(tray).await;
    }

    match finished {
        Some(CaptureEvent::Finished(session)) => {
            finalize_capture(opts, status, tray, *session, log, completions).await;
            stop_capture(capture);
        }
        Some(CaptureEvent::Failed { stage, message }) => {
            eprintln!("resident: capture failed ({stage}): {message}");
            fail_capture(status, log, stage, &message);
            stop_capture(capture);
            refresh_tray(tray).await;
        }
        _ => {}
    }
}

/// Finalize a completed capture: captured buckets REPLACE the matching
/// buckets in `stock.json` (untouched buckets survive), then the plan is
/// regenerated and the tray reports what was collected.
async fn finalize_capture(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    tray: &Option<ksni::Handle<FrackerTray>>,
    session: CaptureSession,
    log: &CaptureLog,
    completions: &Arc<Mutex<Vec<elog::MissionCompletion>>>,
) {
    set_capture_stage(status, log, CaptureStage::Finalizing);
    if !session.has_any() {
        log.record("finalizing", "nothing collected — stock untouched");
        set_capture_stage(status, log, CaptureStage::Idle);
        refresh_tray(tray).await;
        return;
    }

    let path = &opts.report.stock_path;
    let mut existing = match std::fs::read_to_string(path) {
        Ok(text) => match OcrStock::from_json(&text) {
            Ok(s) => s,
            Err(e) => {
                // Never clobber a stock file we cannot read.
                eprintln!("resident: capture finalize: {e}");
                fail_capture(status, log, "finalizing", &format!("capture finalize: {e}"));
                refresh_tray(tray).await;
                return;
            }
        },
        Err(_) => OcrStock::default(),
    };
    session.finalize_into(&mut existing);
    if let Err(e) = existing.write_to(path) {
        eprintln!("resident: capture finalize: {e}");
        fail_capture(status, log, "finalizing", &format!("capture finalize: {e}"));
        refresh_tray(tray).await;
        return;
    }

    let summary = capture::captured_summary(&session);
    println!("resident: {summary} -> {}", path.display());
    log.record("finalizing", &format!("{summary} -> {}", path.display()));
    // Regenerate the plan from the fresh stock (and refresh the tray).
    do_report(opts, status, tray, Trigger::Manual, completions).await;
    set_capture_stage(status, log, CaptureStage::Idle);
}

/// Stop the worker (if any) and reap its thread.
fn stop_capture(capture: &mut Option<CaptureRun>) {
    if let Some(mut run) = capture.take() {
        run.stop.store(true, Ordering::Relaxed);
        if let Some(h) = run.join.take() {
            let _ = h.join();
        }
    }
}

/// Await a tray command, or stay pending forever when there is no tray.
async fn recv_cmd(rx: &mut Option<mpsc::UnboundedReceiver<TrayCommand>>) -> Option<TrayCommand> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

/// Generate one report and update the shared status.
async fn do_report(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    tray: &Option<ksni::Handle<FrackerTray>>,
    trigger: Trigger,
    completions: &Arc<Mutex<Vec<elog::MissionCompletion>>>,
) {
    println!("resident: regenerating page ({trigger:?})");
    let proven: Vec<elog::MissionCompletion> = completions
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    match report::generate(&opts.report, &proven).await {
        Ok(outcome) => {
            if let Ok(mut s) = status.lock() {
                s.last_report = Some(chrono::Local::now().format("%H:%M:%S").to_string());
                s.last_error = None;
            }
            println!(
                "resident: {} -> {}",
                outcome.summary(),
                outcome.out.display()
            );
        }
        Err(e) => {
            eprintln!("resident: report failed: {e}");
            if let Ok(mut s) = status.lock() {
                s.last_error = Some(e);
            }
        }
    }
    refresh_tray(tray).await;
}

/// "Open plan page" (ninth pass §2): open the loopback URL via the
/// OpenURI portal (default browser, no chooser) when the server is up,
/// else fall back to the OpenFile route on the rendered path. The report
/// is generated first when missing. A failure is surfaced, never silent.
async fn open_page(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    tray: &Option<ksni::Handle<FrackerTray>>,
    completions: &Arc<Mutex<Vec<elog::MissionCompletion>>>,
    plan_server: &Option<server::PlanServer>,
) {
    let out = &opts.report.out;
    if !out.exists() {
        println!("resident: no report yet; generating before open");
        let proven: Vec<elog::MissionCompletion> = completions
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        if let Err(e) = report::generate(&opts.report, &proven).await {
            eprintln!("resident: report failed: {e}");
            if let Ok(mut s) = status.lock() {
                s.last_error = Some(e);
            }
            refresh_tray(tray).await;
            return;
        }
    }
    let opened = match plan_server {
        Some(s) => portal::open_url(s.url()).await.map(|()| s.url().to_owned()),
        None => portal::open_plan(out).await.map(|()| out.display().to_string()),
    };
    match opened {
        Ok(target) => {
            if let Ok(mut s) = status.lock() {
                s.last_error = None;
            }
            println!("resident: opened {target}");
        }
        Err(e) => {
            eprintln!("resident: open failed: {e}");
            if let Ok(mut s) = status.lock() {
                s.last_error = Some(format!("open: {e}"));
            }
        }
    }
    refresh_tray(tray).await;
}

/// Nudge the tray to re-read its properties after a status change.
async fn refresh_tray(tray: &Option<ksni::Handle<FrackerTray>>) {
    if let Some(handle) = tray {
        handle.update(|_| ()).await;
    }
}

/// Spawn the blocking EE.log watch on a dedicated OS thread, feeding
/// [`Trigger`]s into the async loop.
fn spawn_watch(
    log_path: PathBuf,
    interval_ms: i32,
    state_dir: PathBuf,
    status: Arc<Mutex<TrayStatus>>,
    trigger_tx: mpsc::UnboundedSender<Trigger>,
    shutdown: watch::Receiver<bool>,
    completions: Arc<Mutex<Vec<elog::MissionCompletion>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(e) = watch_loop(
            log_path,
            interval_ms,
            state_dir,
            status.clone(),
            trigger_tx,
            shutdown,
            completions,
        ) {
            eprintln!("resident: watch stopped: {e}");
            if let Ok(mut s) = status.lock() {
                s.last_error = Some(e);
            }
        }
    })
}

/// The UTC date (`YYYY-MM-DD`) the §4 mission ledger is scoped to.
fn today_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// The UTC wall-clock instant (`RFC 3339`) stamped onto every §4
/// completion so the activation guard can judge its instance (§4).
fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// The watch loop: reconcile, tail, classify, trigger, persist.
fn watch_loop(
    log_path: PathBuf,
    interval_ms: i32,
    state_dir: PathBuf,
    status: Arc<Mutex<TrayStatus>>,
    trigger_tx: mpsc::UnboundedSender<Trigger>,
    shutdown: watch::Receiver<bool>,
    completions: Arc<Mutex<Vec<elog::MissionCompletion>>>,
) -> Result<(), String> {
    let mut st = AppState::load(&state_dir)?;
    let mut watcher = LogWatcher::open(&log_path)?;
    watcher.resume(st.log_position)?;
    if let Ok(mut s) = status.lock() {
        s.watching = true;
        s.dir_watch = watcher.watching_dir();
        s.log_path = Some(log_path.display().to_string());
    }

    let mut tracker = elog::SessionTracker::new();
    let mut last_tick = Instant::now();
    let mut last_inode = watcher.position().inode;

    // Twelfth pass §2: when the log already exists at app start, the first
    // poll reconciles content that predates us. That content is history —
    // never a game session start — so it must not auto-open the plan. A
    // log that appears *after* we are watching is a live session, even if
    // it lands during the first poll window.
    let mut startup_replay = log_path.exists() || st.log_position.inode != 0;
    // A login is promoted to SessionStart only after the startup burst has
    // settled, and each game session may do so exactly once.
    let mut pending_login: Option<Instant> = None;
    let mut session_started = false;
    // When the log has been continuously absent this long, the game
    // session is over and the next settled login may open again.
    let mut absent_since: Option<Instant> = None;

    // Ninth pass §4: the mission ledger. `missions` pairs each armed
    // `MissionInfo` block with its end-of-match success marker; proven
    // completions are stamped with the UTC day, persisted in app state,
    // and published to the shared ledger every report reads.
    let mut missions = elog::MissionLog::new();
    let mut today = today_utc();
    st.mission_completions.retain(|c| c.day == today);
    if let Ok(mut g) = completions.lock() {
        *g = st.mission_completions.clone();
    }

    loop {
        if *shutdown.borrow() {
            break;
        }
        let lines = watcher.next_lines(interval_ms)?;
        let pos = watcher.position();

        // §2: a log that stays gone for a while means the game session
        // ended, arming the next settled login. Brief startup churn
        // (delete → recreate) is far shorter than SESSION_END_QUIET and
        // cannot re-arm the auto-open.
        if pos.inode == 0 {
            let since = *absent_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= SESSION_END_QUIET {
                session_started = false;
                pending_login = None;
            }
        } else {
            absent_since = None;
        }

        // A UTC day rollover retires yesterday's completions: the ledger
        // is today-scoped (ninth pass §4).
        let mut rolled = false;
        let now_day = today_utc();
        if now_day != today {
            today = now_day;
            st.mission_completions.retain(|c| c.day == today);
            if let Ok(mut g) = completions.lock() {
                g.retain(|c| c.day == today);
            }
            rolled = true;
        }

        // A fresh inode (file appeared or rotated) is a hard session edge
        // (addendum #37): the tracker sees an infinite gap so a re-login
        // reads as a start, and the appearance itself fires a report.
        let new_file = pos.inode != 0 && pos.inode != last_inode;
        let gap = if pos.inode != last_inode {
            u64::MAX
        } else {
            last_tick.elapsed().as_secs()
        };
        last_tick = Instant::now();
        last_inode = pos.inode;

        let mut saw_login = false;
        for raw in &lines {
            let line = elog::parse_line(raw);
            let ev = elog::classify(&line);
            let _ = tracker.feed(ev.as_ref(), gap);
            if let Some(elog::Event::LoggedIn { name }) = &ev {
                st.display_name = name.clone();
                saw_login = true;
            }
            if ev.as_ref().map_or(false, |e| e.is_mission_end()) {
                let _ = trigger_tx.send(Trigger::MissionEnd);
            }
            // §4: prove a mission from its armed identity + success marker.
            if let Some(done) = missions.feed(raw, &today, &now_utc()) {
                st.mission_completions.push(done.clone());
                if let Ok(mut g) = completions.lock() {
                    g.push(done);
                }
                // A proven completion is worth a fresh page even when the
                // marker did not classify as a mission-end event.
                let _ = trigger_tx.send(Trigger::MissionEnd);
            }
        }
        // A file appearing or rotating regenerates, but never opens
        // (twelfth pass §2) — including the startup reconciliation read.
        // Churn coalesces into one report in `run`.
        if new_file {
            let _ = trigger_tx.send(Trigger::FileChanged);
        }
        if !startup_replay {
            // Promote a login only once the burst has settled. Repeated
            // logins and recreated files inside the window coalesce into
            // one session start; a started session never emits again.
            if saw_login && !session_started {
                pending_login.get_or_insert_with(Instant::now);
            }
            if let Some(at) = pending_login {
                if at.elapsed() >= SESSION_SETTLE {
                    pending_login = None;
                    session_started = true;
                    let _ = trigger_tx.send(Trigger::SessionStart);
                }
            }
        }
        startup_replay = false;

        let dirty = !lines.is_empty() || pos != st.log_position || rolled;
        st.log_position = pos;
        if dirty {
            st.save(&state_dir)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fracker-resident-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn recv_trigger(rx: &mut mpsc::UnboundedReceiver<Trigger>) -> Option<Trigger> {
        for _ in 0..500 {
            if let Ok(t) = rx.try_recv() {
                return Some(t);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// Drain everything queued for `window`.
    fn collect_triggers(
        rx: &mut mpsc::UnboundedReceiver<Trigger>,
        window: Duration,
    ) -> Vec<Trigger> {
        let deadline = Instant::now() + window;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            while let Ok(t) = rx.try_recv() {
                out.push(t);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        out
    }

    #[test]
    fn shutdown_flag_is_observable() {
        let s = Shutdown::new();
        assert!(!s.is_triggered());
        s.trigger();
        assert!(s.is_triggered());
        assert!(*s.receiver().borrow());
    }

    #[test]
    fn new_log_file_appearance_triggers_a_settled_session_start() {
        // The watch thread must fire without a manual trigger when the
        // EE.log appears — this is the §4/#37 guarantee at unit scale.
        // §2: the file edge regenerates immediately; the login settles
        // into exactly one SessionStart (the only auto-open trigger).
        let dir = tmpdir("appear");
        let log = dir.join("EE.log");
        let state = dir.join("state");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let shutdown = Shutdown::new();
        let status = Arc::new(Mutex::new(TrayStatus::default()));
        let handle = spawn_watch(
            log.clone(),
            20,
            state.clone(),
            status,
            tx,
            shutdown.receiver(),
            Arc::new(Mutex::new(Vec::new())),
        );

        // No file yet: nothing fires.
        assert_eq!(recv_trigger(&mut rx), None, "no trigger before the file exists");

        // A new EE.log lands with a login line.
        let mut f = std::fs::File::create(&log).unwrap();
        use std::io::Write;
        writeln!(f, "14.148 Sys [Info]: Logged in Jenterrobahne").unwrap();
        f.flush().unwrap();

        assert_eq!(recv_trigger(&mut rx), Some(Trigger::FileChanged));
        assert_eq!(recv_trigger(&mut rx), Some(Trigger::SessionStart));

        shutdown.trigger();
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mission_end_event_triggers_a_regeneration() {
        let dir = tmpdir("mission");
        let log = dir.join("EE.log");
        let state = dir.join("state");
        // Pre-existing file at app start is startup replay (§2): the
        // login is history and must NOT emit a session start / open.
        std::fs::write(
            &log,
            "14.148 Sys [Info]: Logged in Jenterrobahne\n",
        )
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let shutdown = Shutdown::new();
        let status = Arc::new(Mutex::new(TrayStatus::default()));
        let handle = spawn_watch(log.clone(), 20, state.clone(), status, tx, shutdown.receiver(), Arc::new(Mutex::new(Vec::new())));
        let startup = collect_triggers(&mut rx, Duration::from_millis(250));
        assert!(
            !startup.contains(&Trigger::SessionStart),
            "startup replay never opens the plan: {startup:?}"
        );

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, "99.001 Script [Info]: ProjectionRewardChoice.lua: Got rewards").unwrap();
        f.flush().unwrap();

        assert_eq!(recv_trigger(&mut rx), Some(Trigger::MissionEnd));

        shutdown.trigger();
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// §2 gate: the v0.4.0 startup sequence is a churn of new-file events
    /// after the app is already running. It must produce exactly one
    /// auto-open (one SessionStart), at the settled login — never at app
    /// start, never per file event, never again for the same session.
    #[test]
    fn startup_burst_opens_exactly_once_at_the_settled_login() {
        let dir = tmpdir("burst");
        let log = dir.join("EE.log");
        let state = dir.join("state");

        // App starts with Seth's recorded 2026-10-18 startup-only session
        // already on disk. Replaying it is history, not a session start.
        let fixture = include_str!("../fixtures/ocr-samples/ee-log-2026-10-18.log");
        std::fs::write(&log, fixture).unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let shutdown = Shutdown::new();
        let status = Arc::new(Mutex::new(TrayStatus::default()));
        let handle = spawn_watch(
            log.clone(),
            20,
            state.clone(),
            status,
            tx,
            shutdown.receiver(),
            Arc::new(Mutex::new(Vec::new())),
        );

        // App start with no new log: a regeneration may run, never an open.
        let startup = collect_triggers(&mut rx, Duration::from_millis(400));
        assert!(
            !startup.contains(&Trigger::SessionStart),
            "app startup must not open the plan: {startup:?}"
        );

        // Warframe startup churn: the log is recreated several times with
        // the login line in the burst. Every edge may regenerate, but the
        // whole burst must coalesce into one session start.
        use std::io::Write;
        for i in 0..5 {
            let _ = std::fs::remove_file(&log);
            let mut f = std::fs::File::create(&log).unwrap();
            writeln!(f, "0.00{i} Sys [Info]: Logged in Jenterrobahne").unwrap();
            writeln!(f, "1.00{i} Sys [Info]: startup churn {i}").unwrap();
            f.flush().unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }

        let burst = collect_triggers(&mut rx, Duration::from_millis(2500));
        assert_eq!(
            burst.iter().filter(|t| **t == Trigger::SessionStart).count(),
            1,
            "exactly one auto-open per session, at login only: {burst:?}"
        );
        assert!(
            burst.contains(&Trigger::FileChanged),
            "file churn still regenerates: {burst:?}"
        );

        // A later login line inside the same (still present) session must
        // not re-open.
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, "2.000 Sys [Info]: Logged in Jenterrobahne").unwrap();
        f.flush().unwrap();
        let later = collect_triggers(&mut rx, Duration::from_millis(2000));
        assert!(
            !later.contains(&Trigger::SessionStart),
            "the same session never opens twice: {later:?}"
        );

        shutdown.trigger();
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
