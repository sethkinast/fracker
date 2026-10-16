//! Report generation (seventh pass §3): the plan + render pipeline as a
//! reusable library call.
//!
//! Pass 6 owned this logic inside `main::run_demo`; the resident runtime
//! (tray + headless) needs the identical pipeline, so it lives here and
//! both the CLI (`--demo`) and the resident loop call [`generate`].
//!
//! Fail-soft chain (BRIEF: report endpoint failures, never die on them):
//! world state falls back to `fixtures/worldstate.json`, the profile to
//! `fixtures/profile-<id>.json`. `offline` skips the network entirely and
//! reads the recorded fixtures — that is how the headless smoke test and
//! the integration test generate a REAL page hermetically.
//!
//! Icon delivery is a mode: the product/resident page hotlinks the WFCD
//! CDN (`embed_icons: false`), the review/demo export embeds ≤128px
//! data-URI thumbnails (`embed_icons: true`, addendum #23).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::model::{OwnedEquipment, RelicStock};
use crate::tuning::Tuning;
use crate::{config, fetch, model, pack, plan, render};

/// Live profile account (addendum #1: resolved from a real fissure log's
/// reward relay and confirmed by the profile endpoint).
pub const PROFILE_ACCOUNT_ID: &str = "5420fba23846322173707618";

/// Everything [`generate`] needs, owned so the resident loop and tests can
/// build it without borrowing `main`'s arg parser.
#[derive(Debug, Clone)]
pub struct ReportConfig {
    /// Content tree (`data/`): tuning tables, backlog, seed caches, packs.
    pub data_dir: PathBuf,
    /// Fallback fixture dir (`fixtures/worldstate.json`, profile).
    pub fixtures: PathBuf,
    /// HTTP cache dir (world state / profile TTL caches).
    pub cache_dir: PathBuf,
    /// Where the rendered HTML page is written (atomic temp + rename).
    pub out: PathBuf,
    /// App config file (pins + steel-path tiebreak).
    pub config_path: PathBuf,
    /// Skip the network; read the recorded fixtures (hermetic).
    pub offline: bool,
    /// Embed ≤128px data-URI icons (demo) instead of CDN hotlinks.
    pub embed_icons: bool,
}

impl ReportConfig {
    /// Product/resident defaults for a report written to the app data dir.
    pub fn resident(out: PathBuf, config_path: PathBuf) -> Self {
        Self {
            data_dir: crate::tuning::data_dir(),
            fixtures: PathBuf::from("fixtures"),
            cache_dir: config::cache_dir(),
            out,
            config_path,
            offline: false,
            embed_icons: false,
        }
    }
}

/// What one report run produced (for stdout, the tray status line, tests).
#[derive(Debug, Clone)]
pub struct ReportOutcome {
    pub goals: usize,
    pub chunks: usize,
    /// Daily bars rendered (pool families + focus).
    pub bars: usize,
    /// Backlog gears with a resolved parent icon.
    pub icons_resolved: usize,
    pub backlog_primes: usize,
    /// Demo export: icons embedded as data URIs / left as hotlinks.
    pub embedded: usize,
    pub hotlinks: usize,
    pub mastery_rank: u32,
    pub endpoint_failures: Vec<String>,
    pub generated_at: DateTime<Utc>,
    pub out: PathBuf,
}

impl ReportOutcome {
    /// One-line status ("61 goals · 11 chunks · live").
    pub fn summary(&self) -> String {
        let endpoints = if self.endpoint_failures.is_empty() {
            "all live".to_owned()
        } else {
            self.endpoint_failures.join("; ")
        };
        format!(
            "{} goals, {} chunks, {} bars ({endpoints})",
            self.goals, self.chunks, self.bars
        )
    }
}

