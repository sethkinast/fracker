//! Resident integration test (seventh pass gate).
//!
//! Proves the final-product shape (addendum #37) end to end, hermetically
//! and offline: a headless resident starts → a NEW EE.log file appears in
//! a watched temp dir → a real report is generated with NO manual
//! trigger. The tray is deliberately absent here, which also exercises
//! the §4 "tray absence never changes watch/report behavior" guarantee.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fracker::report::ReportConfig;
use fracker::resident::{run_headless, ResidentOptions, Shutdown};
use fracker::tray::TrayStatus;

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
        serve_port: 0,
        pop_on_session_start: false,
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
        serve_port: 0,
        pop_on_session_start: false,
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

/// A worldstate whose alert ids match the fixture log's `alertId`s, plus
/// one alert the log never mentions (proving the strike is only ever a
/// positive log match). Expiries sit after the worldstate timestamp so
/// every alert renders.
const SYNTH_WORLDSTATE: &str = r#"{
  "timestamp": "2026-10-17T00:00:00.000Z",
  "alerts": [
    {
      "id": "6ad0d7360dfe89c5d82290a2",
      "expiry": "2026-10-19T00:00:00.000Z",
      "tag": "RoadToTauEvent",
      "mission": {
        "node": "Nimus (Eris)", "type": "Hive", "faction": "Infested",
        "reward": { "countedItems": [
          { "uniqueName": "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource", "type": "Road To Tau Event Resource", "count": 50 }
        ] }
      }
    },
    {
      "id": "6ad0d7360dfe89c5d82290a4",
      "expiry": "2026-10-19T00:00:00.000Z",
      "tag": "RoadToTauEvent",
      "mission": {
        "node": "Brugia (Eris)", "type": "Extermination", "faction": "Infested",
        "reward": { "countedItems": [
          { "uniqueName": "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource", "type": "Road To Tau Event Resource", "count": 50 }
        ] }
      }
    },
    {
      "id": "6ad0d7360dfe89c5d82290a6",
      "expiry": "2026-10-19T00:00:00.000Z",
      "tag": "RoadToTauEvent",
      "mission": {
        "node": "Xini (Eris)", "type": "Survival", "faction": "Infested",
        "reward": { "countedItems": [
          { "uniqueName": "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource", "type": "Road To Tau Event Resource", "count": 50 }
        ] }
      }
    },
    {
      "id": "ffffffffffffffffffffffff",
      "expiry": "2026-10-19T00:00:00.000Z",
      "tag": "SpaceNinjaEvent",
      "mission": {
        "node": "Larunda (Mercury)", "type": "Capture", "faction": "Corpus",
        "reward": { "countedItems": [
          { "uniqueName": "/Lotus/Types/Gameplay/Events/SpaceNinjaEvent/SpaceNinjaEventResource", "type": "Space Ninja Resource", "count": 5 }
        ] }
      }
    }
  ]
}"#;

