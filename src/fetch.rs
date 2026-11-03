//! HTTP fetchers for the §6 sources, each with on-disk cache + TTL.
//! All fetches fail-soft to cache. HTTP via `reqwest` (rustls).
//!
//! TTLs: DE worldstate + profile 15 min (addendum #120), Xuerian
//! `/api/load` 1 h, relic/drop data 7 days, WFCD items dataset 7 days.
//!
//! Worldstate formerly had NO TTL short-circuit (addendum #51): DE's
//! feed is the only source, and freshness was judged by the payload's
//! own `Time` at render, never by how recently we fetched. That rule
//! is superseded for the fetch itself by the 15-minute disk-cache TTL
//! (addendum #120): a TTL hit still renders its true age, and a
//! failed fetch may serve the disk cache, but the report labels that
//! view STALE — STALE stays reserved for the failure path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::model::{OwnedEquipment, RelicTables, Roster, Targets};

/// DE's own worldstate feed (same host family as the profile endpoint).
pub const WORLD_STATE_URL: &str = "https://api.warframe.com/cdn/worldState.php";
pub const PROFILE_URL: &str =
    "https://api.warframe.com/cdn/getProfileViewingData.php";
pub const XUERIAN_LOAD_URL: &str = "https://wf.xuerian.net/api/load";
pub const XUERIAN_RELICTABLES_URL: &str = "https://wf.xuerian.net/json/relictables.json";
pub const XUERIAN_WISHLIST_URL: &str = "https://wf.xuerian.net/json/wishlist.json";
/// WFCD items dataset (uniqueName -> display name, imageName, ...).
pub const WFCD_ITEMS_URL: &str = "https://api.warframestat.us/items";
/// warframestat.us worldstate proxy (fifty-fifth pass): tried
/// before DE for worldstate only — it serves no profiles.
pub const WFS_WORLDSTATE_URL: &str = "https://api.warframestat.us/pc";
/// warframestat.us cache (fifty-fifth pass): 5 minutes bounds the
/// proxy to 12 requests/hour regardless of regen count.
pub const TTL_WFS: Duration = Duration::from_secs(5 * 60);
/// warframestat.us payload freshness (fifty-fifth pass, Seth's
/// starting value — "we'll iterate"): a proxy payload whose own
/// timestamp is older than this is stale, never served, and the
/// generation falls through to DE. The proxy sometimes holds a
/// response for hours; this bar catches that.
pub const WFS_STALE_AFTER: Duration = Duration::from_secs(15 * 60);

pub const TTL_XUERIAN: Duration = Duration::from_secs(60 * 60);
pub const TTL_STATIC_DATA: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// DE endpoints (addendum #120): Seth's IP was rate-limit-blocked
/// after his heaviest-ever mission day with the resident running,
/// and the forum consensus is these endpoints are heavily
/// rate-limited. 15 minutes bounds DE traffic to 4 requests per
/// endpoint per hour regardless of regen count, down from 2 per
/// regen unbounded. Standing / focus remaining may now be up to
/// 15 minutes stale (accepted by Seth); the report dates a cached
/// snapshot by its true age, so nothing hides the staleness.
pub const TTL_DE: Duration = Duration::from_secs(15 * 60);

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// How a fetch resolved. The page/tray surface these as labels
/// ("Xuerian token not set — targets may go stale").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchStatus {
    /// Live response, cache refreshed.
    Fresh,
    /// Served from the disk cache under the TTL (addendum #120),
    /// with NO network request. Distinct from CacheFallback: this
    /// is a healthy hit, not a failure, so its label carries the
    /// cache age in minutes rather than an error reason — and the
    /// worldstate label renders it as "cached", never STALE.
    TtlCache { age: Duration },
    /// Network failed; serving the on-disk cache.
    CacheFallback { reason: String },
    /// No token / no cache: serving the committed seed data.
    SeedFallback { reason: String },
}

impl FetchStatus {
    pub fn label(&self) -> String {
        match self {
            FetchStatus::Fresh => "live".to_owned(),
            FetchStatus::TtlCache { age } => {
                format!("cached ({}m)", age.as_secs() / 60)
            }
            FetchStatus::CacheFallback { reason } => {
                format!("cached ({reason})")
            }
            FetchStatus::SeedFallback { reason } => format!("seed ({reason})"),
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, FetchStatus::Fresh)
    }
}

#[derive(Debug, Clone)]
pub struct Fetched<T> {
    pub value: T,
    pub status: FetchStatus,
}

// ---------------------------------------------------------------------------
// Cache helpers
// ---------------------------------------------------------------------------

pub fn cache_file(cache_dir: &Path, name: &str) -> PathBuf {
    cache_dir.join(name)
}

pub fn is_fresh(mtime: SystemTime, ttl: Duration, now: SystemTime) -> bool {
    now.duration_since(mtime).map(|age| age < ttl).unwrap_or(false)
}

pub fn cache_age(path: &Path) -> Option<Duration> {
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(mtime).ok()
}

fn read_cache(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Cache write that reports failure (fifty-seventh pass). The old
/// `write_cache` swallowed every failure — a broken cache dir then
/// silently uncapped live fetching, because the TTL gates above
/// only work when a good payload actually lands on disk. Every
/// caller now sees the error; the in-scope callers (worldstate,
/// profile, proxy, lock) latch the fetch hold on it, while the
/// out-of-scope static-data callers deliberately ignore it
/// (`let _ =`), exactly as before.
fn try_write_cache(cache_dir: &Path, name: &str, body: &str) -> Result<(), String> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| format!("create cache dir {}: {e}", cache_dir.display()))?;
    let path = cache_file(cache_dir, name);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), path.display()))?;
    Ok(())
}

/// A TTL cache hit (addendum #120): the cache file is younger than
/// `ttl` at `now` AND its contents pass `validate`. Returns the text
/// and its age. Anything else — missing, stale, future-mtime,
/// unreadable, unparseable — is None, and the caller falls through
/// to a live fetch. A corrupt cache is never served as a TTL hit;
/// on the failure path the caller may still serve it raw, exactly
/// as before, but that is CacheFallback, not this.
fn ttl_hit(
    path: &Path,
    ttl: Duration,
    now: SystemTime,
    validate: impl Fn(&str) -> bool,
) -> Option<(String, Duration)> {
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    let age = now.duration_since(mtime).ok()?;
    if age >= ttl {
        return None;
    }
    let text = read_cache(path)?;
    if !validate(&text) {
        return None;
    }
    Some((text, age))
}

/// A worldstate payload is TTL-worthy iff it is valid JSON carrying
/// a `Time` — the same bar the live path applies before caching.
fn valid_worldstate(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .map(|v| v.get("Time").is_some())
        .unwrap_or(false)
}

/// The User-Agent on every outbound fetch. Derived from the package
/// version at build time (fifty-fifth pass) so it never goes stale
/// again — it previously said fracker/0.1 while the app was 0.5.49.
pub fn user_agent() -> String {
    format!(
        "fracker/{} (warframe session planner)",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(user_agent())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))
}

// ---------------------------------------------------------------------------
// DE block lockfile (fifty-fifth pass)
// ---------------------------------------------------------------------------

/// One file blocks BOTH DE endpoints — they share the IP budget, so
/// a 403/429 on either is a block on both. A 403/429 from
/// warframestat.us never writes it. Contents: endpoint, status,
/// observed time as unix seconds, one per line.
pub const DE_LOCK_NAME: &str = "de-block.lock";
/// A lock older than this is ignored as if absent.
pub const DE_LOCK_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeLock {
    pub endpoint: String,
    pub status: u16,
    pub observed: SystemTime,
}

pub fn de_lock_path(cache_dir: &Path) -> PathBuf {
    cache_file(cache_dir, DE_LOCK_NAME)
}

pub fn write_de_lock(
    cache_dir: &Path,
    endpoint: &str,
    status: u16,
    now: SystemTime,
) -> Result<(), String> {
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = format!("{endpoint}\n{status}\n{secs}\n");
    try_write_cache(cache_dir, DE_LOCK_NAME, &body)
}

/// Read a live lockfile, if any. A lock older than 24h, or one whose
/// contents do not parse, is ignored AND deleted on sight — a stale
/// sentinel must never block traffic, and a corrupt one must not
/// block it either (writes are tmp+rename, so corruption means
/// something else went wrong; fail open, and the next generation's
/// normal fetch path is the loud signal). A lock dated in the
/// future (clock skew) counts as age zero — live, not expired.
pub fn read_de_lock(cache_dir: &Path, now: SystemTime) -> Option<DeLock> {
    let path = de_lock_path(cache_dir);
    let text = read_cache(&path)?;
    // Parse inside a closure so EVERY invalid outcome — short file,
    // bad status, bad timestamp — deletes the file on sight, not
    // just the expired/empty-endpoint cases below (the doc above
    // promises corrupt locks never block traffic).
    let parsed = (|| {
        let mut lines = text.lines();
        let endpoint = lines.next()?.to_owned();
        let status: u16 = lines.next()?.trim().parse().ok()?;
        let secs: u64 = lines.next()?.trim().parse().ok()?;
        let observed = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        Some((endpoint, status, observed))
    })();
    let Some((endpoint, status, observed)) = parsed else {
        let _ = std::fs::remove_file(&path);
        return None;
    };
    let age = now.duration_since(observed).unwrap_or(Duration::ZERO);
    if age >= DE_LOCK_TTL || endpoint.is_empty() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(DeLock {
        endpoint,
        status,
        observed,
    })
}

pub fn clear_de_lock(cache_dir: &Path) -> bool {
    std::fs::remove_file(de_lock_path(cache_dir)).is_ok()
}

// ---------------------------------------------------------------------------
// DE fetch hold (fifty-seventh pass: cache-write-failure failsafe)
// ---------------------------------------------------------------------------

/// In-memory hold, keyed per cache dir. Production uses exactly one
/// cache dir, so this is process-wide in practice; keying it per dir
/// (rather than one global flag) keeps tests isolated in a shared
/// test process, where a global latch from one test would poison
/// another test's live-fetch assertions. The map lives for the
/// process only: a restart clears the hold (accepted residual risk
/// for rapid restarts — nothing in-process can distinguish a legit
/// cold start from restart abuse).
struct FetchHold {
    /// Log lines waiting for the resident to record into
    /// `resident.log` (fetch.rs owns no logger; the report job
    /// drains these after each generation — see
    /// `take_fetch_hold_log` and resident.rs `spawn_report_job`).
    /// Stdout alone is invisible from the desktop tray, so the
    /// line must land in the file, not just on a console.
    pending_log: Vec<String>,
}

