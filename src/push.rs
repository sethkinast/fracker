//! Client-side WebSocket reload guard (nineteenth pass §1/§2).
//!
//! Item 58 lets the served plan page reload itself the moment the
//! resident publishes a new generation. The hotfix makes that loop-proof
//! at two levels:
//!
//! - the predicate reloads ONLY when the pushed generation differs from
//!   the generation embedded in the loaded page (`data-generation`), so
//!   the connect-time hello is a no-op when they match;
//! - even a server that keeps reporting different generations cannot
//!   create a storm: reloads are rate-limited to at most one per
//!   [`MIN_RELOAD_INTERVAL_MS`], and after [`BREAKER_MAX_RELOADS`] inside
//!   [`BREAKER_WINDOW_MS`] the page switches the breaker off and relies on
//!   the 60s meta-refresh for the rest of the browser session.
//!
//! The policy is factored here as [`PushGuard`] so it is unit-testable
//! without a browser. [`client_script`] is the JavaScript mirror embedded
//! in the page — keep the two in step.

/// A push-triggered reload is never closer than this to the previous one.
pub const MIN_RELOAD_INTERVAL_MS: u64 = 5_000;

/// The sliding window the circuit breaker counts reloads in.
pub const BREAKER_WINDOW_MS: u64 = 30_000;

/// Push-triggered reloads allowed inside one window before the breaker
/// trips.
pub const BREAKER_MAX_RELOADS: usize = 3;

/// What a pushed generation means for the loaded page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushAction {
    /// The pushed generation equals the page's own generation: no-op.
    NoChange,
    /// A different generation arrived, but a safeguard (minimum interval
    /// or the already-tripped breaker) suppressed the reload. The 60s
    /// meta-refresh still converges.
    Suppressed,
    /// Reload exactly once, now.
    Reload,
    /// This would have crossed the breaker: WS reloading is disabled for
    /// the rest of the session and the meta-refresh is the only path.
    Disabled,
}

/// The client predicate + failure guards, in Rust.
#[derive(Debug, Clone)]
pub struct PushGuard {
    loaded: String,
    last_reload_ms: Option<u64>,
    reloads_ms: Vec<u64>,
    disabled: bool,
}

impl PushGuard {
    /// A guard for a page loaded with `generation`.
    pub fn new(generation: impl Into<String>) -> Self {
        Self {
            loaded: generation.into(),
            last_reload_ms: None,
            reloads_ms: Vec::new(),
            disabled: false,
        }
    }

    /// The generation embedded in the currently loaded page.
    pub fn loaded(&self) -> &str {
        &self.loaded
    }

    /// True once the circuit breaker has dropped WS reloading.
    pub fn disabled(&self) -> bool {
        self.disabled
    }

    /// Decide what to do with a pushed generation observed at `now_ms`.
    ///
    /// Equal generation is always a no-op. A differing one reloads only
    /// when it is at least [`MIN_RELOAD_INTERVAL_MS`] since the previous
    /// reload and fewer than [`BREAKER_MAX_RELOADS`] have already landed
    /// inside [`BREAKER_WINDOW_MS`]; otherwise it is suppressed, and
    /// crossing the cap disables pushes for the session.
    pub fn observe(&mut self, incoming: &str, now_ms: u64) -> PushAction {
        if self.disabled {
            return PushAction::Disabled;
        }
        if incoming == self.loaded {
            return PushAction::NoChange;
        }
        if let Some(last) = self.last_reload_ms {
            if now_ms.saturating_sub(last) < MIN_RELOAD_INTERVAL_MS {
                return PushAction::Suppressed;
            }
        }
        self.reloads_ms
            .retain(|t| now_ms.saturating_sub(*t) < BREAKER_WINDOW_MS);
        if self.reloads_ms.len() >= BREAKER_MAX_RELOADS {
            self.disabled = true;
            return PushAction::Disabled;
        }
        self.reloads_ms.push(now_ms);
        self.last_reload_ms = Some(now_ms);
        // The navigation the caller is about to perform lands on this
        // generation; a duplicate frame for it must not reload again.
        self.loaded = incoming.to_owned();
        PushAction::Reload
    }
}

