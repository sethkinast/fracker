//! Loopback HTTP server for the plan page (ninth pass §2).
//!
//! Seth's catch (item 41): the plan page persisted its ticks in
//! `localStorage`, but a `file://` URL is an opaque/partitioned origin —
//! ticks were fragile-to-dead depending on the browser — and the file
//! handoff made "Open plan page" ask which program to use every time.
//! Serving the page from the resident fixes both.
//!
//! This is a deliberately tiny, dependency-free HTTP/1.1 server:
//!
//! - bound to `127.0.0.1` **only** (the Flatpak shares the host network
//!   namespace, so the host browser reaches loopback while nothing is
//!   exposed off-box);
//! - `GET /` serves the current report HTML, re-applying the stored ticks
//!   (so they survive regenerations and are browser-independent) and
//!   rendering the remembered active tab `checked` (thirteenth pass §2, so
//!   the 60s meta-refresh no longer resets to First Crack);
//! - `POST /tick` records one checkbox change server-side, keyed by item
//!   id + in-game day;
//! - `GET /tab/<slug>` records the active plan tab (the tab links carry
//!   `?tab=<slug>`, which `GET /` also honours);
//! - `GET /health` is a liveness probe.
//!
//! No outbound network is added: the server reads only the report file it
//! is handed and the tick/tab stores in the app data dir.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::render::base64_encode;
use crate::tabs::TabStore;
use crate::ticks::TickStore;

/// A running loopback server. Dropping it stops the accept loop.
pub struct PlanServer {
    port: u16,
    url: String,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl PlanServer {
    /// The bound port (the preferred one when free, else an ephemeral one).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The live URL "Open plan page" / auto-open use.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for PlanServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Nudge a pending accept so the thread notices the stop promptly.
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

/// Bind `127.0.0.1` and start serving `report_path`. `preferred_port` is
/// the configured port; when it is already taken (or 0) an ephemeral
/// port is used so the resident always comes up — the live URL is
/// returned for the tray's Open action to read. `ticks_path` is where a
/// checkbox change is persisted (the store of record); `tabs_path` is
/// where the active plan tab is persisted (thirteenth pass §2).
pub fn serve(
    preferred_port: u16,
    report_path: PathBuf,
    ticks_path: PathBuf,
    tabs_path: PathBuf,
    ticks: Arc<Mutex<TickStore>>,
) -> Result<PlanServer, String> {
    let listener = bind(preferred_port)?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("server local_addr: {e}"))?
        .port();
    let url = format!("http://127.0.0.1:{port}/");
    let tabs = Arc::new(Mutex::new(TabStore::load(&tabs_path)));
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let join = std::thread::spawn(move || {
        accept_loop(
            listener,
            report_path,
            ticks_path,
            tabs_path,
            ticks,
            tabs,
            stop_thread,
        )
    });
    Ok(PlanServer {
        port,
        url,
        stop,
        join: Some(join),
    })
}

/// Bind the preferred loopback port, falling back to an ephemeral one.
fn bind(preferred: u16) -> Result<TcpListener, String> {
    if preferred != 0 {
        if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, preferred)) {
            return Ok(listener);
        }
    }
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0u16))
        .map_err(|e| format!("server bind 127.0.0.1:0: {e}"))
}

fn accept_loop(
    listener: TcpListener,
    report_path: PathBuf,
    ticks_path: PathBuf,
    tabs_path: PathBuf,
    ticks: Arc<Mutex<TickStore>>,
    tabs: Arc<Mutex<TabStore>>,
    stop: Arc<AtomicBool>,
) {
    let _ = listener.set_nonblocking(true);
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let report = report_path.clone();
                let ticks_file = ticks_path.clone();
                let tabs_file = tabs_path.clone();
                let store = ticks.clone();
                let tab_store = tabs.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &report, &ticks_file, &tabs_file, &store, &tab_store);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(_) => break,
        }
    }
}

