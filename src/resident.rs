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

use std::path::{Path, PathBuf};
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

/// Teardown bounds (item 53). A step that exceeds these is logged as
/// "detached" and left to `process::exit`; no step may keep the tray icon
/// (and the process) alive forever.
const WATCH_JOIN_TIMEOUT: Duration = Duration::from_secs(2);
const CAPTURE_JOIN_TIMEOUT: Duration = Duration::from_secs(2);
const TRAY_REFRESH_TIMEOUT: Duration = Duration::from_secs(2);
const TASK_ABORT_TIMEOUT: Duration = Duration::from_secs(2);

use crate::applog::AppLog;
use crate::capture::{self, CaptureData, CaptureLog, CaptureSession, CaptureStage, FrameSource};
use crate::ocr::StockState as OcrStock;
use crate::report::{self, ReportConfig};
use crate::state::AppState;
use crate::tray::{FrackerTray, TrayCommand, TrayStatus};
use crate::watch::LogWatcher;
use crate::{elog, packopen, portal, reward, screencast, server, tabs, ticks};

/// What asked for a report run.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Seth's relic reward was granted (pass 39; rekeyed pass 49 to
    /// the `VoidProjections: <his id> gets reward` line). Carries the
    /// roll's uniqueName path for the arm log line. NOT a report
    /// trigger: it arms the reward burst grab.
    RewardScreen(String),
    /// The reward-choice screen shut down (pass 39). NOT a report
    /// trigger: it closes the reward episode.
    RewardClosed,
    /// A relic pack purchase dialog opened (pass 53). Carries
    /// the BoosterPacks item path for the arm log line. NOT a
    /// report trigger: it arms the pack watch.
    PackScreen(String),
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
    /// Reward-pick detection on the fissure reward screen
    /// (pass 39; config `reward_detect`, default on).
    pub reward_detect: bool,
    /// Pack-open detection on relic pack reveal screens
    /// (pass 53; config `pack_detect`, default on).
    pub pack_detect: bool,
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
    mut opts: ResidentOptions,
    shutdown: Shutdown,
    tray: Option<ksni::Handle<FrackerTray>>,
    status: Arc<Mutex<TrayStatus>>,
    mut cmds: Option<mpsc::UnboundedReceiver<TrayCommand>>,
) -> Result<(), String> {
    let (trigger_tx, mut trigger_rx) = mpsc::unbounded_channel::<Trigger>();
    let capture_log = CaptureLog::open(&opts.state_dir);
    // Item 53: a Quit that leaves a zombie is invisible in unit tests. Every
    // activation and every teardown step lands in `resident.log`, so one live
    // run names the hang point.
    let app_log = AppLog::open(&opts.state_dir);
    app_log.record(
        "resident",
        &format!("start (pid {} tray {})", std::process::id(), tray.is_some()),
    );
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
    // server-side so a fresh load (and a restart) lands back where
    // Seth was, not on First Crack.
    let tab_path = tabs::TabStore::file(&opts.state_dir);
    // Thirtieth pass (#93): the Hunt tab's unverified-stock toggle.
    // The loopback server mutates stock.json through this context
    // (validating against the roster seed); the regen hook is a
    // Manual trigger — a regeneration like the tray verb's, which
    // never auto-opens the page.
    let unverified_toggle = {
        let tx = trigger_tx.clone();
        Arc::new(server::UnverifiedToggle::new(
            opts.report.stock_path.clone(),
            opts.report.data_dir.join("cache/wishlist.json"),
            Arc::new(move || {
                let _ = tx.send(Trigger::Manual);
            }),
        ))
    };
    // Fifty-fifth pass: the DE block clear route. The lockfile lives
    // in the fetch cache dir (the fetch plumbing already threads
    // it); clearing schedules the same Manual regen as the
    // unverified toggle, so the header re-renders without the alert.
    let de_lock_clear = {
        let tx = trigger_tx.clone();
        Arc::new(server::DeLockClear::new(
            crate::fetch::de_lock_path(&opts.report.cache_dir),
            Arc::new(move || {
                let _ = tx.send(Trigger::Manual);
            }),
        ))
    };
    let plan_server = match server::serve(
        opts.serve_port,
        opts.report.out.clone(),
        tick_path.clone(),
        tab_path,
        tick_store.clone(),
        Some(unverified_toggle),
        Some(de_lock_clear),
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
    // Item 58: every report published by the resident pushes its new
    // generation through the server's hub, so open pages fetch the
    // fragment diff the moment a regeneration lands instead of waiting
    // for the slow poll (forty-first pass).
    // Attached before the watch thread or any report job starts.
    if let Some(server) = &plan_server {
        opts.report.push = Some(server.hub());
    }
    // Pass 39: a regen sender for the reward grab task (credit
    // fires the same Manual trigger the /unverified toggle uses).
    let reward_regen_tx = trigger_tx.clone();
    let watch_thread = spawn_watch(
        opts.log_path.clone(),
        opts.watch_interval_ms,
        opts.state_dir.clone(),
        status.clone(),
        trigger_tx,
        shutdown.receiver(),
        completions.clone(),
    );
    let plan_url = plan_server.as_ref().map(|s| s.url().to_owned());

    let mut sd = shutdown.receiver();
    // The Capture Inventory worker is polled on a light tick: draining the
    // event channel here (rather than holding a future borrow across the
    // select) keeps the loop borrow-check-clean while live counts still
    // refresh a few times a second.
    let mut capture: Option<CaptureRun> = None;
    // Item 53 root cause: a report (or a capture consent) used to run INSIDE
    // the control loop, so a Quit clicked while one was in flight was only
    // dequeued after it finished — up to a full network timeout later. The
    // tray icon stayed up and the menu looked dead. Reports now run as their
    // own task; the loop only reacts to completion, so Quit is always read.
    let (job_tx, mut job_rx) = mpsc::unbounded_channel::<()>();
    let mut report_job: Option<tokio::task::JoinHandle<()>> = None;
    let mut queued: Option<(Trigger, bool)> = None;
    let (capture_start_tx, mut capture_start_rx) =
        mpsc::unbounded_channel::<Result<CaptureRun, screencast::NegotiateError>>();
    let mut capture_start: Option<tokio::task::JoinHandle<()>> = None;
    // Pass 39: reward-grab state. The tap is shared with the
    // capture worker (its frame source offers frames while an
    // episode is armed); the arming coalesces an episode's
    // reward-grant events into one burst per episode.
    let reward_tap = Arc::new(reward::FrameTap::new());
    let mut reward_arming = reward::RewardArming::new();
    let (reward_done_tx, mut reward_done_rx) = mpsc::unbounded_channel::<()>();
    let mut reward_task: Option<tokio::task::JoinHandle<()>> = None;
    // Pass 53: pack-watch state. Its own tap (offered the same
    // capture frames as the reward tap) and one task per armed
    // session; a second dialog line while a watch is running
    // is ignored — the running watch is already looking.
    let pack_tap = Arc::new(reward::FrameTap::new());
    let (pack_done_tx, mut pack_done_rx) = mpsc::unbounded_channel::<()>();
    let mut pack_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    macro_rules! schedule_report {
        ($trigger:expr, $open:expr) => {{
            let trigger = $trigger;
            let open = $open;
            if report_job.is_none() {
                report_job = Some(spawn_report_job(
                    opts.clone(),
                    status.clone(),
                    tray.clone(),
                    trigger,
                    completions.clone(),
                    open,
                    plan_url.clone(),
                    app_log.clone(),
                    job_tx.clone(),
                ));
            } else {
                // One follow-up is enough: every trigger means "the page is
                // stale". Preserve an auto-open request from a session start
                // that arrived while a report was already running.
                let queued_open = queued.take().map(|(_, o)| o).unwrap_or(false);
                queued = Some((trigger, queued_open || open));
            }
        }};
    }

    let result = loop {
        tokio::select! {
            _ = sd.changed() => {
                if *sd.borrow() {
                    app_log.record("quit", "shutdown signal observed");
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
                // Pass 39: reward triggers never schedule a report.
                // Partition them out and drive the grab arming: a
                // shutdown closes the episode; a reward grant arms
                // one burst per episode (pass 49: the gets-reward
                // line fires once per true screen; RewardArming
                // still coalesces any repeats within the episode).
                let mut reward_roll: Option<String> = None;
                let mut reward_closed = false;
                let mut pack_path: Option<String> = None;
                burst.retain(|t| match t {
                    Trigger::RewardScreen(roll) => {
                        if reward_roll.is_none() {
                            reward_roll = Some(roll.clone());
                        }
                        false
                    }
                    Trigger::RewardClosed => {
                        reward_closed = true;
                        false
                    }
                    Trigger::PackScreen(path) => {
                        if pack_path.is_none() {
                            pack_path = Some(path.clone());
                        }
                        false
                    }
                    _ => true,
                });
                if reward_closed {
                    reward_arming.on_closed();
                }
                if let Some(roll) = reward_roll {
                    if opts.reward_detect && reward_arming.on_screen() {
                        reward_task = Some(spawn_reward_grab(
                            opts.clone(),
                            capture.is_some(),
                            roll,
                            reward_tap.clone(),
                            reward_regen_tx.clone(),
                            capture_log.clone(),
                            reward_done_tx.clone(),
                        ));
                    }
                }
                if let Some(path) = pack_path {
                    if opts.pack_detect && pack_task.is_none() {
                        pack_task = Some(spawn_pack_watch(
                            opts.clone(),
                            capture.is_some(),
                            path,
                            pack_tap.clone(),
                            reward_regen_tx.clone(),
                            capture_log.clone(),
                            pack_done_tx.clone(),
                        ));
                    }
                }
                if !burst.is_empty() {
                    let session_started = burst.contains(&Trigger::SessionStart);
                    schedule_report!(burst[0].clone(), session_started && opts.pop_on_session_start);
                }
            }
            cmd = recv_cmd(&mut cmds) => {
                match cmd {
                    Some(TrayCommand::Regenerate) => {
                        schedule_report!(Trigger::Manual, false);
                    }
                    Some(TrayCommand::OpenPage) => {
                        spawn_open_job(
                            opts.clone(),
                            status.clone(),
                            tray.clone(),
                            completions.clone(),
                            plan_url.clone(),
                            app_log.clone(),
                        );
                    }
                    Some(TrayCommand::CaptureInventory) => {
                        if let Some(run) = capture.as_ref() {
                            run.stop.store(true, Ordering::Relaxed);
                            set_capture_stage(&status, &capture_log, CaptureStage::Finalizing);
                            capture_log.record("finalizing", "stop requested by the tray toggle");
                            refresh_tray(&tray).await;
                        } else if capture_start.is_none() {
                            set_capture_stage(
                                &status,
                                &capture_log,
                                CaptureStage::ConsentPending,
                            );
                            capture_log.record(
                                "consent pending",
                                "asking the ScreenCast portal for consent; pick the Warframe WINDOW",
                            );
                            refresh_tray(&tray).await;
                            let start_opts = opts.clone();
                            let start_status = status.clone();
                            let start_log = capture_log.clone();
                            let start_tap = reward_tap.clone();
                            let start_pack_tap = pack_tap.clone();
                            let result_tx = capture_start_tx.clone();
                            capture_start = Some(tokio::spawn(async move {
                                let started = start_capture(
                                    &start_opts,
                                    &start_status,
                                    &start_log,
                                    start_tap,
                                    start_pack_tap,
                                )
                                .await;
                                let _ = result_tx.send(started);
                            }));
                        }
                    }
                    Some(TrayCommand::Quit) | None => {
                        app_log.record("quit", "activation received");
                        break Ok(());
                    }
                }
            }
            _ = job_rx.recv(), if report_job.is_some() => {
                report_job = None;
                if let Some((trigger, open)) = queued.take() {
                    report_job = Some(spawn_report_job(
                        opts.clone(),
                        status.clone(),
                        tray.clone(),
                        trigger,
                        completions.clone(),
                        open,
                        plan_url.clone(),
                        app_log.clone(),
                        job_tx.clone(),
                    ));
                }
            }
            started = capture_start_rx.recv(), if capture_start.is_some() => {
                capture_start = None;
                match started {
                    Some(Ok(run)) => {
                        println!("resident: capture started");
                        capture = Some(run);
                    }
                    Some(Err(e)) => {
                        eprintln!(
                            "resident: capture start failed ({}): {}",
                            e.stage, e.message
                        );
                        fail_capture(&status, &capture_log, e.stage, &e.message);
                        refresh_tray(&tray).await;
                    }
                    None => {}
                }
            }
            _ = reward_done_rx.recv() => {
                // Pass 39: the grab resolved (credit or miss); the
                // next reward screen may arm a fresh episode.
                reward_arming.on_resolved();
                reward_task = None;
            }
            _ = pack_done_rx.recv() => {
                // Pass 53: the pack session ended (disarm rule);
                // the next purchase dialog may arm a fresh watch.
                pack_task = None;
            }
            _ = tick.tick() => {
                let rerun = pump_capture(&opts, &status, &tray, &mut capture, &capture_log).await;
                if rerun {
                    schedule_report!(Trigger::Manual, false);
                }
            }
        }
    };

    // Every step is bounded and logged (item 53): a wedged step must not be
    // able to hold the process (or the tray icon) hostage again. A thread
    // that refuses to stop is detached; `process::exit` in `main` reaps it.
    app_log.record("teardown", "start");
    if let Some(job) = report_job.take() {
        app_log.record("teardown", "abort report job: start");
        job.abort();
        // A cancelled future is normally dropped at its next await point, but
        // a report task wedged in a native call would never get there. Bound
        // the wait so that one step cannot become the next zombie.
        match tokio::time::timeout(TASK_ABORT_TIMEOUT, job).await {
            Ok(_) => app_log.record("teardown", "abort report job: done"),
            Err(_) => {
                eprintln!("resident: report task did not abort in time (detached)");
                app_log.record("teardown", "abort report job: timed out (detached)");
            }
        }
    }
    if let Some(job) = capture_start.take() {
        app_log.record("teardown", "abort capture start: start");
        job.abort();
        match tokio::time::timeout(TASK_ABORT_TIMEOUT, job).await {
            Ok(_) => app_log.record("teardown", "abort capture start: done"),
            Err(_) => {
                eprintln!("resident: capture start did not abort in time (detached)");
                app_log.record("teardown", "abort capture start: timed out (detached)");
            }
        }
    }
    if let Some(job) = reward_task.take() {
        app_log.record("teardown", "abort reward grab: start");
        job.abort();
        match tokio::time::timeout(TASK_ABORT_TIMEOUT, job).await {
            Ok(_) => app_log.record("teardown", "abort reward grab: done"),
            Err(_) => {
                eprintln!("resident: reward grab did not abort in time (detached)");
                app_log.record("teardown", "abort reward grab: timed out (detached)");
            }
        }
    }
    if let Some(job) = pack_task.take() {
        app_log.record("teardown", "abort pack watch: start");
        job.abort();
        match tokio::time::timeout(TASK_ABORT_TIMEOUT, job).await {
            Ok(_) => app_log.record("teardown", "abort pack watch: done"),
            Err(_) => {
                eprintln!("resident: pack watch did not abort in time (detached)");
                app_log.record("teardown", "abort pack watch: timed out (detached)");
            }
        }
    }
    app_log.record("teardown", "shutdown signal: start");
    shutdown.trigger();
    app_log.record("teardown", "shutdown signal: done");
    app_log.record("teardown", "capture stop: start");
    let capture_reaped = stop_capture(&mut capture);
    app_log.record(
        "teardown",
        &format!(
            "capture stop: done ({})",
            if capture_reaped { "reaped" } else { "detached" }
        ),
    );
    app_log.record("teardown", "watch join: start");
    let watch_stopped = join_thread_bounded(watch_thread, WATCH_JOIN_TIMEOUT);
    app_log.record(
        "teardown",
        &format!(
            "watch join: done ({})",
            if watch_stopped { "joined" } else { "detached" }
        ),
    );
    if let Some(handle) = &tray {
        app_log.record("teardown", "tray shutdown: start");
        match tokio::time::timeout(Duration::from_secs(3), handle.shutdown()).await {
            Ok(()) => app_log.record("teardown", "tray shutdown: done"),
            Err(_) => {
                eprintln!("resident: tray shutdown timed out");
                app_log.record("teardown", "tray shutdown: timed out (detached)");
            }
        }
    }
    app_log.record("teardown", "plan server drop: start");
    drop(plan_server);
    app_log.record("teardown", "plan server drop: done");
    app_log.record("teardown", "complete");
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

/// Spawn one report (plus the optional session-start auto-open) as its own
/// task (item 53). The control loop stays free to receive Quit; the task
/// reports completion on `done`.
fn spawn_report_job(
    opts: ResidentOptions,
    status: Arc<Mutex<TrayStatus>>,
    tray: Option<ksni::Handle<FrackerTray>>,
    trigger: Trigger,
    completions: Arc<Mutex<Vec<elog::MissionCompletion>>>,
    open_after: bool,
    plan_url: Option<String>,
    log: AppLog,
    done: mpsc::UnboundedSender<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        log.record("report", &format!("start ({trigger:?})"));
        do_report(&opts, &status, &tray, trigger, &completions).await;
        log.record("report", "done");
        // Ninth pass §2/item 40b + twelfth pass §2: the settled login is the
        // only auto-open trigger, at most once per session.
        if open_after && opts.pop_on_session_start {
            open_page(&opts, &status, &tray, &completions, plan_url.as_deref()).await;
        }
        let _ = done.send(());
    })
}

/// Open the plan in a task so a portal that takes its time can never block
/// the Quit path (item 53).
fn spawn_open_job(
    opts: ResidentOptions,
    status: Arc<Mutex<TrayStatus>>,
    tray: Option<ksni::Handle<FrackerTray>>,
    completions: Arc<Mutex<Vec<elog::MissionCompletion>>>,
    plan_url: Option<String>,
    log: AppLog,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        log.record("open", "start");
        open_page(&opts, &status, &tray, &completions, plan_url.as_deref()).await;
        log.record("open", "done");
    })
}