/// Render the checklist page from the configured inputs. Never dies on a
/// dead endpoint: each fetch falls back to its recorded fixture.
pub async fn generate(cfg: &ReportConfig) -> Result<ReportOutcome, String> {
    let data_file = |rel: &str| cfg.data_dir.join(rel);
    let mut endpoint_failures: Vec<String> = Vec::new();

    // 0. Runtime tuning + data-owned universe scrub (no content in Rust).
    let tuning = Tuning::load(&cfg.data_dir)?;
    let scrub_path = data_file("exclusions.toml");
    let scrub_text = std::fs::read_to_string(&scrub_path)
        .map_err(|e| format!("exclusions {}: {e}", scrub_path.display()))?;
    let scrub = model::UniverseScrub::from_toml(&scrub_text)?;

    // Online client when not explicitly offline; a client build failure
    // degrades to the fixture path rather than killing the run.
    let client = if cfg.offline {
        None
    } else {
        match fetch::client() {
            Ok(c) => Some(c),
            Err(e) => {
                endpoint_failures.push(format!("http client: {e}"));
                None
            }
        }
    };

    // 1. World state: live, else recorded fixture.
    let (world_text, world_status) = match &client {
        Some(client) => match fetch::fetch_worldstate(client, &cfg.cache_dir).await {
            Ok(fetched) => {
                let text = std::fs::read_to_string(fetch::cache_file(
                    &cfg.cache_dir,
                    "worldstate.json",
                ))
                .map_err(|e| format!("reading recorded worldstate: {e}"))?;
                (text, fetched.status.label())
            }
            Err(e) => {
                endpoint_failures.push(format!("worldstate: {e}"));
                let path = cfg.fixtures.join("worldstate.json");
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("fixture worldstate {}: {e}", path.display()))?;
                (text, "fixture".to_owned())
            }
        },
        None => {
            let path = cfg.fixtures.join("worldstate.json");
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("fixture worldstate {}: {e}", path.display()))?;
            (text, "fixture".to_owned())
        }
    };
    let world = model::WorldState::from_json(&world_text)?;
    let now = world
        .timestamp
        .parse::<DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());

    // 2. Reconciled backlog (profile-derived primes), exclusions applied.
    let backlog_path = data_file("backlog.json");
    let backlog_text = std::fs::read_to_string(&backlog_path)
        .map_err(|e| format!("backlog {}: {e}", backlog_path.display()))?;
    let backlog = model::Backlog::from_json(&backlog_text)?;
    let wants = scrub.scrub_wants(&backlog.wants());
    let backlog_categories = scrub.scrub_categories(&backlog.unmastered_by_category);

    // 3. Seed relic/roster data.
    let rel_text = std::fs::read_to_string(data_file("cache/relictables.json"))
        .map_err(|e| format!("seed relictables: {e}"))?;
    let wish_text = std::fs::read_to_string(data_file("cache/wishlist.json"))
        .map_err(|e| format!("seed wishlist: {e}"))?;
    let relics = model::RelicTables::from_json(&rel_text)?;
    let roster = model::Roster::from_json(&wish_text)?;

    // 4. Fresh profile pull -> owned equipment + daily REMAINING caps.
    let (owned, mastery_rank, daily, profile_status) = match &client {
        Some(client) => match fetch::fetch_profile(client, &cfg.cache_dir, PROFILE_ACCOUNT_ID).await
        {
            Ok(fetched) => {
                let snap = fetched.value;
                let (o, d) = profile_context(&tuning, &cfg.data_dir, &snap);
                (o, snap.mastery_rank, d, fetched.status.label())
            }
            Err(e) => {
                endpoint_failures.push(format!("profile: {e}"));
                profile_from_fixture(&tuning, &cfg.data_dir, &cfg.fixtures)?
            }
        },
        None => profile_from_fixture(&tuning, &cfg.data_dir, &cfg.fixtures)?,
    };

    // 5. Parent-item icons (addendum #14).
    let icons_text =
        std::fs::read_to_string(data_file("icons.json")).map_err(|e| format!("icons: {e}"))?;
    let icons = model::load_icons(&icons_text)?;
    let icon_resolved = backlog
        .gear_names()
        .iter()
        .filter(|g| icons.contains_key(*g))
        .count();

    // 6. Taubound pack strip.
    let pack_text = std::fs::read_to_string(data_file("packs/taubound.toml"))
        .map_err(|e| format!("taubound pack: {e}"))?;
    let taubound = pack::TauboundPack::from_toml(&pack_text)?;
    let pack_render = render::PackRender {
        live: taubound.is_live(now),
        expires_label: taubound
            .pack
            .expires
            .parse::<chrono::DateTime<chrono::Utc>>()
            .map(|t| t.format("%b %d").to_string())
            .unwrap_or_else(|_| "soon".to_owned()),
        contegorus_health: world
            .events
            .iter()
            .filter(|e| e.is_community && e.matches_pack_event(&taubound.event.match_text))
            .filter_map(|e| e.health)
            .next(),
        arcane_line: taubound.arcane_line(),
        pack_name: taubound.pack.name.clone(),
        event_label: taubound.event.match_text.clone(),
        payout_label: format!(
            "{} {} / {} SP",
            taubound.event.resource,
            taubound.event.alert_payout,
            taubound.event.alert_payout_steel_path,
        ),
        gated_label: format!("{} gated", taubound.community_goal.gated_set),
    };

    let stock = RelicStock::Unknown;

    let (cfg_file, _) = config::Config::load(&cfg.config_path)
        .unwrap_or((config::Config::default(), config::ConfigSource::Defaults));

    let input = plan::PlanInput {
        now,
        world: &world,
        owned: &owned,
        wants: &wants,
        roster: &roster,
        relics: &relics,
        stock: &stock,
        pins: &cfg_file.pins,
        recent_gear: &HashSet::new(),
        steel_path_tiebreak: cfg_file.steel_path_tiebreak,
        tuning: &tuning,
    };
    let plan = plan::plan(&input);

    let weekly_done: HashMap<String, String> = HashMap::new();
    let proven_done: HashSet<String> = HashSet::new();

    // Icon delivery: product/resident page hotlinks (embedded_icons None);
    // demo export embeds ≤128px data-URI thumbnails.
    let (embedded, embedded_count, hotlink_count) = if cfg.embed_icons {
        match &client {
            Some(client) => embed_demo_icons(client, &plan, &icons).await,
            None => (HashMap::new(), 0, 0),
        }
    } else {
        (HashMap::new(), 0, 0)
    };
    let embedded_ref = if cfg.embed_icons {
        Some(&embedded)
    } else {
        None
    };

    let ctx = render::RenderContext {
        world: &world,
        generated_label: now.format("%Y-%m-%d %H:%M UTC").to_string(),
        targets_status: format!(
            "backlog {} primes · profile {} (MR{})",
            backlog.primes.unmastered.len(),
            profile_status,
            mastery_rank,
        ),
        world_status,
        weekly_done: &weekly_done,
        proven_done: &proven_done,
        mastery_rank: Some(mastery_rank),
        icons: &icons,
        embedded_icons: embedded_ref,
        backlog_categories: &backlog_categories,
        daily,
        pack: Some(pack_render),
        tuning: &tuning,
    };
    let html = render::render_plan(&plan, &ctx);

    if let Some(parent) = cfg.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("report out dir {}: {e}", parent.display()))?;
        }
    }
    let tmp = cfg.out.with_extension("html.tmp");
    std::fs::write(&tmp, html).map_err(|e| format!("write report: {e}"))?;
    std::fs::rename(&tmp, &cfg.out).map_err(|e| format!("publish report: {e}"))?;

    // Pass-4 bars: one per standing pool (PvP excluded) + focus.
    let bars = match &ctx.daily {
        Some(d) => 2 + d.families.len(),
        None => 0,
    };

    Ok(ReportOutcome {
        goals: plan.goals.len(),
        chunks: plan.chunks.len(),
        bars,
        icons_resolved: icon_resolved,
        backlog_primes: backlog.primes.unmastered.len(),
        embedded: embedded_count,
        hotlinks: hotlink_count,
        mastery_rank,
        endpoint_failures,
        generated_at: now,
        out: cfg.out.clone(),
    })
}