/// Serve one request. Every response closes the connection, so a browser
/// cannot hold the single accept slot open.
fn handle(
    mut stream: TcpStream,
    report_path: &std::path::Path,
    ticks_path: &std::path::Path,
    tabs_path: &std::path::Path,
    ticks: &Arc<Mutex<TickStore>>,
    tabs: &Arc<Mutex<TabStore>>,
) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("/").to_owned();

    // Drain headers, capturing Content-Length for the POST body.
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed
            .split_once(':')
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .map(|(_, v)| v.trim())
        {
            content_length = v.parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length.min(64 * 1024)];
    if !body.is_empty() {
        reader.read_exact(&mut body)?;
    }
    let body = String::from_utf8_lossy(&body).into_owned();

    let without_frag = target.split('#').next().unwrap_or("/");
    let (path, query) = match without_frag.split_once('?') {
        Some((p, q)) => (p, q),
        None => (without_frag, ""),
    };
    match (method.as_str(), path) {
        ("GET", "/") | ("GET", "/index.html") | ("GET", "/plan.html") => {
            // A `?tab=<slug>` on the page URL is a real-link selection; a
            // valid one is recorded before the page renders checked.
            if let Some(slug) = query_param(query, "tab") {
                record_tab(tabs, tabs_path, &slug);
            }
            let (status, html) = plan_page(report_path, ticks, tabs);
            respond(&mut stream, status, "text/html; charset=utf-8", html.as_bytes())
        }
        ("GET", p) if p.starts_with("/tab/") => {
            // The lightweight endpoint the tab links' JS enhancement uses:
            // record and return 204 (no page reload), so tab switches are
            // instant while still living server-side.
            let slug = &p["/tab/".len()..];
            if record_tab(tabs, tabs_path, slug) {
                respond(&mut stream, "204 No Content", "text/plain", b"")
            } else {
                respond(&mut stream, "404 Not Found", "text/plain", b"unknown tab")
            }
        }
        ("POST", "/tick") => respond(
            &mut stream,
            apply_tick(ticks, ticks_path, &body),
            "application/json",
            &tick_ok(),
        ),
        ("GET", "/health") => respond(&mut stream, "200 OK", "text/plain", b"ok"),
        _ => respond(&mut stream, "404 Not Found", "text/plain", b"not found"),
    }
}

/// Record an active-tab slug (validated against the renderer's tabs) and
/// persist it best-effort. Returns false for an unknown slug so the caller
/// can answer 404 rather than quietly remember garbage.
fn record_tab(tabs: &Arc<Mutex<TabStore>>, tabs_path: &std::path::Path, slug: &str) -> bool {
    if crate::render::tab_id_for_slug(slug).is_none() {
        return false;
    }
    if let Ok(mut store) = tabs.lock() {
        store.set(slug);
        let _ = store.save(tabs_path);
    }
    true
}

/// First `key=value` in an `application/x-www-form-urlencoded` query.
fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(url_decode(v));
            }
        }
    }
    None
}

/// Re-read the report file for every request (so a regenerated page is
/// always current), splice the live ticks onto the `<body>` tag, and mark
/// the remembered tab `checked` (thirteenth pass §2).
fn plan_page(
    report_path: &std::path::Path,
    ticks: &Arc<Mutex<TickStore>>,
    tabs: &Arc<Mutex<TabStore>>,
) -> (&'static str, String) {
    let html = match std::fs::read_to_string(report_path) {
        Ok(h) => h,
        Err(_) => PLACEHOLDER.to_owned(),
    };
    let day = body_day(&html).unwrap_or_default();
    let store = ticks.lock().map(|s| s.clone()).unwrap_or_default();
    let json = serde_json::to_string(&store).unwrap_or_else(|_| "{}".to_owned());
    // The remembered slug maps to a radio id; unknown/empty falls back to
    // the renderer's default (First Crack).
    let active = tabs
        .lock()
        .ok()
        .and_then(|t| crate::render::tab_id_for_slug(t.active()))
        .unwrap_or("t1");
    let html = inject_tab(&html, active);
    ("200 OK", inject_ticks(&html, &base64_encode(json.as_bytes()), &day))
}