static FETCH_HOLDS: std::sync::LazyLock<Mutex<HashMap<PathBuf, FetchHold>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// True once a cache-write (or lock-write) failure has latched the
/// hold for this cache dir. Checked before any DE live fetch.
pub fn fetch_hold_latched(cache_dir: &Path) -> bool {
    FETCH_HOLDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(cache_dir)
}

/// Drain the pending hold log lines for this cache dir (empties the
/// queue). The resident records each line into `resident.log`.
pub fn take_fetch_hold_log(cache_dir: &Path) -> Vec<String> {
    let mut holds = FETCH_HOLDS.lock().unwrap_or_else(|e| e.into_inner());
    holds
        .get_mut(cache_dir)
        .map(|hold| std::mem::take(&mut hold.pending_log))
        .unwrap_or_default()
}

/// Latch the hold for this cache dir, exactly once per process
/// lifetime: later failures for the same dir change nothing and
/// attempt no further lock write. On latch, one `de-block.lock`
/// write is attempted through the same write path — it only lands
/// when the failure is file-specific rather than directory-wide,
/// but when it lands it protects restarts too and raises the
/// existing header alert. A lock-write failure during that attempt
/// is already covered: the hold is latched regardless, and the
/// outcome is recorded in the pending log line. The lock's status
/// is 0 — not an HTTP block, so no HTTP status exists to record;
/// the endpoint names the trigger ("worldstate", "profile",
/// "proxy", or "lock").
fn latch_fetch_hold(cache_dir: &Path, reason: String, lock_endpoint: &str, now: SystemTime) {
    {
        let mut holds = FETCH_HOLDS.lock().unwrap_or_else(|e| e.into_inner());
        if holds.contains_key(cache_dir) {
            return;
        }
        holds.insert(
            cache_dir.to_path_buf(),
            FetchHold {
                pending_log: Vec::new(),
            },
        );
    }
    let lock_outcome = match write_de_lock(cache_dir, lock_endpoint, 0, now) {
        Ok(()) => "de-block.lock written".to_owned(),
        Err(e) => format!("de-block.lock write also failed ({e})"),
    };
    let mut holds = FETCH_HOLDS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hold) = holds.get_mut(cache_dir) {
        hold.pending_log.push(format!(
            "DE fetch hold latched: {reason}; {lock_outcome}"
        ));
    }
}

// ---------------------------------------------------------------------------
// World state
// ---------------------------------------------------------------------------

/// Fetch DE's raw worldstate payload. Returns the body (the caller parses
/// it with the baked decodes) and its fetch status. A disk cache
/// younger than TTL_DE that parses (valid JSON with a `Time`) is
/// served with NO network request (addendum #120); otherwise the
/// report asks DE live, and payload freshness is judged later from
/// the body's `Time` for the live branch. A network failure may fall
/// back to the disk cache, which the caller must label STALE.
pub async fn fetch_worldstate(
    client: &reqwest::Client,
    cache_dir: &Path,
) -> Result<Fetched<String>, String> {
    fetch_worldstate_at(client, cache_dir, SystemTime::now()).await
}

async fn fetch_worldstate_at(
    client: &reqwest::Client,
    cache_dir: &Path,
    now: SystemTime,
) -> Result<Fetched<String>, String> {
    fetch_worldstate_de_at(client, cache_dir, now, WORLD_STATE_URL).await
}

/// DE worldstate fetch with an overridable URL (tests point it at a
/// stub server; production always passes `WORLD_STATE_URL`).
async fn fetch_worldstate_de_at(
    client: &reqwest::Client,
    cache_dir: &Path,
    now: SystemTime,
    url: &str,
) -> Result<Fetched<String>, String> {
    let path = cache_file(cache_dir, "worldstate.json");
    // Fifty-fifth pass: a live DE block lock suppresses ALL DE
    // network traffic, TTL hits included — the stamp deliberately
    // downgrades to the failure rendering while DE is paused, so
    // nothing looks live, and the header alert (read separately by
    // report.rs) explains why. Any-age disk cache serves; without
    // one this errors exactly as the unreachable path does.
    // Fifty-seventh pass: the in-memory fetch hold gates exactly
    // like the lock, with its own reason — a held fetch makes no
    // network request and serves any-age cache as CacheFallback,
    // so the page renders STALE exactly as it does under the lock.
    let held_reason = if fetch_hold_latched(cache_dir) {
        Some("DE requests paused (fetch hold)")
    } else if read_de_lock(cache_dir, now).is_some() {
        Some("DE requests paused (lockfile)")
    } else {
        None
    };
    if let Some(reason) = held_reason {
        let cached = read_cache(&path).ok_or_else(|| {
            if reason.contains("fetch hold") {
                "worldstate paused (fetch hold), no cache".to_owned()
            } else {
                "worldstate paused (lockfile), no cache".to_owned()
            }
        })?;
        return Ok(Fetched {
            value: cached,
            status: FetchStatus::CacheFallback {
                reason: reason.to_owned(),
            },
        });
    }
    if let Some((text, age)) = ttl_hit(&path, TTL_DE, now, valid_worldstate) {
        return Ok(Fetched {
            value: text,
            status: FetchStatus::TtlCache { age },
        });
    }
    match client.get(url).send().await {
        Ok(resp) => {
            // Fifty-fifth pass: a DE 403/429 locks BOTH endpoints
            // (shared IP budget) and serves cache in this same
            // generation; the lockfile the report layer reads for
            // the header alert is already in place by render time.
            let status = resp.status();
            if status == reqwest::StatusCode::FORBIDDEN
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            {
                // Fifty-seventh pass: a lock-write failure itself
                // latches the hold — the block must not silently
                // uncap the next generation.
                if let Err(e) = write_de_lock(cache_dir, "worldstate", status.as_u16(), now) {
                    latch_fetch_hold(
                        cache_dir,
                        format!("de-block.lock write failed ({e})"),
                        "lock",
                        now,
                    );
                }
                let cached = read_cache(&path).ok_or_else(|| {
                    format!("worldstate blocked (HTTP {status}), no cache")
                })?;
                return Ok(Fetched {
                    value: cached,
                    status: FetchStatus::CacheFallback {
                        reason: format!("DE HTTP {status} (requests paused)"),
                    },
                });
            }
            let resp = resp
                .error_for_status()
                .map_err(|e| format!("worldstate: {e}"))?;
            let text = resp.text().await.map_err(|e| format!("worldstate: {e}"))?;
            // Only cache a real DE payload (its `Time` is the freshness
            // stamp). A proxy/error page must not masquerade as data.
            let v: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("worldstate (DE payload): {e}"))?;
            if v.get("Time").is_none() {
                return Err("worldstate (DE payload): missing Time".to_owned());
            }
            // Fifty-seventh pass: a failed cache write latches the
            // hold — this generation still serves its live payload,
            // but no further DE live fetch happens from this
            // process while the cache cannot persist it.
            if let Err(e) = try_write_cache(cache_dir, "worldstate.json", &text) {
                latch_fetch_hold(
                    cache_dir,
                    format!("worldstate cache write failed ({e})"),
                    "worldstate",
                    now,
                );
            }
            Ok(Fetched {
                value: text,
                status: FetchStatus::Fresh,
            })
        }
        Err(e) => {
            let path = cache_file(cache_dir, "worldstate.json");
            let cached = read_cache(&path)
                .ok_or_else(|| format!("worldstate unreachable, no cache: {e}"))?;
            Ok(Fetched {
                value: cached,
                status: FetchStatus::CacheFallback {
                    reason: e.to_string(),
                },
            })
        }
    }
}

// ---------------------------------------------------------------------------
// warframestat.us worldstate proxy (fifty-fifth pass)
// ---------------------------------------------------------------------------

/// Which origin served the worldstate text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldstateOrigin {
    Wfs,
    De,
}

pub struct ResolvedWorldstate {
    pub text: String,
    pub origin: WorldstateOrigin,
    pub status: FetchStatus,
}

