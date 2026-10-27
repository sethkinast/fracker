//! Client-side generation sync (item 58; fragment swap, forty-first
//! pass).
//!
//! The served plan page learns about a new generation two ways: the
//! WebSocket push (instant) and a slow poll of `/fragments` (the
//! convergence backstop for a dropped socket). Either way the page
//! answers by fetching `GET /fragments?gen=<shown>` and swapping ONLY
//! the changed sections in place — never by navigating. Scroll,
//! focus, the active tab, and every unchanged panel's `<img>`
//! elements survive a regeneration; that is the blink fix Seth
//! asked for when he killed the full-refresh behaviour (the 60s
//! meta-refresh and the push-triggered `location.reload()` are both
//! gone).
//!
//! The only navigation left is a last resort: after
//! [`MAX_FETCH_FAILURES`] consecutive *HTTP* failures (the server
//! answers but is wedged or skewed — a network failure never counts,
//! because reloading into a dead server would trade a good stale
//! page for a browser error page), the page reloads once, behind
//! the session breaker inherited from the reload-guard era: at most
//! [`BREAKER_MAX_RELOADS`] reloads inside [`BREAKER_WINDOW_MS`],
//! spaced by [`MIN_RELOAD_INTERVAL_MS`], then none for the rest of
//! the browser session (fragment sync keeps running — only the
//! reload is disabled).
//!
//! The policy is factored here as [`PushGuard`] so it is
//! unit-testable without a browser. [`client_script`] is the
//! JavaScript mirror embedded in the page — keep the two in step.

/// Fragment fetches are never decided closer than this; a burst of
/// pushes coalesces into one fetch of the latest generation.
pub const MIN_FETCH_INTERVAL_MS: u64 = 1_000;

/// Consecutive HTTP failures of the fragment fetch before the
/// guarded last-resort reload is even considered.
pub const MAX_FETCH_FAILURES: u32 = 3;

/// A last-resort reload is never closer than this to the previous
/// one.
pub const MIN_RELOAD_INTERVAL_MS: u64 = 5_000;

/// The sliding window the reload breaker counts reloads in.
pub const BREAKER_WINDOW_MS: u64 = 30_000;

/// Last-resort reloads allowed inside one window before the breaker
/// trips.
pub const BREAKER_MAX_RELOADS: usize = 3;

/// The slow fragment poll that replaces the 60s meta-refresh as the
/// convergence backstop (a fetch, not a navigation).
pub const POLL_INTERVAL_MS: u64 = 60_000;

/// What a generation observation or a fetch outcome means for the
/// page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushAction {
    /// The pushed generation equals the page's applied generation:
    /// no-op.
    NoChange,
    /// Fetch the fragment diff for the newer generation now.
    Fetch,
    /// A safeguard (fetch floor, failure budget, or reload spacing)
    /// suppressed the action for now; a later push or the poll
    /// retries.
    Suppressed,
    /// The last-resort reload: navigate exactly once, now.
    Reload,
    /// The reload breaker has tripped: no more automatic reloads
    /// this session. Fragment sync (push + poll) continues.
    Disabled,
}

/// The client sync predicate + failure guards, in Rust.
#[derive(Debug, Clone)]
pub struct PushGuard {
    loaded: String,
    last_fetch_ms: Option<u64>,
    failures: u32,
    last_reload_ms: Option<u64>,
    reloads_ms: Vec<u64>,
    reloads_disabled: bool,
}

impl PushGuard {
    /// A guard for a page loaded with `generation`.
    pub fn new(generation: impl Into<String>) -> Self {
        Self {
            loaded: generation.into(),
            last_fetch_ms: None,
            failures: 0,
            last_reload_ms: None,
            reloads_ms: Vec::new(),
            reloads_disabled: false,
        }
    }

    /// The generation the page has applied (loaded with, or swapped
    /// up to by the last successful fragment fetch).
    pub fn loaded(&self) -> &str {
        &self.loaded
    }

    /// True once the breaker has dropped last-resort reloading.
    pub fn disabled(&self) -> bool {
        self.reloads_disabled
    }