/// A frame-source decorator (pass 39): offers every frame to
/// the reward tap before the capture pipeline consumes it.
struct TapSource {
    inner: screencast::ScreenCastSource,
    tap: Arc<reward::FrameTap>,
    pack_tap: Arc<reward::FrameTap>,
}

impl capture::FrameSource for TapSource {
    fn next_frame(&mut self) -> Result<Option<crate::ocr::GrayImage>, String> {
        let frame = self.inner.next_frame()?;
        if let Some(f) = &frame {
            self.tap.offer(f);
            // Pass 53: the pack watch taps the same stream.
            self.pack_tap.offer(f);
        }
        Ok(frame)
    }
}

/// Spawn the one-shot reward burst (pass 39; burst per the
/// pass 43 amendment): grab frames every ~2 s from T+0
/// through T+14 from the reward screen's appearance,
/// detect the bordered card's part on the latest
/// gate-clearing frame, credit it, and regen. Completion
/// is always reported on `done` so the loop can re-arm.
fn spawn_reward_grab(
    opts: ResidentOptions,
    capture_live: bool,
    roll: String,
    tap: Arc<reward::FrameTap>,
    regen_tx: mpsc::UnboundedSender<Trigger>,
    log: CaptureLog,
    done: mpsc::UnboundedSender<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        reward_grab(opts, capture_live, roll, tap, regen_tx, log).await;
        let _ = done.send(());
    })
}

