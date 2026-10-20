//! D-Bus-level Quit gate (fifteenth pass §1, item 53).
//!
//! Pass 14's Quit fix shipped with a logic test and still left a live
//! zombie: the resident stopped working but the process (and tray icon)
//! stayed up. The only test that can catch that is the real shape —
//!
//!   1. spawn the actual `fracker` binary under a private D-Bus session,
//!   2. let it start a report that is deliberately wedged on the network
//!      (this is exactly what happened live: `do_report` ran inside the
//!      control loop, so Quit was never dequeued until the fetch timed out),
//!   3. activate "Quit" through the tray's `com.canonical.dbusmenu`
//!      interface, as a desktop shell does,
//!   4. assert the process exits within a bounded time and that
//!      `resident.log` records the full teardown (`start` → `complete`).
//!
//! A pure logic test is not evidence (addendum #53); this one drives the
//! shipped binary over the wire.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zbus::zvariant::OwnedValue;

/// How long the resident may take to exit after the Quit activation. The
/// pre-fix hang was indefinite (still alive after 20 s); the fixed path
/// finishes in well under a second, so 10 s is a generous gate.
const QUIT_BUDGET: Duration = Duration::from_secs(10);

struct PrivateBus {
    address: String,
    pid: i32,
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        // SAFETY: SIGTERM to the pid we spawned ourselves.
        unsafe {
            libc::kill(self.pid, libc::SIGTERM);
        }
    }
}

fn start_private_bus() -> PrivateBus {
    let out = Command::new("dbus-daemon")
        .args([
            "--session",
            "--fork",
            "--print-address=1",
            "--print-pid=1",
        ])
        .output()
        .expect("dbus-daemon must be installed (it is part of the test gate)");
    assert!(
        out.status.success(),
        "dbus-daemon failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("dbus-daemon address is utf-8");
    let mut lines = text.lines();
    let address = lines.next().expect("dbus address").to_owned();
    let pid = lines
        .next()
        .expect("dbus pid")
        .trim()
        .parse()
        .expect("dbus pid is a number");
    PrivateBus { address, pid }
}

fn work_dir(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "fracker-tray-quit-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    for sub in ["data/fracker", "config/fracker", "cache", "log"] {
        std::fs::create_dir_all(base.join(sub)).unwrap();
    }
    base
}

/// A TCP listener that accepts proxy connections and never answers. This
/// is the network wedge: `reqwest` waits for a response (its request
/// timeout), so the report stays in flight until Quit cancels it.
fn hang_proxy() -> (
    u16,
    Arc<AtomicBool>,
    Arc<std::sync::atomic::AtomicUsize>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let accepted_thread = accepted.clone();
    let handle = std::thread::spawn(move || {
        let mut held = Vec::new();
        while !stop_thread.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    held.push(stream);
                    accepted_thread.fetch_add(1, Ordering::Relaxed);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        drop(held);
    });
    (port, stop, accepted, handle)
}

fn write_config(root: &Path) {
    // `serve_port = 0` keeps the test off Seth's live port; the auto-open
    // is irrelevant here and off so the Quit path is the only tray verb.
    std::fs::write(
        root.join("config/fracker/config.toml"),
        "serve_port = 0\npop_on_session_start = false\n",
    )
    .unwrap();
}

fn log_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn wait_for_app_log(path: &Path, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if log_text(path).contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => return Some(status),
            None if Instant::now() >= deadline => return None,
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// The unique bus name owned by `pid` that exposes a dbusmenu at
/// `/MenuBar`. Discovery is by PID (not by a guessed well-known name):
/// ksni's StatusNotifierItem connection is the one the child owns.
async fn find_menu_owner(
    conn: &zbus::Connection,
    pid: u32,
) -> Option<(String, Vec<(i32, HashMap<String, OwnedValue>)>)> {
    let reply = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        )
        .await
        .ok()?;
    let names: Vec<String> = reply.body().deserialize().ok()?;
    for name in names.into_iter().filter(|n| n.starts_with(':')) {
        let reply = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetConnectionUnixProcessID",
                &(name.as_str(),),
            )
            .await
            .ok()?;
        let owner: u32 = reply.body().deserialize().ok()?;
        if owner != pid {
            continue;
        }
        // Empty id list = all items (com.canonical.dbusmenu contract).
        let reply = conn
            .call_method(
                Some(name.as_str()),
                "/MenuBar",
                Some("com.canonical.dbusmenu"),
                "GetGroupProperties",
                &(Vec::<i32>::new(), Vec::<String>::new()),
            )
            .await
            .ok()?;
        let items: Vec<(i32, HashMap<String, OwnedValue>)> = reply.body().deserialize().ok()?;
        return Some((name, items));
    }
    None
}

fn item_id(items: &[(i32, HashMap<String, OwnedValue>)], label: &str) -> Option<i32> {
    items.iter().find_map(|(id, props)| {
        let value = props.get("label")?;
        let text: &str = <&str>::try_from(value).ok()?;
        (text == label).then_some(*id)
    })
}