    /// Decide what to do with a pushed generation observed at
    /// `now_ms`.
    ///
    /// Equal generation is always a no-op. A differing one fetches
    /// the fragment diff, at most once per
    /// [`MIN_FETCH_INTERVAL_MS`] (a burst inside the floor is
    /// suppressed; the trailing fetch or the poll picks the latest
    /// generation up).
    pub fn observe(&mut self, incoming: &str, now_ms: u64) -> PushAction {
        if incoming == self.loaded {
            return PushAction::NoChange;
        }
        if let Some(last) = self.last_fetch_ms {
            if now_ms.saturating_sub(last) < MIN_FETCH_INTERVAL_MS {
                return PushAction::Suppressed;
            }
        }
        self.last_fetch_ms = Some(now_ms);
        PushAction::Fetch
    }

    /// A fragment fetch succeeded and its payload was applied: the
    /// page now shows `generation`, and the failure budget resets.
    pub fn observe_applied(&mut self, generation: &str) {
        self.loaded = generation.to_owned();
        self.failures = 0;
    }

    /// A fragment fetch failed at the HTTP level (the server
    /// answered, but wrongly). Below [`MAX_FETCH_FAILURES`]
    /// consecutive failures the page just retries later; at the cap
    /// the last-resort reload fires — once, spaced and capped by
    /// the breaker — after which the budget resets. Crossing the
    /// breaker disables reloading for the session.
    pub fn observe_fetch_failure(&mut self, now_ms: u64) -> PushAction {
        if self.reloads_disabled {
            return PushAction::Disabled;
        }
        self.failures += 1;
        if self.failures < MAX_FETCH_FAILURES {
            return PushAction::Suppressed;
        }
        if let Some(last) = self.last_reload_ms {
            if now_ms.saturating_sub(last) < MIN_RELOAD_INTERVAL_MS {
                return PushAction::Suppressed;
            }
        }
        self.reloads_ms
            .retain(|t| now_ms.saturating_sub(*t) < BREAKER_WINDOW_MS);
        if self.reloads_ms.len() >= BREAKER_MAX_RELOADS {
            self.reloads_disabled = true;
            return PushAction::Disabled;
        }
        self.reloads_ms.push(now_ms);
        self.last_reload_ms = Some(now_ms);
        self.failures = 0;
        PushAction::Reload
    }
}