/// The reward burst itself. Every failure path logs a miss
/// and credits nothing (the design of record: silent MISS,
/// never a wrong credit).
async fn reward_grab(
    opts: ResidentOptions,
    capture_live: bool,
    roll: String,
    tap: Arc<reward::FrameTap>,
    regen_tx: mpsc::UnboundedSender<Trigger>,
    log: CaptureLog,
) {
    let start = tokio::time::Instant::now();
    // Pass 49: the arm line names his ROLL when the item-names
    // data resolves the grant path (diagnostic only — the roll
    // is what his relic produced, never what he picked; the
    // burst still detects the pick from the screen).
    let names = crate::fetch::load_item_names(&opts.report.data_dir);
    // Pass 51: the event log wants the roll's display name or
    // NOTHING (never the raw path), so keep the Option beside
    // the log line's fallback label.
    let roll_name = reward::roll_display_name(&roll, &names);
    let roll_label = roll_name.clone().unwrap_or(roll);
    log.record(
        "reward",
        &format!(
            "armed: reward screen seen (his roll: {roll_label}); burst grabs every ~2s from T+0 through T+14"
        ),
    );
    let frames: Vec<(u64, crate::ocr::GrayImage)> = if capture_live {
        // Reuse the live capture stream through the tap: portal
        // startup is already paid and the stream is warm.
        tap.arm();
        let mut grabbed = Vec::new();
        for &off in reward::BURST_OFFSETS_SECS.iter() {
            tokio::time::sleep_until(start + Duration::from_secs(off)).await;
            let mut f = tap.latest_fresh(Duration::from_secs(3));
            if f.is_none() && grabbed.is_empty() {
                // First sample: the tap only retains frames
                // from arming on, so give the stream a brief
                // grace to deliver its first frame — the
                // late-arm catch depends on the earliest
                // samples existing at all.
                for _ in 0..15 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    f = tap.latest_fresh(Duration::from_secs(3));
                    if f.is_some() {
                        break;
                    }
                }
            }
            if let Some(f) = f {
                grabbed.push((off, f));
            }
        }
        tap.disarm();
        if grabbed.is_empty() {
            log.record(
                "reward",
                "miss: capture stream gave no fresh frame at any burst grab",
            );
        }
        grabbed
    } else {
        reward_grab_own_stream(&opts, start, &log).await
    };
    if frames.is_empty() {
        // Pass 51: an episode that produced no frames at all
        // is still a crack with no detection for the Log tab
        // (the specific stream failure is in capture.log).
        let events_dir = event_log_dir(&opts);
        crate::events::append(
            &events_dir,
            &crate::events::EventRecord::crack_miss(
                crate::events::now_ts(),
                "no frames captured".to_owned(),
                roll_name.clone(),
            ),
        );
        return;
    }
    // Audit first: the grabbed frames are the evidence either way.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned());
    let debug_dir = crate::config::data_dir().join("capture-debug");
    for (off, frame) in &frames {
        match reward::save_audit_frame(frame, &debug_dir, &format!("{stamp}-t{off}")) {
            Ok(p) => log.record("reward", &format!("frame saved: {}", p.display())),
            Err(e) => log.record("reward", &format!("audit save failed: {e}")),
        }
    }
    let data_dir = opts.report.data_dir.clone();
    let stock_path = opts.report.stock_path.clone();
    let detected = tokio::task::spawn_blocking(move || -> Result<reward::BurstVerdict, String> {
        let data = CaptureData::load(&data_dir).map_err(|e| format!("capture data load: {e}"))?;
        let layout = data
            .layouts
            .get("relic")
            .cloned()
            .ok_or_else(|| "no relic layout".to_owned())?;
        let engine = crate::ocr::TesseractCliEngine::probe(layout)
            .ok_or_else(|| "tesseract unavailable".to_owned())?;
        Ok(reward::detect_burst(&engine, &frames, &data.universes.parts))
    })
    .await;
    // Pass 51: every terminal outcome of the burst joins the
    // event log — the credit (which also drives the toast via
    // the regen below) and the misses, each under its short
    // plain reason class.
    let events_dir = event_log_dir(&opts);
    match detected {
        Ok(Ok(reward::BurstVerdict::Pick {
            offset_secs,
            name,
        })) => match reward::credit_part(&stock_path, &name) {
            Ok(count) => {
                log.record(
                    "reward",
                    &format!(
                        "credited {name} (verified count now {count}) — burst pick from the T+{offset_secs} frame"
                    ),
                );
                crate::events::append(
                    &events_dir,
                    &crate::events::EventRecord::credit(
                        crate::events::now_ts(),
                        name.clone(),
                        parent_gear_for(&opts.report.data_dir, &name),
                    ),
                );
                let _ = regen_tx.send(Trigger::Manual);
            }
            Err(e) => log.record("reward", &format!("credit for {name} failed: {e}")),
        },
        Ok(Ok(reward::BurstVerdict::Miss(why))) => {
            log.record("reward", &format!("miss: {why}"));
            crate::events::append(
                &events_dir,
                &crate::events::EventRecord::crack_miss(
                    crate::events::now_ts(),
                    reward::miss_reason_class(&why).to_owned(),
                    roll_name.clone(),
                ),
            );
        }
        Ok(Err(why)) => {
            log.record("reward", &format!("miss: {why}"));
            crate::events::append(
                &events_dir,
                &crate::events::EventRecord::crack_miss(
                    crate::events::now_ts(),
                    "detection failed".to_owned(),
                    roll_name.clone(),
                ),
            );
        }
        Err(e) => {
            log.record("reward", &format!("miss: detection task failed: {e}"));
            crate::events::append(
                &events_dir,
                &crate::events::EventRecord::crack_miss(
                    crate::events::now_ts(),
                    "detection failed".to_owned(),
                    roll_name.clone(),
                ),
            );
        }
    }
}