/// The proxy payload's own timestamp, if present and parsable.
/// WFCD timestamps are RFC 3339 ("2026-10-28T20:49:56.000Z").
fn wfcd_timestamp(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let ts = v.get("timestamp")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Payload-timestamp freshness at `now`: present, not from the
/// future, and no older than `WFS_STALE_AFTER`.
fn wfcd_timestamp_fresh(text: &str, now: SystemTime) -> bool {
    let Some(stamp) = wfcd_timestamp(text) else {
        return false;
    };
    let now_dt: chrono::DateTime<chrono::Utc> = now.into();
    let age = now_dt.signed_duration_since(stamp).num_seconds();
    age >= 0 && age <= WFS_STALE_AFTER.as_secs() as i64
}

/// A proxy cache hit: file younger than `TTL_WFS` AND payload
/// timestamp fresh. Both bars, deliberately: a cache written fresh
/// can age past the payload bar inside the file window, and a
/// stale proxy cache is NEVER served — staleness is the reason to
/// fall through to DE, not a value to display.
fn wfs_ttl_hit(path: &Path, now: SystemTime) -> Option<(String, Duration)> {
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    let age = now.duration_since(mtime).ok()?;
    if age >= TTL_WFS {
        return None;
    }
    let text = read_cache(path)?;
    if !wfcd_timestamp_fresh(&text, now) {
        return None;
    }
    Some((text, age))
}

/// Worldstate resolution order (fifty-fifth pass):
/// (a) fresh proxy cache with a fresh payload timestamp — no
/// request; (b) live proxy attempt — ANY failure (network,
/// non-200, unparseable, missing timestamp, stale timestamp)
/// falls through, and a proxy 403/429 deliberately does NOT write
/// the DE lock; (c) the DE path exactly as pass 54 (15-min TTL,
/// lock gate, failure fallback).
pub async fn fetch_worldstate_resolved(
    client: &reqwest::Client,
    cache_dir: &Path,
) -> Result<ResolvedWorldstate, String> {
    fetch_worldstate_resolved_at(client, cache_dir, SystemTime::now()).await
}

async fn fetch_worldstate_resolved_at(
    client: &reqwest::Client,
    cache_dir: &Path,
    now: SystemTime,
) -> Result<ResolvedWorldstate, String> {
    fetch_worldstate_resolved_at_with_url(client, cache_dir, now, WFS_WORLDSTATE_URL).await
}

/// Resolution with an overridable proxy URL (tests point it at a
/// stub server; production always passes `WFS_WORLDSTATE_URL`).
async fn fetch_worldstate_resolved_at_with_url(
    client: &reqwest::Client,
    cache_dir: &Path,
    now: SystemTime,
    wfs_url: &str,
) -> Result<ResolvedWorldstate, String> {
    fetch_worldstate_resolved_at_with_urls(client, cache_dir, now, wfs_url, WORLD_STATE_URL)
        .await
}

/// Resolution with overridable proxy AND DE URLs (tests point both
/// at stub servers, so no test below touches the real endpoints;
/// production always passes the real URLs).
async fn fetch_worldstate_resolved_at_with_urls(
    client: &reqwest::Client,
    cache_dir: &Path,
    now: SystemTime,
    wfs_url: &str,
    de_url: &str,
) -> Result<ResolvedWorldstate, String> {
    let wfs_path = cache_file(cache_dir, "worldstate-wfs.json");
    if let Some((text, age)) = wfs_ttl_hit(&wfs_path, now) {
        return Ok(ResolvedWorldstate {
            text,
            origin: WorldstateOrigin::Wfs,
            status: FetchStatus::TtlCache { age },
        });
    }
    let wfs_live = match client.get(wfs_url).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.text().await {
                    Ok(text) if wfcd_timestamp_fresh(&text, now) => Some(text),
                    _ => None,
                }
            } else {
                None
            }
        }
        Err(_) => None,
    };
    if let Some(text) = wfs_live {
        // Fifty-seventh pass: a proxy cache-write failure latches
        // the DE hold (the proxy budget is separate, so proxy
        // attempts continue unchanged — only DE live fetches stop).
        if let Err(e) = try_write_cache(cache_dir, "worldstate-wfs.json", &text) {
            latch_fetch_hold(
                cache_dir,
                format!("proxy cache write failed ({e})"),
                "proxy",
                now,
            );
        }
        return Ok(ResolvedWorldstate {
            text,
            origin: WorldstateOrigin::Wfs,
            status: FetchStatus::Fresh,
        });
    }
    let fetched = fetch_worldstate_de_at(client, cache_dir, now, de_url).await?;
    Ok(ResolvedWorldstate {
        text: fetched.value,
        origin: WorldstateOrigin::De,
        status: fetched.status,
    })
}

// ---------------------------------------------------------------------------
// Public profile
// ---------------------------------------------------------------------------

/// Raw profile payload: `Results[0].LoadOutInventory.XPInfo[]` plus
/// standing/focus state (addendum #2: daily bars render from the
/// profile's REMAINING-cap fields, never estimated).
#[derive(Debug, Clone, Default)]
pub struct Affiliation {
    pub tag: String,
    pub standing: i64,
    pub title: i64,
}

/// Remaining daily caps straight from the profile response.
#[derive(Debug, Clone, Default)]
pub struct DailyCaps {
    /// `DailyAffiliation`: faction-pool standing REMAINING of the
    /// MR-scaled cap (see the tuning tables).
    pub affiliation_remaining: u32,
    /// `DailyFocus`: focus REMAINING of the MR-scaled cap (see the
    /// tuning tables).
    pub focus_remaining: u32,
    /// Per-family `DailyAffiliation*` remaining values (one per
    /// syndicate family; the family list lives in the tuning tables).
    pub per_family: std::collections::HashMap<String, u32>,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileSnapshot {
    /// Equipment uniqueNames (`ItemType`).
    pub item_types: Vec<String>,
    pub display_name: String,
    /// The response's own account id (`AccountId.$oid`).
    pub account_id: String,
    /// Mastery rank (`PlayerLevel`).
    pub mastery_rank: u32,
    pub affiliations: Vec<Affiliation>,
    pub daily: DailyCaps,
}

/// How a live profile fetch failed: a `Definitive` answer (409 —
/// the account does not exist) is a real error, never masked by the
/// cache; anything else falls back to the on-disk snapshot.
enum LiveFail {
    Definitive(String),
    Transient(String),
    /// DE 403/429 (fifty-fifth pass): a block, not a failure — the
    /// caller writes the lockfile and serves cache.
    Blocked(u16),
}

async fn fetch_profile_live(client: &reqwest::Client, url: &str) -> Result<String, LiveFail> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| LiveFail::Transient(format!("profile: {e}")))?;
    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(LiveFail::Blocked(status.as_u16()));
    }
    if status == reqwest::StatusCode::CONFLICT {
        return Err(LiveFail::Definitive(
            "profile: no such account (HTTP 409)".to_owned(),
        ));
    }
    let resp = resp
        .error_for_status()
        .map_err(|e| LiveFail::Transient(format!("profile: {e}")))?;
    resp.text()
        .await
        .map_err(|e| LiveFail::Transient(format!("profile: {e}")))
}

fn profile_cache_fallback(
    path: &Path,
    reason: String,
) -> Result<Fetched<ProfileSnapshot>, String> {
    let cached =
        read_cache(path).ok_or_else(|| format!("profile unreachable, no cache: {reason}"))?;
    let snap = parse_profile(&cached)?;
    Ok(Fetched {
        value: snap,
        status: FetchStatus::CacheFallback { reason },
    })
}

pub async fn fetch_profile(
    client: &reqwest::Client,
    cache_dir: &Path,
    account_id: &str,
) -> Result<Fetched<ProfileSnapshot>, String> {
    fetch_profile_at(client, cache_dir, account_id, SystemTime::now()).await
}

async fn fetch_profile_at(
    client: &reqwest::Client,
    cache_dir: &Path,
    account_id: &str,
    now: SystemTime,
) -> Result<Fetched<ProfileSnapshot>, String> {
    let url = format!("{PROFILE_URL}?playerId={account_id}");
    fetch_profile_url_at(client, cache_dir, account_id, now, &url).await
}

/// Profile fetch with an overridable URL (tests point it at a stub
/// server; production always passes the DE profile URL).
async fn fetch_profile_url_at(
    client: &reqwest::Client,
    cache_dir: &Path,
    account_id: &str,
    now: SystemTime,
    url: &str,
) -> Result<Fetched<ProfileSnapshot>, String> {
    let name = format!("profile-{account_id}.json");
    let path = cache_file(cache_dir, &name);
    // Fifty-fifth pass: same lock gate as worldstate — no DE network
    // traffic while a live lock holds; any-age cache serves, else
    // the existing failure path (report.rs profile_fallback).
    // Fifty-seventh pass: the fetch hold gates identically, with
    // its own reason — no request, CacheFallback from any-age
    // cache, else the same parse/no-cache error as the lock path.
    if fetch_hold_latched(cache_dir) {
        return profile_cache_fallback(
            &path,
            "DE requests paused (fetch hold)".to_owned(),
        );
    }
    if read_de_lock(cache_dir, now).is_some() {
        return profile_cache_fallback(
            &path,
            "DE requests paused (lockfile)".to_owned(),
        );
    }
    // Addendum #71 formerly kept this TTL-free: DE's endpoint already
    // caches (~5 min per its own Cache-Control), and a second
    // 30-minute cache only ever served pre-reset numbers after the
    // 00:00 UTC reset. Addendum #120 supersedes it with a 15-minute
    // TTL: a parseable cache younger than TTL_DE is served with no
    // network request, so standing / focus remaining may be up to
    // 15 minutes stale (accepted by Seth). The TTL check runs
    // before any network call, so it cannot mask a 409 or any other
    // error that was never observed; a live 409 still errors
    // outright below. The disk file remains the failure fallback,
    // and the report layer dates a cached snapshot by its true age.
    if let Some((text, age)) = ttl_hit(&path, TTL_DE, now, |t| parse_profile(t).is_ok()) {
        // The hit was validated, so this parse cannot fail; unwrap
        // safety is by construction, not by hope — but keep the
        // fall-through honest: an unexpected failure drops to live.
        if let Ok(snap) = parse_profile(&text) {
            return Ok(Fetched {
                value: snap,
                status: FetchStatus::TtlCache { age },
            });
        }
    }
    match fetch_profile_live(client, &url).await {
        Ok(text) => match parse_profile(&text) {
            Ok(snap) => {
                // Fifty-seventh pass: same latch as worldstate —
                // this generation still serves its live snapshot.
                if let Err(e) = try_write_cache(cache_dir, &name, &text) {
                    latch_fetch_hold(
                        cache_dir,
                        format!("profile cache write failed ({e})"),
                        "profile",
                        now,
                    );
                }
                Ok(Fetched {
                    value: snap,
                    status: FetchStatus::Fresh,
                })
            }
            Err(e) => profile_cache_fallback(&path, e),
        },
        Err(LiveFail::Definitive(e)) => Err(e),
        Err(LiveFail::Blocked(status)) => {
            // Fifty-fifth pass: a DE block locks BOTH endpoints.
            // Fifty-seventh pass: a lock-write failure latches the
            // hold, as in the worldstate 403/429 path.
            if let Err(e) = write_de_lock(cache_dir, "profile", status, now) {
                latch_fetch_hold(
                    cache_dir,
                    format!("de-block.lock write failed ({e})"),
                    "lock",
                    now,
                );
            }
            profile_cache_fallback(
                &path,
                format!("DE HTTP {status} (requests paused)"),
            )
        }
        Err(LiveFail::Transient(e)) => profile_cache_fallback(&path, e),
    }
}