/// Ninth pass §4 integration gate: run the resident headless against the
/// recorded EE.log (gitignored) with a worldstate whose alert ids match
/// the log's completions, and assert the generated report strikes them —
/// and only them. Skipped when Seth's log fixture is absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_resident_strikes_log_completed_alerts_in_the_report() {
    let log_fixture = root().join("fixtures/ocr-samples/ee-log-2026-10-17.log");
    let Ok(log_text) = std::fs::read_to_string(&log_fixture) else {
        eprintln!("skip §4 integration: need {}", log_fixture.display());
        return;
    };

    let base = scrape_dir("completions");
    let fx = base.join("fixtures");
    std::fs::create_dir_all(&fx).unwrap();
    std::fs::write(fx.join("worldstate.json"), SYNTH_WORLDSTATE).unwrap();
    // The offline profile fallback (copied from the tracked fixture).
    std::fs::copy(
        root().join("fixtures/profile-5420fba23846322173707618.json"),
        fx.join("profile-5420fba23846322173707618.json"),
    )
    .unwrap();

    // The watched EE.log IS Seth's recorded session (all three completions).
    let log_path = base.join("log/EE.log");
    std::fs::write(&log_path, &log_text).unwrap();
    let out = base.join("out/plan.html");

    let opts = ResidentOptions {
        report: ReportConfig {
            fixtures: fx,
            ..offline_report_config(&base)
        },
        log_path: log_path.clone(),
        state_dir: base.join("state"),
        watch_interval_ms: 25,
        serve_port: 0,
        pop_on_session_start: false,
    };
    let shutdown = Shutdown::new();
    let sd = shutdown.clone();
    let task = tokio::spawn(async move { run_headless(opts, sd).await });

    // Wait for a report that strikes an alert (the watch reads the whole
    // log, proves the completions, and regenerates).
    let deadline = Instant::now() + Duration::from_secs(90);
    let html = loop {
        if let Ok(text) = std::fs::read_to_string(&out) {
            if text.contains("evt done") {
                break text;
            }
        }
        if Instant::now() > deadline {
            shutdown.trigger();
            let _ = task.await;
            panic!("resident never struck a log-completed alert");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Every one of the three completed alerts is struck...
    assert_eq!(
        html.matches("class=\"card exp evt done\"").count(),
        3,
        "all three log-completed alerts render struck"
    );
    // ...exactly three carry the proof (never a fourth by inference)...
    assert_eq!(
        html.matches("log &check;").count(),
        3,
        "only positively matched alerts carry the log proof"
    );
    // ...and the never-mentioned fourth alert still renders unstruck.
    assert!(
        html.contains("Larunda (Mercury) (Capture)"),
        "the unmatched alert still renders"
    );
    assert!(
        html.matches("class=\"card exp evt").count() == 4,
        "all four event alerts render (three struck, one live)"
    );

    shutdown.trigger();
    task.await.expect("resident task did not panic").expect("clean exit");
    // The proof is persisted in app state, not just the page.
    let state = std::fs::read_to_string(base.join("state/state.json")).unwrap();
    assert!(state.contains("mission_completions"), "completions persisted");
    assert!(state.contains("6ad0d7360dfe89c5d82290a2"), "Hive completion recorded");
    let _ = std::fs::remove_dir_all(&base);
}

/// One blocking loopback HTTP round-trip (the resident's own §2 server).
fn http(base_url: &str, method: &str, path: &str, body: Option<&str>) -> String {
    let addr = base_url
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let mut stream = std::net::TcpStream::connect(addr).expect("connect loopback server");
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    out
}

/// The first `data-id="…"` value in a page.
fn first_data_id(html: &str) -> Option<String> {
    let start = html.find("data-id=\"")? + "data-id=\"".len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// Ninth pass §2 integration gate: the resident serves the plan on
/// 127.0.0.1, re-applies server-side ticks, and records the live URL for
/// the tray's Open action. Offline + hermetic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_serves_the_plan_over_loopback_and_persists_ticks() {
    let base = scrape_dir("serve");
    let log_path = base.join("log/EE.log");
    let out = base.join("out/plan.html");
    let status = Arc::new(Mutex::new(TrayStatus::default()));
    let opts = ResidentOptions {
        report: offline_report_config(&base),
        log_path: log_path.clone(),
        state_dir: base.join("state"),
        watch_interval_ms: 25,
        serve_port: 0,
        pop_on_session_start: false,
    };
    let shutdown = Shutdown::new();
    let sd = shutdown.clone();
    let st = status.clone();
    let task = tokio::spawn(async move { fracker::resident::run(opts, sd, None, st, None).await });

    // A session-start EE.log drives one report, and the server records its URL.
    {
        let mut f = std::fs::File::create(&log_path).unwrap();
        writeln!(f, "14.148 Sys [Info]: Logged in Jenterrobahne").unwrap();
        f.flush().unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    let url = loop {
        let recorded = status.lock().unwrap().page_url.clone();
        if out.exists() {
            if let Some(url) = recorded {
                break url;
            }
        }
        if Instant::now() > deadline {
            shutdown.trigger();
            let _ = task.await;
            panic!("resident never served the plan");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        url.starts_with("http://127.0.0.1:"),
        "server binds loopback only, got {url}"
    );

    // GET / serves the real report with the tick JSON spliced in.
    let page = http(&url, "GET", "/", None);
    assert!(page.contains("200 OK"));
    assert!(page.contains("<html"), "the plan page is served");
    assert!(page.contains("data-fracker-ticks="), "ticks injected");
    let id = first_data_id(&page).expect("the page has checkboxes");
    let day = {
        let s = page.find("data-day=\"").unwrap() + "data-day=\"".len();
        let rest = &page[s..];
        rest[..rest.find('"').unwrap()].to_owned()
    };
    assert!(day.len() == 10, "plan day present, got {day:?}");

    // POST /tick stores it server-side...
    let body = format!(r#"{{"id":"{id}","day":"{day}","done":true}}"#);
    let posted = http(&url, "POST", "/tick", Some(&body));
    assert!(posted.contains("200 OK"), "tick accepted: {posted}");
    // ...persisted to the app data dir, keyed by id + day...
    let ticks = std::fs::read_to_string(base.join("state/ticks.json")).unwrap();
    assert!(
        ticks.contains(&id) && ticks.contains(&day),
        "tick persisted keyed by id + day: {ticks}"
    );
    // ...and re-applied on the next serve.
    let page2 = http(&url, "GET", "/", None);
    assert!(page2.contains("data-fracker-ticks="));

    shutdown.trigger();
    task.await.expect("resident task did not panic").expect("clean exit");
    let _ = std::fs::remove_dir_all(&base);
}