/// The event log's directory: beside `stock.json` (pass 51
/// spec), falling back to the app data dir when the stock
/// path has no parent.
fn event_log_dir(opts: &ResidentOptions) -> PathBuf {
    opts.report
        .stock_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(crate::config::data_dir)
}

/// Parent item for a credited part, for the event record's
/// toast icon key (pass 51). Best-effort from the same
/// roster seed the report consumes; unresolved stays None
/// (the toast then falls back to the gear monogram — and
/// the Log line never needed it).
fn parent_gear_for(data_dir: &Path, part: &str) -> Option<String> {
    let text = std::fs::read_to_string(data_dir.join("cache/wishlist.json")).ok()?;
    let roster = crate::model::Roster::from_json(&text).ok()?;
    roster.parents_of(part).into_iter().next()
}

/// Spawn the one-shot pack watch (pass 53): grab frames
/// every 1 s while the purchase session lasts, detect each
/// reveal screen's relic trio, credit proven trios, and
/// disarm on the session-end rules. Completion is always
/// reported on `done` so the loop can arm a fresh watch.
fn spawn_pack_watch(
    opts: ResidentOptions,
    capture_live: bool,
    path: String,
    tap: Arc<reward::FrameTap>,
    regen_tx: mpsc::UnboundedSender<Trigger>,
    log: CaptureLog,
    done: mpsc::UnboundedSender<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        pack_watch(opts, capture_live, path, tap, regen_tx, log).await;
        let _ = done.send(());
    })
}

/// Credit one proven pack trio (pass 53): +1 Intact each
/// into relic stock, a `pack_open` event (whose beacon
/// plays the page's chime), a capture.log line, and a
/// regen. A failed credit logs and emits nothing — the
/// screen must not read as captured when it was not.
fn pack_credit(
    trio: &[String; 3],
    stock_path: &Path,
    events_dir: &Path,
    log: &CaptureLog,
    regen_tx: &mpsc::UnboundedSender<Trigger>,
) {
    match packopen::credit_relics(stock_path, trio) {
        Ok(counts) => {
            log.record(
                "pack",
                &format!(
                    "credited {} (Intact counts now {:?})",
                    trio.join(", "),
                    counts
                ),
            );
            crate::events::append(
                events_dir,
                &crate::events::EventRecord::pack_open(
                    crate::events::now_ts(),
                    trio.to_vec(),
                ),
            );
            let _ = regen_tx.send(Trigger::Manual);
        }
        Err(e) => log.record(
            "pack",
            &format!("credit for {} failed: {e}", trio.join(", ")),
        ),
    }
}

