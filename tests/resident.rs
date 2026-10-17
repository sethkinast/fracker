//! Resident integration test (seventh pass gate).
//!
//! Proves the final-product shape (addendum #37) end to end, hermetically
//! and offline: a headless resident starts → a NEW EE.log file appears in
//! a watched temp dir → a real report is generated with NO manual
//! trigger. The tray is deliberately absent here, which also exercises
//! the §4 "tray absence never changes watch/report behavior" guarantee.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fracker::report::ReportConfig;
use fracker::resident::{run_headless, ResidentOptions, Shutdown};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scrape_dir(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "fracker-resident-it-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    for sub in ["log", "state", "out", "cache"] {
        std::fs::create_dir_all(base.join(sub)).unwrap();
    }
    base
}

fn offline_report_config(base: &std::path::Path) -> ReportConfig {
    ReportConfig {
        data_dir: root().join("data"),
        fixtures: root().join("fixtures"),
        cache_dir: base.join("cache"),
        out: base.join("out/plan.html"),
        stock_path: base.join("out/stock.json"),
        config_path: base.join("config.toml"),
        offline: true,
        embed_icons: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_resident_reports_on_new_ee_log_without_manual_trigger() {
    let base = scrape_dir("newlog");
    let log_path = base.join("log/EE.log");
    let out = base.join("out/plan.html");

    let opts = ResidentOptions {
        report: offline_report_config(&base),
        log_path: log_path.clone(),
        state_dir: base.join("state"),
        watch_interval_ms: 25,
    };
    let shutdown = Shutdown::new();
    let sd = shutdown.clone();
    let task = tokio::spawn(async move { run_headless(opts, sd).await });

    // Let the resident come up and start watching an as-yet-absent log.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!out.exists(), "no report before any EE.log exists");

    // A NEW EE.log appears with a fresh session login — no manual step.
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&log_path).unwrap();
        writeln!(f, "14.148 Sys [Info]: Logged in Jenterrobahne").unwrap();
        f.flush().unwrap();
    }

    // The watch must produce the report entirely on its own.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !out.exists() {
        if Instant::now() > deadline {
            shutdown.trigger();
            let _ = task.await;
            panic!("report was not generated from the new EE.log without a manual trigger");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let html = std::fs::read_to_string(&out).unwrap();
    assert!(html.contains("<html"), "a real HTML page was written");
    assert!(
        html.contains("cdn.warframestat.us"),
        "product/resident page keeps CDN hotlinks"
    );

    // Clean cooperative shutdown (the Quit verb path).
    shutdown.trigger();
    task.await.expect("resident task did not panic").expect("clean exit");
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_starts_and_stops_cleanly_with_no_log_at_all() {
    // Tray absence + no EE.log yet must be a quiet, stable state.
    let base = scrape_dir("nolog");
    let opts = ResidentOptions {
        report: offline_report_config(&base),
        log_path: base.join("log/EE.log"),
        state_dir: base.join("state"),
        watch_interval_ms: 25,
    };
    let shutdown = Shutdown::new();
    let sd = shutdown.clone();
    let task = tokio::spawn(async move { run_headless(opts, sd).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!base.join("out/plan.html").exists());
    shutdown.trigger();
    task.await.expect("no panic").expect("clean exit");
    let _ = std::fs::remove_dir_all(&base);
}