/// The page script for the guard. The tokens are substituted so the
/// browser and [`PushGuard`] share one set of tuning values.
pub fn client_script() -> String {
    const TEMPLATE: &str = r#"// Live plan push (item 58, hotfix 19 §1/§2): the loopback server
// pushes the current generation over WebSocket the moment a regeneration
// lands. Reload ONLY when that generation differs from the one this page
// was loaded with; the connect-time hello naming an equal generation is a
// no-op. Even a misbehaving server cannot storm: reloads are at least
// MIN_MS apart, and BREAKER_MAX inside BREAKER_WINDOW_MS disables WS
// reloading for the session (sessionStorage), leaving the 60s meta-refresh
// as the only path. A dropped socket reconnects; the next hello catches a
// page up after a missed push.
(function () {
  if (!window.WebSocket || location.protocol !== 'http:') { return; }
  var MIN_MS = __MIN_MS__;
  var WINDOW_MS = __WINDOW_MS__;
  var BREAKER_MAX = __BREAKER_MAX__;
  var OFF = 'fracker:push-off';
  var LAST = 'fracker:push-last';
  var TIMES = 'fracker:push-times';
  var gen = document.body.getAttribute('data-generation') || '';
  var timer = null;
  var ws = null;
  function read(k) {
    try { return sessionStorage.getItem(k) || ''; } catch (e) { return ''; }
  }
  function write(k, v) {
    try { sessionStorage.setItem(k, v); } catch (e) {}
  }
  function broken() { return read(OFF) === '1'; }
  function disable() {
    write(OFF, '1');
    if (timer) { clearTimeout(timer); timer = null; }
    if (ws) { try { ws.close(); } catch (e) {} }
  }
  function recent(now) {
    var a;
    try { a = JSON.parse(read(TIMES) || '[]'); } catch (e) { a = []; }
    if (!Array.isArray(a)) { a = []; }
    return a.filter(function (t) {
      return typeof t === 'number' && now - t < WINDOW_MS;
    });
  }
  // The predicate: equal is a no-op; differing is suppressed when it
  // arrives inside the minimum interval or after the breaker cap.
  function shouldReload(incoming, now) {
    if (incoming === gen || broken()) { return false; }
    var last = Number(read(LAST) || '0');
    if (last && now - last < MIN_MS) { return false; }
    var times = recent(now);
    if (times.length >= BREAKER_MAX) { disable(); return false; }
    times.push(now);
    write(TIMES, JSON.stringify(times));
    write(LAST, String(now));
    return true;
  }
  function reconnect() {
    if (timer || broken()) { return; }
    timer = setTimeout(function () { timer = null; connect(); }, 2000);
  }
  function connect() {
    if (broken()) { return; }
    try { ws = new WebSocket('ws://' + location.host + '/ws'); }
    catch (e) { reconnect(); return; }
    ws.onmessage = function (ev) {
      var msg;
      try { msg = JSON.parse(ev.data); } catch (e) { return; }
      if (msg && typeof msg.generation === 'string' && msg.generation !== gen) {
        if (shouldReload(msg.generation, Date.now())) { location.reload(); }
      }
    };
    ws.onclose = reconnect;
    ws.onerror = function () { try { ws.close(); } catch (e) {} };
  }
  connect();
})();
"#;
    TEMPLATE
        .replace("__MIN_MS__", &MIN_RELOAD_INTERVAL_MS.to_string())
        .replace("__WINDOW_MS__", &BREAKER_WINDOW_MS.to_string())
        .replace("__BREAKER_MAX__", &BREAKER_MAX_RELOADS.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §3: the connect-time hello naming the page's own generation is a
    /// no-op — never a reload.
    #[test]
    fn equal_generation_on_hello_does_not_reload() {
        let mut guard = PushGuard::new("gen-1");
        assert_eq!(guard.observe("gen-1", 1_000), PushAction::NoChange);
        // Still a no-op later, and it does not consume the breaker budget.
        assert_eq!(guard.observe("gen-1", 90_000), PushAction::NoChange);
        assert!(!guard.disabled());
    }

    /// §3: a genuinely different generation reloads exactly once, even if
    /// the same push frame is delivered again before the navigation.
    #[test]
    fn differing_generation_reloads_exactly_once() {
        let mut guard = PushGuard::new("gen-1");
        assert_eq!(guard.observe("gen-2", 1_000), PushAction::Reload);
        assert_eq!(guard.loaded(), "gen-2");
        assert_eq!(guard.observe("gen-2", 1_001), PushAction::NoChange);
    }

    /// §2: reloads are at least one interval apart.
    #[test]
    fn push_reloads_are_rate_limited() {
        let mut guard = PushGuard::new("gen-1");
        assert_eq!(guard.observe("gen-2", 1_000), PushAction::Reload);
        assert_eq!(
            guard.observe("gen-3", 1_000 + MIN_RELOAD_INTERVAL_MS - 1),
            PushAction::Suppressed
        );
        assert_eq!(
            guard.observe("gen-3", 1_000 + MIN_RELOAD_INTERVAL_MS),
            PushAction::Reload
        );
    }

    /// §3: repeated differing pushes inside the window stop reloading
    /// once the cap is crossed, and the breaker stays off.
    #[test]
    fn breaker_stops_reloads_after_the_cap() {
        let mut guard = PushGuard::new("gen-0");
        let mut now = 1_000;
        for n in 1..=BREAKER_MAX_RELOADS {
            let incoming = format!("gen-{n}");
            assert_eq!(
                guard.observe(&incoming, now),
                PushAction::Reload,
                "reload {n} is inside the cap"
            );
            now += MIN_RELOAD_INTERVAL_MS;
        }
        // The next differing push inside the window trips the breaker
        // instead of reloading.
        assert_eq!(guard.observe("gen-cap", now), PushAction::Disabled);
        assert!(guard.disabled());
        // Every later push is inert for the session.
        assert_eq!(
            guard.observe("gen-later", now + MIN_RELOAD_INTERVAL_MS),
            PushAction::Disabled
        );
        assert_eq!(
            guard.observe("gen-later-still", now + 2 * MIN_RELOAD_INTERVAL_MS),
            PushAction::Disabled
        );
    }

    /// Reloads older than the window do not count against the breaker.
    #[test]
    fn breaker_window_slides() {
        let mut guard = PushGuard::new("gen-0");
        let mut now = 1_000;
        for n in 1..=BREAKER_MAX_RELOADS {
            assert_eq!(guard.observe(&format!("gen-{n}"), now), PushAction::Reload);
            now += MIN_RELOAD_INTERVAL_MS;
        }
        // Far outside the window: the old reloads have aged out.
        now += BREAKER_WINDOW_MS;
        assert_eq!(guard.observe("gen-new", now), PushAction::Reload);
        assert!(!guard.disabled());
    }

    /// The embedded script carries the same policy + the meta-refresh
    /// fallback, so the shipped page cannot drift from the tested guard.
    #[test]
    fn client_script_mirrors_the_guard() {
        let js = client_script();
        assert!(js.contains("msg.generation !== gen"), "{js}");
        assert!(js.contains("location.reload()"), "{js}");
        assert!(
            js.contains(&format!("var MIN_MS = {MIN_RELOAD_INTERVAL_MS};")),
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
        assert!(js.contains("sessionStorage"), "breaker survives reloads");
        assert!(js.contains("fracker:push-off"), "session-scoped disable");
        assert!(!js.contains("__MIN_MS__"), "tokens are substituted");
    }
}