/// Record one ended-unproven pack screen (pass 53): a
/// capture.log line and a `pack_miss` event naming the
/// unresolved slots. A proven screen owes nothing.
fn pack_miss(ended: &packopen::EndedScreen, events_dir: &Path, log: &CaptureLog) {
    if ended.proven {
        return;
    }
    let reason = ended.unresolved_reason();
    log.record(
        "pack",
        &format!("miss: pack screen left unproven ({reason})"),
    );
    crate::events::append(
        events_dir,
        &crate::events::EventRecord::pack_miss(crate::events::now_ts(), reason),
    );
}

/// The pack watch itself (pass 53). Arms on the purchase
/// dialog, grabs at the 1 s cadence from the live capture
/// stream when one is running (via the tap) or from its
/// own negotiated stream otherwise, and runs the
/// [`packopen::PackWatch`] machine until a disarm rule
/// fires. Every pack screen's first frame is saved under
/// capture-debug/packs/ (and the crediting frame too when
/// it is a later grab), so a miss is debuggable after the
/// fact.
async fn pack_watch(
    opts: ResidentOptions,
    capture_live: bool,
    path: String,
    tap: Arc<reward::FrameTap>,
    regen_tx: mpsc::UnboundedSender<Trigger>,
    log: CaptureLog,
) {
    log.record(
        "pack",
        &format!(
            "armed: pack purchase dialog ({path}); grabs every 1s; disarm at 15s no-screen or 60s total"
        ),
    );
    // Detection machinery, loaded once for the session: the
    // relic layout drives the engine probe exactly as the
    // reward burst's does, and the caption universe is the
    // capture pipeline's scoped relic set.
    let data = match CaptureData::load(&opts.report.data_dir) {
        Ok(d) => d,
        Err(e) => {
            log.record("pack", &format!("disarmed: capture data load failed: {e}"));
            return;
        }
    };
    let Some(layout) = data.layouts.get("relic").cloned() else {
        log.record("pack", "disarmed: no relic layout");
        return;
    };
    let Some(engine) = crate::ocr::TesseractCliEngine::probe(layout) else {
        log.record("pack", "disarmed: tesseract unavailable");
        return;
    };
    let engine = Arc::new(engine);
    let universe = Arc::new(data.universes.relics.clone());
    let stock_path = opts.report.stock_path.clone();
    let events_dir = event_log_dir(&opts);
    let debug_dir = crate::config::data_dir().join("capture-debug");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned());

    // Frame supply. The live-capture path reuses the warm
    // stream through the tap; the standalone path negotiates
    // its own stream and drains it into a shared latest.
    let own_stop = Arc::new(AtomicBool::new(false));
    let own_latest = Arc::new(Mutex::new(None::<crate::ocr::GrayImage>));
    let mut own_drain: Option<std::thread::JoinHandle<Option<String>>> = None;
    if capture_live {
        tap.arm();
    } else {
        if let Err(e) = screencast::preflight_pipewire() {
            log.record("pack", &format!("disarmed: pipewire preflight: {e}"));
            return;
        }
        let token = screencast::load_restore_token(&opts.state_dir);
        let negotiated = match screencast::negotiate(token, &opts.state_dir, &log).await {
            Ok(n) => n,
            Err(e) => {
                log.record(
                    "pack",
                    &format!("disarmed: stream negotiate ({}): {}", e.stage, e.message),
                );
                return;
            }
        };
        let drain_stop = own_stop.clone();
        let drain_log = log.clone();
        let drain_latest = own_latest.clone();
        own_drain = Some(std::thread::spawn(move || {
            let mut source =
                match screencast::ScreenCastSource::new(negotiated, drain_stop, drain_log) {
                    Ok(s) => s,
                    Err(e) => return Some(e),
                };
            loop {
                match source.next_frame() {
                    Ok(Some(f)) => {
                        if let Ok(mut g) = drain_latest.lock() {
                            *g = Some(f);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => return Some(e),
                }
            }
            None
        }));
    }

    let mut watch = packopen::PackWatch::new();
    watch.arm(std::time::Instant::now());
    let mut instance: u32 = 0;
    let mut in_instance = false;
    loop {
        tokio::time::sleep(packopen::GRAB_INTERVAL).await;
        let frame = if capture_live {
            tap.latest_fresh(Duration::from_secs(3))
        } else {
            own_latest.lock().ok().and_then(|g| g.clone())
        };
        if let Some(frame) = frame {
            let now = std::time::Instant::now();
            if !packopen::is_pack_screen(&frame) {
                if let Some(ended) = watch.note_frame(now, false) {
                    pack_miss(&ended, &events_dir, &log);
                }
                in_instance = false;
            } else {
                if !in_instance {
                    instance += 1;
                    in_instance = true;
                    log.record(
                        "pack",
                        &format!("pack screen seen (instance {instance})"),
                    );
                    match packopen::save_pack_frame(
                        &frame,
                        &debug_dir,
                        &format!("{stamp}-{instance}"),
                    ) {
                        Ok(p) => log.record(
                            "pack",
                            &format!("frame saved: {}", p.display()),
                        ),
                        Err(e) => {
                            log.record("pack", &format!("audit save failed: {e}"))
                        }
                    }
                }
                let _ = watch.note_frame(now, true);
                let detect_engine = engine.clone();
                let detect_universe = universe.clone();
                let detect_frame = frame.clone();
                let read = tokio::task::spawn_blocking(move || {
                    packopen::detect_pack(&*detect_engine, &detect_frame, &detect_universe)
                })
                .await
                .ok()
                .flatten();
                let resolved = read
                    .as_ref()
                    .map(|r| r.resolved.clone())
                    .unwrap_or([None, None, None]);
                match watch.note_resolution(resolved) {
                    packopen::ResolutionStep::Pending => {}
                    packopen::ResolutionStep::ReadyToCredit(trio) => {
                        pack_credit(&trio, &stock_path, &events_dir, &log, &regen_tx);
                        watch.mark_proven();
                    }
                    packopen::ResolutionStep::Replaced { ended, ready } => {
                        pack_miss(&ended, &events_dir, &log);
                        if let Some(trio) = ready {
                            pack_credit(&trio, &stock_path, &events_dir, &log, &regen_tx);
                            watch.mark_proven();
                        }
                    }
                }
            }
        }
        if let Some(reason) = watch.disarm_due(std::time::Instant::now()) {
            if let Some(ended) = watch.finish_instance() {
                pack_miss(&ended, &events_dir, &log);
            }
            log.record("pack", &format!("disarmed: {}", reason.label()));
            break;
        }
    }

    if capture_live {
        tap.disarm();
    }
    own_stop.store(true, Ordering::Relaxed);
    if let Some(drain) = own_drain {
        match tokio::task::spawn_blocking(move || drain.join()).await {
            Ok(Ok(Some(e))) => log.record("pack", &format!("stream note: {e}")),
            Ok(Ok(None)) => {}
            _ => log.record("pack", "stream note: stream drain did not join"),
        }
    }
}

/// The standalone-stream path of the reward burst: negotiate
/// a portal stream at T+0 (so consent/startup latency is paid
/// immediately, not at grab time), drain frames into a shared
/// "latest", and snapshot it at each burst offset.
async fn reward_grab_own_stream(
    opts: &ResidentOptions,
    start: tokio::time::Instant,
    log: &CaptureLog,
) -> Vec<(u64, crate::ocr::GrayImage)> {
    if let Err(e) = screencast::preflight_pipewire() {
        log.record("reward", &format!("miss: pipewire preflight: {e}"));
        return Vec::new();
    }
    let token = screencast::load_restore_token(&opts.state_dir);
    let negotiated = match screencast::negotiate(token, &opts.state_dir, log).await {
        Ok(n) => n,
        Err(e) => {
            log.record(
                "reward",
                &format!("miss: stream negotiate ({}): {}", e.stage, e.message),
            );
            return Vec::new();
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let latest = Arc::new(Mutex::new(None::<crate::ocr::GrayImage>));
    let drain_stop = stop.clone();
    let drain_log = log.clone();
    let drain_latest = latest.clone();
    let drain = std::thread::spawn(move || {
        let mut source =
            match screencast::ScreenCastSource::new(negotiated, drain_stop, drain_log) {
                Ok(s) => s,
                Err(e) => return Some(e),
            };
        loop {
            match source.next_frame() {
                Ok(Some(f)) => {
                    if let Ok(mut g) = drain_latest.lock() {
                        *g = Some(f);
                    }
                }
                Ok(None) => break,
                Err(e) => return Some(e),
            }
        }
        None
    });
    let mut grabbed = Vec::new();
    for &off in reward::BURST_OFFSETS_SECS.iter() {
        tokio::time::sleep_until(start + Duration::from_secs(off)).await;
        let mut f = latest.lock().ok().and_then(|g| g.clone());
        if f.is_none() && grabbed.is_empty() {
            // First sample: give the drain a brief grace to
            // deliver its first frame (see the tap path).
            for _ in 0..15 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                f = latest.lock().ok().and_then(|g| g.clone());
                if f.is_some() {
                    break;
                }
            }
        }
        if let Some(f) = f {
            grabbed.push((off, f));
        }
    }
    stop.store(true, Ordering::Relaxed);
    match tokio::task::spawn_blocking(move || drain.join()).await {
        Ok(Ok(err)) => {
            if let Some(e) = err {
                log.record("reward", &format!("stream note: {e}"));
            }
        }
        _ => log.record("reward", "stream note: stream drain did not join"),
    }
    if grabbed.is_empty() {
        log.record("reward", "miss: own stream delivered no frame at any burst grab");
    }
    grabbed
}

/// Open the ScreenCast portal session and spawn the capture worker thread.
async fn start_capture(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    log: &CaptureLog,
    tap: Arc<reward::FrameTap>,
    pack_tap: Arc<reward::FrameTap>,
) -> Result<CaptureRun, screencast::NegotiateError> {
    // Item 60b: resolve every capture symbol before the portal flow
    // begins. One error names every missing name, so a live run can never
    // discover them one consent dialog at a time.
    if let Err(e) = screencast::preflight_pipewire() {
        log.record("pipewire", &e);
        return Err(screencast::NegotiateError::pipewire(e));
    }
    let token = screencast::load_restore_token(&opts.state_dir);
    match token.as_ref() {
        Some(t) => log.record(
            "consent pending",
            &format!(
                "using saved restore token {} with persist mode",
                screencast::token_hint(t)
            ),
        ),
        None => log.record(
            "consent pending",
            "no restore token yet (first run) — full consent required",
        ),
    }
    let negotiated = screencast::negotiate(token, &opts.state_dir, log).await?;
    let token_state = match negotiated.restore_token.as_ref() {
        Some(t) => format!("issued {}", screencast::token_hint(t)),
        None => "unchanged".to_owned(),
    };
    log.record(
        "consent pending",
        &format!(
            "consent granted; selected ScreenCast node {}; remote fd ready (restore_token {token_state})",
            negotiated.node_id,
        ),
    );
    // Consent is done; the worker owns connecting → capturing.
    set_capture_stage(status, log, CaptureStage::Connecting);
    let (tx, events) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let data_dir = opts.report.data_dir.clone();
    let worker_log = log.clone();
    let join = std::thread::spawn(move || {
        capture_worker(
            data_dir,
            negotiated,
            stop_thread,
            tx,
            worker_log,
            tap,
            pack_tap,
        )
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
    tap: Arc<reward::FrameTap>,
    pack_tap: Arc<reward::FrameTap>,
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
    let raw_source = match screencast::ScreenCastSource::new(negotiated, stop.clone(), log.clone())
    {
        Ok(s) => s,
        Err(e) => {
            return send_failed(&tx, "connecting", format!("capture: {e}"));
        }
    };
    // Pass 39: offer every frame to the reward tap on its way
    // into the pipeline (the tap retains frames only while a
    // reward episode is armed).
    let mut source = TapSource {
        inner: raw_source,
        tap,
        pack_tap,
    };
    // Item 49: "capturing" begins at the first real frame, not at connect.
    // Connecting a targetless stream used to sit idle forever, so labelling
    // the lifecycle "capturing" at connect lied about a stream that had not
    // delivered anything yet; hold the stage at "connecting" until the first
    // frame makes it through, then announce it exactly once.
    log.record("connecting", "stream connected; awaiting first frame");
    let ctx = data.context();
    let idle = data.classifier.idle_stop_frames;
    let progress_tx = tx.clone();
    let frame_log = log.clone();
    let mut announced = false;
    let mut queue_drops_seen = 0u64;
    let result = capture::run_capture(
        &ctx,
        &engine,
        &mut source,
        &stop,
        idle,
        move |s| {
            if !announced {
                announced = true;
                let _ = progress_tx.send(CaptureEvent::Stage(CaptureStage::Capturing));
                frame_log.record("capturing", "draining frames (scroll your inventory)");
            }
            // Pass 37: bundle-queue drops are logged when they
            // happen (this callback runs after every frame), not
            // only in the finalize failsafe summary — a drop is
            // newest-pages content loss and the moment matters.
            let drops = s.bundle_queue_dropped_frames();
            if drops > queue_drops_seen {
                frame_log.record(
                    "capturing",
                    &format!(
                        "bundle queue dropped {} frame(s) at its cap ({} total this session)",
                        drops - queue_drops_seen,
                        drops
                    ),
                );
                queue_drops_seen = drops;
            }
            let _ = progress_tx.send(CaptureEvent::Progress(s.counts()));
        },
    );
    match result {
        Ok(session) => {
            let _ = tx.send(CaptureEvent::Finished(Box::new(session)));
        }
        Err(e) => {
            // Item 54: a failure before the first frame is a connect
            // failure, not "capturing". The stage comes from what the
            // pipeline actually achieved.
            let stage = screencast::failure_stage(announced);
            log.record(stage, &format!("capture: {e}"));
            send_failed(&tx, stage, format!("capture: {e}"));
        }
    }
}

/// Send a staged failure to the resident loop.
fn send_failed(tx: &std::sync::mpsc::Sender<CaptureEvent>, stage: &'static str, message: String) {
    let _ = tx.send(CaptureEvent::Failed { stage, message });
}

/// Drain the capture worker's events: apply lifecycle stages, refresh live
/// counts, and on completion finalize the stock. Returns true when a plan
/// regeneration is due (the caller schedules it as a task, item 53).
async fn pump_capture(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    tray: &Option<ksni::Handle<FrackerTray>>,
    capture: &mut Option<CaptureRun>,
    log: &CaptureLog,
) -> bool {
    let Some(run) = capture.as_ref() else { return false };
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
                // The worker vanished without a terminal event. Label it by
                // what the lifecycle actually reached (item 54): a worker
                // that died before the first frame was still connecting.
                let stage = if status
                    .lock()
                    .map(|s| matches!(s.capture.stage(), CaptureStage::Capturing))
                    .unwrap_or(false)
                {
                    "capturing"
                } else {
                    "connecting"
                };
                finished = Some(CaptureEvent::Failed {
                    stage,
                    message: "capture worker stopped unexpectedly".to_owned(),
                });
                break;
            }
        }
    }

    // Item 49: a terminal failure supersedes any in-flight stage labels. A
    // stage queued before the failure (e.g. the first-frame "capturing")
    // would otherwise be applied after the error and mask it as progress.
    let terminal_failure = matches!(finished, Some(CaptureEvent::Failed { .. }));
    let mut changed = false;
    if !terminal_failure {
        for stage in stages {
            if let Ok(mut s) = status.lock() {
                s.capture.set_stage(stage);
                log.record("stage", &s.capture.status_line());
                changed = true;
            }
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
            let rerun = finalize_capture(opts, status, tray, *session, log).await;
            stop_capture(capture);
            rerun
        }
        Some(CaptureEvent::Failed { stage, message }) => {
            eprintln!("resident: capture failed ({stage}): {message}");
            fail_capture(status, log, stage, &message);
            stop_capture(capture);
            refresh_tray(tray).await;
            false
        }
        _ => false,
    }
}

/// Finalize a completed capture: captured buckets REPLACE the matching
/// buckets in `stock.json` (untouched buckets survive). Returns true when
/// the plan should be regenerated from the fresh stock.
async fn finalize_capture(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    tray: &Option<ksni::Handle<FrackerTray>>,
    mut session: CaptureSession,
    log: &CaptureLog,
) -> bool {
    session.flush_debug_dumps();
    set_capture_stage(status, log, CaptureStage::Finalizing);
    log.record(
        "finalizing",
        &format!(
            "capture evidence: {} frames seen, {} read, {} skipped",
            session.frames_seen(),
            session.frames_read(),
            session.frames_skipped()
        ),
    );
    // Pass 37: the intake's own accounting and the stitch path
    // ride the finalize log (LIVE_DIAG2 §3/§4: the counters
    // existed but had no caller, and no record named which
    // stitch path a session took — the two gaps that made the
    // v0.5.30 run's evidence undecidable from the log alone).
    log.record(
        "finalizing",
        &format!(
            "intake: {} accepted, {} duplicates dropped, {} off-lattice dropped, {} interval dropped, {} latched dropped, {} session-cap refused",
            session.parts_frames_accepted(),
            session.parts_dupes_dropped(),
            session.parts_off_lattice_dropped(),
            session.parts_interval_dropped(),
            session.parts_latched_dropped(),
            session.parts_frames_capped()
        ),
    );
    log.record(
        "finalizing",
        &format!("stitch path: {}", session.stitch_path_label()),
    );
    // Header samples and the debug-frame list are evidence on
    // EVERY finalize (addendum #83): a partial capture is exactly
    // when the classification record matters, and gating them on
    // zero-collection hid the 2026-10-24 misclassification run.
    for (class, text) in &session.header_samples {
        log.record("finalizing", &format!("header OCR [{}]: {text}", class.label()));
    }
    if !session.debug_files.is_empty() {
        log.record(
            "finalizing",
            &format!("debug frames written: {}", session.debug_files.join(", ")),
        );
    }
    // Pass 37: a debug dump that stopped at its per-name cap
    // says so — the capped folder is evidence about the DUMP,
    // not about where the capture ended (LIVE_DIAG2 §1).
    for name in session.debug_capped_names() {
        log.record(
            "finalizing",
            &format!("debug dump cap reached for {name}; further frames not dumped"),
        );
    }
    if !session.has_any() {
        log.record("finalizing", "nothing collected — stock untouched");
        set_capture_stage(status, log, CaptureStage::Idle);
        refresh_tray(tray).await;
        return false;
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
                return false;
            }
        },
        Err(_) => OcrStock::default(),
    };
    session.finalize_into(&mut existing);
    if let Err(e) = existing.write_to(path) {
        eprintln!("resident: capture finalize: {e}");
        fail_capture(status, log, "finalizing", &format!("capture finalize: {e}"));
        refresh_tray(tray).await;
        return false;
    }

    let summary = capture::captured_summary(&session);
    println!("resident: {summary} -> {}", path.display());
    log.record("finalizing", &format!("{summary} -> {}", path.display()));
    // Pass 33: the stitch coverage accounting rides the finalize
    // log — what the parts capture covered, not just what it held.
    if let Some(cov) = session.coverage_summary() {
        log.record("finalizing", &cov);
    }
    // Pass 36: any failsafe that tripped (frame cap, bundle
    // queue cap, duration cap) is stated in the finalize output
    // — caps trip loud, never silent.
    if let Some(fs) = session.failsafe_summary() {
        log.record("finalizing", &fs);
    }
    // Pass 38: relic owned-gate exclusions ride the finalize log —
    // a browse sync states which catalogue tiles it refused, so a
    // smaller relic bucket reads as a decision, not a loss.
    if let Some(gs) = session.relic_gate_summary() {
        log.record("finalizing", &gs);
    }
    // Pass 43: refinement-rule exclusions ride the finalize log
    // on the same principle — a refinement sync states which
    // label-less tiles it refused.
    if let Some(vs) = session.relic_variant_summary() {
        log.record("finalizing", &vs);
    }
    // Pass 46: name-admission refusals ride the finalize log
    // on the same principle — a sync states which resolved
    // names it refused to import as unproven, with the raw
    // read and the margin that failed to prove them.
    if let Some(ns) = session.relic_name_summary() {
        log.record("finalizing", &ns);
    }
    set_capture_stage(status, log, CaptureStage::Idle);
    true
}

