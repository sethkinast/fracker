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
//!   rendering the remembered active tab `checked` (thirteenth pass §2);
//! - `GET /fragments?gen=<known>` returns the current generation, the
//!   tick/day payload exactly as `GET /` carries it, and only the page
//!   fragments whose rendered HTML changed since <known> (forty-first
//!   pass) — the page swaps those in place, so updates never navigate;
//! - `POST /tick` records one checkbox change server-side, keyed by item
//!   id + in-game day;
//! - `POST /unverified` toggles one part's unverified (⍻) stock
//!   membership in `stock.json` (thirtieth pass) — validated against
//!   the roster parts universe, persisted through the stock writer,
//!   and followed by a regeneration request so the plan re-renders;
//! - `GET /tab/<slug>` records the active plan tab (the tab links carry
//!   `?tab=<slug>`, which `GET /` also honours);
//! - `GET /ws` upgrades to a WebSocket and pushes the plan generation the
//!   moment a regeneration is published (item 58), so open pages fetch
//!   the fragment diff immediately instead of waiting for the slow poll;
//! - `GET /health` is a liveness probe.
//!
//! The WebSocket side is a minimal RFC 6455 implementation — handshake,
//! unmasked server text frames, masked client frame parsing, and close —
//! in keeping with the hand-rolled server (no new crates: SHA-1 and
//! base64 are local).
//!
//! No outbound network is added: the server reads only the report file it
//! is handed and the tick/tab stores in the app data dir.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::render::base64_encode;
use crate::tabs::TabStore;
use crate::ticks::TickStore;

/// The generation push fan-out (item 58).
///
/// The report pipeline publishes every new generation token here at the
/// exact point the page is atomically renamed into place; the server then
/// writes a text frame to every open WebSocket page. A page that connects
/// late is handed the current generation immediately, so it can catch up
/// after a dropped socket without waiting for the fragment poll.
pub struct GenerationHub {
    current: Mutex<String>,
    subscribers: Mutex<Vec<mpsc::Sender<String>>>,
}

impl GenerationHub {
    /// A hub seeded with the generation already on disk (empty when no
    /// report has been published yet).
    pub fn new(generation: String) -> Self {
        Self {
            current: Mutex::new(generation),
            subscribers: Mutex::new(Vec::new()),
        }
    }

    /// The most recently published generation.
    pub fn current(&self) -> String {
        self.current
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Publish a freshly rendered generation to every open page.
    pub fn publish(&self, generation: &str) {
        if let Ok(mut current) = self.current.lock() {
            *current = generation.to_owned();
        }
        let message = generation_message(generation);
        if let Ok(mut subscribers) = self.subscribers.lock() {
            // A closed page's receiver fails the send; retire it here so a
            // long-lived resident does not accumulate dead sockets.
            subscribers.retain(|tx| tx.send(message.clone()).is_ok());
        }
    }

    /// Register an open page. Returns the generation to send first plus the
    /// receiver carrying subsequent pushes.
    pub(crate) fn subscribe(&self) -> (String, Receiver<String>) {
        let (tx, rx) = mpsc::channel();
        let current = self.current();
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.push(tx);
        }
        (current, rx)
    }
}

impl Default for GenerationHub {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl std::fmt::Debug for GenerationHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenerationHub")
            .field("current", &self.current())
            .finish_non_exhaustive()
    }
}

/// The push payload: the new generation token, JSON-encoded for the page
/// script (tokens are numeric today, but never interpolate raw text).
fn generation_message(generation: &str) -> String {
    serde_json::json!({ "generation": generation }).to_string()
}

/// A running loopback server. Dropping it stops the accept loop.
pub struct PlanServer {
    port: u16,
    url: String,
    hub: Arc<GenerationHub>,
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

    /// The generation hub the report pipeline publishes into (item 58).
    pub fn hub(&self) -> Arc<GenerationHub> {
        self.hub.clone()
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

/// The unverified-stock toggle wiring (thirtieth pass, addendum
/// #93): everything `POST /unverified` needs to mutate `stock.json`
/// from a Hunt-row click. The resident supplies it; a server
/// without it (tests, headless tools) answers the route 404.
pub struct UnverifiedToggle {
    /// The stock file to mutate — the same `stock.json` capture
    /// finalize rewrites and the report reads.
    pub stock_path: PathBuf,
    /// The roster seed (`<data_dir>/cache/wishlist.json`); its part
    /// names are the only legal toggle targets.
    pub wishlist_path: PathBuf,
    /// Invoked after a toggle has been persisted, so the resident
    /// regenerates the plan and the generation push carries the new
    /// ⍻ state to open pages (the resident passes a
    /// `Trigger::Manual` send; a toggle never auto-opens the page).
    pub regen: Arc<dyn Fn() + Send + Sync>,
    /// Serialises read-modify-write cycles across connection
    /// threads (the TickStore's shared-Mutex pattern, for a file
    /// that is otherwise rewritten whole).
    gate: Mutex<()>,
}

impl UnverifiedToggle {
    pub fn new(
        stock_path: PathBuf,
        wishlist_path: PathBuf,
        regen: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            stock_path,
            wishlist_path,
            regen,
            gate: Mutex::new(()),
        }
    }

    /// Toggle one part's unverified membership and persist it.
    /// Returns the membership state after the toggle. The name is
    /// validated against the roster parts universe first: an
    /// unknown name changes nothing (and never regenerates). An
    /// unreadable roster or an unparseable stock file is
    /// `Unavailable` — like capture finalize, a stock file we
    /// cannot read is never clobbered.
    pub fn toggle(&self, name: &str) -> Result<bool, ToggleError> {
        let _gate = self.gate.lock().unwrap_or_else(|p| p.into_inner());
        let wish_text = std::fs::read_to_string(&self.wishlist_path)
            .map_err(|_| ToggleError::Unavailable)?;
        let roster = crate::model::Roster::from_json(&wish_text)
            .map_err(|_| ToggleError::Unavailable)?;
        if !roster_part_names(&roster).contains(name) {
            return Err(ToggleError::UnknownPart);
        }
        let mut stock = match std::fs::read_to_string(&self.stock_path) {
            Ok(text) => crate::ocr::StockState::from_json(&text)
                .map_err(|_| ToggleError::Unavailable)?,
            Err(_) => crate::ocr::StockState::default(),
        };
        let on = stock.toggle_unverified(name);
        stock
            .write_to(&self.stock_path)
            .map_err(|_| ToggleError::Unavailable)?;
        (self.regen)();
        Ok(on)
    }
}

/// Why a toggle did not happen (thirtieth pass).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleError {
    /// The submitted name is not a roster part name.
    UnknownPart,
    /// The roster or stock file could not be read/parsed/written.
    Unavailable,
}