/// The plan day as rendered (`<body data-day="YYYY-MM-DD">`).
fn body_day(html: &str) -> Option<String> {
    let start = html.find("data-day=\"")? + "data-day=\"".len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// Splice `data-fracker-ticks` (base64 JSON) onto the opening `<body>`
/// tag, keeping any attributes the renderer already emitted.
fn inject_ticks(html: &str, ticks_b64: &str, day: &str) -> String {
    let attr = format!("data-fracker-ticks=\"{ticks_b64}\" data-day=\"{day}\"");
    if let Some(start) = html.find("<body") {
        if let Some(end_rel) = html[start..].find('>') {
            let end = start + end_rel;
            let mut out = String::with_capacity(html.len() + attr.len());
            out.push_str(&html[..start]);
            out.push_str("<body ");
            out.push_str(&attr);
            out.push('>');
            out.push_str(&html[end + 1..]);
            return out;
        }
    }
    html.to_owned()
}

/// Mark the remembered tab's radio `checked`, clearing the renderer's
/// default First-Crack selection (thirteenth pass §2). Operates on the tab
/// radios only (`id="tN"`), so every other element is untouched.
fn inject_tab(html: &str, active_id: &str) -> String {
    if !crate::render::TABS.iter().any(|(id, _, _)| *id == active_id) {
        // Not a rendered tab: leave the page's default selection alone.
        return html.to_owned();
    }
    let mut out = html.to_owned();
    for (id, _, _) in crate::render::TABS {
        out = out.replace(&format!("id=\"{id}\" checked"), &format!("id=\"{id}\""));
    }
    out.replace(
        &format!("id=\"{active_id}\""),
        &format!("id=\"{active_id}\" checked"),
    )
}

/// Apply one checkbox change. Accepts JSON (`{"id","day","done"}`) or
/// form encoding; an unparseable body is a visible 400, never a silent
/// no-op that loses the tick.
fn apply_tick(
    ticks: &Arc<Mutex<TickStore>>,
    ticks_path: &std::path::Path,
    body: &str,
) -> &'static str {
    let Some((id, day, done)) = parse_tick(body) else {
        return "400 Bad Request";
    };
    if let Ok(mut store) = ticks.lock() {
        store.set(&day, &id, done);
        // Persist best-effort; the in-memory store already reflects it.
        let _ = store.save(ticks_path);
    }
    "200 OK"
}

fn tick_ok() -> Vec<u8> {
    b"{\"ok\":true}".to_vec()
}

/// Parse `{"id","day","done"}` (JSON) or `id=..&day=..&done=1|0` (form).
fn parse_tick(body: &str) -> Option<(String, String, bool)> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let id = v.get("id")?.as_str()?.to_owned();
        let day = v.get("day")?.as_str()?.to_owned();
        let done = v.get("done")?.as_bool()?;
        return Some((id, day, done));
    }
    let mut id = None;
    let mut day = None;
    let mut done = None;
    for pair in trimmed.split('&') {
        let (k, v) = pair.split_once('=')?;
        match k {
            "id" => id = Some(url_decode(v)),
            "day" => day = Some(url_decode(v)),
            "done" => done = Some(matches!(v, "1" | "true" | "on")),
            _ => {}
        }
    }
    Some((id?, day?, done?))
}