/// The page script for the guard. The tokens are substituted so the
/// browser and [`PushGuard`] share one set of tuning values.
pub fn client_script() -> String {
    const TEMPLATE: &str = r#"// Live plan sync (item 58; forty-first pass): the loopback server
// pushes the new generation over WebSocket the moment a regeneration
// lands, and the page answers by fetching /fragments?gen=<shown> and
// swapping ONLY the changed sections in place — no navigation, so
// scroll, focus, the active tab, and every unchanged panel's images
// survive. A slow poll of the same endpoint converges a dropped
// socket. The ONLY reload left is a last resort after FAIL_MAX
// consecutive HTTP failures, behind the session breaker (at most
// BREAKER_MAX reloads inside WINDOW_MS, spaced by MIN_RELOAD_MS,
// persisted in sessionStorage so the accounting survives the reload
// itself). Network failures never reload: reloading into a dead
// server would trade a good stale page for a browser error.
(function () {
  if (!window.WebSocket || location.protocol !== 'http:') { return; }
  var MIN_FETCH_MS = __MIN_FETCH_MS__;
  var FAIL_MAX = __FAIL_MAX__;
  var MIN_RELOAD_MS = __MIN_RELOAD_MS__;
  var WINDOW_MS = __WINDOW_MS__;
  var BREAKER_MAX = __BREAKER_MAX__;
  var POLL_MS = __POLL_MS__;
  var OFF = 'fracker:push-off';
  var LAST = 'fracker:push-last';
  var TIMES = 'fracker:push-times';
  var gen = document.body.getAttribute('data-generation') || '';
  var failures = 0;
  var fetching = false;
  var queued = false;
  var fetchTimer = null;
  var lastFetch = 0;
  var ws = null;
  var reconnectTimer = null;
  function read(k) {
    try { return sessionStorage.getItem(k) || ''; } catch (e) { return ''; }
  }
  function write(k, v) {
    try { sessionStorage.setItem(k, v); } catch (e) {}
  }
  function reloadOff() { return read(OFF) === '1'; }
  function recent(now) {
    var a;
    try { a = JSON.parse(read(TIMES) || '[]'); } catch (e) { a = []; }
    if (!Array.isArray(a)) { a = []; }
    return a.filter(function (t) {
      return typeof t === 'number' && now - t < WINDOW_MS;
    });
  }
  // The last-resort reload predicate (push::PushGuard's failure
  // path): the failure budget must be spent AND the session breaker
  // must allow one more navigation inside the window.
  function lastResort(now) {
    if (reloadOff()) { return false; }
    var last = Number(read(LAST) || '0');
    if (last && now - last < MIN_RELOAD_MS) { return false; }
    var times = recent(now);
    if (times.length >= BREAKER_MAX) { write(OFF, '1'); return false; }
    times.push(now);
    write(TIMES, JSON.stringify(times));
    write(LAST, String(now));
    return true;
  }
  function fragEl(id) {
    if (id === 'header') { return document.querySelector('header.top'); }
    return document.getElementById(id);
  }
  function resync() {
    if (window.__frackerResync) {
      try { window.__frackerResync(); } catch (e) {}
    }
  }
  function applyPayload(p) {
    (p.fragments || []).forEach(function (f) {
      var el = fragEl(f.id);
      if (el && typeof f.html === 'string') { el.innerHTML = f.html; }
    });
    if (typeof p.day === 'string') {
      document.body.setAttribute('data-day', p.day);
    }
    if (typeof p.ticks === 'string') {
      document.body.setAttribute('data-fracker-ticks', p.ticks);
    }
    gen = p.generation;
    document.body.setAttribute('data-generation', gen);
    resync();
  }
  function sync() {
    if (fetching) { queued = true; return; }
    var wait = MIN_FETCH_MS - (Date.now() - lastFetch);
    if (wait > 0) {
      if (!fetchTimer) {
        fetchTimer = setTimeout(function () {
          fetchTimer = null;
          sync();
        }, wait);
      }
      return;
    }
    fetching = true;
    lastFetch = Date.now();
    var answered = false;
    fetch('/fragments?gen=' + encodeURIComponent(gen))
      .then(function (r) {
        answered = true;
        if (!r.ok) { throw new Error('http ' + r.status); }
        return r.json();
      })
      .then(function (p) {
        failures = 0;
        if (p && typeof p.generation === 'string' && p.generation) {
          if (p.generation !== gen) {
            applyPayload(p);
          } else if (typeof p.ticks === 'string') {
            // Same generation: still adopt the tick payload, so a
            // tick posted from another page lands without a regen.
            document.body.setAttribute('data-fracker-ticks', p.ticks);
            resync();
          }
        }
      })
      .catch(function (e) {
        // Only an answered-but-wrong server counts toward the last
        // resort; a network failure just waits for the next poll.
        if (answered) {
          failures += 1;
          if (failures >= FAIL_MAX && lastResort(Date.now())) {
            location.reload();
          }
        }
      })
      .then(function () {
        fetching = false;
        if (queued) { queued = false; sync(); }
      });
  }
  function reconnect() {
    if (reconnectTimer) { return; }
    reconnectTimer = setTimeout(function () {
      reconnectTimer = null;
      connect();
    }, 2000);
  }
  function connect() {
    try { ws = new WebSocket('ws://' + location.host + '/ws'); }
    catch (e) { reconnect(); return; }
    ws.onmessage = function (ev) {
      var msg;
      try { msg = JSON.parse(ev.data); } catch (e) { return; }
      if (msg && typeof msg.generation === 'string' && msg.generation !== gen) {
        sync();
      }
    };
    ws.onclose = reconnect;
    ws.onerror = function () { try { ws.close(); } catch (e) {} };
  }
  connect();
  setInterval(sync, POLL_MS);
})();
"#;
    TEMPLATE
        .replace("__MIN_FETCH_MS__", &MIN_FETCH_INTERVAL_MS.to_string())
        .replace("__FAIL_MAX__", &MAX_FETCH_FAILURES.to_string())
        .replace(
            "__MIN_RELOAD_MS__",
            &MIN_RELOAD_INTERVAL_MS.to_string(),
        )
        .replace("__WINDOW_MS__", &BREAKER_WINDOW_MS.to_string())
        .replace("__BREAKER_MAX__", &BREAKER_MAX_RELOADS.to_string())
        .replace("__POLL_MS__", &POLL_INTERVAL_MS.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The connect-time hello naming the page's own generation is a
    /// no-op — never a fetch.
    #[test]
    fn equal_generation_on_hello_is_a_no_op() {
        let mut guard = PushGuard::new("gen-1");
        assert_eq!(guard.observe("gen-1", 1_000), PushAction::NoChange);
        assert_eq!(guard.observe("gen-1", 90_000), PushAction::NoChange);
        assert!(!guard.disabled());
    }

    /// A genuinely different generation fetches the fragment diff;
    /// once the payload is applied, the same generation is a no-op.
    #[test]
    fn differing_generation_fetches_until_applied() {
        let mut guard = PushGuard::new("gen-1");
        assert_eq!(guard.observe("gen-2", 1_000), PushAction::Fetch);
        guard.observe_applied("gen-2");
        assert_eq!(guard.loaded(), "gen-2");
        assert_eq!(guard.observe("gen-2", 5_000), PushAction::NoChange);
    }

    /// Fetch decisions are floored: a burst of differing pushes
    /// inside the interval coalesces instead of fetching per push.
    #[test]
    fn fetch_decisions_are_rate_limited() {
        let mut guard = PushGuard::new("gen-1");
        assert_eq!(guard.observe("gen-2", 1_000), PushAction::Fetch);
        assert_eq!(
            guard.observe("gen-3", 1_000 + MIN_FETCH_INTERVAL_MS - 1),
            PushAction::Suppressed
        );
        assert_eq!(
            guard.observe("gen-3", 1_000 + MIN_FETCH_INTERVAL_MS),
            PushAction::Fetch
        );
    }

    /// Failures below the cap never navigate: the page keeps its
    /// (stale but good) content and retries on the next push/poll.
    #[test]
    fn failures_below_the_cap_never_reload() {
        let mut guard = PushGuard::new("gen-1");
        for i in 0..MAX_FETCH_FAILURES - 1 {
            assert_eq!(
                guard.observe_fetch_failure(10_000 + i as u64 * 1_000),
                PushAction::Suppressed,
                "failure {i} is below the cap"
            );
        }
        assert!(!guard.disabled());
    }

    /// At the failure cap the last-resort reload fires once, and the
    /// failure budget resets behind it.
    #[test]
    fn failure_cap_reloads_once() {
        let mut guard = PushGuard::new("gen-1");
        let mut now = 10_000;
        for _ in 0..MAX_FETCH_FAILURES - 1 {
            assert_eq!(guard.observe_fetch_failure(now), PushAction::Suppressed);
            now += 1_000;
        }
        assert_eq!(guard.observe_fetch_failure(now), PushAction::Reload);
        // Budget spent and reset: another full cap of failures is
        // needed before a second reload is even considered.
        now += MIN_RELOAD_INTERVAL_MS;
        for _ in 0..MAX_FETCH_FAILURES - 1 {
            assert_eq!(guard.observe_fetch_failure(now), PushAction::Suppressed);
            now += 1_000;
        }
        assert_eq!(guard.observe_fetch_failure(now), PushAction::Reload);
    }

    /// Reloads are spaced, and crossing the window cap disables
    /// last-resort reloading for the session (fragment sync itself
    /// is unaffected — only the navigation is gone).
    #[test]
    fn breaker_stops_reloads_after_the_cap() {
        let mut guard = PushGuard::new("gen-0");
        let mut now = 1_000;
        for n in 0..BREAKER_MAX_RELOADS {
            for _ in 0..MAX_FETCH_FAILURES - 1 {
                assert_eq!(guard.observe_fetch_failure(now), PushAction::Suppressed);
                now += 100;
            }
            assert_eq!(
                guard.observe_fetch_failure(now),
                PushAction::Reload,
                "reload {n} is inside the cap"
            );
            now += MIN_RELOAD_INTERVAL_MS;
        }
        // The next cap of failures inside the window trips the
        // breaker instead of reloading.
        for _ in 0..MAX_FETCH_FAILURES - 1 {
            assert_eq!(guard.observe_fetch_failure(now), PushAction::Suppressed);
            now += 100;
        }
        assert_eq!(guard.observe_fetch_failure(now), PushAction::Disabled);
        assert!(guard.disabled());
        // Later failures stay inert, but generation pushes still
        // fetch fragments.
        assert_eq!(
            guard.observe_fetch_failure(now + 60_000),
            PushAction::Disabled
        );
        assert_eq!(guard.observe("gen-new", now + 60_000), PushAction::Fetch);
    }

    /// Reloads older than the window do not count against the
    /// breaker.
    #[test]
    fn breaker_window_slides() {
        let mut guard = PushGuard::new("gen-0");
        let mut now = 1_000;
        for _ in 0..BREAKER_MAX_RELOADS {
            for _ in 0..MAX_FETCH_FAILURES - 1 {
                assert_eq!(guard.observe_fetch_failure(now), PushAction::Suppressed);
                now += 100;
            }
            assert_eq!(guard.observe_fetch_failure(now), PushAction::Reload);
            now += MIN_RELOAD_INTERVAL_MS;
        }
        // Far outside the window: the old reloads have aged out.
        now += BREAKER_WINDOW_MS;
        for _ in 0..MAX_FETCH_FAILURES - 1 {
            assert_eq!(guard.observe_fetch_failure(now), PushAction::Suppressed);
            now += 100;
        }
        assert_eq!(guard.observe_fetch_failure(now), PushAction::Reload);
        assert!(!guard.disabled());
    }

    /// The embedded script carries the same policy, so the shipped
    /// page cannot drift from the tested guard.
    #[test]
    fn client_script_mirrors_the_guard() {
        let js = client_script();
        assert!(js.contains("msg.generation !== gen"), "{js}");
        assert!(js.contains("/fragments?gen="), "a push fetches fragments: {js}");
        assert!(js.contains("innerHTML = f.html"), "fragments swap in place: {js}");
        assert!(
            js.contains("location.reload()"),
            "the last-resort reload survives: {js}"
        );
        assert!(
            js.contains(&format!("var MIN_FETCH_MS = {MIN_FETCH_INTERVAL_MS};")),
            "{js}"
        );
        assert!(
            js.contains(&format!("var FAIL_MAX = {MAX_FETCH_FAILURES};")),
            "{js}"
        );
        assert!(
            js.contains(&format!("var MIN_RELOAD_MS = {MIN_RELOAD_INTERVAL_MS};")),
            "{js}"
        );
        assert!(
            js.contains(&format!("var WINDOW_MS = {BREAKER_WINDOW_MS};")),
            "{js}"
        );
        assert!(
            js.contains(&format!("var BREAKER_MAX = {BREAKER_MAX_RELOADS};")),
            "{js}"
        );
        assert!(js.contains(&format!("var POLL_MS = {POLL_INTERVAL_MS};")), "{js}");
        assert!(js.contains("setInterval(sync, POLL_MS)"), "the poll converges: {js}");
        assert!(js.contains("__frackerResync"), "swaps re-apply page state: {js}");
        assert!(js.contains("sessionStorage"), "breaker survives reloads");
        assert!(js.contains("fracker:push-off"), "session-scoped disable");
        assert!(!js.contains("__MIN_FETCH_MS__"), "tokens are substituted");
        assert!(!js.contains("__FAIL_MAX__"), "tokens are substituted");
        assert!(!js.contains("__POLL_MS__"), "tokens are substituted");
        assert!(
            !js.contains("http-equiv"),
            "the script carries no meta-refresh: {js}"
        );
    }
}
