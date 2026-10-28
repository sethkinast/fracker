//! Report generation (seventh pass §3): the plan + render pipeline as a
//! reusable library call.
//!
//! Pass 6 owned this logic inside `main::run_demo`; the resident runtime
//! (tray + headless) needs the identical pipeline, so it lives here and
//! both the CLI (`--demo`) and the resident loop call [`generate`].
//!
//! Fail-soft chain (BRIEF: report endpoint failures, never die on them):
//! DE's worldstate feed is the ONLY source (addendum #51) — a failed
//! fetch renders as unavailable or as an explicitly-labeled stale cache
//! view, never a silent substitution. The profile falls back to
//! `fixtures/profile-<id>.json`. `offline` skips the network entirely and
//! reads the recorded fixtures — that is how the headless smoke test and
//! the integration test generate a REAL page hermetically.
//!
//! Icon delivery is a mode: the product/resident page hotlinks the WFCD
//! CDN (`embed_icons: false`), the review/demo export embeds ≤128px
//! data-URI thumbnails (`embed_icons: true`, addendum #23).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::elog::MissionCompletion;
use crate::model::{OwnedEquipment, RelicStock};
use crate::server::GenerationHub;
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
    /// Offline/fixture dir (`worldstate.json`, profile). Online mode never
    /// substitutes a fixture for a failed DE fetch (addendum #51).
    pub fixtures: PathBuf,
    /// HTTP cache dir (world state / profile TTL caches).
    pub cache_dir: PathBuf,
    /// Where the rendered HTML page is written (atomic temp + rename).
    pub out: PathBuf,
    /// OCR-captured stock (`stock.json`): relics/parts/arcanes captured by
    /// the §2 Capture Inventory flow. Read at plan time; a missing or
    /// unreadable file is treated as unknown stock, never an error.
    pub stock_path: PathBuf,
    /// App config file (pins + steel-path tiebreak).
    pub config_path: PathBuf,
    /// Skip the network; read the recorded fixtures (hermetic).
    pub offline: bool,
    /// Embed ≤128px data-URI icons (demo) instead of CDN hotlinks.
    pub embed_icons: bool,
    /// Generation push (item 58). When set (the resident's loopback server
    /// provides one), the freshly rendered generation is published to open
    /// WebSocket pages at the same instant the report file is renamed into
    /// place. `--demo` and tests leave it `None`.
    pub push: Option<Arc<GenerationHub>>,
}

impl ReportConfig {
    /// Product/resident defaults for a report written to the app data dir.
    pub fn resident(out: PathBuf, config_path: PathBuf) -> Self {
        let stock_path = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.join("stock.json"))
            .unwrap_or_else(|| PathBuf::from("stock.json"));
        Self {
            data_dir: crate::tuning::data_dir(),
            fixtures: PathBuf::from("fixtures"),
            cache_dir: config::cache_dir(),
            out,
            stock_path,
            config_path,
            offline: false,
            embed_icons: false,
            push: None,
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
    /// The run skipped the network and read fixtures (`--offline`).
    pub offline: bool,
    pub generated_at: DateTime<Utc>,
    pub out: PathBuf,
    /// The generation token stamped on this page (item 52/58).
    pub generation: String,
}

impl ReportOutcome {
    /// Honest endpoints label: explicit failures, else "offline
    /// (fixtures)" for a fixture-only run, else "all live".
    pub fn endpoints_label(&self) -> String {
        if !self.endpoint_failures.is_empty() {
            self.endpoint_failures.join("; ")
        } else if self.offline {
            "offline (fixtures)".to_owned()
        } else {
            "all live".to_owned()
        }
    }