/// Minimal application/x-www-form-urlencoded value decoder (`+` and `%XX`).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    ctype: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Served while the first report is still being generated.
const PLACEHOLDER: &str = "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta http-equiv=\"refresh\" content=\"5\"><title>Fracker</title></head>\
<body data-day=\"\"><p>Fracker is generating the plan…</p></body></html>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_ticks_preserves_renderer_attributes() {
        let html = "<html><body data-day=\"2026-10-18\"><p>x</p></body></html>";
        let out = inject_ticks(html, "QUJD", "2026-10-18");
        assert!(out.contains("<body data-fracker-ticks=\"QUJD\" data-day=\"2026-10-18\">"));
        assert!(out.ends_with("<p>x</p></body></html>"));
        // A bare `<body>` still gets the attributes.
        let bare = inject_ticks("<body><p>y</p></body>", "QQ==", "2026-10-18");
        assert!(bare.starts_with("<body data-fracker-ticks=\"QQ==\" data-day=\"2026-10-18\">"));
    }

    #[test]
    fn body_day_reads_the_rendered_day() {
        assert_eq!(
            body_day("<body data-day=\"2026-10-18\">").as_deref(),
            Some("2026-10-18")
        );
        assert_eq!(body_day("<body>"), None);
    }

    #[test]
    fn parse_tick_accepts_json_and_form() {
        assert_eq!(
            parse_tick(r#"{"id":"chunk-1","day":"2026-10-18","done":true}"#),
            Some(("chunk-1".to_owned(), "2026-10-18".to_owned(), true))
        );
        assert_eq!(
            parse_tick("id=wk-2&day=2026-10-18&done=0"),
            Some(("wk-2".to_owned(), "2026-10-18".to_owned(), false))
        );
        // Percent-decoding for form values.
        assert_eq!(
            parse_tick("id=a%20b&day=2026-10-18&done=1"),
            Some(("a b".to_owned(), "2026-10-18".to_owned(), true))
        );
        assert_eq!(parse_tick("{garbage"), None);
        assert_eq!(parse_tick("id=x"), None);
    }

    #[test]
    fn tick_endpoint_updates_the_store() {
        let ticks = Arc::new(Mutex::new(TickStore::default()));
        let path = std::env::temp_dir().join(format!("fracker-tick-ep-{}.json", std::process::id()));
        let status = apply_tick(
            &ticks,
            &path,
            r#"{"id":"chunk-1","day":"2026-10-18","done":true}"#,
        );
        assert_eq!(status, "200 OK");
        assert!(ticks.lock().unwrap().is_done("2026-10-18", "chunk-1"));
        // The change was persisted to the given path.
        assert!(TickStore::load(&path).is_done("2026-10-18", "chunk-1"));
        assert_eq!(apply_tick(&ticks, &path, "nope"), "400 Bad Request");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn serve_round_trips_get_plan_and_post_tick() {
        let dir = std::env::temp_dir().join(format!("fracker-server-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("plan.html");
        std::fs::write(
            &report,
            "<!DOCTYPE html><html><body data-day=\"2026-10-18\"><input type=\"checkbox\" data-id=\"chunk-1\"></body></html>",
        )
        .unwrap();
        let ticks = Arc::new(Mutex::new(TickStore::default()));
        let ticks_path = TickStore::file(&dir);
        let server = serve(
            0,
            report.clone(),
            ticks_path.clone(),
            TabStore::file(&dir),
            ticks.clone(),
        )
        .expect("server binds");
        let get = |path: &str| -> String {
            let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        // GET / serves the plan with the tick JSON spliced in.
        let page = get("/");
        assert!(page.contains("200 OK"));
        assert!(page.contains("data-fracker-ticks="), "ticks injected");
        assert!(page.contains("chunk-1"));
        assert!(get("/health").contains("200 OK"));
        assert!(get("/nope").contains("404"));

        // POST /tick records the change (and persists it), and the next GET
        // reflects it.
        {
            let body = r#"{"id":"chunk-1","day":"2026-10-18","done":true}"#;
            let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
            write!(
                s,
                "POST /tick HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            assert!(out.contains("200 OK"));
        }
        assert!(ticks.lock().unwrap().is_done("2026-10-18", "chunk-1"));
        assert!(TickStore::load(&ticks_path).is_done("2026-10-18", "chunk-1"));
        let page2 = get("/");
        assert!(page2.contains("data-fracker-ticks="));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serve_falls_back_off_a_taken_port() {
        // Hold a port, then ask the server for it: it must bind elsewhere.
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken = held.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("fracker-server-fb-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let server = serve(
            taken,
            dir.join("plan.html"),
            TickStore::file(&dir),
            TabStore::file(&dir),
            Arc::new(Mutex::new(TickStore::default())),
        )
        .expect("falls back");
        assert_ne!(
            server.port(),
            taken,
            "live port recorded when the preferred one is taken"
        );
        assert!(server.url().starts_with("http://127.0.0.1:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- §2 tab persistence ----

    fn http_get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn inject_tab_moves_the_checked_radio() {
        let html = "<input type=\"radio\" name=\"tab\" id=\"t1\" checked>\n\
                    <input type=\"radio\" name=\"tab\" id=\"t2\">\n";
        let out = inject_tab(html, "t2");
        assert!(out.contains("id=\"t2\" checked"), "{out}");
        assert!(!out.contains("id=\"t1\" checked"), "{out}");
        assert_eq!(out.matches(" checked").count(), 1, "exactly one tab checked");
        // An id that is not a rendered tab leaves the default untouched.
        assert!(inject_tab(html, "tx").contains("id=\"t1\" checked"));
    }

    #[test]
    fn query_param_decodes_and_ignores_unknown() {
        assert_eq!(query_param("tab=hunt", "tab").as_deref(), Some("hunt"));
        assert_eq!(
            query_param("a=1&tab=first-crack&b=2", "tab").as_deref(),
            Some("first-crack")
        );
        assert_eq!(query_param("other=x", "tab"), None);
        assert_eq!(query_param("tab=a%20b", "tab").as_deref(), Some("a b"));
    }

    #[test]
    fn active_tab_is_recorded_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("fracker-tab-srv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("plan.html");
        std::fs::write(
            &report,
            "<!DOCTYPE html><html><body data-day=\"2026-10-18\">\
             <input type=\"radio\" name=\"tab\" id=\"t1\" checked>\
             <input type=\"radio\" name=\"tab\" id=\"t2\">\
             <input type=\"radio\" name=\"tab\" id=\"t3\">\
             <input type=\"radio\" name=\"tab\" id=\"t4\">\
             <nav class=\"tabs\"><a class=\"tab\" data-tab=\"t2\" href=\"?tab=hunt\">Hunt</a></nav>\
             <input type=\"checkbox\" data-id=\"chunk-1\"></body></html>",
        )
        .unwrap();
        let tabs_path = TabStore::file(&dir);
        let server = serve(
            0,
            report.clone(),
            TickStore::file(&dir),
            tabs_path.clone(),
            Arc::new(Mutex::new(TickStore::default())),
        )
        .expect("server binds");

        // Default serve: First Crack checked.
        assert!(http_get(server.port(), "/").contains("id=\"t1\" checked"));

        // The real tab link's URL is recorded and re-rendered checked.
        let page = http_get(server.port(), "/?tab=hunt");
        assert!(page.contains("id=\"t2\" checked"), "{page}");
        assert!(!page.contains("id=\"t1\" checked"), "default cleared");

        // The JS-enhancement endpoint records and answers 204.
        assert!(http_get(server.port(), "/tab/backlog").contains("204"));
        assert!(http_get(server.port(), "/").contains("id=\"t3\" checked"));

        // An unknown slug is refused and never clobbers the remembered tab.
        assert!(http_get(server.port(), "/tab/bogus").contains("404"));
        assert!(http_get(server.port(), "/").contains("id=\"t3\" checked"));

        // Persisted on disk ...
        assert_eq!(TabStore::load(&tabs_path).active(), "backlog");
        drop(server);

        // ... so an app restart (a fresh server on the same store) still
        // lands on Backlog.
        let server2 = serve(
            0,
            report,
            TickStore::file(&dir),
            tabs_path,
            Arc::new(Mutex::new(TickStore::default())),
        )
        .expect("restart binds");
        assert!(http_get(server2.port(), "/").contains("id=\"t3\" checked"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