/// Every roster part name — the toggle endpoint's validation
/// universe, the same keys the planner's canonical part-stock map
/// uses (addendum #84).
fn roster_part_names(roster: &crate::model::Roster) -> std::collections::HashSet<&str> {
    roster
        .items
        .iter()
        .flat_map(|item| item.parts.iter().map(|p| p.name.as_str()))
        .collect()
}

/// Bind `127.0.0.1` and start serving `report_path`. `preferred_port` is
/// the configured port; when it is already taken (or 0) an ephemeral
/// port is used so the resident always comes up — the live URL is
/// returned for the tray's Open action to read. `ticks_path` is where a
/// checkbox change is persisted (the store of record); `tabs_path` is
/// where the active plan tab is persisted (thirteenth pass §2).
/// `toggle` wires `POST /unverified` (thirtieth pass); `None` leaves
/// the route answering 404.
pub fn serve(
    preferred_port: u16,
    report_path: PathBuf,
    ticks_path: PathBuf,
    tabs_path: PathBuf,
    ticks: Arc<Mutex<TickStore>>,
    toggle: Option<Arc<UnverifiedToggle>>,
) -> Result<PlanServer, String> {
    let listener = bind(preferred_port)?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("server local_addr: {e}"))?
        .port();
    let url = format!("http://127.0.0.1:{port}/");
    let tabs = Arc::new(Mutex::new(TabStore::load(&tabs_path)));
    // Seed the hub from the report already on disk so a late page's initial
    // push names its own generation (no spurious reload).
    let hub = Arc::new(GenerationHub::new(report_generation(&report_path)));
    let hub_thread = hub.clone();
    let frag_cache = Arc::new(Mutex::new(FragmentCache::new()));
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
            hub_thread,
            frag_cache,
            stop_thread,
            toggle,
        )
    });
    Ok(PlanServer {
        port,
        url,
        hub,
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
    hub: Arc<GenerationHub>,
    frag_cache: Arc<Mutex<FragmentCache>>,
    stop: Arc<AtomicBool>,
    toggle: Option<Arc<UnverifiedToggle>>,
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
                let push = hub.clone();
                let frags = frag_cache.clone();
                let stopped = stop.clone();
                let toggle_thread = toggle.clone();
                std::thread::spawn(move || {
                    let _ = handle(
                        stream,
                        &report,
                        &ticks_file,
                        &tabs_file,
                        &store,
                        &tab_store,
                        &push,
                        &frags,
                        &stopped,
                        toggle_thread.as_ref(),
                    );
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
    hub: &Arc<GenerationHub>,
    frag_cache: &Arc<Mutex<FragmentCache>>,
    stop: &Arc<AtomicBool>,
    toggle: Option<&Arc<UnverifiedToggle>>,
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

    // Drain headers, capturing Content-Length for the POST body and the
    // WebSocket key for the upgrade handshake.
    let mut content_length = 0usize;
    let mut ws_key: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("sec-websocket-key") {
                ws_key = Some(value.to_owned());
            }
        }
    }
    // A WebSocket upgrade has no body and takes over the raw stream below,
    // so the buffered reader must not be the one reading frames.
    let mut body = Vec::new();
    if ws_key.is_none() && content_length > 0 {
        body.resize(content_length.min(64 * 1024), 0);
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
                record_tab(tabs, tabs_path, report_path, &slug);
            }
            let (status, html) = plan_page(report_path, tabs_path, ticks, tabs, frag_cache);
            respond(&mut stream, status, "text/html; charset=utf-8", html.as_bytes())
        }
        ("GET", p) if p.starts_with("/tab/") => {
            // The lightweight endpoint the tab links' JS enhancement uses:
            // record and return 204 (no page reload), so tab switches are
            // instant while still living server-side.
            let slug = &p["/tab/".len()..];
            if record_tab(tabs, tabs_path, report_path, slug) {
                respond(&mut stream, "204 No Content", "text/plain", b"")
            } else {
                respond(&mut stream, "404 Not Found", "text/plain", b"unknown tab")
            }
        }
        ("GET", "/fragments") => {
            // The kill-full-refresh endpoint (forty-first pass): the
            // diff since the generation the page says it shows.
            let known = query_param(query, "gen").unwrap_or_default();
            let (status, body) = fragments_page(report_path, ticks, frag_cache, &known);
            respond(&mut stream, status, "application/json", body.as_bytes())
        }
        ("GET", "/search-index") => {
            // The search overlay's index (forty-seventh pass): the
            // per-generation sidecar the report pipeline published
            // beside the page. The client prefetches it once per
            // generation and filters locally — a missing sidecar
            // (report not yet generated, older install) degrades
            // the overlay to an empty index, never an error page.
            let (status, body) = search_index_page(report_path);
            respond(&mut stream, status, "text/html; charset=utf-8", body.as_bytes())
        }
        ("POST", "/tick") => respond(
            &mut stream,
            apply_tick(ticks, ticks_path, &body),
            "application/json",
            &tick_ok(),
        ),
        ("POST", "/unverified") => {
            let (status, resp_body) = apply_unverified(toggle, &body);
            respond(&mut stream, status, "application/json", resp_body.as_bytes())
        }
        ("GET", "/ws") => match ws_key.as_deref() {
            Some(key) => ws_upgrade(&mut stream, key, report_path, hub, stop),
            None => respond(
                &mut stream,
                "400 Bad Request",
                "text/plain",
                b"websocket upgrade requires Sec-WebSocket-Key",
            ),
        },
        ("GET", "/chime.mp3") => {
            // The acquisition toast's chime (fifty-first
            // pass): an immutable embedded asset, so unlike
            // every other route it is served cacheable — the
            // browser fetches it once and keeps it.
            respond_cached(&mut stream, "200 OK", "audio/mpeg", CHIME_MP3)
        }
        ("GET", "/health") => respond(&mut stream, "200 OK", "text/plain", b"ok"),
        _ => respond(&mut stream, "404 Not Found", "text/plain", b"not found"),
    }
}