    /// One-line status ("61 goals · 11 chunks · live").
    pub fn summary(&self) -> String {
        format!(
            "{} goals, {} chunks, {} bars ({})",
            self.goals,
            self.chunks,
            self.bars,
            self.endpoints_label()
        )
    }
}

/// Render the checklist page from the configured inputs. Never dies on a
/// dead endpoint: each fetch falls back to its recorded fixture.
///
/// `completed` is the resident's log-proven, today-scoped mission ledger
/// (ninth pass §4). Every report — auto, manual, or session-start — takes
/// it so a finished worldstate alert renders struck regardless of any
/// manual tick; the CLI demo passes an empty slice.
/// A token unique to one regeneration (item 52). Wall-clock nanos alone
/// are not enough in tests/tight loops, so a process-local sequence is
/// appended; across restarts the timestamp keeps it distinct.
fn next_generation() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let now = Utc::now();
    let nanos = now
        .timestamp_nanos_opt()
        .unwrap_or_else(|| now.timestamp_millis() * 1_000_000);
    format!("{nanos}-{seq}")
}

pub async fn generate(
    cfg: &ReportConfig,
    completed: &[MissionCompletion],
) -> Result<ReportOutcome, String> {
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

    // 1. World state: DE is the ONLY source (addendum #51). No fallback
    // chain and no silent substitution: a failed fetch renders as
    // unavailable; the disk cache may serve only as an explicitly-labeled
    // stale view. A TTL hit (addendum #120) renders as "cached" with
    // its age — never STALE, which stays reserved for the failure
    // path — and the offline fixture renders as STALE, as before.
    let decodes = model::Decodes::load(&cfg.data_dir)?;
    let (world_text, world_source) = match &client {
        Some(client) => match fetch::fetch_worldstate(client, &cfg.cache_dir).await {
            Ok(fetched) => (
                fetched.value,
                match fetched.status {
                    fetch::FetchStatus::Fresh => model::WorldstateSource::Live,
                    fetch::FetchStatus::TtlCache { age } => {
                        model::WorldstateSource::TtlCache {
                            age_mins: age.as_secs() / 60,
                        }
                    }
                    _ => model::WorldstateSource::Fallback,
                },
            ),
            Err(e) => {
                endpoint_failures.push(format!("worldstate: {e}"));
                (String::new(), model::WorldstateSource::Fallback)
            }
        },
        None => {
            let path = cfg.fixtures.join("worldstate.json");
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("fixture worldstate {}: {e}", path.display()))?;
            (text, model::WorldstateSource::Fallback)
        }
    };
    let world = if world_text.trim().is_empty() {
        model::WorldState::default()
    } else {
        model::WorldState::from_de_json(&world_text, &decodes)?
    };
    if !world.decode_gaps.is_empty() {
        eprintln!(
            "worldstate: {} undecoded key(s): {}",
            world.decode_gaps.len(),
            world.decode_gaps.join(", ")
        );
    }
    let now = world
        .timestamp
        .parse::<DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());
    // Freshness is judged by the payload's own Time against the render
    // clock, not by the fetch time — except for a TTL hit, whose
    // label carries the cache age instead (addendum #120).
    let world_status = model::worldstate_status_label(&world, Utc::now(), world_source);

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
    let mut roster = model::Roster::from_json(&wish_text)?;
    // Pass 42: required build quantities (data/required.toml) —
    // the wishlist feed carries part names only. A missing or
    // unparseable table degrades to the all-x1 default (the
    // pre-pass behaviour) with the failure surfaced, never a
    // guessed count.
    match std::fs::read_to_string(data_file("required.toml"))
        .map_err(|e| e.to_string())
        .and_then(|text| model::load_required_counts(&text))
    {
        Ok(counts) => roster.apply_required_counts(&counts),
        Err(e) => endpoint_failures.push(format!("required counts: {e}")),
    }

    // 4. Fresh profile pull -> owned equipment + daily REMAINING caps.
    let (owned, mastery_rank, daily, profile_status) = match &client {
        Some(client) => match fetch::fetch_profile(client, &cfg.cache_dir, PROFILE_ACCOUNT_ID).await
        {
            Ok(fetched) => {
                let snap = fetched.value;
                let (o, d) = profile_context(&tuning, &cfg.data_dir, &snap, Some(now), now);
                (o, snap.mastery_rank, d, fetched.status.label())
            }
            Err(e) => {
                endpoint_failures.push(format!("profile: {e}"));
                profile_fallback(&tuning, &cfg.data_dir, &cfg.cache_dir, &cfg.fixtures, now)?
            }
        },
        None => profile_from_fixture(&tuning, &cfg.data_dir, &cfg.fixtures, now)?,
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

    // Addendum #84: the parsed capture state feeds BOTH planner
    // shapes — relic stock as before, plus the canonical part-stock
    // map (built + blueprint forms summed per roster part, #89) that
    // lights the Hunt/Backlog part states. Addendum #93 adds the
    // third shape: Seth's unverified (⍻) assertions, already keyed
    // by roster part name, driving PartState::HaveUnverified.
    let (stock, part_stock, unverified_parts) = match std::fs::read_to_string(&cfg.stock_path) {
        Ok(text) => match crate::ocr::StockState::from_json(&text) {
            Ok(s) => (
                s.to_relic_stock(),
                s.canonical_part_counts(&roster),
                s.unverified_part_counts(),
            ),
            Err(e) => {
                endpoint_failures.push(format!("stock {}: {e}", cfg.stock_path.display()));
                (RelicStock::Unknown, HashMap::new(), HashMap::new())
            }
        },
        Err(_) => (RelicStock::Unknown, HashMap::new(), HashMap::new()),
    };

    let (cfg_file, _) = config::Config::load(&cfg.config_path)
        .unwrap_or((config::Config::default(), config::ConfigSource::Defaults));

    let item_names = fetch::load_item_names(&cfg.data_dir);
    // Pass 48: the overlay's relic icons, resolved at generation
    // time from the baked ExportManifest texture classes joined
    // through item-names (Intact class preferred; pass 52). No
    // network — the textures bake with the other DE data
    // (data/cache/README.md); an absent table degrades every
    // relic header to its monogram.
    let relic_textures = fetch::load_relic_textures(&cfg.data_dir);
    let relic_icons = model::relic_icon_urls(&item_names, &relic_textures);
    let input = plan::PlanInput {
        now,
        world: &world,
        item_names: &item_names,
        owned: &owned,
        wants: &wants,
        roster: &roster,
        relics: &relics,
        stock: &stock,
        part_stock: &part_stock,
        unverified_parts: &unverified_parts,
        pins: &cfg_file.pins,
        recent_gear: &HashSet::new(),
        steel_path_tiebreak: cfg_file.steel_path_tiebreak,
        completed_missions: completed,
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

    // Item 52: unique per REGENERATION (not the worldstate's timestamp,
    // which can repeat across runs). The server records this value with
    // the remembered tab; since addendum #86 the choice itself persists
    // across generations. Item 58 publishes this token to open pages
    // once the page file is in place.
    let generation = next_generation();
    // The core-event log (fifty-first pass): beside stock.json,
    // read fresh on every report so the Log tab and the toast
    // beacon carry the resident's latest state changes. A
    // missing file (fresh install) is an empty log.
    let events_dir = cfg
        .stock_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let events = crate::events::load(&events_dir);
    let ctx = render::RenderContext {
        world: &world,
        generated_label: now.format("%Y-%m-%d %H:%M UTC").to_string(),
        generation: generation.clone(),
        targets_status: format!(
            "backlog {} primes · profile {}",
            backlog.primes.unmastered.len(),
            profile_status,
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
        // The search overlay's data (forty-seventh pass): the
        // same locals the planner just consumed — relic tables,
        // relic totals, canonical part stock, ⍻ assertions — so
        // the overlay index renders this plan's own truth.
        search: Some(render::SearchData {
            relics: &relics,
            stock: &stock,
            part_stock: &part_stock,
            unverified_parts: &unverified_parts,
            relic_icons: &relic_icons,
        }),
        events: &events,
    };
    let html = render::render_plan(&plan, &ctx);

    if let Some(parent) = cfg.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("report out dir {}: {e}", parent.display()))?;
        }
    }
    // The search overlay's index (forty-seventh pass): rendered
    // from the same plan + context and published as a sidecar
    // beside the report, BEFORE the page itself — a client that
    // sees the new generation and refetches the index always
    // finds the matching one on disk. Same tmp+rename discipline;
    // a sidecar failure degrades the overlay only, never the
    // report, so it is swallowed here (best-effort sidecar,
    // load-bearing page — the reward-detection precedent).
    let search_html = render::search_index_html(&plan, &ctx);
    let search_path = crate::search::search_index_path(&cfg.out);
    let search_tmp = search_path.with_extension("html.tmp");
    if std::fs::write(&search_tmp, &search_html).is_ok() {
        let _ = std::fs::rename(&search_tmp, &search_path);
    }
    let tmp = cfg.out.with_extension("html.tmp");
    std::fs::write(&tmp, html).map_err(|e| format!("write report: {e}"))?;
    std::fs::rename(&tmp, &cfg.out).map_err(|e| format!("publish report: {e}"))?;
    // Item 58: the page is now atomically in place, so this is the exact
    // publication point that pushes the new generation to open pages. A
    // reload lands on the generation whose token rode the same run
    // (ticks stay server-side; the tab choice persists per addendum
    // #86).
    if let Some(hub) = &cfg.push {
        hub.publish(&generation);
    }

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
        offline: cfg.offline,
        generated_at: now,
        out: cfg.out.clone(),
        generation,
    })
}

