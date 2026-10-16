//! HTTP fetchers for the §6 sources, each with on-disk cache + TTL.
//! All fetches fail-soft to cache. HTTP via `reqwest` (rustls).
//!
//! TTLs (BRIEF §6): world state 5 min (during session), public profile
//! 30 min, Xuerian `/api/load` 1 h, relic/drop data 7 days, WFCD items
//! dataset 7 days.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::model::{OwnedEquipment, RelicTables, Roster, Targets, WorldState};

pub const WORLD_STATE_URL: &str = "https://api.warframestat.us/pc/";
pub const PROFILE_URL: &str =
    "https://api.warframe.com/cdn/getProfileViewingData.php";
pub const XUERIAN_LOAD_URL: &str = "https://wf.xuerian.net/api/load";
pub const XUERIAN_RELICTABLES_URL: &str = "https://wf.xuerian.net/json/relictables.json";
pub const XUERIAN_WISHLIST_URL: &str = "https://wf.xuerian.net/json/wishlist.json";
/// WFCD items dataset (uniqueName -> display name, imageName, ...).
pub const WFCD_ITEMS_URL: &str = "https://api.warframestat.us/items";

pub const TTL_WORLD_STATE: Duration = Duration::from_secs(5 * 60);
pub const TTL_PROFILE: Duration = Duration::from_secs(30 * 60);
pub const TTL_XUERIAN: Duration = Duration::from_secs(60 * 60);
pub const TTL_STATIC_DATA: Duration = Duration::from_secs(7 * 24 * 60 * 60);

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// How a fetch resolved. The page/tray surface these as labels
/// ("Xuerian token not set — targets may go stale").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchStatus {
    /// Live response, cache refreshed.
    Fresh,
    /// Network failed; serving the on-disk cache.
    CacheFallback { reason: String },
    /// No token / no cache: serving the committed seed data.
    SeedFallback { reason: String },
}

impl FetchStatus {
    pub fn label(&self) -> String {
        match self {
            FetchStatus::Fresh => "live".to_owned(),
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

fn write_cache(cache_dir: &Path, name: &str, body: &str) {
    if std::fs::create_dir_all(cache_dir).is_err() {
        return;
    }
    let path = cache_file(cache_dir, name);
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("fracker/0.1 (warframe session planner)")
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))
}

// ---------------------------------------------------------------------------
// World state
// ---------------------------------------------------------------------------

pub async fn fetch_worldstate(
    client: &reqwest::Client,
    cache_dir: &Path,
) -> Result<Fetched<WorldState>, String> {
    let path = cache_file(cache_dir, "worldstate.json");
    let fresh_enough = cache_age(&path).map(|a| a < TTL_WORLD_STATE).unwrap_or(false);
    if fresh_enough {
        if let Some(text) = read_cache(&path) {
            if let Ok(ws) = WorldState::from_json(&text) {
                return Ok(Fetched {
                    value: ws,
                    status: FetchStatus::Fresh,
                });
            }
        }
    }
    match client.get(WORLD_STATE_URL).send().await {
        Ok(resp) => {
            let resp = resp.error_for_status().map_err(|e| format!("worldstate: {e}"))?;
            let text = resp.text().await.map_err(|e| format!("worldstate: {e}"))?;
            let ws = WorldState::from_json(&text)?;
            write_cache(cache_dir, "worldstate.json", &text);
            Ok(Fetched {
                value: ws,
                status: FetchStatus::Fresh,
            })
        }
        Err(e) => {
            let cached = read_cache(&path)
                .ok_or_else(|| format!("worldstate unreachable, no cache: {e}"))?;
            let ws = WorldState::from_json(&cached)?;
            Ok(Fetched {
                value: ws,
                status: FetchStatus::CacheFallback {
                    reason: e.to_string(),
                },
            })
        }
    }
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

pub async fn fetch_profile(
    client: &reqwest::Client,
    cache_dir: &Path,
    account_id: &str,
) -> Result<Fetched<ProfileSnapshot>, String> {
    let name = format!("profile-{account_id}.json");
    let path = cache_file(cache_dir, &name);
    if cache_age(&path).map(|a| a < TTL_PROFILE).unwrap_or(false) {
        if let Some(text) = read_cache(&path) {
            if let Ok(snap) = parse_profile(&text) {
                return Ok(Fetched {
                    value: snap,
                    status: FetchStatus::Fresh,
                });
            }
        }
    }
    let url = format!("{PROFILE_URL}?playerId={account_id}");
    match client.get(&url).send().await {
        Ok(resp) => {
            if resp.status() == reqwest::StatusCode::CONFLICT {
                return Err("profile: no such account (HTTP 409)".to_owned());
            }
            let resp = resp.error_for_status().map_err(|e| format!("profile: {e}"))?;
            let text = resp.text().await.map_err(|e| format!("profile: {e}"))?;
            let snap = parse_profile(&text)?;
            write_cache(cache_dir, &name, &text);
            Ok(Fetched {
                value: snap,
                status: FetchStatus::Fresh,
            })
        }
        Err(e) => {
            let cached = read_cache(&path)
                .ok_or_else(|| format!("profile unreachable, no cache: {e}"))?;
            let snap = parse_profile(&cached)?;
            Ok(Fetched {
                value: snap,
                status: FetchStatus::CacheFallback {
                    reason: e.to_string(),
                },
            })
        }
    }
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
        .and_then(|p| p.as_u64())
        .unwrap_or(0) as u32;
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
    let num = |key: &str| -> u32 {
        first.get(key).and_then(|x| x.as_u64()).unwrap_or(0) as u32
    };
    let mut per_family = std::collections::HashMap::new();
    if let Some(obj) = v
        .get("Results")
        .and_then(|r| r.get(0))
        .and_then(|f| f.as_object())
    {
        for (k, val) in obj {
            if let Some(fam) = k.strip_prefix("DailyAffiliation") {
                if !fam.is_empty() {
                    if let Some(n) = val.as_u64() {
                        per_family.insert(fam.to_owned(), n as u32);
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
                    write_cache(cache_dir, "xuerian.json", &text);
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
    Ok(Fetched {
        value: RelicTables::from_json(&text)?,
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
            write_cache(cache_dir, name, &text);
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
    use std::time::SystemTime;

    #[test]
    fn freshness_math() {
        let now = SystemTime::now();
        assert!(is_fresh(now, TTL_WORLD_STATE, now));
        assert!(!is_fresh(
            now - TTL_WORLD_STATE - Duration::from_secs(1),
            TTL_WORLD_STATE,
            now
        ));
        assert!(!is_fresh(now + Duration::from_secs(60), TTL_WORLD_STATE, now));
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
}