/// Record an active-tab slug (validated against the renderer's tabs) and
/// persist it best-effort, scoped to the current report generation (item
/// 52). A tab clicked on an old page after a regeneration is recorded
/// against the new generation only if the page it was clicked from is
/// current; the generation is read from the report file that / serves.
/// Returns false for an unknown slug so the caller can answer 404 rather
/// than quietly remember garbage.
fn record_tab(
    tabs: &Arc<Mutex<TabStore>>,
    tabs_path: &std::path::Path,
    report_path: &std::path::Path,
    slug: &str,
) -> bool {
    if crate::render::tab_id_for_slug(slug).is_none() {
        return false;
    }
    let generation = report_generation(report_path);
    if let Ok(mut store) = tabs.lock() {
        store.scope_to(&generation);
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
/// the remembered tab `checked` (thirteenth pass §2; addendum #86 keeps
/// the choice across generations — see [`crate::tabs`]).
fn plan_page(
    report_path: &std::path::Path,
    tabs_path: &std::path::Path,
    ticks: &Arc<Mutex<TickStore>>,
    tabs: &Arc<Mutex<TabStore>>,
    frag_cache: &Arc<Mutex<FragmentCache>>,
) -> (&'static str, String) {
    let html = match std::fs::read_to_string(report_path) {
        Ok(h) => h,
        Err(_) => PLACEHOLDER.to_owned(),
    };
    let day = body_day(&html).unwrap_or_default();
    let generation = body_generation(&html).unwrap_or_default();
    // Warm the fragment cache with the generation being served, so
    // the page's first push-driven /fragments call diffs against it
    // instead of falling back to a full swap (forty-first pass).
    if let Ok(mut c) = frag_cache.lock() {
        let hashes = crate::render::extract_fragments(&html)
            .iter()
            .map(|(id, frag)| (id.clone(), crate::render::fragment_hash(frag)))
            .collect();
        c.remember(&generation, hashes);
    }
    let ticks_b64 = tick_payload_b64(ticks);
    // The remembered slug applies across generations (addendum #86):
    // scoping only records the generation being served and persists it
    // best-effort. A slug naming no current tab falls back to t1 below.
    let active = tabs
        .lock()
        .ok()
        .map(|mut t| {
            if t.scope_to(&generation) {
                let _ = t.save(tabs_path);
            }
            crate::render::tab_id_for_slug(t.active()).unwrap_or("t1")
        })
        .unwrap_or("t1");
    let html = inject_tab(&html, active);
    ("200 OK", inject_ticks(&html, &ticks_b64, &day))
}

/// The tick payload exactly as `GET /` carries it on the `<body>`
/// tag: the tick store as JSON, base64-encoded. Shared by the full
/// page and the fragment endpoint so the two paths can never drift
/// (forty-first pass).
fn tick_payload_b64(ticks: &Arc<Mutex<TickStore>>) -> String {
    let store = ticks.lock().map(|s| s.clone()).unwrap_or_default();
    let json = serde_json::to_string(&store).unwrap_or_else(|_| "{}".to_owned());
    base64_encode(json.as_bytes())
}

/// Per-generation fragment hashes (forty-first pass): the server
/// retains the last few generations' fragment hash lists so a page
/// asking "what changed since the generation I show?" gets exactly
/// the fragments that differ. A forgotten generation is answered
/// with every fragment — a fuller swap, never a navigation.
struct FragmentCache {
    recent: std::collections::VecDeque<(String, Vec<(String, u64)>)>,
}

/// How many generations of fragment hashes the cache retains.
const FRAGMENT_CACHE_DEPTH: usize = 4;

impl FragmentCache {
    fn new() -> Self {
        Self {
            recent: std::collections::VecDeque::new(),
        }
    }

    /// The retained fragment hashes for one generation, if any.
    fn hashes(&self, generation: &str) -> Option<Vec<(String, u64)>> {
        self.recent
            .iter()
            .find(|(g, _)| g == generation)
            .map(|(_, h)| h.clone())
    }

    /// Remember one generation's fragment hashes (newest last,
    /// depth-capped).
    fn remember(&mut self, generation: &str, hashes: Vec<(String, u64)>) {
        if generation.is_empty() {
            return;
        }
        self.recent.retain(|(g, _)| g != generation);
        self.recent.push_back((generation.to_owned(), hashes));
        while self.recent.len() > FRAGMENT_CACHE_DEPTH {
            self.recent.pop_front();
        }
    }
}

/// `GET /search-index` (forty-seventh pass): the search overlay's
/// per-generation index, read straight from the sidecar the report
/// pipeline published beside the report (see
/// [`crate::search::search_index_path`]). A missing sidecar is a
/// 200 with an empty body: the client's overlay simply has no
/// blocks to show until the next generation lands one.
fn search_index_page(report_path: &std::path::Path) -> (&'static str, String) {
    let path = crate::search::search_index_path(report_path);
    match std::fs::read_to_string(&path) {
        Ok(body) => ("200 OK", body),
        Err(_) => ("200 OK", String::new()),
    }
}

/// `GET /fragments?gen=<known>` (forty-first pass): the current
/// generation, the day, the tick payload exactly as `GET /`
/// carries it, and only the fragments whose rendered HTML changed
/// since <known>. The page swaps those in place — updates arrive
/// with no navigation, no icon repaints, no lost scroll. A request
/// naming the current generation gets an empty change set.
fn fragments_page(
    report_path: &std::path::Path,
    ticks: &Arc<Mutex<TickStore>>,
    cache: &Arc<Mutex<FragmentCache>>,
    known: &str,
) -> (&'static str, String) {
    let html = match std::fs::read_to_string(report_path) {
        Ok(h) => h,
        Err(_) => PLACEHOLDER.to_owned(),
    };
    let day = body_day(&html).unwrap_or_default();
    let generation = body_generation(&html).unwrap_or_default();
    let fragments = crate::render::extract_fragments(&html);
    let hashes: Vec<(String, u64)> = fragments
        .iter()
        .map(|(id, html)| (id.clone(), crate::render::fragment_hash(html)))
        .collect();
    let known_hashes = if known.is_empty() || known == generation {
        None
    } else {
        cache.lock().ok().and_then(|c| c.hashes(known))
    };
    let changed: Vec<serde_json::Value> = if generation.is_empty() || known == generation {
        Vec::new()
    } else {
        fragments
            .iter()
            .filter(|(id, html)| match &known_hashes {
                Some(old) => old
                    .iter()
                    .find(|(oid, _)| oid == id)
                    .map(|(_, oh)| *oh != crate::render::fragment_hash(html))
                    .unwrap_or(true),
                None => true,
            })
            .map(|(id, html)| serde_json::json!({ "id": id, "html": html }))
            .collect()
    };
    if let Ok(mut c) = cache.lock() {
        c.remember(&generation, hashes);
    }
    let body = serde_json::json!({
        "generation": generation,
        "day": day,
        "ticks": tick_payload_b64(ticks),
        "fragments": changed,
    });
    ("200 OK", body.to_string())
}

/// The plan generation as rendered (`<body ... data-generation="…">`).
fn body_generation(html: &str) -> Option<String> {
    let start = html.find("data-generation=\"")? + "data-generation=\"".len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// The current generation of the report on disk (`""` when absent).
fn report_generation(report_path: &std::path::Path) -> String {
    std::fs::read_to_string(report_path)
        .ok()
        .and_then(|html| body_generation(&html))
        .unwrap_or_default()
}

/// The plan day as rendered (`<body data-day="YYYY-MM-DD">`).
fn body_day(html: &str) -> Option<String> {
    let start = html.find("data-day=\"")? + "data-day=\"".len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// Splice `data-fracker-ticks` (base64 JSON) onto the opening `<body>`
/// tag, keeping every attribute the renderer already emitted.
///
/// Hotfix 19 §1: this used to rebuild the tag from `data-day` alone, which
/// silently dropped `data-generation` from every SERVED page. The page's
/// WebSocket predicate then saw `""` against the hello's real token and
/// reloaded forever (the v0.5.4 reload storm). Preserve the renderer's
/// attributes verbatim; only add the tick payload (and a day when the
/// renderer had none).
fn inject_ticks(html: &str, ticks_b64: &str, day: &str) -> String {
    let Some(start) = html.find("<body") else {
        return html.to_owned();
    };
    let Some(end_rel) = html[start..].find('>') else {
        return html.to_owned();
    };
    let end = start + end_rel;
    let attrs = &html[start + "<body".len()..end];
    let mut injected = format!("data-fracker-ticks=\"{ticks_b64}\"");
    if !attrs.contains("data-day=") {
        injected.push_str(&format!(" data-day=\"{day}\""));
    }
    let mut out = String::with_capacity(html.len() + injected.len() + 1);
    out.push_str(&html[..start]);
    out.push_str("<body ");
    out.push_str(&injected);
    out.push_str(attrs);
    out.push('>');
    out.push_str(&html[end + 1..]);
    out
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

/// Apply one unverified-stock toggle (thirtieth pass). Accepts JSON
/// (`{"name": "..."}`) or form encoding, mirroring `/tick`. The
/// endpoint is view-agnostic by necessity — only the Hunt tab
/// renders rows that call it; the control here is validation:
/// unknown part names are a 404 with no state change, and a
/// successful toggle persists stock.json and fires the regen hook
/// so the ⍻ lands through the standard generation push.
fn apply_unverified(
    toggle: Option<&Arc<UnverifiedToggle>>,
    body: &str,
) -> (&'static str, String) {
    let Some(toggle) = toggle else {
        return ("404 Not Found", "{\"ok\":false}".to_owned());
    };
    let Some(name) = parse_unverified_name(body) else {
        return ("400 Bad Request", "{\"ok\":false}".to_owned());
    };
    match toggle.toggle(&name) {
        Ok(on) => (
            "200 OK",
            format!("{{\"ok\":true,\"unverified\":{on}}}"),
        ),
        Err(ToggleError::UnknownPart) => (
            "404 Not Found",
            "{\"ok\":false,\"error\":\"unknown part\"}".to_owned(),
        ),
        Err(ToggleError::Unavailable) => (
            "500 Internal Server Error",
            "{\"ok\":false}".to_owned(),
        ),
    }
}

/// Parse `{"name": "..."}` (JSON) or `name=...` (form).
fn parse_unverified_name(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        return v.get("name")?.as_str().map(str::to_owned);
    }
    for pair in trimmed.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "name" {
                return Some(url_decode(v));
            }
        }
    }
    None
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

/// The toast chime, embedded like the vendored font
/// (fifty-first pass). Source: Freesound #398496, "wind
/// chimes - single 04.wav" by Anthousai, CC0 (attribution
/// not required; provenance recorded in
/// docs/fifty-first-pass.md).
const CHIME_MP3: &[u8] = include_bytes!("../assets/sounds/chime.mp3");

/// [`respond`] with a long immutable cache lifetime, for
/// embedded assets whose bytes can never change under a
/// given URL (the chime is the only one).
fn respond_cached(
    stream: &mut TcpStream,
    status: &str,
    ctype: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: public, max-age=31536000, immutable\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

// ---------------------------------------------------------------------------
// WebSocket push (item 58): a minimal RFC 6455 implementation
// ---------------------------------------------------------------------------

/// RFC 6455 §1.3 handshake GUID.
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The `Sec-WebSocket-Accept` value for a client key:
/// base64(SHA-1(key + GUID)).
fn ws_accept(key: &str) -> String {
    let mut material = Vec::with_capacity(key.len() + WS_GUID.len());
    material.extend_from_slice(key.trim().as_bytes());
    material.extend_from_slice(WS_GUID.as_bytes());
    crate::render::base64_encode(&sha1(&material))
}

/// Upgrade one connection and stream generation pushes until the page
/// closes (or the server stops). The handshake is the only HTTP this
/// connection carries; after the 101 the raw socket speaks frames.
fn ws_upgrade(
    stream: &mut TcpStream,
    key: &str,
    report_path: &std::path::Path,
    hub: &GenerationHub,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        ws_accept(key)
    );
    stream.write_all(head.as_bytes())?;
    stream.flush()?;

    let (current, pushes) = hub.subscribe();
    // Catch a page up after a dropped socket: its first frame names the
    // generation `/` actually serves. Hotfix 19 §1: prefer the report on
    // disk over the hub's cached token, so a page can never be told it is
    // stale when it loaded exactly what `/` handed it. An empty disk
    // generation (no report yet) falls back to the hub.
    let disk = report_generation(report_path);
    let current = if disk.is_empty() { current } else { disk };
    // Equal generation is a no-op in the page predicate; only a genuinely
    // different one triggers a fragment fetch.
    stream.write_all(&ws_text_frame(&generation_message(&current)))?;
    stream.flush()?;

    // A short read timeout lets this thread alternate between pushing and
    // noticing a client close without blocking the push path.
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = stream.write_all(&ws_close_frame());
            return Ok(());
        }
        while let Ok(message) = pushes.try_recv() {
            stream.write_all(&ws_text_frame(&message))?;
            stream.flush()?;
        }
        match stream.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                let mut consumed = 0usize;
                let mut closed = false;
                while let Some((opcode, total)) = ws_frame(&pending[consumed..]) {
                    if opcode == 0x8 {
                        closed = true;
                    }
                    consumed += total;
                }
                pending.drain(..consumed);
                if closed {
                    let _ = stream.write_all(&ws_close_frame());
                    return Ok(());
                }
                // A page that never reads its own frames is not ours to
                // buffer forever.
                if pending.len() > 64 * 1024 {
                    return Ok(());
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => return Ok(()),
        }
    }
}

/// One unmasked server text frame (FIN + opcode 0x1).
fn ws_text_frame(text: &str) -> Vec<u8> {
    let payload = text.as_bytes();
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x81);
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

/// A close frame with no status payload.
fn ws_close_frame() -> [u8; 2] {
    [0x88, 0x00]
}

/// Parse one complete client frame from `buf`: `(opcode, total bytes)`.
/// `None` means the frame is still incomplete. Client frames are masked
/// (RFC 6455 §5.3); the mask key is skipped, not applied — the server only
/// needs control opcodes, never client payload text.
fn ws_frame(buf: &[u8]) -> Option<(u8, usize)> {
    let header = buf.get(..2)?;
    let opcode = header[0] & 0x0f;
    let masked = header[1] & 0x80 != 0;
    let short = (header[1] & 0x7f) as usize;
    let (mut offset, payload): (usize, usize) = match short {
        126 => {
            let ext = buf.get(2..4)?;
            (4, u16::from_be_bytes([ext[0], ext[1]]) as usize)
        }
        127 => {
            let ext = buf.get(2..10)?;
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(ext);
            (10, usize::try_from(u64::from_be_bytes(bytes)).ok()?)
        }
        n => (2, n),
    };
    if masked {
        offset += 4;
    }
    let total = offset.checked_add(payload)?;
    (buf.len() >= total).then_some((opcode, total))
}

/// SHA-1 (FIPS 180-4). Hand-rolled because the packaging contract only
/// vendors real dependencies and the handshake needs exactly this one
/// primitive.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).take(16).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
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
        let html = "<html><body data-day=\"2026-10-18\" data-generation=\"gen-7\"><p>x</p></body></html>";
        let out = inject_ticks(html, "QUJD", "2026-10-18");
        assert!(
            out.contains(
                "<body data-fracker-ticks=\"QUJD\" data-day=\"2026-10-18\" data-generation=\"gen-7\">"
            ),
            "{out}"
        );
        assert!(out.ends_with("<p>x</p></body></html>"));
        // Hotfix 19 §1: the generation MUST survive injection — dropping it
        // is what made the served page reload against its own hello.
        assert_eq!(body_generation(&out).as_deref(), Some("gen-7"));
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
    fn parse_unverified_name_accepts_json_and_form() {
        assert_eq!(
            parse_unverified_name(r#"{"name":"Protea Prime Systems Blueprint"}"#),
            Some("Protea Prime Systems Blueprint".to_owned())
        );
        assert_eq!(
            parse_unverified_name("name=Protea%20Prime%20Systems%20Blueprint"),
            Some("Protea Prime Systems Blueprint".to_owned())
        );
        assert_eq!(parse_unverified_name("{}"), None);
        assert_eq!(parse_unverified_name("id=chunk-1"), None);
    }

    #[test]
    fn unverified_endpoint_toggles_stock_and_rejects_unknown_parts() {
        // Thirtieth pass (#93): POST /unverified toggles membership
        // in stock.json's unverified section — validated against the
        // roster seed, persisted through the stock writer, and each
        // successful toggle fires the regen hook exactly once.
        let dir = std::env::temp_dir().join(format!(
            "fracker-unverified-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("plan.html");
        std::fs::write(&report, "<!DOCTYPE html><html><body></body></html>").unwrap();
        let wishlist = dir.join("wishlist.json");
        std::fs::write(
            &wishlist,
            r#"[{"name": "Protea Prime", "type": "Warframe",
                 "parts": [{"name": "Protea Prime Blueprint"},
                           {"name": "Protea Prime Systems Blueprint"}]}]"#,
        )
        .unwrap();
        let stock_path = dir.join("stock.json");
        let regens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let regens_hook = regens.clone();
        let toggle = Arc::new(UnverifiedToggle::new(
            stock_path.clone(),
            wishlist.clone(),
            Arc::new(move || {
                regens_hook.fetch_add(1, Ordering::Relaxed);
            }),
        ));
        let server = serve(
            0,
            report.clone(),
            TickStore::file(&dir),
            TabStore::file(&dir),
            Arc::new(Mutex::new(TickStore::default())),
            Some(toggle),
        )
        .expect("server binds");
        let post = |body: &str| -> String {
            let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
            write!(
                s,
                "POST /unverified HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        let on_disk = |path: &std::path::Path| {
            crate::ocr::StockState::from_json(&std::fs::read_to_string(path).unwrap())
                .unwrap()
        };

        // Toggle ON: 200, the membership lands in stock.json's
        // unverified section at count 1, regen fired once.
        let resp = post(r#"{"name":"Protea Prime Systems Blueprint"}"#);
        assert!(resp.contains("200 OK"), "{resp}");
        assert!(resp.contains("\"unverified\":true"), "{resp}");
        assert_eq!(
            on_disk(&stock_path)
                .unverified
                .get("Protea Prime Systems Blueprint"),
            Some(&1)
        );
        assert_eq!(regens.load(Ordering::Relaxed), 1);

        // Toggle OFF: membership removed (not zeroed), regen again.
        let resp = post(r#"{"name":"Protea Prime Systems Blueprint"}"#);
        assert!(resp.contains("\"unverified\":false"), "{resp}");
        assert!(on_disk(&stock_path).unverified.is_empty());
        assert_eq!(regens.load(Ordering::Relaxed), 2);

        // Unknown part name: 404, no state change, no regen.
        let resp = post(r#"{"name":"Excalibur Prime Systems Blueprint"}"#);
        assert!(resp.contains("404"), "{resp}");
        assert!(on_disk(&stock_path).unverified.is_empty());
        assert_eq!(regens.load(Ordering::Relaxed), 2, "rejections never regenerate");

        // Unparseable body: a visible 400, like /tick.
        let resp = post("not a toggle");
        assert!(resp.contains("400"), "{resp}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Gate (forty-seventh pass): the overlay index route serves
    /// the sidecar the report pipeline published beside the
    /// report, and a missing sidecar is an empty 200 (the overlay
    /// degrades silently), never a 404.
    #[test]
    fn serve_search_index_reads_the_sidecar() {
        let dir =
            std::env::temp_dir().join(format!("fracker-server-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("plan.html");
        std::fs::write(&report, "<!DOCTYPE html><html><body></body></html>").unwrap();
        let ticks = Arc::new(Mutex::new(TickStore::default()));
        let server = serve(
            0,
            report.clone(),
            TickStore::file(&dir),
            TabStore::file(&dir),
            ticks,
            None,
        )
        .expect("server binds");
        let get = |path: &str| -> String {
            let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        // No sidecar yet: empty 200, not an error.
        let resp = get("/search-index");
        assert!(resp.contains("200 OK"), "{resp}");
        // The pipeline's sidecar lands: the route serves it.
        std::fs::write(
            crate::search::search_index_path(&report),
            "<section class=\"sr\" data-name=\"Meso C11\"></section>",
        )
        .unwrap();
        let resp = get("/search-index");
        assert!(resp.contains("200 OK"), "{resp}");
        assert!(resp.contains("Meso C11"), "sidecar served: {resp}");
        assert!(resp.contains("text/html"), "{resp}");
        let _ = std::fs::remove_dir_all(&dir);
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
            None,
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
    fn serve_chime_route_is_cacheable_audio() {
        // Pass 51: the toast chime is an embedded immutable
        // asset — 200, audio/mpeg, the full byte count, and a
        // long cache lifetime (every other route is no-store).
        let dir = std::env::temp_dir().join(format!(
            "fracker-server-chime-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let server = serve(
            0,
            dir.join("plan.html"),
            TickStore::file(&dir),
            TabStore::file(&dir),
            Arc::new(Mutex::new(TickStore::default())),
            None,
        )
        .expect("server binds");
        // Byte-level fetch (the body is binary; http_get's
        // read_to_string cannot carry it).
        let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        write!(s, "GET /chime.mp3 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("head/body split")
            + 4;
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        assert!(head.contains("200 OK"), "{head}");
        assert!(head.contains("audio/mpeg"), "content type: {head}");
        assert!(
            head.contains("Cache-Control: public, max-age=31536000, immutable"),
            "cacheable: {head}"
        );
        assert!(
            head.contains(&format!("Content-Length: {}", CHIME_MP3.len())),
            "full asset: {head}"
        );
        assert_eq!(&raw[split..], CHIME_MP3, "the served bytes are the asset");
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
            None,
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
            "<!DOCTYPE html><html><body data-day=\"2026-10-18\" data-generation=\"gen-1\">\
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
            None,
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
            None,
        )
        .expect("restart binds");
        assert!(http_get(server2.port(), "/").contains("id=\"t3\" checked"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Addendum #86 (superseding item 52): the choice survives the 60s
    /// refreshes of one report AND a new regeneration — generations
    /// churn on every worldstate tick, so resetting on them made the
    /// tab reset on ordinary refreshes.
    #[test]
    fn a_new_generation_keeps_the_remembered_tab() {
        let dir = std::env::temp_dir().join(format!(
            "fracker-tab-gen-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("plan.html");
        let page = |generation: &str| {
            format!(
                "<!DOCTYPE html><html><body data-day=\"2026-10-18\" data-generation=\"{generation}\">\
                 <input type=\"radio\" name=\"tab\" id=\"t1\" checked>\
                 <input type=\"radio\" name=\"tab\" id=\"t2\">\
                 <input type=\"radio\" name=\"tab\" id=\"t3\">\
                 <input type=\"radio\" name=\"tab\" id=\"t4\"></body></html>"
            )
        };
        std::fs::write(&report, page("gen-1")).unwrap();
        let tabs_path = TabStore::file(&dir);
        let server = serve(
            0,
            report.clone(),
            TickStore::file(&dir),
            tabs_path.clone(),
            Arc::new(Mutex::new(TickStore::default())),
            None,
        )
        .expect("server binds");

        // Choose Backlog, then refresh the same generation several times.
        assert!(http_get(server.port(), "/?tab=backlog").contains("id=\"t3\" checked"));
        for _ in 0..2 {
            let refreshed = http_get(server.port(), "/");
            assert!(refreshed.contains("id=\"t3\" checked"), "{refreshed}");
        }

        // A new report lands (a regeneration): the tab is KEPT
        // (addendum #86) — a refresh must land back on the viewed tab.
        std::fs::write(&report, page("gen-2")).unwrap();
        let kept = http_get(server.port(), "/");
        assert!(kept.contains("id=\"t3\" checked"), "{kept}");
        assert!(!kept.contains("id=\"t1\" checked"), "{kept}");

        // A fresh choice under gen-2 likewise survives its refreshes.
        assert!(http_get(server.port(), "/?tab=hunt").contains("id=\"t2\" checked"));
        assert!(http_get(server.port(), "/").contains("id=\"t2\" checked"));
        let disk = TabStore::load(&tabs_path);
        assert_eq!(disk.generation, "gen-2");
        assert_eq!(disk.active(), "hunt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn body_generation_reads_the_renderer_attribute() {
        assert_eq!(
            body_generation("<body data-day=\"x\" data-generation=\"42\">").as_deref(),
            Some("42")
        );
        assert_eq!(body_generation("<body data-day=\"x\">"), None);
    }

    // ---- §41 fragments (forty-first pass) ----

    /// A minimal rendered page: body attributes + the six fragment
    /// markers, parameterised so a test can change one section.
    fn frag_report(generation: &str, p3_note: &str) -> String {
        format!(
            "<!DOCTYPE html><html><body data-day=\"2026-10-18\" data-generation=\"{generation}\">\
             <!--@frag:header-->\nHEADER\
             <!--/@frag-->\
             <!--@frag:p1-->\nP1\
             <!--/@frag-->\
             <!--@frag:p2-->\nP2\
             <!--/@frag-->\
             <!--@frag:p3-->\nP3 {p3_note}\
             <!--/@frag-->\
             <!--@frag:p4-->\nP4\
             <!--/@frag-->\
             <!--@frag:p5-->\nP5\
             <!--/@frag-->\
             </body></html>"
        )
    }

    fn frag_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fracker-frag-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An up-to-date page gets an empty change set, and the fragment
    /// payload's tick field is byte-identical to the attribute the
    /// full page carries — the two paths share one payload builder.
    #[test]
    fn fragments_unchanged_generation_returns_an_empty_diff() {
        let dir = frag_dir("same");
        let report = dir.join("plan.html");
        std::fs::write(&report, frag_report("gen-1", "a")).unwrap();
        let ticks = Arc::new(Mutex::new(TickStore::default()));
        let tabs = Arc::new(Mutex::new(TabStore::default()));
        let cache = Arc::new(Mutex::new(FragmentCache::new()));
        // The full-page serve comes first in production and warms
        // the cache with the generation it handed out.
        let (_, page) = plan_page(&report, &TabStore::file(&dir), &ticks, &tabs, &cache);
        let (status, body) = fragments_page(&report, &ticks, &cache, "gen-1");
        assert_eq!(status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["generation"], "gen-1");
        assert_eq!(v["day"], "2026-10-18");
        assert_eq!(v["fragments"].as_array().unwrap().len(), 0, "{body}");
        let attr_start =
            page.find("data-fracker-ticks=\"").unwrap() + "data-fracker-ticks=\"".len();
        let attr_end = page[attr_start..].find('"').unwrap() + attr_start;
        assert_eq!(
            v["ticks"].as_str().unwrap(),
            &page[attr_start..attr_end],
            "fragment tick payload must equal the full page's"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A regeneration that changes one section produces a payload
    /// naming only that fragment.
    #[test]
    fn fragments_diff_names_only_the_changed_section() {
        let dir = frag_dir("diff");
        let report = dir.join("plan.html");
        std::fs::write(&report, frag_report("gen-1", "a")).unwrap();
        let ticks = Arc::new(Mutex::new(TickStore::default()));
        let tabs = Arc::new(Mutex::new(TabStore::default()));
        let cache = Arc::new(Mutex::new(FragmentCache::new()));
        let _ = plan_page(&report, &TabStore::file(&dir), &ticks, &tabs, &cache);
        // The regeneration lands: new token, one section changed.
        std::fs::write(&report, frag_report("gen-2", "b")).unwrap();
        let (_, body) = fragments_page(&report, &ticks, &cache, "gen-1");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["generation"], "gen-2");
        let frags = v["fragments"].as_array().unwrap();
        assert_eq!(frags.len(), 1, "{body}");
        assert_eq!(frags[0]["id"], "p3");
        assert!(frags[0]["html"].as_str().unwrap().contains("P3 b"));
        // And the page that applied gen-2 is now up to date.
        let (_, body) = fragments_page(&report, &ticks, &cache, "gen-2");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["fragments"].as_array().unwrap().len(), 0, "{body}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A generation the cache has forgotten (or never saw) is
    /// answered with every fragment — a fuller swap, not a reload.
    #[test]
    fn fragments_unknown_generation_returns_everything() {
        let dir = frag_dir("stale");
        let report = dir.join("plan.html");
        std::fs::write(&report, frag_report("gen-1", "a")).unwrap();
        let ticks = Arc::new(Mutex::new(TickStore::default()));
        let cache = Arc::new(Mutex::new(FragmentCache::new()));
        let (_, body) = fragments_page(&report, &ticks, &cache, "gen-never-seen");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["generation"], "gen-1");
        let ids: Vec<&str> = v["fragments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, crate::render::FRAGMENT_IDS);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The endpoint over real HTTP: JSON out, empty diff when
    /// current, full set when the caller's generation is unknown.
    #[test]
    fn fragments_endpoint_serves_json_over_http() {
        let dir = frag_dir("http");
        let report = dir.join("plan.html");
        std::fs::write(&report, frag_report("gen-1", "a")).unwrap();
        let server = serve(
            0,
            report,
            TickStore::file(&dir),
            TabStore::file(&dir),
            Arc::new(Mutex::new(TickStore::default())),
            None,
        )
        .expect("server binds");
        let current = http_get(server.port(), "/fragments?gen=gen-1");
        assert!(current.contains("200"), "{current}");
        assert!(current.contains("\"generation\":\"gen-1\""), "{current}");
        assert!(current.contains("\"fragments\":[]"), "{current}");
        let stale = http_get(server.port(), "/fragments?gen=gen-0");
        assert!(stale.contains("\"id\":\"p3\""), "{stale}");
        assert!(stale.contains("\"ticks\":\""), "{stale}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- §2 WebSocket push (item 58) ----

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha1_matches_the_fips_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    /// The RFC 6455 §1.3 worked example: this exact key/accept pair is what
    /// a real browser's handshake is checked against.
    #[test]
    fn websocket_accept_matches_the_rfc_example() {
        assert_eq!(
            ws_accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn websocket_frames_encode_and_parse_across_lengths() {
        // Server text frame, short payload.
        assert_eq!(ws_text_frame("hi"), vec![0x81, 0x02, b'h', b'i']);
        // 16-bit extended length.
        let long = ws_text_frame(&"x".repeat(300));
        assert_eq!(&long[..4], &[0x81, 0x7e, 0x01, 0x2c]);
        assert_eq!(long.len(), 304);
        assert_eq!(ws_frame(&long), Some((1, 304)));
        // A masked client close frame (browser-style) parses once complete.
        let close = [0x88, 0x82, 0x01, 0x02, 0x03, 0x04, 0x03, 0xe8];
        assert_eq!(ws_frame(&close[..7]), None, "incomplete frame waits");
        assert_eq!(ws_frame(&close), Some((8, 8)));
        // An unmasked frame is still parsed (the server only needs opcodes).
        assert_eq!(ws_frame(&ws_text_frame("x")), Some((1, 3)));
    }

    /// Read one unmasked server frame (handshake responses are read first
    /// byte-by-byte so no frame bytes are swallowed).
    fn read_ws_frame_e2e(stream: &mut TcpStream) -> (u8, Vec<u8>) {
        let mut header = [0u8; 2];
        stream.read_exact(&mut header).unwrap();
        let opcode = header[0] & 0x0f;
        let short = (header[1] & 0x7f) as usize;
        let len = match short {
            126 => {
                let mut ext = [0u8; 2];
                stream.read_exact(&mut ext).unwrap();
                u16::from_be_bytes(ext) as usize
            }
            127 => {
                let mut ext = [0u8; 8];
                stream.read_exact(&mut ext).unwrap();
                u64::from_be_bytes(ext) as usize
            }
            n => n,
        };
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).unwrap();
        (opcode, payload)
    }

    fn read_http_head(stream: &mut TcpStream) -> String {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
            assert!(head.len() < 16 * 1024, "handshake head is bounded");
        }
        String::from_utf8(head).unwrap()
    }

    fn generation_of(payload: &[u8]) -> String {
        let v: serde_json::Value = serde_json::from_slice(payload).unwrap();
        v["generation"].as_str().unwrap().to_owned()
    }

    /// The §2 gate at server scale: a real upgrade handshake, the current
    /// generation on connect, and a pushed frame the instant a new one is
    /// published — no 60s refresh involved.
    #[test]
    fn websocket_pushes_a_new_generation_to_an_open_page() {
        let dir = std::env::temp_dir().join(format!(
            "fracker-ws-push-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("plan.html");
        std::fs::write(
            &report,
            "<!DOCTYPE html><html><body data-day=\"2026-10-21\" data-generation=\"gen-1\"></body></html>",
        )
        .unwrap();
        let server = serve(
            0,
            report,
            TickStore::file(&dir),
            TabStore::file(&dir),
            Arc::new(Mutex::new(TickStore::default())),
            None,
        )
        .expect("server binds");

        // Hotfix 19 §1 regression: the SERVED page must carry the same
        // generation the hello names. v0.5.4's tick injection rebuilt the
        // <body> tag without data-generation, so the page read "" and
        // reloaded against every hello — a permanent storm.
        let served = http_get(server.port(), "/");
        assert_eq!(
            body_generation(&served).as_deref(),
            Some("gen-1"),
            "served page keeps the renderer's generation: {served}"
        );

        let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        )
        .unwrap();
        let head = read_http_head(&mut s);
        assert!(head.starts_with("HTTP/1.1 101 Switching Protocols"), "{head}");
        assert!(
            head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "{head}"
        );

        // The first frame names the generation already on disk (catch-up for
        // a page that reconnected after missing a push).
        let (opcode, payload) = read_ws_frame_e2e(&mut s);
        assert_eq!(opcode, 0x1, "server frames are text");
        assert_eq!(generation_of(&payload), "gen-1");

        // A regeneration is published: the open page is pushed immediately.
        server.hub().publish("gen-2");
        let (opcode, payload) = read_ws_frame_e2e(&mut s);
        assert_eq!(opcode, 0x1);
        assert_eq!(generation_of(&payload), "gen-2");

        // A browser close is answered and the connection retires.
        s.write_all(&[0x88, 0x82, 0x01, 0x02, 0x03, 0x04, 0x03, 0xe8])
            .unwrap();
        let (opcode, payload) = read_ws_frame_e2e(&mut s);
        assert_eq!(opcode, 0x8, "close is acknowledged");
        assert!(payload.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Hotfix 19 §1: the hello names the generation `/` serves, even when
    /// the hub's cached token is older (an external writer, e.g. a CLI
    /// run, published to the same report path without the hub). If the
    /// hello named the stale token, the page would reload against it
    /// forever.
    #[test]
    fn websocket_hello_matches_the_served_page_not_a_stale_hub() {
        let dir = std::env::temp_dir().join(format!(
            "fracker-ws-hello-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("plan.html");
        let page = |generation: &str| {
            format!(
                "<!DOCTYPE html><html><body data-day=\"2026-10-21\" data-generation=\"{generation}\"></body></html>"
            )
        };
        std::fs::write(&report, page("gen-1")).unwrap();
        let server = serve(
            0,
            report.clone(),
            TickStore::file(&dir),
            TabStore::file(&dir),
            Arc::new(Mutex::new(TickStore::default())),
            None,
        )
        .expect("server binds");

        // A second writer replaces the report without going through the
        // hub, leaving the hub's cached generation at gen-1.
        std::fs::write(&report, page("gen-2")).unwrap();
        let served = http_get(server.port(), "/");
        assert_eq!(body_generation(&served).as_deref(), Some("gen-2"));

        let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        )
        .unwrap();
        let _head = read_http_head(&mut s);
        let (_opcode, payload) = read_ws_frame_e2e(&mut s);
        assert_eq!(
            generation_of(&payload),
            "gen-2",
            "hello names the generation / actually serves"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websocket_upgrade_without_a_key_is_refused() {
        let dir = std::env::temp_dir().join(format!("fracker-ws-nokey-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let server = serve(
            0,
            dir.join("plan.html"),
            TickStore::file(&dir),
            TabStore::file(&dir),
            Arc::new(Mutex::new(TickStore::default())),
            None,
        )
        .expect("server binds");
        let response = http_get(server.port(), "/ws");
        assert!(response.contains("400 Bad Request"), "{response}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