/// Owned equipment + profile-backed daily bars from one snapshot.
/// Name resolution uses `data/cache/item-names.json` (uniqueName ->
/// display name, built from the WFCD items dataset); unresolvable
/// entries are skipped, never guessed. Caps scale with mastery rank
/// per the runtime tuning tables (profile values are REMAINING).
///
/// Twelfth-pass §3 (reset-aware standing): the daily pools reset at
/// 00:00 UTC. When the snapshot was fetched before the most recent
/// reset (`fetched_at < today's 00:00Z`), the pools render as reset —
/// `remaining = cap` — until a fresh post-reset fetch lands. A fresh
/// snapshot always wins verbatim. `fetched_at = None` means "no
/// trustworthy fetch time" (e.g. a recorded fixture without a clock),
/// which renders verbatim rather than guessing.
fn profile_context(
    tuning: &Tuning,
    data_dir: &std::path::Path,
    snap: &fetch::ProfileSnapshot,
    fetched_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> (OwnedEquipment, Option<render::DailyRender>) {
    let names = fetch::load_item_names(data_dir);
    let owned = fetch::owned_names_from_profile(snap, &names);
    let mr = snap.mastery_rank;
    let caps = &tuning.planner.daily_caps;
    let focus_cap = caps.focus_base + caps.focus_per_mr * mr;
    let affiliation_cap = caps.standing_base + caps.standing_per_mr * mr;
    // 00:00 UTC on `now`'s date is the most recent reset boundary.
    let most_recent_reset = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight is a valid time")
        .and_utc();
    let reset = fetched_at.map(|t| t < most_recent_reset).unwrap_or(false);
    let daily = render::DailyRender {
        affiliation_remaining: if reset {
            affiliation_cap
        } else {
            snap.daily.affiliation_remaining
        },
        affiliation_cap,
        focus_remaining: if reset {
            focus_cap
        } else {
            snap.daily.focus_remaining
        },
        focus_cap,
        families: {
            // Pass 4 §28: PvP leaves with the Conclave cut — never a bar.
            // A reset pool reads at cap; a fresh fetch is verbatim.
            let mut f: Vec<(String, u32)> = snap
                .daily
                .per_family
                .iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("pvp"))
                .map(|(k, v)| (k.clone(), if reset { affiliation_cap } else { *v }))
                .collect();
            f.sort_by(|a, b| a.0.cmp(&b.0));
            f
        },
    };
    (owned, Some(daily))
}