/// Owned equipment + profile-backed daily bars from one snapshot.
/// Name resolution uses `data/cache/item-names.json` (uniqueName ->
/// display name, built from the WFCD items dataset); unresolvable
/// entries are skipped, never guessed. Caps scale with mastery rank
/// per the runtime tuning tables (profile values are REMAINING).
fn profile_context(
    tuning: &Tuning,
    data_dir: &std::path::Path,
    snap: &fetch::ProfileSnapshot,
) -> (OwnedEquipment, Option<render::DailyRender>) {
    let names: HashMap<String, String> =
        std::fs::read_to_string(data_dir.join("cache/item-names.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    let owned = fetch::owned_names_from_profile(snap, &names);
    let mr = snap.mastery_rank;
    let caps = &tuning.planner.daily_caps;
    let daily = render::DailyRender {
        affiliation_remaining: snap.daily.affiliation_remaining,
        affiliation_cap: caps.standing_base + caps.standing_per_mr * mr,
        focus_remaining: snap.daily.focus_remaining,
        focus_cap: caps.focus_base + caps.focus_per_mr * mr,
        families: {
            // Pass 4 §28: PvP leaves with the Conclave cut — never a bar.
            let mut f: Vec<(String, u32)> = snap
                .daily
                .per_family
                .iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("pvp"))
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            f.sort_by(|a, b| a.0.cmp(&b.0));
            f
        },
    };
    (owned, Some(daily))
}

/// Profile from the recorded fixture (used offline and on fetch failure).
fn profile_from_fixture(
    tuning: &Tuning,
    data_dir: &std::path::Path,
    fixtures: &std::path::Path,
) -> Result<(OwnedEquipment, u32, Option<render::DailyRender>, String), String> {
    let fixture = fixtures.join(format!("profile-{PROFILE_ACCOUNT_ID}.json"));
    let text =
        std::fs::read_to_string(&fixture).map_err(|e| format!("fixture profile: {e}"))?;
    let snap = fetch::parse_profile(&text)?;
    let (o, d) = profile_context(tuning, data_dir, &snap);
    Ok((o, snap.mastery_rank, d, "fixture".to_owned()))
}

/// Demo icon embedding: download each goal's parent icon, shrink to a
/// ≤128px thumbnail (python3 + PIL when available), and return the
/// parent-name -> data-URI map plus embedded/fallback counts. Any
/// failure keeps the CDN hotlink for that icon (counted, never fatal).
async fn embed_demo_icons(
    client: &reqwest::Client,
    plan: &plan::Plan,
    icons: &HashMap<String, String>,
) -> (HashMap<String, String>, usize, usize) {
    let mut embedded: HashMap<String, String> = HashMap::new();
    let mut fallback = 0usize;
    let mut gears: Vec<&str> = plan.goals.iter().map(|g| g.gear.as_str()).collect();
    gears.sort();
    gears.dedup();
    for gear in gears {
        let image = match icons.get(gear) {
            Some(i) => i,
            None => continue,
        };
        let url = format!("https://cdn.warframestat.us/img/{image}");
        let bytes = match client.get(&url).send().await {
            Ok(resp) => match resp.bytes().await {
                Ok(b) => b.to_vec(),
                Err(_) => {
                    fallback += 1;
                    continue;
                }
            },
            Err(_) => {
                fallback += 1;
                continue;
            }
        };
        // Thumbnail when possible; otherwise embed the original bytes
        // (still hotlink-free) — counts stay honest either way.
        let thumb = thumbnail_png(&bytes, 128).unwrap_or(bytes);
        embedded.insert(
            gear.to_owned(),
            format!("data:image/png;base64,{}", render::base64_encode(&thumb)),
        );
    }
    let done = embedded.len();
    (embedded, done, fallback)
}

/// Shrink PNG bytes to a ≤`max_px` thumbnail via python3 + PIL.
/// Returns None when the toolchain is unavailable (caller falls back).
fn thumbnail_png(png_bytes: &[u8], max_px: u32) -> Option<Vec<u8>> {
    use std::io::Write;
    let script = format!(
        "import sys\n\
         from PIL import Image\n\
         im = Image.open(sys.stdin.buffer).convert('RGBA')\n\
         im.thumbnail(({m}, {m}))\n\
         im.save(sys.stdout.buffer, 'PNG')\n",
        m = max_px,
    );
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(&script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.as_mut()?.write_all(png_bytes).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    Some(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(out: PathBuf) -> ReportConfig {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        ReportConfig {
            data_dir: root.join("data"),
            fixtures: root.join("fixtures"),
            cache_dir: std::env::temp_dir().join("fracker-report-test-cache"),
            out,
            config_path: root.join("does-not-exist.toml"),
            offline: true,
            embed_icons: false,
        }
    }

    #[tokio::test]
    async fn offline_generation_renders_a_real_page() {
        let dir = std::env::temp_dir().join(format!(
            "fracker-report-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("plan.html");
        let cfg = test_cfg(out.clone());
        let outcome = generate(&cfg).await.expect("offline generate");
        assert!(out.exists(), "page written");
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.contains("<html"), "real HTML page");
        assert!(outcome.goals > 0, "goal cards from the backlog");
        assert!(!outcome.endpoint_failures.iter().any(|f| f.starts_with("worldstate")));
        // Product page keeps CDN hotlinks (embedded_icons None).
        assert!(html.contains("cdn.warframestat.us"), "product page hotlinks icons");
        // No leftover temp file after the atomic publish.
        assert!(!out.with_extension("html.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resident_defaults_point_at_the_app_data_dir() {
        let cfg = ReportConfig::resident(
            PathBuf::from("/tmp/plan.html"),
            PathBuf::from("/tmp/config.toml"),
        );
        assert!(!cfg.embed_icons);
        assert_eq!(cfg.out, PathBuf::from("/tmp/plan.html"));
    }
}