/// Extract a non-negative integer from a JSON value tolerantly.
///
/// The daily-cap fields are the numbers the page shows verbatim, so a
/// field the endpoint encodes as anything other than a bare JSON `u64`
/// must not silently collapse to `0`. The old code used
/// `Value::as_u64().unwrap_or(0)`, which turns a float (`7823.0`) or a
/// numeric string (`"7823"`) into `0` — a partial "remaining" pool then
/// wrongly renders as *fully spent* (ninth pass §3: the app showed
/// Syndicates "0 left" while the profile said 7,823). Accept every
/// numeric shape the endpoint might emit (integer, signed integer,
/// float, numeric string); reject everything else rather than guess.
fn json_u32(v: &serde_json::Value) -> Option<u32> {
    let n: f64 = match v {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                u as f64
            } else if let Some(i) = n.as_i64() {
                i as f64
            } else {
                n.as_f64()?
            }
        }
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    // These are integer counts; round half away from zero and saturate.
    Some(n.round().min(u32::MAX as f64) as u32)
}

pub fn parse_profile(text: &str) -> Result<ProfileSnapshot, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("profile: {e}"))?;
    let first = v
        .get("Results")
        .and_then(|r| r.get(0))
        .ok_or_else(|| "profile: no Results[0]".to_owned())?;
    let display_name = first
        .get("DisplayName")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_owned();
    let account_id = first
        .get("AccountId")
        .and_then(|a| a.get("$oid"))
        .and_then(|o| o.as_str())
        .unwrap_or("")
        .to_owned();
    let mastery_rank = first
        .get("PlayerLevel")
        .and_then(json_u32)
        .unwrap_or(0);
    let xp = first
        .get("LoadOutInventory")
        .and_then(|l| l.get("XPInfo"))
        .and_then(|x| x.as_array())
        .ok_or_else(|| "profile: no XPInfo".to_owned())?;
    let item_types = xp
        .iter()
        .filter_map(|e| e.get("ItemType")?.as_str().map(str::to_owned))
        .collect();
    let affiliations = first
        .get("Affiliations")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .map(|e| Affiliation {
                    tag: e
                        .get("Tag")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_owned(),
                    standing: e.get("Standing").and_then(|s| s.as_i64()).unwrap_or(0),
                    title: e.get("Title").and_then(|t| t.as_i64()).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    let num = |key: &str| -> u32 { first.get(key).and_then(json_u32).unwrap_or(0) };
    let mut per_family = std::collections::HashMap::new();
    if let Some(obj) = v
        .get("Results")
        .and_then(|r| r.get(0))
        .and_then(|f| f.as_object())
    {
        for (k, val) in obj {
            if let Some(fam) = k.strip_prefix("DailyAffiliation") {
                if !fam.is_empty() {
                    if let Some(n) = json_u32(val) {
                        per_family.insert(fam.to_owned(), n);
                    }
                }
            }
        }
    }
    Ok(ProfileSnapshot {
        item_types,
        display_name,
        account_id,
        mastery_rank,
        affiliations,
        daily: DailyCaps {
            affiliation_remaining: num("DailyAffiliation"),
            focus_remaining: num("DailyFocus"),
            per_family,
        },
    })
}

/// Load the uniqueName -> display name map (`data/cache/item-names.json`,
/// built from the WFCD items dataset). Missing/unreadable = empty map:
/// name resolution degrades, never guesses.
pub fn load_item_names(
    data_dir: &Path,
) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(data_dir.join("cache/item-names.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Load the baked relic-texture class table
/// (`data/cache/relic-textures.json`: (era, tier) -> Public
/// Export textureLocation, pass 48; class-table shape,
/// pass 52). Missing/unreadable = empty table: relic icons
/// degrade to the monogram, never a guessed URL. Baked data
/// — refreshed per data/cache/README.md, never fetched at
/// runtime.
pub fn load_relic_textures(data_dir: &Path) -> crate::model::RelicTextureClasses {
    std::fs::read_to_string(data_dir.join("cache/relic-textures.json"))
        .ok()
        .and_then(|t| crate::model::load_relic_textures(&t).ok())
        .unwrap_or_default()
}

/// Resolve profile XPInfo uniqueNames to display names via the
/// uniqueName -> name map (`data/cache/item-names.json`, built from the
/// WFCD items dataset). Unresolvable entries are skipped, never guessed.
pub fn owned_names_from_profile(
    snap: &ProfileSnapshot,
    names: &std::collections::HashMap<String, String>,
) -> OwnedEquipment {
    OwnedEquipment {
        names: snap
            .item_types
            .iter()
            .filter_map(|u| names.get(u).cloned())
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Xuerian targets
// ---------------------------------------------------------------------------

pub async fn fetch_xuerian_targets(
    client: &reqwest::Client,
    cache_dir: &Path,
    token: &str,
    seed_path: &Path,
) -> Result<Fetched<Targets>, String> {
    if token.trim().is_empty() {
        let text = std::fs::read_to_string(seed_path)
            .map_err(|e| format!("seed targets {}: {e}", seed_path.display()))?;
        let targets = Targets::from_seed_json(&text)?;
        return Ok(Fetched {
            value: targets,
            status: FetchStatus::SeedFallback {
                reason: "Xuerian token not set — targets may go stale".to_owned(),
            },
        });
    }
    let path = cache_file(cache_dir, "xuerian.json");
    if cache_age(&path).map(|a| a < TTL_XUERIAN).unwrap_or(false) {
        if let Some(text) = read_cache(&path) {
            if let Ok(t) = Targets::from_json(&text) {
                return Ok(Fetched {
                    value: t,
                    status: FetchStatus::Fresh,
                });
            }
        }
    }
    let form_token = token.to_owned();
    match client
        .post(XUERIAN_LOAD_URL)
        .form(&[("tokens", form_token)])
        .send()
        .await
    {
        Ok(resp) => {
            let resp = resp.error_for_status().map_err(|e| format!("xuerian: {e}"))?;
            let text = resp.text().await.map_err(|e| format!("xuerian: {e}"))?;
            match Targets::from_json(&text) {
                Ok(t) if !t.wants.is_empty() => {
                    // Out of scope for the fifty-seventh pass hold:
                    // Xuerian is not DE traffic, so its write
                    // failure stays silent exactly as before.
                    let _ = try_write_cache(cache_dir, "xuerian.json", &text);
                    Ok(Fetched {
                        value: t,
                        status: FetchStatus::Fresh,
                    })
                }
                _ => {
                    // Empty/unknown shape (e.g. bad token): fail-soft.
                    if let Some(cached) = read_cache(&path) {
                        if let Ok(t) = Targets::from_json(&cached) {
                            return Ok(Fetched {
                                value: t,
                                status: FetchStatus::CacheFallback {
                                    reason: "unexpected /api/load shape".to_owned(),
                                },
                            });
                        }
                    }
                    let text = std::fs::read_to_string(seed_path).map_err(|e| {
                        format!("seed targets {}: {e}", seed_path.display())
                    })?;
                    let targets = Targets::from_seed_json(&text)?;
                    Ok(Fetched {
                        value: targets,
                        status: FetchStatus::SeedFallback {
                            reason: "Xuerian load failed — targets may go stale".to_owned(),
                        },
                    })
                }
            }
        }
        Err(e) => {
            if let Some(cached) = read_cache(&path) {
                if let Ok(t) = Targets::from_json(&cached) {
                    return Ok(Fetched {
                        value: t,
                        status: FetchStatus::CacheFallback {
                            reason: e.to_string(),
                        },
                    });
                }
            }
            let text = std::fs::read_to_string(seed_path)
                .map_err(|e2| format!("xuerian unreachable, no seed: {e} / {e2}"))?;
            let targets = Targets::from_seed_json(&text)?;
            Ok(Fetched {
                value: targets,
                status: FetchStatus::SeedFallback {
                    reason: format!("xuerian unreachable: {e}"),
                },
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Static drop data + items dataset
// ---------------------------------------------------------------------------

pub async fn fetch_relic_tables(
    client: &reqwest::Client,
    cache_dir: &Path,
    seed_path: &Path,
) -> Result<Fetched<RelicTables>, String> {
    let (text, status) =
        fetch_static(client, cache_dir, "relictables.json", XUERIAN_RELICTABLES_URL, seed_path)
            .await?;
    // Bake the legacy era alias into the on-disk cache as well as the
    // parsed value, so a future upstream pull is corrected at the source
    // and never re-seeds a "Vanguard" era (addendum #66).
    let baked = crate::model::bake_relic_tables(&text)?;
    if baked != text {
        // Out of scope for the fifty-seventh pass hold (static
        // data, not DE traffic): failure stays silent as before.
        let _ = try_write_cache(cache_dir, "relictables.json", &baked);
    }
    Ok(Fetched {
        value: RelicTables::from_json(&baked)?,
        status,
    })
}

pub async fn fetch_roster(
    client: &reqwest::Client,
    cache_dir: &Path,
    seed_path: &Path,
) -> Result<Fetched<Roster>, String> {
    let (text, status) =
        fetch_static(client, cache_dir, "wishlist.json", XUERIAN_WISHLIST_URL, seed_path)
            .await?;
    Ok(Fetched {
        value: Roster::from_json(&text)?,
        status,
    })
}

async fn fetch_static(
    client: &reqwest::Client,
    cache_dir: &Path,
    name: &str,
    url: &str,
    seed_path: &Path,
) -> Result<(String, FetchStatus), String> {
    let path = cache_file(cache_dir, name);
    if cache_age(&path).map(|a| a < TTL_STATIC_DATA).unwrap_or(false) {
        if let Some(text) = read_cache(&path) {
            return Ok((text, FetchStatus::Fresh));
        }
    }
    match client.get(url).send().await {
        Ok(resp) => {
            let resp = resp.error_for_status().map_err(|e| format!("{name}: {e}"))?;
            let text = resp.text().await.map_err(|e| format!("{name}: {e}"))?;
            // Out of scope for the fifty-seventh pass hold (static
            // data, not DE traffic): failure stays silent as before.
            let _ = try_write_cache(cache_dir, name, &text);
            Ok((text, FetchStatus::Fresh))
        }
        Err(e) => {
            if let Some(cached) = read_cache(&path) {
                return Ok((
                    cached,
                    FetchStatus::CacheFallback {
                        reason: e.to_string(),
                    },
                ));
            }
            let text = std::fs::read_to_string(seed_path)
                .map_err(|e2| format!("{name} unreachable, no seed: {e} / {e2}"))?;
            Ok((
                text,
                FetchStatus::SeedFallback {
                    reason: format!("{name} unreachable: {e}"),
                },
            ))
        }
    }
}

/// Fetch the WFCD items dataset (uniqueName -> name/imageName) into the
/// 7-day cache. ~80 MB; streamed to disk, never held fully in memory by
/// callers (this function still buffers one response body — acceptable
/// once per week; the planner resolves names lazily from the file).
pub async fn fetch_items_dataset(
    client: &reqwest::Client,
    cache_dir: &Path,
) -> Result<Fetched<PathBuf>, String> {
    let path = cache_file(cache_dir, "items.json");
    if cache_age(&path).map(|a| a < TTL_STATIC_DATA).unwrap_or(false) {
        return Ok(Fetched {
            value: path,
            status: FetchStatus::Fresh,
        });
    }
    match client.get(WFCD_ITEMS_URL).send().await {
        Ok(resp) => {
            let resp = resp.error_for_status().map_err(|e| format!("items: {e}"))?;
            let bytes = resp.bytes().await.map_err(|e| format!("items: {e}"))?;
            if std::fs::create_dir_all(cache_dir).is_err() {
                return Err("items: cannot create cache dir".to_owned());
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, &bytes).map_err(|e| format!("items: {e}"))?;
            std::fs::rename(&tmp, &path).map_err(|e| format!("items: {e}"))?;
            Ok(Fetched {
                value: path,
                status: FetchStatus::Fresh,
            })
        }
        Err(e) => {
            if path.exists() {
                return Ok(Fetched {
                    value: path,
                    status: FetchStatus::CacheFallback {
                        reason: e.to_string(),
                    },
                });
            }
            Err(format!("items unreachable, no cache: {e}"))
        }
    }
}

/// Minimal uniqueName -> display-name resolution against a cached items
/// file. Streams with serde_json from disk; callers needing one lookup
/// should build the map once per run.
pub fn resolve_item_name(items_json: &str, unique_name: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(items_json).ok()?;
    let arr = v.as_array()?;
    arr.iter()
        .find(|e| e.get("uniqueName").and_then(|u| u.as_str()) == Some(unique_name))
        .and_then(|e| e.get("name")?.as_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::SystemTime;

    #[test]
    fn freshness_math() {
        let now = SystemTime::now();
        let ttl = Duration::from_secs(30 * 60);
        assert!(is_fresh(now, ttl, now));
        assert!(!is_fresh(now - ttl - Duration::from_secs(1), ttl, now));
        assert!(!is_fresh(now + Duration::from_secs(60), ttl, now));
    }

    // --- Addendum #120: DE TTL gates -------------------------------------
    //
    // Harness: a client whose every request fails fast (dead proxy).
    // Any test asserting a TtlCache status with this client proves
    // the TTL branch ran, because a bypassed TTL could only produce
    // CacheFallback here — never Fresh, never TtlCache. No test below
    // touches the real DE endpoints.

    fn dead_client() -> reqwest::Client {
        reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").expect("dead proxy parses"))
            .timeout(Duration::from_secs(5))
            .build()
            .expect("dead client builds")
    }

    fn de_test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("fracker-de-ttl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const WS_TEXT: &str = r#"{"Time": "2026-10-28T15:00:00Z", "Fissures": []}"#;
    const PROFILE_TEXT: &str = r#"{"Results": [{"DisplayName": "T",
        "AccountId": {"$oid": "ttltest"}, "PlayerLevel": 30,
        "LoadOutInventory": {"XPInfo": [{"ItemType": "/Lotus/Powersuits/Excalibur", "XP": 1000}]},
        "DailyAffiliation": 7823, "DailyFocus": 357977}]}"#;

    #[tokio::test]
    async fn de_ttl_hit_serves_fresh_cache_without_network() {
        let dir = de_test_dir("hit");
        std::fs::write(dir.join("worldstate.json"), WS_TEXT).unwrap();
        std::fs::write(dir.join("profile-ttltest.json"), PROFILE_TEXT).unwrap();
        let c = dead_client();
        let now = SystemTime::now();
        let ws = fetch_worldstate_at(&c, &dir, now).await.unwrap();
        assert_eq!(ws.value, WS_TEXT);
        assert!(
            matches!(ws.status, FetchStatus::TtlCache { .. }),
            "fresh worldstate cache must TTL-hit, got {:?}",
            ws.status
        );
        let prof = fetch_profile_at(&c, &dir, "ttltest", now).await.unwrap();
        assert!(
            matches!(prof.status, FetchStatus::TtlCache { .. }),
            "fresh profile cache must TTL-hit, got {:?}",
            prof.status
        );
        assert_eq!(prof.status.label(), "cached (0m)");
        assert_eq!(prof.value.display_name, "T");
        assert_eq!(prof.value.daily.affiliation_remaining, 7823);
        assert_eq!(prof.value.daily.focus_remaining, 357_977);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn de_ttl_stale_cache_is_refetched_and_failure_falls_back() {
        let dir = de_test_dir("stale");
        std::fs::write(dir.join("worldstate.json"), WS_TEXT).unwrap();
        std::fs::write(dir.join("profile-ttltest.json"), PROFILE_TEXT).unwrap();
        let c = dead_client();
        // 20 minutes past the write: past the 15-minute TTL.
        let now = SystemTime::now() + Duration::from_secs(20 * 60);
        let ws = fetch_worldstate_at(&c, &dir, now).await.unwrap();
        assert!(
            !matches!(ws.status, FetchStatus::TtlCache { .. }),
            "stale worldstate cache must NOT TTL-hit, got {:?}",
            ws.status
        );
        // Live failed (dead proxy), so the old cache serves as the
        // failure fallback — the pre-existing behaviour, unchanged.
        assert!(matches!(ws.status, FetchStatus::CacheFallback { .. }));
        assert_eq!(ws.value, WS_TEXT);
        let prof = fetch_profile_at(&c, &dir, "ttltest", now).await.unwrap();
        assert!(
            matches!(prof.status, FetchStatus::CacheFallback { .. }),
            "stale profile cache + failed live must fall back, got {:?}",
            prof.status
        );
        assert_eq!(prof.value.daily.affiliation_remaining, 7823);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn de_ttl_corrupt_cache_falls_through_to_live() {
        let dir = de_test_dir("corrupt");
        std::fs::write(dir.join("worldstate.json"), "not json{{{").unwrap();
        std::fs::write(dir.join("profile-ttltest.json"), "not json{{{").unwrap();
        let c = dead_client();
        let now = SystemTime::now();
        // Corrupt cache, fresh mtime: never a TTL hit. Live fails
        // (dead proxy), so worldstate serves the raw cache as the
        // failure fallback while the profile errors on parse —
        // both exactly the pre-existing failure behaviour.
        let ws = fetch_worldstate_at(&c, &dir, now).await.unwrap();
        assert!(
            matches!(ws.status, FetchStatus::CacheFallback { .. }),
            "corrupt worldstate cache must fall through, got {:?}",
            ws.status
        );
        assert!(fetch_profile_at(&c, &dir, "ttltest", now).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn de_ttl_hit_profile_caps_match_live_parse() {
        let dir = de_test_dir("caps");
        std::fs::write(dir.join("profile-ttltest.json"), PROFILE_TEXT).unwrap();
        let live = parse_profile(PROFILE_TEXT).unwrap();
        let path = dir.join("profile-ttltest.json");
        let (text, _) = ttl_hit(&path, TTL_DE, SystemTime::now(), |t| {
            parse_profile(t).is_ok()
        })
        .expect("fresh valid profile cache hits");
        let cached = parse_profile(&text).unwrap();
        assert_eq!(
            cached.daily.affiliation_remaining, live.daily.affiliation_remaining,
            "a TTL-hit snapshot carries the same daily caps as live"
        );
        assert_eq!(
            cached.daily.focus_remaining, live.daily.focus_remaining,
            "a TTL-hit snapshot carries the same daily caps as live"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ttl_hit_decision_boundaries() {
        let dir = de_test_dir("bounds");
        let path = dir.join("worldstate.json");
        std::fs::write(&path, WS_TEXT).unwrap();
        let now = SystemTime::now();
        assert!(ttl_hit(&path, TTL_DE, now, valid_worldstate).is_some());
        assert!(ttl_hit(&path, TTL_DE, now + Duration::from_secs(20 * 60), valid_worldstate)
            .is_none());
        assert!(ttl_hit(&path, TTL_DE, now, |_| false).is_none());
        // A cache mtime in the future relative to `now` is not fresh.
        assert!(ttl_hit(&path, TTL_DE, now - Duration::from_secs(60 * 60), valid_worldstate)
            .is_none());
        assert!(ttl_hit(&dir.join("missing.json"), TTL_DE, now, |_| true).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ttl_cache_label_renders_minutes_distinct_from_fallback() {
        assert_eq!(
            FetchStatus::TtlCache {
                age: Duration::from_secs(3 * 60 + 20)
            }
            .label(),
            "cached (3m)"
        );
        assert_eq!(
            FetchStatus::TtlCache {
                age: Duration::from_secs(90)
            }
            .label(),
            "cached (1m)"
        );
        // Failure labelling is unchanged and cannot collide with the
        // TTL form: it carries the error reason, not an age.
        assert_eq!(
            FetchStatus::CacheFallback {
                reason: "boom".to_owned()
            }
            .label(),
            "cached (boom)"
        );
        assert_eq!(FetchStatus::Fresh.label(), "live");
    }

    #[test]
    fn parses_profile_shape() {
        let text = r#"{"Results": [{"DisplayName": "SethPrime", "AccountId": {"$oid": "abcdef0123456789abcdef01"}, "PlayerLevel": 30, "LoadOutInventory": {"XPInfo": [{"ItemType": "/Lotus/Powersuits/Excalibur", "XP": 1000}]}}]}"#;
        let snap = parse_profile(text).unwrap();
        assert_eq!(snap.display_name, "SethPrime");
        assert_eq!(snap.account_id, "abcdef0123456789abcdef01");
        assert_eq!(snap.mastery_rank, 30);
        assert_eq!(snap.item_types.len(), 1);
    }

    /// Golden test against the REAL account (addendum #1:
    /// 5420fba23846322173707618, DisplayName Jenterrobahne, MR 30).
    /// Fixture recorded live; asserts XPInfo, Affiliations, and the
    /// daily-cap REMAINING fields (addendum #2) all parse.
    #[test]
    fn live_profile_fixture_parses_caps_and_affiliations() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "fixtures/profile-5420fba23846322173707618.json",
        );
        let text = std::fs::read_to_string(&path).unwrap();
        let snap = parse_profile(&text).unwrap();
        assert!(
            snap.display_name.contains("Jenterrobahne"),
            "DisplayName confirms the account: {}",
            snap.display_name
        );
        assert_eq!(snap.account_id, "5420fba23846322173707618");
        assert_eq!(snap.mastery_rank, 30);
        assert!(
            snap.item_types.len() >= 600,
            "XPInfo equipment entries: {}",
            snap.item_types.len()
        );
        assert!(
            !snap.affiliations.is_empty(),
            "Affiliations (syndicate standings) parse"
        );
        assert!(
            snap.affiliations
                .iter()
                .any(|a| a.tag.contains("Cetus")),
            "known syndicate present"
        );
        // Daily REMAINING caps exist and are within their maxima
        // (faction pool 31,000 at MR30; focus 400,000 at MR30).
        assert!(
            snap.daily.affiliation_remaining <= 31_000,
            "DailyAffiliation remaining: {}",
            snap.daily.affiliation_remaining
        );
        assert!(
            snap.daily.focus_remaining <= 400_000,
            "DailyFocus remaining: {}",
            snap.daily.focus_remaining
        );
        assert!(
            !snap.daily.per_family.is_empty(),
            "per-family DailyAffiliation* values parse"
        );
    }

    #[test]
    fn profile_missing_results_errors() {
        assert!(parse_profile("{}").is_err());
    }

    /// Ninth pass §3: a partial daily-cap value must survive whatever
    /// numeric shape the endpoint uses. A float or numeric string used to
    /// be silently zeroed by `as_u64()`, which rendered a partly-spent
    /// pool as "0 left" — collapse to the raw value instead.
    #[test]
    fn daily_caps_parse_float_and_numeric_string_without_zeroing() {
        let text = r#"{"Results": [{
            "DisplayName": "T", "AccountId": {"$oid": "x"}, "PlayerLevel": 30.0,
            "LoadOutInventory": {"XPInfo": []},
            "DailyAffiliation": 7823.0,
            "DailyAffiliationCetus": "31000",
            "DailyAffiliationQuills": 31000,
            "DailyFocus": 357977.0
        }]}"#;
        let snap = parse_profile(text).unwrap();
        assert_eq!(snap.mastery_rank, 30, "float PlayerLevel");
        assert_eq!(snap.daily.affiliation_remaining, 7823, "float bare pool");
        assert_eq!(snap.daily.focus_remaining, 357_977, "float focus");
        assert_eq!(snap.daily.per_family.get("Cetus"), Some(&31_000), "string family");
        assert_eq!(snap.daily.per_family.get("Quills"), Some(&31_000), "int family");
        // Garbage / negative values are rejected, not guessed.
        let bad = r#"{"Results": [{"LoadOutInventory": {"XPInfo": []},
            "DailyAffiliation": "not a number", "DailyFocus": -5}]}"#;
        let snap = parse_profile(bad).unwrap();
        assert_eq!(snap.daily.affiliation_remaining, 0);
        assert_eq!(snap.daily.focus_remaining, 0);
    }

    #[tokio::test]
    async fn empty_token_falls_back_to_seed_with_status() {
        let dir = std::env::temp_dir().join("fracker-xu-test");
        let _ = std::fs::create_dir_all(&dir);
        let seed = dir.join("targets.json");
        std::fs::write(&seed, r#"{"wants": ["Khora Prime Blueprint"], "has": [], "mastered": []}"#)
            .unwrap();
        let c = client().unwrap();
        let fetched = fetch_xuerian_targets(&c, &dir, "", &seed).await.unwrap();
        assert_eq!(fetched.value.wants.len(), 1);
        assert!(matches!(fetched.status, FetchStatus::SeedFallback { .. }));
        assert!(fetched.status.label().contains("token not set"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_names_from_mini_items() {
        let mini = r#"[{"uniqueName": "/Lotus/A", "name": "Alpha"}, {"uniqueName": "/Lotus/B", "name": "Beta"}]"#;
        assert_eq!(
            resolve_item_name(mini, "/Lotus/B"),
            Some("Beta".to_owned())
        );
        assert_eq!(resolve_item_name(mini, "/Lotus/Z"), None);
    }

    // --- Fifty-fifth pass: proxy resolution + DE lock gates -----------
    //
    // Harness: a counting loopback stub server. Zero stub hits
    // proves a branch made no request; one hit proves the request
    // went exactly where claimed. No test below touches the real
    // warframestat.us or DE endpoints (both URLs are overridden).

    fn stub_client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("stub client builds")
    }

    struct StubServer {
        url: String,
        hits: Arc<std::sync::atomic::AtomicUsize>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl StubServer {
        fn start(status: u16, body: &str) -> Self {
            use std::io::{Read, Write};
            let listener =
                std::net::TcpListener::bind("127.0.0.1:0").expect("stub binds");
            listener.set_nonblocking(true).expect("stub nonblocking");
            let port = listener.local_addr().expect("stub addr").port();
            let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let body = body.to_owned();
            let hits_t = hits.clone();
            let stop_t = stop.clone();
            let thread = std::thread::spawn(move || {
                while !stop_t.load(std::sync::atomic::Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            hits_t.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let mut buf = [0u8; 4096];
                            let _ = stream.read(&mut buf);
                            let reason = match status {
                                200 => "OK",
                                403 => "Forbidden",
                                409 => "Conflict",
                                429 => "Too Many Requests",
                                _ => "Error",
                            };
                            let resp = format!(
                                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        }
                        Err(ref e)
                            if e.kind() == std::io::ErrorKind::WouldBlock =>
                        {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                url: format!("http://127.0.0.1:{port}/x"),
                hits,
                stop,
                thread: Some(thread),
            }
        }

        fn hits(&self) -> usize {
            self.hits.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl Drop for StubServer {
        fn drop(&mut self) {
            self.stop
                .store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    /// The committed /pc sample with its top-level timestamp
    /// rewritten — the only field the freshness bar reads.
    fn wfs_payload_with_timestamp(ts: &str) -> String {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let text =
            std::fs::read_to_string(dir.join("fixtures/wfcd-pc-sample.json")).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["timestamp"] = serde_json::Value::String(ts.to_owned());
        serde_json::to_string(&v).unwrap()
    }

    fn rfc3339(now: SystemTime) -> String {
        chrono::DateTime::<chrono::Utc>::from(now)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    #[test]
    fn ua_carries_package_version() {
        assert_eq!(
            user_agent(),
            format!(
                "fracker/{} (warframe session planner)",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[tokio::test]
    async fn wfs_fresh_cache_served_with_zero_requests() {
        let dir = de_test_dir("wfs-cache");
        let now = SystemTime::now();
        try_write_cache(&dir, "worldstate-wfs.json", &wfs_payload_with_timestamp(&rfc3339(now))).expect("test cache write");
        let stub = StubServer::start(500, "boom");
        // Fetch with a fresh clock reading: a `now` captured before
        // the cache write predates the file's mtime, and a
        // future-dated mtime correctly fails the TTL hit
        // (fail-closed to a live fetch) — the test must order time
        // the way a real generation does.
        let resolved = fetch_worldstate_resolved_at_with_urls(
            &stub_client(),
            &dir,
            SystemTime::now(),
            &stub.url,
            &stub.url,
        )
        .await
        .unwrap();
        assert!(matches!(resolved.origin, WorldstateOrigin::Wfs));
        assert!(matches!(
            resolved.status,
            FetchStatus::TtlCache { .. }
        ));
        assert_eq!(stub.hits(), 0, "a fresh proxy cache makes no request");
    }

    #[tokio::test]
    async fn wfs_live_serves_and_writes_cache() {
        let dir = de_test_dir("wfs-live");
        let now = SystemTime::now();
        let stub = StubServer::start(200, &wfs_payload_with_timestamp(&rfc3339(now)));
        let resolved = fetch_worldstate_resolved_at_with_urls(
            &stub_client(),
            &dir,
            now,
            &stub.url,
            &stub.url,
        )
        .await
        .unwrap();
        assert!(matches!(resolved.origin, WorldstateOrigin::Wfs));
        assert!(matches!(resolved.status, FetchStatus::Fresh));
        assert_eq!(stub.hits(), 1);
        // Second generation: the just-written cache serves, no
        // request (fresh clock reading, as in the cache test above
        // — the first call's `now` predates the cache file's mtime).
        let again = fetch_worldstate_resolved_at_with_urls(
            &stub_client(),
            &dir,
            SystemTime::now(),
            &stub.url,
            &stub.url,
        )
        .await
        .unwrap();
        assert!(matches!(
            again.status,
            FetchStatus::TtlCache { .. }
        ));
        assert_eq!(stub.hits(), 1);
    }

    #[tokio::test]
    async fn wfs_stale_payload_falls_through_to_de() {
        let dir = de_test_dir("wfs-stale");
        let now = SystemTime::now();
        // 20 minutes old: past the 15-minute bar.
        let stale = wfs_payload_with_timestamp(&rfc3339(
            now - Duration::from_secs(20 * 60),
        ));
        let wfs = StubServer::start(200, &stale);
        let de = StubServer::start(200, WS_TEXT);
        let resolved = fetch_worldstate_resolved_at_with_urls(
            &stub_client(),
            &dir,
            now,
            &wfs.url,
            &de.url,
        )
        .await
        .unwrap();
        assert!(matches!(resolved.origin, WorldstateOrigin::De));
        assert!(matches!(resolved.status, FetchStatus::Fresh));
        assert_eq!(wfs.hits(), 1);
        assert_eq!(de.hits(), 1);
    }

    #[tokio::test]
    async fn wfs_failures_fall_through_without_lock() {
        // Non-200, unparseable, and missing-timestamp proxy payloads
        // each fall through to DE — and none of them writes the DE
        // lock (a proxy 403/429 is not a DE block).
        for (tag, body) in [
            ("non200", None),
            ("unparseable", Some("not json".to_owned())),
            ("no-timestamp", None),
        ] {
            let dir = de_test_dir(&format!("wfs-fail-{tag}"));
            let now = SystemTime::now();
            let wfs_body = match tag {
                "unparseable" => body.unwrap(),
                "no-timestamp" => {
                    let dir0 = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                    let text = std::fs::read_to_string(
                        dir0.join("fixtures/wfcd-pc-sample.json"),
                    )
                    .unwrap();
                    let mut v: serde_json::Value =
                        serde_json::from_str(&text).unwrap();
                    v.as_object_mut().unwrap().remove("timestamp");
                    serde_json::to_string(&v).unwrap()
                }
                _ => wfs_payload_with_timestamp(&rfc3339(now)),
            };
            let wfs_status = if tag == "non200" { 403 } else { 200 };
            let wfs = StubServer::start(wfs_status, &wfs_body);
            let de = StubServer::start(200, WS_TEXT);
            let resolved = fetch_worldstate_resolved_at_with_urls(
                &stub_client(),
                &dir,
                now,
                &wfs.url,
                &de.url,
            )
            .await
            .unwrap();
            assert!(
                matches!(resolved.origin, WorldstateOrigin::De),
                "{tag}: proxy failure must fall through to DE"
            );
            assert!(
                read_de_lock(&dir, now).is_none(),
                "{tag}: a proxy failure must not write the DE lock"
            );
        }
    }

    #[tokio::test]
    async fn wfs_network_failure_falls_through_to_de() {
        // Port 9 refuses connections on loopback: a genuine network
        // failure for the proxy attempt, not a stubbed error status.
        let dir = de_test_dir("wfs-netfail");
        let now = SystemTime::now();
        let de = StubServer::start(200, WS_TEXT);
        let resolved = fetch_worldstate_resolved_at_with_urls(
            &stub_client(),
            &dir,
            now,
            "http://127.0.0.1:9/pc",
            &de.url,
        )
        .await
        .unwrap();
        assert!(matches!(resolved.origin, WorldstateOrigin::De));
        assert!(matches!(resolved.status, FetchStatus::Fresh));
        assert_eq!(de.hits(), 1);
        assert!(
            read_de_lock(&dir, now).is_none(),
            "a proxy network failure must not write the DE lock"
        );
    }

    #[tokio::test]
    async fn wfs_stale_cache_is_never_served() {
        // Fresh mtime, stale payload timestamp: the cache must be
        // skipped (not served), and the generation falls through.
        // The fetch uses a fresh clock reading so the mtime bar
        // passes and the PAYLOAD staleness is what rejects the hit.
        let dir = de_test_dir("wfs-stale-cache");
        let now = SystemTime::now();
        try_write_cache(
            &dir,
            "worldstate-wfs.json",
            &wfs_payload_with_timestamp(&rfc3339(now - Duration::from_secs(20 * 60))),
        ).expect("test cache write");
        let wfs = StubServer::start(500, "proxy down");
        let de = StubServer::start(200, WS_TEXT);
        let resolved = fetch_worldstate_resolved_at_with_urls(
            &stub_client(),
            &dir,
            SystemTime::now(),
            &wfs.url,
            &de.url,
        )
        .await
        .unwrap();
        assert!(matches!(resolved.origin, WorldstateOrigin::De));
        assert!(matches!(resolved.status, FetchStatus::Fresh));
    }

    #[tokio::test]
    async fn de_403_writes_lock_and_blocks_further_requests() {
        let dir = de_test_dir("de-403");
        let now = SystemTime::now();
        try_write_cache(&dir, "worldstate.json", WS_TEXT).expect("test cache write");
        let de = StubServer::start(403, "blocked");
        let fetched = fetch_worldstate_de_at(&stub_client(), &dir, now, &de.url)
            .await
            .unwrap();
        assert!(matches!(
            fetched.status,
            FetchStatus::CacheFallback { .. }
        ));
        assert_eq!(de.hits(), 1);
        let lock = read_de_lock(&dir, now).expect("lock written on 403");
        assert_eq!(lock.endpoint, "worldstate");
        assert_eq!(lock.status, 403);
        // While held, the DE fetch makes ZERO further requests.
        let de2 = StubServer::start(200, WS_TEXT);
        let held = fetch_worldstate_de_at(&stub_client(), &dir, now, &de2.url)
            .await
            .unwrap();
        assert_eq!(de2.hits(), 0);
        assert!(matches!(
            held.status,
            FetchStatus::CacheFallback { ref reason }
            if reason.contains("lockfile")
        ));
    }

    #[tokio::test]
    async fn profile_429_locks_both_endpoints() {
        let dir = de_test_dir("profile-429");
        let now = SystemTime::now();
        try_write_cache(&dir, "profile-locktest.json", PROFILE_TEXT).expect("test cache write");
        try_write_cache(&dir, "worldstate.json", WS_TEXT).expect("test cache write");
        let prof = StubServer::start(429, "slow down");
        let fetched = fetch_profile_url_at(&stub_client(), &dir, "locktest", now, &prof.url)
            .await
            .unwrap();
        assert!(matches!(
            fetched.status,
            FetchStatus::CacheFallback { .. }
        ));
        let lock = read_de_lock(&dir, now).expect("lock written on 429");
        assert_eq!(lock.endpoint, "profile");
        assert_eq!(lock.status, 429);
        // One lock blocks BOTH DE endpoints: worldstate makes no request.
        let de = StubServer::start(200, WS_TEXT);
        let held = fetch_worldstate_de_at(&stub_client(), &dir, now, &de.url)
            .await
            .unwrap();
        assert_eq!(de.hits(), 0);
        assert!(matches!(
            held.status,
            FetchStatus::CacheFallback { ref reason }
            if reason.contains("lockfile")
        ));
    }

    #[tokio::test]
    async fn lock_expiry_and_corruption_are_ignored_and_removed() {
        // A 25-hour-old lock is ignored as if absent AND deleted.
        let dir = de_test_dir("lock-expiry");
        let now = SystemTime::now();
        write_de_lock(&dir, "worldstate", 403, now - Duration::from_secs(25 * 3600)).expect("test lock write");
        let de = StubServer::start(200, WS_TEXT);
        let fetched = fetch_worldstate_de_at(&stub_client(), &dir, now, &de.url)
            .await
            .unwrap();
        assert!(matches!(fetched.status, FetchStatus::Fresh));
        assert_eq!(de.hits(), 1);
        assert!(!de_lock_path(&dir).exists(), "expired lock deleted on sight");
        // An unparseable lock is likewise ignored and removed.
        let dir2 = de_test_dir("lock-corrupt");
        std::fs::write(de_lock_path(&dir2), "garbage{{{").unwrap();
        let de2 = StubServer::start(200, WS_TEXT);
        let fetched2 = fetch_worldstate_de_at(&stub_client(), &dir2, now, &de2.url)
            .await
            .unwrap();
        assert!(matches!(fetched2.status, FetchStatus::Fresh));
        assert!(!de_lock_path(&dir2).exists());
    }

    #[test]
    fn clear_de_lock_removes_file() {
        let dir = de_test_dir("lock-clear");
        write_de_lock(&dir, "profile", 429, SystemTime::now()).expect("test lock write");
        assert!(clear_de_lock(&dir));
        assert!(!de_lock_path(&dir).exists());
        assert!(!clear_de_lock(&dir), "second clear reports nothing to do");
    }

    #[tokio::test]
    async fn profile_409_still_errors_outright_without_lock() {
        let dir = de_test_dir("profile-409");
        let now = SystemTime::now();
        let prof = StubServer::start(409, "no such account");
        let err = fetch_profile_url_at(&stub_client(), &dir, "ghost", now, &prof.url)
            .await
            .unwrap_err();
        assert!(err.contains("409"), "409 must error outright: {err}");
        assert!(
            read_de_lock(&dir, now).is_none(),
            "a 409 is not a block and must not lock"
        );
    }

    // --- Fifty-seventh pass: cache-write-failure hold ------------------
    //
    // Write failure is forced robustly (tests may run as root, where
    // chmod read-only dirs still allow writes): either the cache
    // dir IS a regular file (create_dir_all fails — directory-wide
    // failure) or the cache tmp path is pre-created as a directory
    // (the tmp write fails — file-specific failure, so the latch's
    // lock attempt still lands). No test below touches the real DE
    // endpoints; stub hit counts prove zero network requests on
    // the held path.

    #[tokio::test]
    async fn hold_worldstate_write_failure_latches_and_serves_stale() {
        let dir = de_test_dir("hold-ws-tmpdir");
        // Stale cache (fetched with a clock 20 minutes ahead, past
        // the 15-minute TTL) plus a blocked tmp path: the live
        // write fails, but the stale cache stays readable.
        std::fs::write(dir.join("worldstate.json"), WS_TEXT).unwrap();
        std::fs::create_dir_all(dir.join("worldstate.tmp")).unwrap();
        let now = SystemTime::now() + Duration::from_secs(20 * 60);
        let de = StubServer::start(200, WS_TEXT);
        let first = fetch_worldstate_de_at(&stub_client(), &dir, now, &de.url)
            .await
            .unwrap();
        // This generation still serves its live payload.
        assert!(matches!(first.status, FetchStatus::Fresh));
        assert_eq!(de.hits(), 1);
        assert!(
            fetch_hold_latched(&dir),
            "a failed worldstate cache write must latch the hold"
        );
        // File-specific failure: the latch's lock attempt lands.
        let lock = read_de_lock(&dir, now).expect("latch must attempt the lock");
        assert_eq!(lock.endpoint, "worldstate");
        assert_eq!(lock.status, 0, "a latch lock is not an HTTP block");
        let log = take_fetch_hold_log(&dir);
        assert_eq!(log.len(), 1, "one latch, one log line: {log:?}");
        assert!(
            log[0].contains("worldstate cache write failed"),
            "log names the failure: {}",
            log[0]
        );
        assert!(
            log[0].contains("de-block.lock written"),
            "log records the lock landing: {}",
            log[0]
        );
        assert!(
            take_fetch_hold_log(&dir).is_empty(),
            "draining empties the queue"
        );
        // Held: zero further requests, stale cache as CacheFallback.
        let de2 = StubServer::start(200, WS_TEXT);
        let held = fetch_worldstate_de_at(&stub_client(), &dir, now, &de2.url)
            .await
            .unwrap();
        assert_eq!(de2.hits(), 0, "a held fetch makes no request");
        assert!(matches!(
            held.status,
            FetchStatus::CacheFallback { ref reason }
            if reason.contains("fetch hold")
        ));
        assert_eq!(held.value, WS_TEXT);
        // One lock attempt per latch: deleting the landed lock and
        // fetching again must NOT re-attempt it.
        std::fs::remove_file(de_lock_path(&dir)).unwrap();
        let de3 = StubServer::start(200, WS_TEXT);
        let held2 = fetch_worldstate_de_at(&stub_client(), &dir, now, &de3.url)
            .await
            .unwrap();
        assert_eq!(de3.hits(), 0);
        assert!(matches!(held2.status, FetchStatus::CacheFallback { .. }));
        assert!(
            !de_lock_path(&dir).exists(),
            "no second lock attempt on a subsequent held fetch"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hold_directory_wide_failure_latches_without_lock() {
        // The cache dir is a regular file: create_dir_all fails, so
        // every cache and lock write fails — the hold is the only
        // protection, and no lock can land.
        let parent = de_test_dir("hold-dirfile");
        let dir = parent.join("cachefile");
        std::fs::write(&dir, b"x").unwrap();
        let now = SystemTime::now();
        let de = StubServer::start(200, WS_TEXT);
        let first = fetch_worldstate_de_at(&stub_client(), &dir, now, &de.url)
            .await
            .unwrap();
        assert!(matches!(first.status, FetchStatus::Fresh));
        assert_eq!(de.hits(), 1);
        assert!(fetch_hold_latched(&dir), "dir-wide failure must latch");
        assert!(
            !de_lock_path(&dir).exists(),
            "no lock can land when the dir itself is broken"
        );
        let log = take_fetch_hold_log(&dir);
        assert_eq!(log.len(), 1);
        assert!(log[0].contains("worldstate cache write failed"), "{}", log[0]);
        assert!(
            log[0].contains("de-block.lock write also failed"),
            "the failed lock attempt is recorded, not hidden: {}",
            log[0]
        );
        // Held with no readable cache: the same error today's
        // lock-gate path produces, and still zero requests.
        let de2 = StubServer::start(200, WS_TEXT);
        let err = fetch_worldstate_de_at(&stub_client(), &dir, now, &de2.url)
            .await
            .unwrap_err();
        assert_eq!(de2.hits(), 0);
        assert!(err.contains("fetch hold"), "held error names the hold: {err}");
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[tokio::test]
    async fn hold_lock_write_failure_latches() {
        // A 403 whose lock write fails (cache dir is a file) must
        // latch the hold, not silently leave the next generation
        // uncapped.
        let parent = de_test_dir("hold-lockfile");
        let dir = parent.join("cachefile");
        std::fs::write(&dir, b"x").unwrap();
        let now = SystemTime::now();
        let de = StubServer::start(403, "blocked");
        let err = fetch_worldstate_de_at(&stub_client(), &dir, now, &de.url)
            .await
            .unwrap_err();
        assert!(err.contains("blocked"), "403 with no cache errors: {err}");
        assert!(
            fetch_hold_latched(&dir),
            "a failed lock write must latch the hold"
        );
        let log = take_fetch_hold_log(&dir);
        assert_eq!(log.len(), 1);
        assert!(log[0].contains("de-block.lock write failed"), "{}", log[0]);
        let de2 = StubServer::start(200, WS_TEXT);
        let err2 = fetch_worldstate_de_at(&stub_client(), &dir, now, &de2.url)
            .await
            .unwrap_err();
        assert_eq!(de2.hits(), 0, "a held fetch makes no request");
        assert!(err2.contains("fetch hold"), "{err2}");
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[tokio::test]
    async fn hold_profile_write_failure_blocks_both_endpoints() {
        let dir = de_test_dir("hold-profile-tmpdir");
        std::fs::write(dir.join("profile-holdtest.json"), PROFILE_TEXT).unwrap();
        std::fs::write(dir.join("worldstate.json"), WS_TEXT).unwrap();
        std::fs::create_dir_all(dir.join("profile-holdtest.tmp")).unwrap();
        let now = SystemTime::now() + Duration::from_secs(20 * 60);
        let prof = StubServer::start(200, PROFILE_TEXT);
        let first = fetch_profile_url_at(&stub_client(), &dir, "holdtest", now, &prof.url)
            .await
            .unwrap();
        assert!(matches!(first.status, FetchStatus::Fresh));
        assert_eq!(prof.hits(), 1);
        assert!(fetch_hold_latched(&dir), "profile write failure must latch");
        let lock = read_de_lock(&dir, now).expect("latch lock lands");
        assert_eq!(lock.endpoint, "profile");
        // One hold blocks BOTH DE endpoints: worldstate and profile
        // alike make zero requests and serve stale cache.
        let de = StubServer::start(200, WS_TEXT);
        let held_ws = fetch_worldstate_de_at(&stub_client(), &dir, now, &de.url)
            .await
            .unwrap();
        assert_eq!(de.hits(), 0);
        assert!(matches!(
            held_ws.status,
            FetchStatus::CacheFallback { ref reason }
            if reason.contains("fetch hold")
        ));
        assert_eq!(held_ws.value, WS_TEXT);
        let prof2 = StubServer::start(200, PROFILE_TEXT);
        let held_prof =
            fetch_profile_url_at(&stub_client(), &dir, "holdtest", now, &prof2.url)
                .await
                .unwrap();
        assert_eq!(prof2.hits(), 0);
        assert!(matches!(
            held_prof.status,
            FetchStatus::CacheFallback { ref reason }
            if reason.contains("fetch hold")
        ));
        assert_eq!(held_prof.value.display_name, "T");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hold_proxy_write_failure_latches_de_but_proxy_continues() {
        let dir = de_test_dir("hold-proxy-tmpdir");
        std::fs::write(dir.join("worldstate.json"), WS_TEXT).unwrap();
        std::fs::create_dir_all(dir.join("worldstate-wfs.tmp")).unwrap();
        // The fetch clock must match the proxy payload timestamp:
        // a clock 20 minutes ahead would trip the 15-minute
        // payload-freshness bar and never reach the cache write.
        let now = SystemTime::now();
        let wfs =
            StubServer::start(200, &wfs_payload_with_timestamp(&rfc3339(now)));
        let resolved = fetch_worldstate_resolved_at_with_urls(
            &stub_client(),
            &dir,
            now,
            &wfs.url,
            "http://127.0.0.1:9/de",
        )
        .await
        .unwrap();
        assert!(matches!(resolved.status, FetchStatus::Fresh));
        assert_eq!(resolved.origin, WorldstateOrigin::Wfs);
        assert_eq!(wfs.hits(), 1);
        assert!(
            fetch_hold_latched(&dir),
            "a proxy cache-write failure must latch the DE hold"
        );
        let lock = read_de_lock(&dir, now).expect("latch lock lands");
        assert_eq!(lock.endpoint, "proxy");
        // Next generation: the proxy is still attempted (different
        // host, different budget), but the DE fall-through is held.
        let wfs2 = StubServer::start(500, "proxy down");
        let de = StubServer::start(200, WS_TEXT);
        let resolved2 = fetch_worldstate_resolved_at_with_urls(
            &stub_client(),
            &dir,
            now,
            &wfs2.url,
            &de.url,
        )
        .await
        .unwrap();
        assert_eq!(wfs2.hits(), 1, "proxy attempts continue unchanged");
        assert_eq!(de.hits(), 0, "the DE fall-through stays held");
        assert_eq!(resolved2.origin, WorldstateOrigin::De);
        assert!(matches!(
            resolved2.status,
            FetchStatus::CacheFallback { ref reason }
            if reason.contains("fetch hold")
        ));
        assert_eq!(resolved2.text, WS_TEXT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hold_success_path_does_not_latch() {
        let dir = de_test_dir("hold-success");
        let now = SystemTime::now();
        let de = StubServer::start(200, WS_TEXT);
        let fetched = fetch_worldstate_de_at(&stub_client(), &dir, now, &de.url)
            .await
            .unwrap();
        assert!(matches!(fetched.status, FetchStatus::Fresh));
        let prof = StubServer::start(200, PROFILE_TEXT);
        let snap = fetch_profile_url_at(&stub_client(), &dir, "holdtest", now, &prof.url)
            .await
            .unwrap();
        assert!(matches!(snap.status, FetchStatus::Fresh));
        assert!(
            !fetch_hold_latched(&dir),
            "normal writes must not latch the hold"
        );
        assert!(take_fetch_hold_log(&dir).is_empty());
        assert!(
            read_de_lock(&dir, now).is_none(),
            "no lock on the success path"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hold_failure_is_recorded_in_resident_log() {
        // The real observability path, end to end: latch via a
        // genuine fetch failure, drain through the same helper the
        // resident report job uses, record through a real AppLog,
        // and read the line back out of resident.log itself.
        let dir = de_test_dir("hold-resident-log");
        std::fs::write(dir.join("worldstate.json"), WS_TEXT).unwrap();
        std::fs::create_dir_all(dir.join("worldstate.tmp")).unwrap();
        let now = SystemTime::now() + Duration::from_secs(20 * 60);
        let de = StubServer::start(200, WS_TEXT);
        let first = fetch_worldstate_de_at(&stub_client(), &dir, now, &de.url)
            .await
            .unwrap();
        assert!(matches!(first.status, FetchStatus::Fresh));
        assert!(fetch_hold_latched(&dir));
        let log_dir = std::env::temp_dir()
            .join(format!("fracker-hold-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&log_dir);
        let log = crate::applog::AppLog::open(&log_dir);
        assert_eq!(log.path(), log_dir.join("resident.log"));
        for line in take_fetch_hold_log(&dir) {
            log.record("fetch", &line);
        }
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(
            text.contains("[fetch] DE fetch hold latched"),
            "resident.log carries the latch line: {text}"
        );
        assert!(
            text.contains("worldstate cache write failed"),
            "resident.log names the failure: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&log_dir);
    }
}