async fn activate(conn: &zbus::Connection, name: &str, id: i32) {
    conn.call_method(
        Some(name),
        "/MenuBar",
        Some("com.canonical.dbusmenu"),
        "Event",
        &(id, "clicked", zbus::zvariant::Value::I32(0), 0u32),
    )
    .await
    .expect("dbusmenu Event(clicked)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tray_quit_exits_while_a_report_is_blocked_on_the_network() {
    let root = work_dir("blocked-report");
    write_config(&root);
    let log_path = root.join("log/EE.log");
    // Startup replay plus a report trigger, like a live session's log.
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ocr-samples/ee-log-2026-10-18.log"),
        &log_path,
    )
    .unwrap();

    let bus = start_private_bus();
    let (proxy_port, proxy_stop, proxy_accepted, proxy_thread) = hang_proxy();

    let out_log = std::fs::File::create(root.join("out.log")).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fracker"))
        .args([
            "--log",
            log_path.to_str().unwrap(),
            "--out",
            root.join("plan.html").to_str().unwrap(),
            "--watch-interval-ms",
            "25",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("HTTP_PROXY", format!("http://127.0.0.1:{proxy_port}"))
        .env("HTTPS_PROXY", format!("http://127.0.0.1:{proxy_port}"))
        .env("http_proxy", format!("http://127.0.0.1:{proxy_port}"))
        .env("https_proxy", format!("http://127.0.0.1:{proxy_port}"))
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .stdout(Stdio::from(out_log))
        .stderr(Stdio::from(err_log))
        .spawn()
        .expect("spawn the fracker resident");
    let pid = child.id();
    let resident_log = root.join("data/fracker/resident.log");

    // The tray must come up and the app log must show the resident start.
    assert!(
        wait_for_app_log(&resident_log, "[resident] start", Duration::from_secs(10)),
        "resident did not start:\n{}\n--- out ---\n{}",
        log_text(&resident_log),
        log_text(&root.join("out.log"))
    );

    let conn = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .await
        .expect("connect to the private bus");

    let deadline = Instant::now() + Duration::from_secs(10);
    let (name, mut items) = loop {
        if let Some(found) = find_menu_owner(&conn, pid).await {
            break found;
        }
        assert!(
            Instant::now() < deadline,
            "no dbusmenu appeared for pid {pid}:\n{}",
            log_text(&resident_log)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let quit = match item_id(&items, "Quit") {
        Some(id) => id,
        None => panic!("Quit missing from the tray menu: {items:?}"),
    };

    // If the startup report has not started yet, ask for one explicitly;
    // either way the proxy wedge keeps it in flight.
    if !log_text(&resident_log).contains("[report] start") {
        if let Some(regen) = item_id(&items, "Regenerate now") {
            activate(&conn, &name, regen).await;
        }
    }
    assert!(
        wait_for_app_log(&resident_log, "[report] start", Duration::from_secs(10)),
        "no report started before Quit:\n{}",
        log_text(&resident_log)
    );
    // Wait until the report has actually opened a connection to the
    // black-hole proxy and been left waiting for a response. That is the
    // exact live shape: the fetch is in flight, not merely scheduled.
    let deadline = Instant::now() + Duration::from_secs(10);
    while proxy_accepted.load(Ordering::Relaxed) == 0 {
        assert!(
            Instant::now() < deadline,
            "report never reached the network:\n{}",
            log_text(&resident_log)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Give the request time to be genuinely outstanding (a fast-failing
    // fetch would write `[report] done` by now).
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The report must still be running: if it already finished, the test
    // would not be exercising Quit-during-report at all.
    items = {
        let reply = conn
            .call_method(
                Some(name.as_str()),
                "/MenuBar",
                Some("com.canonical.dbusmenu"),
                "GetGroupProperties",
                &(Vec::<i32>::new(), Vec::<String>::new()),
            )
            .await
            .expect("menu refresh");
        reply.body().deserialize().expect("menu properties")
    };
    assert!(item_id(&items, "Quit").is_some(), "tray stayed alive");
    assert!(
        !log_text(&resident_log).contains("[report] done"),
        "report finished before Quit; the wedge failed to hold it:\n{}",
        log_text(&resident_log)
    );
    assert!(
        child.try_wait().unwrap().is_none(),
        "resident exited before Quit was activated"
    );

    // The live-message path: a desktop shell's dbusmenu activation.
    activate(&conn, &name, quit).await;

    let status = wait_for_exit(&mut child, QUIT_BUDGET);
    let app_log = log_text(&resident_log);
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    proxy_stop.store(true, Ordering::Relaxed);
    let _ = proxy_thread.join();

    assert!(
        status.is_some(),
        "Quit left the process alive for {}s (the v0.5.0 zombie):\n{}",
        QUIT_BUDGET.as_secs(),
        app_log
    );
    assert_eq!(status.unwrap().code(), Some(0), "clean exit");

    // Item 53: the app log must name the activation and every teardown
    // step, ending in `complete`. This is the evidence the live run gives
    // Seth: the last `start` without a `done` was the hang point.
    assert!(
        app_log.contains("[quit] activation received"),
        "Quit activation was not recorded:\n{app_log}"
    );
    assert!(app_log.contains("[teardown] start"), "{app_log}");
    assert!(
        app_log.contains("[teardown] abort report job: start"),
        "teardown must cancel the in-flight report:\n{app_log}"
    );
    assert!(
        app_log.contains("[teardown] abort report job: done")
            || app_log.contains("[teardown] abort report job: timed out"),
        "report abort was not bounded/logged:\n{app_log}"
    );
    assert!(
        app_log.contains("[teardown] complete"),
        "teardown never reached complete:\n{app_log}"
    );

    drop(bus);
}
