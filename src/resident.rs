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

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::{mpsc, watch};

use crate::report::{self, ReportConfig};
use crate::state::AppState;
use crate::tray::{FrackerTray, TrayCommand, TrayStatus};
use crate::watch::LogWatcher;
use crate::{elog, portal};

/// What asked for a report run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A new EE.log appeared / rotated, or a fresh session started
    /// (addendum #37).
    SessionStart,
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
    let watch_thread = spawn_watch(
        opts.log_path.clone(),
        opts.watch_interval_ms,
        opts.state_dir.clone(),
        status.clone(),
        trigger_tx,
        shutdown.receiver(),
    );

    let mut sd = shutdown.receiver();
    let result = loop {
        tokio::select! {
            _ = sd.changed() => {
                if *sd.borrow() {
                    break Ok(());
                }
            }
            Some(trigger) = trigger_rx.recv() => {
                // Coalesce a burst (e.g. session start + mission end in
                // one read) into a single report run.
                while trigger_rx.try_recv().is_ok() {}
                do_report(&opts, &status, &tray, trigger).await;
            }
            cmd = recv_cmd(&mut cmds) => {
                match cmd {
                    Some(TrayCommand::Regenerate) => {
                        do_report(&opts, &status, &tray, Trigger::Manual).await;
                    }
                    Some(TrayCommand::OpenPage) => {
                        open_page(&opts, &status, &tray).await;
                    }
                    Some(TrayCommand::Quit) | None => break Ok(()),
                }
            }
        }
    };

    // Stop the watch thread and wait for it (it checks the flag each tick).
    shutdown.trigger();
    let _ = watch_thread.join();
    result
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
) {
    println!("resident: regenerating page ({trigger:?})");
    match report::generate(&opts.report).await {
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

/// "Open plan page": generate when the report does not exist yet, then
/// open it via the OpenURI portal (with an `xdg-open` fallback).
async fn open_page(
    opts: &ResidentOptions,
    status: &Arc<Mutex<TrayStatus>>,
    tray: &Option<ksni::Handle<FrackerTray>>,
) {
    let out = &opts.report.out;
    if !out.exists() {
        println!("resident: no report yet; generating before open");
        if let Err(e) = report::generate(&opts.report).await {
            eprintln!("resident: report failed: {e}");
            if let Ok(mut s) = status.lock() {
                s.last_error = Some(e);
            }
            refresh_tray(tray).await;
            return;
        }
    }
    match portal::open_uri(&portal::file_uri(out)).await {
        Ok(()) => println!("resident: opened {}", out.display()),
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
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(e) = watch_loop(
            log_path,
            interval_ms,
            state_dir,
            status.clone(),
            trigger_tx,
            shutdown,
        ) {
            eprintln!("resident: watch stopped: {e}");
            if let Ok(mut s) = status.lock() {
                s.last_error = Some(e);
            }
        }
    })
}

/// The watch loop: reconcile, tail, classify, trigger, persist.
fn watch_loop(
    log_path: PathBuf,
    interval_ms: i32,
    state_dir: PathBuf,
    status: Arc<Mutex<TrayStatus>>,
    trigger_tx: mpsc::UnboundedSender<Trigger>,
    shutdown: watch::Receiver<bool>,
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

    loop {
        if *shutdown.borrow() {
            break;
        }
        let lines = watcher.next_lines(interval_ms)?;
        let pos = watcher.position();
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

        let mut session_started = new_file;
        for raw in &lines {
            let line = elog::parse_line(raw);
            let ev = elog::classify(&line);
            if tracker.feed(ev.as_ref(), gap) {
                session_started = true;
            }
            if let Some(elog::Event::LoggedIn { name }) = &ev {
                st.display_name = name.clone();
            }
            if ev.as_ref().map_or(false, |e| e.is_mission_end()) {
                let _ = trigger_tx.send(Trigger::MissionEnd);
            }
        }
        // One session start per read, whether from the file edge or a
        // `Logged in` line (they often arrive together).
        if session_started {
            let _ = trigger_tx.send(Trigger::SessionStart);
        }

        let dirty = !lines.is_empty() || pos != st.log_position;
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
        for _ in 0..200 {
            if let Ok(t) = rx.try_recv() {
                return Some(t);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
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
    fn new_log_file_appearance_triggers_a_session_start() {
        // The watch thread must fire without a manual trigger when the
        // EE.log appears — this is the §4/#37 guarantee at unit scale.
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
        );

        // No file yet: nothing fires.
        assert_eq!(recv_trigger(&mut rx), None, "no trigger before the file exists");

        // A new EE.log lands with a login line.
        let mut f = std::fs::File::create(&log).unwrap();
        use std::io::Write;
        writeln!(f, "14.148 Sys [Info]: Logged in Jenterrobahne").unwrap();
        f.flush().unwrap();

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
        // Pre-existing file, persisted position 0 == fresh: the first
        // poll opens it, which fires a session start; we only care that
        // a mission-end line also triggers.
        std::fs::write(
            &log,
            "14.148 Sys [Info]: Logged in Jenterrobahne\n",
        )
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let shutdown = Shutdown::new();
        let status = Arc::new(Mutex::new(TrayStatus::default()));
        let handle = spawn_watch(log.clone(), 20, state.clone(), status, tx, shutdown.receiver());
        assert_eq!(recv_trigger(&mut rx), Some(Trigger::SessionStart));
        // Drain any stragglers.
        while rx.try_recv().is_ok() {}

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, "99.001 Script [Info]: ProjectionRewardChoice.lua: Got rewards").unwrap();
        f.flush().unwrap();

        assert_eq!(recv_trigger(&mut rx), Some(Trigger::MissionEnd));

        shutdown.trigger();
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