/// Stop the worker (if any) and reap its thread, bounded (item 53). A
/// worker stuck in a native call is detached rather than blocking Quit
/// forever; `process::exit` reaps it.
fn stop_capture(capture: &mut Option<CaptureRun>) -> bool {
    if let Some(mut run) = capture.take() {
        run.stop.store(true, Ordering::Relaxed);
        if let Some(h) = run.join.take() {
            return join_thread_bounded(h, CAPTURE_JOIN_TIMEOUT);
        }
    }
    true
}

/// Wait up to `timeout` for a thread, returning true when it was joined.
/// On timeout the handle is dropped (detached): the process is about to
/// exit and no teardown step may hang on it.
fn join_thread_bounded(handle: std::thread::JoinHandle<()>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if handle.is_finished() {
        let _ = handle.join();
        true
    } else {
        false
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
    plan_url: Option<&str>,
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
    let opened = match plan_url {
        Some(url) => portal::open_url(url).await.map(|()| url.to_owned()),
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

/// Nudge the tray to re-read its properties after a status change. Bounded
/// (item 53): a wedged D-Bus service must never stall Quit behind a report
/// or a menu refresh.
async fn refresh_tray(tray: &Option<ksni::Handle<FrackerTray>>) {
    if let Some(handle) = tray {
        let _ = tokio::time::timeout(TRAY_REFRESH_TIMEOUT, handle.update(|_| ())).await;
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
            // Pass 49: classification carries Seth's account id so
            // the reward-grant event can key on it (the id's single
            // source is report::PROFILE_ACCOUNT_ID).
            let ev = elog::classify_with_account(&line, report::PROFILE_ACCOUNT_ID);
            let _ = tracker.feed(ev.as_ref(), gap);
            if let Some(elog::Event::LoggedIn { name }) = &ev {
                st.display_name = name.clone();
                saw_login = true;
            }
            if ev.as_ref().map_or(false, |e| e.is_mission_end()) {
                let _ = trigger_tx.send(Trigger::MissionEnd);
            }
            // Pass 39/49: his reward grant arms the burst grab; the
            // screen's shutdown (RelicCrack) closes the episode.
            // Neither is a report trigger — the resident loop
            // partitions them out of report bursts. (The grant also
            // fires the MissionEnd regen above via is_mission_end.)
            if let Some(elog::Event::RelicReward { roll }) = &ev {
                let _ = trigger_tx.send(Trigger::RewardScreen(roll.clone()));
            }
            if matches!(ev, Some(elog::Event::RelicCrack)) {
                let _ = trigger_tx.send(Trigger::RewardClosed);
            }
            // Pass 53: a pack purchase dialog arms the pack
            // watch. Not a mission end — no regen rides it;
            // the pack task regens itself on a proven credit.
            if let Some(elog::Event::PackPurchase { path }) = &ev {
                let _ = trigger_tx.send(Trigger::PackScreen(path.clone()));
            }
            // §4: prove a mission from its armed identity + success marker.
            if let Some(done) = missions.feed(raw, &today, &now_utc()) {
                // Pass 51: a proven ALERT completion joins the
                // event log (the Log tab's second kind). The
                // label is the identity known at arm time: the
                // event tag when the block carries one (with
                // the node appended), else the node itself,
                // else the internal mission name.
                if done.info.is_alert() {
                    let info = &done.info;
                    let label = if !info.alert_tag.is_empty() {
                        if info.location.is_empty() {
                            info.alert_tag.clone()
                        } else {
                            format!("{} · {}", info.alert_tag, info.location)
                        }
                    } else if !info.location.is_empty() {
                        info.location.clone()
                    } else {
                        info.name.clone()
                    };
                    crate::events::append(
                        &state_dir,
                        &crate::events::EventRecord::alert_done(
                            crate::events::now_ts(),
                            label,
                        ),
                    );
                }
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
        // Pass 49: the mission-end marker for a crack is now the
        // gets-reward grant line (transcribed shape, EE-2.log).
        writeln!(f, "150.530 Sys [Info]: VoidProjections: 5420fba23846322173707618 gets reward /Lotus/StoreItems/Types/Recipes/WarframeRecipes/HarrowPrimeHelmetBlueprint").unwrap();
        f.flush().unwrap();

        assert_eq!(recv_trigger(&mut rx), Some(Trigger::MissionEnd));

        shutdown.trigger();
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pass 49 (Seth's direction): the burst arms ONLY on his
    /// gets-reward line. The EE-2 true-screen sequence arms
    /// exactly once (with the roll attached) and regenerates;
    /// the survival fake-init sequence — `Relic rewards
    /// initialized` re-firing as the relic-selection flow
    /// closes, no grant behind it — arms nothing; a squadmate's
    /// grant line arms nothing.
    #[test]
    fn reward_trigger_keys_on_his_gets_reward_line_only() {
        let dir = tmpdir("rekey");
        let log = dir.join("EE.log");
        let state = dir.join("state");
        std::fs::write(&log, "14.148 Sys [Info]: Logged in Jenterrobahne\n").unwrap();
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
        let _ = collect_triggers(&mut rx, Duration::from_millis(250));

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        // The fake init (Seth's survival snippet, game t=3075.684):
        // selection votes close, the reward-choice UI re-inits,
        // no screen follows.
        writeln!(f, "3075.671 Script [Info]: ThemedProjectionManager.lua: We got all the votes so we can close now!").unwrap();
        writeln!(f, "3075.684 Script [Info]: ProjectionRewardChoice.lua: Relic rewards initialized").unwrap();
        // A squadmate's grant line: another id entirely.
        writeln!(f, "3076.000 Sys [Info]: VoidProjections: 65a6fbd660283fc6b30c4ec3 gets reward /Lotus/StoreItems/Types/Recipes/Weapons/BurstonPrimeBlueprint").unwrap();
        f.flush().unwrap();
        let phantom = collect_triggers(&mut rx, Duration::from_millis(600));
        assert!(
            !phantom.iter().any(|t| matches!(t, Trigger::RewardScreen(_))),
            "no arm without his grant line: {phantom:?}"
        );
        assert!(
            !phantom.contains(&Trigger::MissionEnd),
            "the fake init must not regenerate either: {phantom:?}"
        );

        // The true screen (EE-2 sequence): initialized, HIS grant,
        // Got rewards — exactly one arm, roll attached, one regen.
        writeln!(f, "150.472 Script [Info]: ProjectionRewardChoice.lua: Relic rewards initialized").unwrap();
        writeln!(f, "150.530 Sys [Info]: VoidProjections: 5420fba23846322173707618 gets reward /Lotus/StoreItems/Types/Recipes/WarframeRecipes/HarrowPrimeHelmetBlueprint").unwrap();
        writeln!(f, "150.843 Script [Info]: ProjectionRewardChoice.lua: Got rewards").unwrap();
        f.flush().unwrap();
        let true_screen = collect_triggers(&mut rx, Duration::from_millis(600));
        let arms: Vec<&Trigger> = true_screen
            .iter()
            .filter(|t| matches!(t, Trigger::RewardScreen(_)))
            .collect();
        assert_eq!(arms.len(), 1, "exactly one arm per true screen: {true_screen:?}");
        assert_eq!(
            arms[0],
            &Trigger::RewardScreen(
                "/Lotus/StoreItems/Types/Recipes/WarframeRecipes/HarrowPrimeHelmetBlueprint"
                    .to_owned()
            )
        );
        assert!(
            true_screen.contains(&Trigger::MissionEnd),
            "the grant still regenerates the plan: {true_screen:?}"
        );

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