/// Profile from the recorded fixture (used offline and on fetch failure).
/// A recorded fixture carries no trustworthy fetch clock, so it renders
/// verbatim (`fetched_at = None`); the reset rule applies to live and
/// cache snapshots, whose ages are known.
fn profile_from_fixture(
    tuning: &Tuning,
    data_dir: &std::path::Path,
    fixtures: &std::path::Path,
    now: DateTime<Utc>,
) -> Result<(OwnedEquipment, u32, Option<render::DailyRender>, String), String> {
    let fixture = fixtures.join(format!("profile-{PROFILE_ACCOUNT_ID}.json"));
    let text =
        std::fs::read_to_string(&fixture).map_err(|e| format!("fixture profile: {e}"))?;
    let snap = fetch::parse_profile(&text)?;
    let (o, d) = profile_context(tuning, data_dir, &snap, None, now);
    Ok((o, snap.mastery_rank, d, "fixture".to_owned()))
}

/// Profile for a report when the live fetch failed. The on-disk live cache
/// is a real per-day REMAINING snapshot (possibly an hour or two stale, but
/// the profile's own numbers); the recorded fallback fixture is a fixed
/// older snapshot. Prefer the cache so a transient endpoint failure never
/// silently swaps in an unrelated (and possibly stale/zeroed) daily state
/// — then fall back to the fixture. Ninth pass §3. Twelfth pass §3: the
/// cache's file mtime is its fetch time, so a cache written before the
/// latest 00:00 UTC reset renders the daily pools as reset.
fn profile_fallback(
    tuning: &Tuning,
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    fixtures: &std::path::Path,
    now: DateTime<Utc>,
) -> Result<(OwnedEquipment, u32, Option<render::DailyRender>, String), String> {
    let cached = cache_dir.join(format!("profile-{PROFILE_ACCOUNT_ID}.json"));
    if let Ok(text) = std::fs::read_to_string(&cached) {
        if let Ok(snap) = fetch::parse_profile(&text) {
            let fetched_at = std::fs::metadata(&cached)
                .and_then(|m| m.modified())
                .ok()
                .map(DateTime::<Utc>::from);
            let (o, d) = profile_context(tuning, data_dir, &snap, fetched_at, now);
            return Ok((o, snap.mastery_rank, d, "cache".to_owned()));
        }
    }
    profile_from_fixture(tuning, data_dir, fixtures, now)
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

    /// Item 52: every regeneration gets a distinct token even when the
    /// worldstate payload (and therefore `now`) is unchanged.
    #[test]
    fn generation_tokens_are_unique_per_run() {
        let a = next_generation();
        let b = next_generation();
        assert_ne!(a, b);
        assert!(!a.is_empty() && !b.is_empty());
    }

    /// §3 end to end: two real generations from the SAME fixture payload
    /// still carry different `data-generation` values, so the second run
    /// resets the remembered tab. (The worldstate timestamp repeats, so
    /// deriving the token from `now` would have been a silent bug.)
    #[tokio::test]
    async fn consecutive_generations_stamp_different_tokens() {
        let dir = std::env::temp_dir().join(format!(
            "fracker-report-gen-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("plan.html");
        let cfg = test_cfg(out.clone());
        let attr = |html: &str| {
            let start = html.find("data-generation=\"").unwrap() + "data-generation=\"".len();
            let rest = &html[start..];
            rest[..rest.find('"').unwrap()].to_owned()
        };
        generate(&cfg, &[]).await.unwrap();
        let first = attr(&std::fs::read_to_string(&out).unwrap());
        generate(&cfg, &[]).await.unwrap();
        let second = attr(&std::fs::read_to_string(&out).unwrap());
        assert_ne!(first, second, "a new regeneration must invalidate tabs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Item 58 gate: the generation is pushed at publication, and the token
    /// on the wire is exactly the one stamped into the served page.
    #[tokio::test]
    async fn publishing_a_generation_pushes_it_to_open_pages() {
        let dir = std::env::temp_dir().join(format!(
            "fracker-report-push-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("plan.html");
        let hub = Arc::new(GenerationHub::default());
        let (_current, pushes) = hub.subscribe();

        let mut cfg = test_cfg(out.clone());
        cfg.push = Some(hub.clone());
        let outcome = generate(&cfg, &[]).await.expect("offline generate");

        // One frame per publication, naming the page's own generation.
        let pushed: String = pushes.try_recv().expect("push on publication");
        let message: serde_json::Value = serde_json::from_str(&pushed).unwrap();
        assert_eq!(message["generation"].as_str(), Some(outcome.generation.as_str()));

        let html = std::fs::read_to_string(&out).unwrap();
        let start = html.find("data-generation=\"").unwrap() + "data-generation=\"".len();
        let rest = &html[start..];
        let stamped = &rest[..rest.find('"').unwrap()];
        assert_eq!(
            stamped, outcome.generation,
            "the pushed token is the one on the page"
        );
        assert_eq!(hub.current(), outcome.generation);
        // A second regeneration pushes a different token.
        generate(&cfg, &[]).await.expect("second generate");
        let second: String = pushes.try_recv().expect("second push");
        assert_ne!(second, pushed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_cfg(out: PathBuf) -> ReportConfig {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        ReportConfig {
            data_dir: root.join("data"),
            fixtures: root.join("fixtures"),
            cache_dir: std::env::temp_dir().join("fracker-report-test-cache"),
            out,
            stock_path: root.join("data/stock.json"),
            config_path: root.join("does-not-exist.toml"),
            offline: true,
            embed_icons: false,
            push: None,
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
        let outcome = generate(&cfg, &[]).await.expect("offline generate");
        assert!(out.exists(), "page written");
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.contains("<html"), "real HTML page");
        assert!(outcome.goals > 0, "goal cards from the backlog");
        assert!(!outcome.endpoint_failures.iter().any(|f| f.starts_with("worldstate")));
        assert_eq!(outcome.endpoints_label(), "offline (fixtures)");
        // Product page keeps CDN hotlinks (embedded_icons None).
        assert!(html.contains("cdn.warframestat.us"), "product page hotlinks icons");
        // Addendum #51: the worldstate stamp is rendered and an offline
        // fixture view is explicitly labeled STALE, never silently live.
        assert!(
            html.contains("worldstate STALE ("),
            "worldstate freshness stamp is rendered"
        );
        assert!(!html.contains("worldstate live ("));
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

    /// Ninth pass §3 audit: the page must render each pool's REMAINING
    /// exactly as the profile reports it — including the bare shared
    /// `DailyAffiliation` pool, whose partial value the app reported as
    /// "0 left". Runs against the live snapshot when present (gitignored;
    /// skipped on a box that lacks it).
    #[test]
    fn live_profile_renders_every_pool_at_its_raw_remaining() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let live = root.join("fixtures/ocr-samples/profile-2026-10-17.json");
        let Ok(text) = std::fs::read_to_string(&live) else {
            eprintln!(
                "skip §3 audit: need the live profile fixture at {}",
                live.display()
            );
            return;
        };
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
        let raw = &raw["Results"][0];
        let snap = fetch::parse_profile(&text).unwrap();
        let tuning = Tuning::load(&root.join("data")).unwrap();
        let caps = &tuning.planner.daily_caps;
        let (_owned, daily) = profile_context(
            &tuning,
            &root.join("data"),
            &snap,
            None,
            "2026-10-17T12:00:00Z".parse().unwrap(),
        );
        let daily = daily.expect("daily render from the live profile");
        let html = render::pool_bars_html(&daily, caps);

        // Parse the rendered `.cap` blocks back into label -> remaining.
        // Twelfth pass §1: the visible number is compact (`349.6k`); the
        // raw values ride `data-left` / `data-cap` for this audit.
        let mut rendered = HashMap::new();
        for chunk in html.split("<div class=\"cap\"").skip(1) {
            let left = chunk
                .split("data-left=\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
                .expect("data-left")
                .parse::<u32>()
                .unwrap();
            let label = chunk
                .split("<span>")
                .nth(1)
                .and_then(|s| s.split("</span>").next())
                .expect("cap label")
                .to_owned();
            rendered.insert(label, left);
        }

        // Focus.
        let raw_focus = raw["DailyFocus"].as_u64().unwrap() as u32;
        assert_eq!(
            rendered.get("Focus"),
            Some(&raw_focus),
            "Focus renders its raw remaining"
        );
        assert_eq!(snap.daily.focus_remaining, raw_focus);

        // The bare shared pool ("Syndicates") — the reported defect.
        let raw_main = raw["DailyAffiliation"].as_u64().unwrap() as u32;
        assert_eq!(
            rendered.get(caps.label_for("main")),
            Some(&raw_main),
            "the bare DailyAffiliation pool renders its raw remaining"
        );
        assert_eq!(snap.daily.affiliation_remaining, raw_main);

        // Every family pool, including the untouched (at-cap) ones.
        let mut families = 0;
        for (k, v) in raw.as_object().unwrap() {
            let Some(fam) = k.strip_prefix("DailyAffiliation") else {
                continue;
            };
            if fam.is_empty() || fam.eq_ignore_ascii_case("pvp") {
                continue;
            }
            let raw_val = v.as_u64().unwrap() as u32;
            assert_eq!(
                rendered.get(caps.label_for(fam)),
                Some(&raw_val),
                "family {fam} renders its raw remaining"
            );
            families += 1;
        }
        assert_eq!(families, 12, "all 12 family pools audited");
        assert_eq!(rendered.len(), families + 2, "Focus + bare pool + families");
    }

    /// Twelfth pass §3: a snapshot fetched before the latest 00:00 UTC
    /// reset renders every daily pool as reset (remaining = cap); a
    /// post-reset snapshot renders verbatim.
    #[test]
    fn reset_boundary_is_reset_aware_and_fresh_fetch_wins() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture = root.join(format!(
            "fixtures/profile-{PROFILE_ACCOUNT_ID}.json"
        ));
        let Ok(text) = std::fs::read_to_string(&fixture) else {
            eprintln!("skip §3 reset audit: need {}", fixture.display());
            return;
        };
        let snap = fetch::parse_profile(&text).unwrap();
        let tuning = Tuning::load(&root.join("data")).unwrap();
        let data = root.join("data");

        // Pre-reset fetch (23:50 on Oct 16), rendered after the Oct 17
        // 00:00Z boundary: every pool reads at cap.
        let (_owned, daily) = profile_context(
            &tuning,
            &data,
            &snap,
            Some("2026-10-16T23:50:00Z".parse().unwrap()),
            "2026-10-17T00:10:00Z".parse().unwrap(),
        );
        let daily = daily.unwrap();
        assert_eq!(
            daily.focus_remaining, daily.focus_cap,
            "pre-reset focus renders at cap"
        );
        assert_eq!(
            daily.affiliation_remaining, daily.affiliation_cap,
            "pre-reset shared pool renders at cap"
        );
        for (fam, remaining) in &daily.families {
            assert_eq!(
                *remaining, daily.affiliation_cap,
                "pre-reset family {fam} renders at cap"
            );
        }

        // Fresh post-reset fetch: verbatim (pass 9 §3 rule stands).
        let (_owned, daily) = profile_context(
            &tuning,
            &data,
            &snap,
            Some("2026-10-17T00:05:00Z".parse().unwrap()),
            "2026-10-17T00:10:00Z".parse().unwrap(),
        );
        let daily = daily.unwrap();
        assert_eq!(daily.focus_remaining, snap.daily.focus_remaining);
        assert_eq!(
            daily.affiliation_remaining,
            snap.daily.affiliation_remaining
        );
        for (fam, remaining) in &daily.families {
            assert_eq!(
                *remaining,
                snap.daily.per_family[fam],
                "fresh family {fam} renders verbatim"
            );
        }
    }
}
