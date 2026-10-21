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
        push: None,
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

/// Item 48 (tray Quit): the Quit verb must break the resident loop and
/// return cleanly. The process-level teardown (explicit `Handle::shutdown`
/// plus the deterministic `process::exit` in `main`) is reasoned rather
/// than exercised here — this box has no DBus session bus to spawn a real
/// tray against — but the loop half of the fix is proven: no restart, no
/// re-entry, no hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tray_quit_command_exits_the_resident_loop() {
    let base = scrape_dir("quit");
    let opts = ResidentOptions {
        report: offline_report_config(&base),
        log_path: base.join("log/EE.log"),
        state_dir: base.join("state"),
        watch_interval_ms: 25,
        serve_port: 0,
        pop_on_session_start: false,
    };
    let shutdown = Shutdown::new();
    let status = Arc::new(Mutex::new(TrayStatus::default()));
    let (cmds_tx, cmds_rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        fracker::resident::run(opts, shutdown, None, status, Some(cmds_rx)).await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    cmds_tx.send(fracker::tray::TrayCommand::Quit).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("resident loop must exit promptly on Quit")
        .expect("resident task did not panic");
    assert!(result.is_ok(), "clean exit: {result:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// DE-shaped worldstate whose alert ids/locations match the fixture log's
/// `alertId`/`location`s, plus one alert the log never mentions (proving
/// the strike is only ever a positive log match). Expiries sit after the
/// payload's `Time` so every alert renders.
const SYNTH_WORLDSTATE: &str = r#"{
  "Time": 1792195200,
  "Alerts": [
    {
      "_id": { "$oid": "6ad0d7360dfe89c5d82290a2" },
      "Activation": { "$date": { "$numberLong": "1792108800000" } },
      "Expiry": { "$date": { "$numberLong": "1792368000000" } },
      "Tag": "RoadToTauEvent",
      "MissionInfo": {
        "missionType": "MT_HIVE", "faction": "FC_INFESTATION",
        "location": "SolNode175",
        "descText": "/Lotus/Language/TauPrologue/EventAlertTitle",
        "missionReward": { "credits": 17000, "countedItems": [
          { "ItemType": "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource", "ItemCount": 50 }
        ] }
      }
    },
    {
      "_id": { "$oid": "6ad0d7360dfe89c5d82290a4" },
      "Activation": { "$date": { "$numberLong": "1792108800000" } },
      "Expiry": { "$date": { "$numberLong": "1792368000000" } },
      "Tag": "RoadToTauEvent",
      "MissionInfo": {
        "missionType": "MT_EXTERMINATION", "faction": "FC_INFESTATION",
        "location": "SolNode172",
        "descText": "/Lotus/Language/TauPrologue/EventAlertTitle",
        "missionReward": { "credits": 17000, "countedItems": [
          { "ItemType": "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource", "ItemCount": 50 }
        ] }
      }
    },
    {
      "_id": { "$oid": "6ad0d7360dfe89c5d82290a6" },
      "Activation": { "$date": { "$numberLong": "1792108800000" } },
      "Expiry": { "$date": { "$numberLong": "1792368000000" } },
      "Tag": "RoadToTauEvent",
      "MissionInfo": {
        "missionType": "MT_SURVIVAL", "faction": "FC_INFESTATION",
        "location": "SolNode153",
        "descText": "/Lotus/Language/TauPrologue/EventAlertTitle",
        "missionReward": { "credits": 17000, "countedItems": [
          { "ItemType": "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource", "ItemCount": 50 }
        ] }
      }
    },
    {
      "_id": { "$oid": "ffffffffffffffffffffffff" },
      "Activation": { "$date": { "$numberLong": "1792108800000" } },
      "Expiry": { "$date": { "$numberLong": "1792368000000" } },
      "Tag": "SpaceNinjaEvent",
      "MissionInfo": {
        "missionType": "MT_CAPTURE", "faction": "FC_CORPUS",
        "location": "SolNode166",
        "descText": "/Lotus/Language/Events/SpaceNinjaEventTitle",
        "missionReward": { "credits": 5000, "countedItems": [
          { "ItemType": "/Lotus/Types/Gameplay/Events/SpaceNinjaEvent/SpaceNinjaEventResource", "ItemCount": 5 }
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

    // Wait for the report that strikes all three completions. The watch
    // feeds the log incrementally and regenerates after each proven
    // completion, so an earlier report may carry only the first strike;
    // the §4 SolNode gate (one node, one alert) makes that visible.
    let deadline = Instant::now() + Duration::from_secs(90);
    let html = loop {
        if let Ok(text) = std::fs::read_to_string(&out) {
            if text.matches("class=\"row ev done\"").count() == 3 {
                break text;
            }
        }
        if Instant::now() > deadline {
            shutdown.trigger();
            let _ = task.await;
            panic!("resident never struck all three log-completed alerts");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Every one of the three completed alerts is struck...
    assert_eq!(
        html.matches("class=\"row ev done\"").count(),
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
        html.contains("<b>Nimus</b>") && html.contains("Capture"),
        "the unmatched alert still renders"
    );
    assert!(
        html.matches("class=\"row ev").count() == 4,
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
