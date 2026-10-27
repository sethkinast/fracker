//! Capture Inventory — the tray-driven live screen flow (eighth pass §2,
//! addendum #39).
//!
//! Seth rejected the screenshot-folder + CLI ingest UX: the final product
//! is a **tray toggle**. He clicks *Capture Inventory* and simply scrolls
//! his inventory in-game; Fracker watches the screen (ONE ScreenCast
//! consent — see [`crate::screencast`]), classifies which screen is
//! showing (Void Relics / Sell / Arcanes), and collects stacks as they
//! scroll past. Stop finalises: captured buckets replace the matching
//! buckets in `stock.json`, the plan regenerates, and the tray reports
//! what was collected.
//!
//! This module is the *pipeline* — deliberately free of the portal and
//! PipeWire so it is fully exercisable on the build box (the §2 gate:
//! feed the real shots as overlapping frame sequences and prove the union
//! equals the §1 validation result with no double counts):
//!
//! - [`ScreenClassifier`] reads one header strip and matches data-owned
//!   keywords (`data/capture/screens.toml`) to a [`ScreenClass`];
//! - [`CaptureSession::process_frame`] skips unchanged frames, classifies
//!   stabilised ones, routes cells to the matching §1 layout, and folds
//!   held reads into a **union** of stacks (see [`crate::ocr::StockState`]);
//! - [`CaptureSession::finalize_into`] replaces only the buckets that saw
//!   classified frames, so a partial capture never zeroes anything.
//!
//! Frames arrive through the [`FrameSource`] trait: the real implementation
//! is the ScreenCast/PipeWire source (`crate::screencast`); tests use
//! [`SequenceSource`]. Nothing here touches the network.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::ocr::{
    self, GrayImage, Layout, OcrEngine, RefinementTier, RelicScreenVariant, StockState, Universe,
};

/// Which inventory screen a frame shows. `Other` is any frame with no
/// matching class (menus, gameplay, loading) — collection pauses silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenClass {
    Relics,
    Parts,
    Arcanes,
    Other,
}

impl ScreenClass {
    /// The `data/capture/layouts.toml` table for this screen, if any.
    pub fn layout_key(self) -> Option<&'static str> {
        match self {
            ScreenClass::Relics => Some("relic"),
            ScreenClass::Parts => Some("parts"),
            ScreenClass::Arcanes => Some("arcanes"),
            ScreenClass::Other => None,
        }
    }

    /// The human name used in tray status.
    pub fn label(self) -> &'static str {
        match self {
            ScreenClass::Relics => "relics",
            ScreenClass::Parts => "parts",
            ScreenClass::Arcanes => "arcanes",
            ScreenClass::Other => "unclassified",
        }
    }
}

/// Per-screen header-keyword classifier (data-owned). `classify` OCRs the
/// `header` strip and scores each class by how many of its keywords appear;
/// the highest score wins, ties fall to `Other` (never a coin flip).
#[derive(Debug, Clone, Default)]
pub struct ScreenClassifier {
    /// `[x0,y0,x1,y1]` fraction-of-frame region holding the screen banner.
    pub header: [f32; 4],
    pub header_psm: u32,
    pub header_upscale: u32,
    /// Auto-finalise after this many consecutive unclassified frames
    /// (0 = never; the tray toggle is always the primary stop).
    pub idle_stop_frames: u64,
    pub relic_keywords: Vec<String>,
    pub parts_keywords: Vec<String>,
    pub arcane_keywords: Vec<String>,
}

#[derive(Deserialize)]
struct ScreensFile {
    #[serde(default = "default_header")]
    header: [f32; 4],
    #[serde(default = "default_header_psm")]
    header_psm: u32,
    #[serde(default = "default_header_upscale")]
    header_upscale: u32,
    #[serde(default)]
    idle_stop_frames: u64,
    #[serde(default)]
    relic: KeywordTable,
    #[serde(default)]
    parts: KeywordTable,
    #[serde(default)]
    arcanes: KeywordTable,
}

#[derive(Deserialize, Default)]
struct KeywordTable {
    #[serde(default)]
    keywords: Vec<String>,
}

fn default_header() -> [f32; 4] {
    [0.0, 0.0, 1.0, 0.12]
}
fn default_header_psm() -> u32 {
    6
}
fn default_header_upscale() -> u32 {
    2
}

impl ScreenClassifier {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let f: ScreensFile = toml::from_str(text).map_err(|e| format!("screens: {e}"))?;
        Ok(Self {
            header: f.header,
            header_psm: f.header_psm,
            header_upscale: f.header_upscale,
            idle_stop_frames: f.idle_stop_frames,
            relic_keywords: f.relic.keywords,
            parts_keywords: f.parts.keywords,
            arcane_keywords: f.arcanes.keywords,
        })
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("screens {}: {e}", path.display()))?;
        Self::from_toml(&text)
    }

    /// Score a header string against the keyword tables. Pure + tested.
    pub fn classify_text(&self, text: &str) -> ScreenClass {
        // Normalise before matching (addendum #83): the title
        // font loses and splits glyphs under OCR — "INVENTORY"
        // reads back "VENTORY" (the IN ligature drops) and
        // "SELL" as "SEL L". Squashing to alphanumerics removes
        // the split artefacts; the fuzzy window pass forgives
        // the dropped ones. Exact substring matching on the raw
        // text classified by luck: all three of Seth's real
        // 2026-10-24 header reads lost "INVENTORY", and one
        // split "SELL" too — that frame went Other.
        let squashed: String = text
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let score = |kws: &[String]| {
            kws.iter()
                .filter(|k| {
                    let k: String = k
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .map(|c| c.to_ascii_uppercase())
                        .collect();
                    squashed.contains(&k) || fuzzy_keyword_present(&squashed, &k)
                })
                .count()
        };
        let mut best = (ScreenClass::Other, 0usize);
        for (class, kws) in [
            (ScreenClass::Relics, &self.relic_keywords),
            (ScreenClass::Parts, &self.parts_keywords),
            (ScreenClass::Arcanes, &self.arcane_keywords),
        ] {
            let n = score(kws);
            if n > best.1 {
                best = (class, n);
            }
        }
        best.0
    }

    /// Pass 43: which relic screen variant a header belongs
    /// to. The refinement screen's title names itself —
    /// "VOID RELICS / REFINEMENT" — while the selection
    /// picker's does not, so the variant is the presence of
    /// the REFINEMENT keyword under the same squash + fuzzy
    /// discipline as [`classify_text`](Self::classify_text)
    /// (a long keyword: 2 edits of allowance). Measured on
    /// Seth's 25 native refinement headers (2026-10-26): the
    /// word read verbatim in 25/25, inside a header that also
    /// carries the COLLECTED counters the picker lacks.
    pub fn relic_variant(&self, header_text: &str) -> RelicScreenVariant {
        let squashed: String = header_text
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if squashed.contains("REFINEMENT") || fuzzy_keyword_present(&squashed, "REFINEMENT") {
            RelicScreenVariant::Refinement
        } else {
            RelicScreenVariant::Selection
        }
    }

    /// OCR the header strip of `frame` and classify it.
    pub fn classify(&self, engine: &dyn OcrEngine, frame: &GrayImage) -> ScreenClass {
        self.classify_detailed(engine, frame).0
    }

    /// Like [`classify`](Self::classify), but also returns the raw
    /// header text — the evidence a zero-collection diagnosis needs.
    pub fn classify_detailed(
        &self,
        engine: &dyn OcrEngine,
        frame: &GrayImage,
    ) -> (ScreenClass, String) {
        let text = engine.read_text(frame, self.header, self.header_psm, self.header_upscale);
        let class = self.classify_text(&text);
        (class, text)
    }
}

/// Is `keyword` present in the squashed header text up to a small
/// edit distance? Slides windows of keyword-length ±2 over the
/// text and takes the best confusable-aware distance; the
/// allowance is 1 edit for short keywords, 2 for long ones (a
/// dropped ligature like INVENTORY's "IN" costs exactly 2). The
/// window count is tiny — this runs once per fresh header.
fn fuzzy_keyword_present(squashed: &str, keyword: &str) -> bool {
    let hay: Vec<char> = squashed.chars().collect();
    let needle: Vec<char> = keyword.chars().collect();
    if needle.len() < 4 || hay.len() < needle.len() - 2 {
        return false;
    }
    let allowance = if needle.len() >= 8 { 2 } else { 1 };
    let lo = needle.len().saturating_sub(2).max(1);
    let hi = needle.len() + 2;
    for wlen in lo..=hi {
        if wlen > hay.len() {
            break;
        }
        for start in 0..=(hay.len() - wlen) {
            let window: String = hay[start..start + wlen].iter().collect();
            if crate::ocr::levenshtein_confusable(&window, keyword) <= allowance {
                return true;
            }
        }
    }
    false
}

/// The static inputs a capture run needs (loaded once from `data/`). Built
/// by [`CaptureData::load`]; the resident reuses it for every capture.
pub struct CaptureData {
    pub layouts: HashMap<String, Layout>,
    pub classifier: ScreenClassifier,
    pub universes: UniverseSet,
    pub relic_names: HashSet<String>,
    pub arcane_names: HashSet<String>,
    /// The directory the data was loaded from (the install tree in
    /// the Flatpak — read-only there, so nothing may write into it).
    pub data_dir: PathBuf,
    /// Where capture debug dumps (evidence frames) land:
    /// `capture-debug` under the USER data dir (beside stock.json).
    /// This was `data_dir/capture-debug` until addendum #82 — in
    /// the Flatpak that path is the read-only bundle tree, so every
    /// dump failed silently and no folder ever appeared.
    pub debug_dir: PathBuf,
}

impl CaptureData {
    /// Load the layouts + classifier + item universe from a `data/`
    /// directory. The universe is the same scrubbed set the one-shot
    /// `--ocr-ingest` path builds (roster + relic names + the arcane
    /// universe), so live capture and the debug path agree.
    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let layouts = Layout::load_file(&data_dir.join("capture/layouts.toml"))?;
        let classifier = ScreenClassifier::load(&data_dir.join("capture/screens.toml"))?;
        let universes = load_universe(data_dir)?;
        Ok(Self {
            layouts,
            classifier,
            relic_names: universes.relic_names.clone(),
            arcane_names: universes.arcane_names.clone(),
            universes,
            data_dir: data_dir.to_path_buf(),
            debug_dir: crate::config::data_dir().join("capture-debug"),
        })
    }

    /// Borrow the per-frame capture context.
    pub fn context(&self) -> CaptureContext<'_> {
        CaptureContext {
            layouts: &self.layouts,
            universes: &self.universes,
            relic_names: &self.relic_names,
            arcane_names: &self.arcane_names,
            classifier: &self.classifier,
            min_confidence: 0.5,
            radius_decoder: true,
            radius_strip: true,
            radius_tail_budget: ocr::RADIUS_TAIL_BUDGET,
            line_reader: true,
            strip_offsets: true,
            snap_cells: false,
            synthesis: true,
            debug_dir: Some(self.debug_dir.clone()),
        }
    }
}

/// The OCR validation universes (pass 27): the historical combined
/// set plus one SCOPED set per inventory screen. Capture validates a
/// frame against its own screen's set only — under the combined set
/// a parts read could land on a relic or gear name (the census's
/// false-accept classes: "Axi O4", bare gear names from composite
/// frames), and the wider candidate field blunted every margin.
/// The combined set remains for callers that do not know the screen
/// (and as the `Other` fallback, which is never read in practice).
#[derive(Debug, Clone, Default)]
pub struct UniverseSet {
    pub combined: Universe,
    pub parts: Universe,
    pub relics: Universe,
    pub arcanes: Universe,
    /// Bucket-routing sets for [`StockState::from_results`] (the raw,
    /// unscrubbed name sets, as before).
    pub relic_names: HashSet<String>,
    pub arcane_names: HashSet<String>,
}

impl UniverseSet {
    /// The scoped universe for a classified screen.
    pub fn for_class(&self, class: ScreenClass) -> &Universe {
        match class {
            ScreenClass::Parts => &self.parts,
            ScreenClass::Relics => &self.relics,
            ScreenClass::Arcanes => &self.arcanes,
            ScreenClass::Other => &self.combined,
        }
    }

    /// The scoped universe for a layout key ("parts" / "relic" /
    /// "arcanes"); any other key falls back to the combined set.
    pub fn for_layout(&self, key: &str) -> &Universe {
        match key {
            "parts" => &self.parts,
            "relic" => &self.relics,
            "arcanes" => &self.arcanes,
            _ => &self.combined,
        }
    }
}

/// Build the OCR item universes exactly as the one-shot `--ocr-ingest`
/// path does (roster + relic names + the arcane universe, scrubbed). Kept
/// as a free function so both the live capture and the debug ingest share
/// one definition — they can never drift.
pub fn load_universe(data_dir: &Path) -> Result<UniverseSet, String> {
    let scrub_path = data_dir.join("exclusions.toml");
    let scrub = crate::model::UniverseScrub::from_toml(
        &std::fs::read_to_string(&scrub_path)
            .map_err(|e| format!("exclusions {}: {e}", scrub_path.display()))?,
    )?;
    let arcane_path = data_dir.join("arcanes.toml");
    let arcanes = crate::model::ArcaneManifest::from_toml(
        &std::fs::read_to_string(&arcane_path)
            .map_err(|e| format!("arcanes {}: {e}", arcane_path.display()))?,
    )?;
    let rel_text = std::fs::read_to_string(data_dir.join("cache/relictables.json"))
        .map_err(|e| format!("seed relictables: {e}"))?;
    let wish_text = std::fs::read_to_string(data_dir.join("cache/wishlist.json"))
        .map_err(|e| format!("seed wishlist: {e}"))?;
    let relics = crate::model::RelicTables::from_json(&rel_text)?;
    let roster = crate::model::Roster::from_json(&wish_text)?;

    // Parts candidates: roster part names plus the BUILT form of
    // every component blueprint (addendum #89) — the Parts screen
    // shows crafted components without the "Blueprint" suffix, and
    // both forms are first-class stock. Derivation happens BEFORE
    // the scrub, so an excluded item's built forms (Kavasa Prime,
    // addendum #22) are dropped by the same prefix rule as its
    // blueprint forms.
    let mut gear_names: Vec<String> = Vec::new();
    let mut part_names: Vec<String> = Vec::new();
    for item in &roster.items {
        gear_names.push(item.name.clone());
        for p in &item.parts {
            part_names.push(p.name.clone());
            if let Some(built) = crate::model::built_component_form(&item.name, &p.name) {
                part_names.push(built);
            }
        }
    }
    let relic_list: Vec<String> = relics.relics.keys().cloned().collect();
    let arcane_list: Vec<String> = arcanes.names.clone();

    let scrubbed = |v: &[String]| {
        let mut x = scrub.scrub_names(v);
        x.sort();
        x.dedup();
        x
    };
    let parts = scrubbed(&part_names);
    let relic_names_scrubbed = scrubbed(&relic_list);
    let arcane_names_scrubbed = scrubbed(&arcane_list);
    let mut combined_in = gear_names;
    combined_in.extend(part_names);
    combined_in.extend(relic_list.iter().cloned());
    combined_in.extend(arcane_list.iter().cloned());
    let combined = scrubbed(&combined_in);

    Ok(UniverseSet {
        combined: Universe::from_names(&combined),
        parts: Universe::from_names(&parts),
        relics: Universe::from_names(&relic_names_scrubbed),
        arcanes: Universe::from_names(&arcane_names_scrubbed),
        relic_names: relic_list.into_iter().collect(),
        arcane_names: arcanes.name_set(),
    })
}

/// Per-frame, borrow-only inputs to the pipeline.
pub struct CaptureContext<'a> {
    pub layouts: &'a HashMap<String, Layout>,
    pub universes: &'a UniverseSet,
    pub relic_names: &'a HashSet<String>,
    pub arcane_names: &'a HashSet<String>,
    pub classifier: &'a ScreenClassifier,
    pub min_confidence: f32,
    /// Pass 29 (addendum #92): the radius decoder runs on refused
    /// parts reads (see `ocr::Universe::radius_decode_budget`). The
    /// flags exist so the census replay can A/B the decoder against
    /// the pass-27 baseline through the production path itself.
    pub radius_decoder: bool,
    /// Whether a refused parts cell whose band fragment does not
    /// decode earns a line-1 strip read + strip decode.
    pub radius_strip: bool,
    /// Tail-alignment budget for the decoder (see
    /// `ocr::RADIUS_TAIL_BUDGET`).
    pub radius_tail_budget: usize,
    /// Pass 34 (Pass B): the line reader runs alongside the
    /// band reader in the stitch path and the two arbitrate
    /// under the form channels (see `crate::lineread`). The
    /// flag exists so the census replay can A/B the second
    /// reader through the production path itself.
    pub line_reader: bool,
    /// Pass 34 (brief §4): the radius decoder's strip phase
    /// reads at a small fixed set of vertical offsets around
    /// the fitted placement and arbitrates the decodes by
    /// agreement — strip OCR on fitted cells is extremely
    /// phase-sensitive (measured clean at −8px, garbage at 0),
    /// and a single fixed placement lands on the bad phase for
    /// exactly the cells that need the strip.
    pub strip_offsets: bool,
    /// Pass 34 (brief §4, ablation): snap the stitch's crop
    /// origins to content-detected tile boundaries instead of
    /// the fitted grid. Measurement only; production keeps the
    /// fitted grid unless the ablation adopts snapping.
    pub snap_cells: bool,
    /// Pass 35: analysis-by-synthesis post-pass over the
    /// stitch report (see `crate::synth`). It adjudicates only
    /// slots the readers left unheld and never overrules a
    /// reader hold.
    pub synthesis: bool,
    /// When set, capture dumps evidence frames (the first frame, and
    /// the first frame of each recognised class) here as PNGs.
    pub debug_dir: Option<PathBuf>,
}

/// The read-path subset of [`CaptureContext`] (pass 36): the
/// scalar evidence/decoding switches [`read_cells`] and the line
/// reader actually consume. Split out so a stitch bundle's inputs
/// are owned values — a background stitch thread cannot borrow a
/// `CaptureContext`, but it can own this.
#[derive(Debug, Clone, Copy)]
pub struct ReadOptions {
    pub min_confidence: f32,
    pub radius_decoder: bool,
    pub radius_strip: bool,
    pub radius_tail_budget: usize,
    pub strip_offsets: bool,
    /// Pass 38: the session's owned-brightness reference for the
    /// relic owned-gate — the running maximum icon highlight over
    /// the session's earlier relic frames. `None` outside a
    /// session (folder ingest, replay seams): the gate then works
    /// from the frame's own split alone.
    pub relic_bright_ref: Option<f64>,
    /// Pass 43: which relic screen variant the frame came from,
    /// when the caller knows (the session reads it off the
    /// header). `Unknown` lets the refinement rule infer from
    /// the reads.
    pub relic_variant: RelicScreenVariant,
}

/// Everything one stitch bundle needs beyond its frames, layout,
/// and universe (pass 36): the read options plus the stitch-path
/// orchestration switches. Owned and `Send` by construction.
#[derive(Debug, Clone, Copy)]
pub struct StitchOptions {
    pub read: ReadOptions,
    pub line_reader: bool,
    pub snap_cells: bool,
}

impl CaptureContext<'_> {
    /// The read-path options this context implies.
    pub fn read_options(&self) -> ReadOptions {
        ReadOptions {
            min_confidence: self.min_confidence,
            radius_decoder: self.radius_decoder,
            radius_strip: self.radius_strip,
            radius_tail_budget: self.radius_tail_budget,
            strip_offsets: self.strip_offsets,
            relic_bright_ref: None,
            relic_variant: RelicScreenVariant::Unknown,
        }
    }

    /// The stitch-bundle options this context implies.
    pub fn stitch_options(&self) -> StitchOptions {
        StitchOptions {
            read: self.read_options(),
            line_reader: self.line_reader,
            snap_cells: self.snap_cells,
        }
    }
}

/// Live per-bucket stack counts shown in the tray while capturing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureCounts {
    pub relic_stacks: usize,
    pub parts: usize,
    pub arcanes: usize,
}

impl CaptureCounts {
    pub fn total(&self) -> usize {
        self.relic_stacks + self.parts + self.arcanes
    }
}

/// Pass 36 failsafe + intake constants. The arithmetic behind
/// each is stated in docs/thirty-sixth-pass.md; in short, they
/// are sized from Seth's real inventory (~293 parts ≈ 27 rows
/// ≈ 9 viewports) and his observed scroll (~48 s for the parts
/// list at ~3 s per viewport settle).

/// Accepted frames per viewport position at intake (scope 1):
/// the canonical median needs 3 pooled instances; a fourth
/// leaves headroom for the gain check to exclude a transient
/// and still pool 3. Seth's stuck run accepted 31 of one page.
pub const VIEWPORT_ACCEPT_CAP: usize = 4;
/// Most frames in one stitch bundle (the pass-33 flush cap's
/// memory envelope, kept: 48 frames × ~3.7 MB ≈ 180 MB peak).
pub const BUNDLE_MAX_FRAMES: usize = 48;
/// Fewest frames in one stitch bundle before a progress flush
/// is honoured. The stitch's pitch/segment fit needs context:
/// measured on the HC census, a 4-frame head bundle fit its
/// region independently of the main bundle and produced 29
/// instance-disagreement flags and +15 marginal slots against
/// the single-bundle run (0 flags), costing 2 distinct holds at
/// merge; the unsplit run reproduces the shipped numbers
/// exactly. 8 frames ≈ 2–3 viewport positions at the accept cap
/// is the smallest bundle whose fit is self-supporting; live
/// bundles collect it from 2–3 settles, so the floor costs no
/// responsiveness — bundles still close every ~2 viewports of
/// scroll, nowhere near the incident's 48-of-two-viewports.
pub const BUNDLE_MIN_FRAMES: usize = 8;
/// Frames of drained bundles that may wait (coalesced) for the
/// stitch thread: two bundles' worth. Together with one bundle
/// in flight and the accumulating one, worst-case buffered
/// frames stay under ~200 (≈740 MB transient at 1440p, only
/// when processing has fallen badly behind a fast scroll).
pub const BUNDLE_QUEUE_MAX_FRAMES: usize = 96;
/// Failsafe (addendum item 1): hard cap on accepted parts
/// frames per session. A full scroll needs ≈ 9 viewports ×
/// (4 settles + a few transition frames) ≈ 60; a hesitant,
/// micro-stopped scroll might triple that. 512 is ~3× the
/// padded estimate and ~8× the typical need — past it the
/// session is not scrolling, it is leaking, and intake stops
/// (the frames are counted, the session still finalizes).
pub const PARTS_SESSION_FRAME_CAP: u64 = 512;
/// Failsafe (addendum item 3): a capture session's maximum
/// wall-clock duration before `run_capture` auto-finalizes.
/// Seth's full three-screen scroll takes ~2–4 minutes; 15 is
/// ~4× that, and bounds how long a forgotten capture can hold
/// the portal stream open (the idle auto-stop never fires on a
/// static inventory screen — it classifies Parts forever).
pub const SESSION_MAX_DURATION: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Pass 37 settle gate: widest |lattice phase| (page px) at
/// which a parts frame still counts as settled. Measured gap
/// (snap study + intake replay, two estimators agreeing within
/// ~0.5 px on real frames): settled frames and their landing
/// frames sit at |phase| ≤ ~3 px; the nearest scroll transient
/// sits at 14 px. 8.0 px rests inside the empty band with
/// ≥2.6× headroom over the worst settled measurement and
/// 1.75× under the nearest transient.
pub const SETTLED_PHASE_MAX_PX: f32 = 8.0;
/// Pass 37 time gate: minimum wall-clock interval between
/// accepted viewports — the owner's acceptance criterion
/// (~1 capture per couple of seconds; a rested page captured
/// exactly once). Offers inside the interval are dropped and
/// counted.
pub const MIN_ACCEPT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// What the parts intake decided about one frame (pass 36).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeVerdict {
    /// Into the stitch pool; `flush` asks the session to close
    /// the pending bundle BEFORE this frame is pushed (the
    /// frame opens a new bundle): a viewport of panorama
    /// progress has accumulated over the positions already
    /// pooled, and the boundary falls between positions.
    Accept { flush: bool },
    /// Same viewport as the last accepted frame, past the
    /// per-position cap: dropped at intake, counted.
    DropDuplicate,
    /// The session frame cap tripped: dropped at intake,
    /// counted; the session still finalizes normally.
    DropSessionCap,
    /// Pass 37: the frame's lattice phase is measurably off
    /// the settled band (a scroll transient): dropped at
    /// intake, counted.
    DropOffLattice,
    /// Pass 37: the frame arrived inside
    /// [`MIN_ACCEPT_INTERVAL`] of the last accepted viewport:
    /// dropped at intake, counted.
    DropInterval,
    /// Pass 37: the frame is the position the previous
    /// acceptance just displaced (an immediate scroll-back
    /// within the current bundle): dropped at intake, counted.
    DropLatched,
}

/// Content-aware parts intake (pass 36, brief scope 1+2). Each
/// offered frame is fingerprinted on the grid region in
/// gradient form ([`crate::stitch::viewport_print`]) and
/// compared against the last ACCEPTED frame: same viewport
/// (≥90% of cells the same content, ≤3 differing) is accepted
/// only up to [`VIEWPORT_ACCEPT_CAP`] frames per position —
/// the stitch pool wants a few instances per position, not
/// thirty-one — and contributes its small displacement to the
/// bundle progress regardless, so a slow creep still flushes.
/// A genuinely new position resets the run, adds its measured
/// displacement to the progress, and asks for a flush once a
/// viewport of panorama has accumulated since the last one.
/// Pure frame-in/decision-out (no engine, no session state), so
/// the whole policy is unit-testable.
///
/// Pass 37 gates, evaluated in [`Self::offer_at`] before the
/// pass-36 content policy: the SETTLE gate (a frame whose
/// lattice phase is measurably outside [`SETTLED_PHASE_MAX_PX`]
/// is a scroll transient and never becomes a candidate) and
/// the TIME gate (at most one accepted viewport per
/// [`MIN_ACCEPT_INTERVAL`] of wall clock — arrival times are
/// the caller's input, so the policy stays a pure function of
/// (frame, time)). Inside the content policy, the pass-37
/// LATCH keeps the position the previous acceptance displaced
/// from being re-accepted within the same bundle (an immediate
/// scroll-back); it is replaced by each new displacement and
/// cleared at bundle flush and on screen change.
pub struct PartsIntake {
    anchor: Option<crate::stitch::ViewportPrint>,
    /// Accepted frames at the anchor's position (incl. the
    /// frame that set the anchor).
    run: usize,
    /// |displacement| accumulated since the last flush (px).
    progress: f32,
    accepted_total: u64,
    session_cap: u64,
    cap_tripped: bool,
    /// Pass 37 latch: the print of the position the most
    /// recent acceptance displaced, within the current bundle.
    latched: Option<crate::stitch::ViewportPrint>,
    /// Pass 37 time gate: when the last viewport was accepted
    /// (the caller's clock). Session-scoped: a screen change
    /// does not rewind it.
    last_accept_at: Option<std::time::Instant>,
    /// Pass 37 gate counters (the finalize log's intake line).
    off_lattice_dropped: u64,
    interval_dropped: u64,
    latched_dropped: u64,
}

impl Default for PartsIntake {
    fn default() -> Self {
        Self::new()
    }
}

impl PartsIntake {
    pub fn new() -> Self {
        Self::with_session_cap(PARTS_SESSION_FRAME_CAP)
    }

    pub fn with_session_cap(session_cap: u64) -> Self {
        Self {
            anchor: None,
            run: 0,
            progress: 0.0,
            accepted_total: 0,
            session_cap,
            cap_tripped: false,
            latched: None,
            last_accept_at: None,
            off_lattice_dropped: 0,
            interval_dropped: 0,
            latched_dropped: 0,
        }
    }

    pub fn accepted_total(&self) -> u64 {
        self.accepted_total
    }

    pub fn cap_tripped(&self) -> bool {
        self.cap_tripped
    }

    /// Pass 37: frames dropped by the settle gate.
    pub fn off_lattice_dropped(&self) -> u64 {
        self.off_lattice_dropped
    }

    /// Pass 37: frames dropped by the time gate.
    pub fn interval_dropped(&self) -> u64 {
        self.interval_dropped
    }

    /// Pass 37: frames dropped by the per-position latch.
    pub fn latched_dropped(&self) -> u64 {
        self.latched_dropped
    }

    /// The screen changed away from Parts: the next parts frame
    /// is a new position wherever it lands. (The session total
    /// and the cap are session-scoped and persist.)
    pub fn reset_position(&mut self) {
        self.anchor = None;
        self.run = 0;
        self.progress = 0.0;
        self.latched = None;
    }

    /// A bundle drained: progress restarts and the latch clears
    /// (the latch is bundle-scoped — a position in a NEW bundle
    /// may be captured afresh).
    pub fn note_flush(&mut self) {
        self.progress = 0.0;
        self.latched = None;
    }

    /// Offer one parts-classified frame, delivered at `now`
    /// (the driver's clock; production passes real arrival
    /// times, drivers and tests model them). Gate order: the
    /// session cap, the settle gate, the time gate, then the
    /// content policy. An off-lattice verdict is charged to the
    /// settle gate even inside the interval — phase is a
    /// property of the frame, the interval of its timing, and
    /// the settle count is the one the evidence reviews read.
    pub fn offer_at(
        &mut self,
        frame: &GrayImage,
        layout: &Layout,
        now: std::time::Instant,
    ) -> IntakeVerdict {
        if self.accepted_total >= self.session_cap {
            self.cap_tripped = true;
            return IntakeVerdict::DropSessionCap;
        }
        // Pass 37 settle gate: a measurably off-lattice frame
        // is mid-scroll; its content straddles cell boundaries
        // and the pipeline is only ever measured on settled
        // rows. An UNMEASURABLE frame (no lattice fit — blank
        // or synthetic content) is not charged: the content
        // policy below is the judge of those.
        if let Some(phase) = crate::stitch::lattice_phase(frame, layout) {
            if phase.abs() > SETTLED_PHASE_MAX_PX {
                self.off_lattice_dropped += 1;
                return IntakeVerdict::DropOffLattice;
            }
        }
        // Pass 37 time gate: at most one accepted viewport per
        // MIN_ACCEPT_INTERVAL. A dropped offer does not move
        // the clock — the interval runs from the last ACCEPT.
        if let Some(last) = self.last_accept_at {
            if now.duration_since(last) < MIN_ACCEPT_INTERVAL {
                self.interval_dropped += 1;
                return IntakeVerdict::DropInterval;
            }
        }
        let verdict = self.offer_content(frame, layout);
        if matches!(verdict, IntakeVerdict::Accept { .. }) {
            self.last_accept_at = Some(now);
        }
        verdict
    }

    /// The pass-36 content policy (anchor/run dedupe + progress
    /// flush + the pass-37 latch), behind the gates.
    fn offer_content(&mut self, frame: &GrayImage, layout: &Layout) -> IntakeVerdict {
        let print = crate::stitch::viewport_print(frame, layout);
        let rect = crate::stitch::GridRect::of(layout, frame);
        let pitch = rect.layout_row_pitch(layout).max(1.0);
        let viewport_px = pitch * layout.rows.max(1) as f32;
        let Some(anchor) = &self.anchor else {
            self.anchor = Some(print);
            self.run = 1;
            self.accepted_total += 1;
            return IntakeVerdict::Accept { flush: false };
        };
        let dy = crate::stitch::estimate_scroll_dy(anchor, &print, pitch);
        if crate::stitch::same_viewport(anchor, &print) {
            // Creep counts toward progress even when the frame
            // itself is dropped: fifty 2px creeps are 100px of
            // travel the bundle boundary must not lose.
            self.progress += dy.abs().min(pitch * 0.5);
            if self.run < VIEWPORT_ACCEPT_CAP {
                self.anchor = Some(print);
                self.run += 1;
                self.accepted_total += 1;
                // Never a boundary: this frame belongs to the
                // current position, and a bundle boundary inside
                // a position would stitch that position twice.
                return IntakeVerdict::Accept { flush: false };
            }
            return IntakeVerdict::DropDuplicate;
        }
        // Pass 37 latch: a frame of the position the previous
        // acceptance displaced is an immediate scroll-back
        // within this bundle — that position already
        // contributed its instances here, and re-accepting it
        // would pool it twice under two anchors. It contributes
        // no progress either: its displacement measures the
        // trip back, not new panorama. The latch is replaced by
        // the next displacement, so A→B→C→A re-admits A; only
        // the immediate return is refused.
        if let Some(latched) = &self.latched {
            if crate::stitch::same_viewport(latched, &print) {
                self.latched_dropped += 1;
                return IntakeVerdict::DropLatched;
            }
        }
        // A new position. If the positions already pooled cover
        // a viewport of progress, the bundle closes before this
        // frame: boundaries fall between positions, never inside
        // one. A position split across bundles is stitched twice
        // under independent fits and its counts sum at merge —
        // measured on the HC duplicate-inflation simulation
        // (7 names doubled, 6 lost, 1 slot-level form false
        // accept); positions are atomic to the stitch, whose own
        // segments likewise break at frames, never within one.
        let boundary = self.progress >= viewport_px;
        self.progress += dy.abs();
        self.latched = self.anchor.replace(print);
        self.run = 1;
        self.accepted_total += 1;
        IntakeVerdict::Accept { flush: boundary }
    }
}

/// The worker-side queue of drained bundles waiting for the
/// stitch thread (pass 36, addendum item 2). Bundles are
/// consecutive ranges of one scroll, so a waiting bundle simply
/// concatenates onto the queue (coalescing — the stitch plans
/// the union exactly as it would have planned the concatenation
/// of both bundles). The queue is frame-capped; overflow drops
/// the incoming frames, counts them, and trips its flag rather
/// than growing without bound behind a slow stitch.
pub struct BundleQueue {
    frames: Vec<GrayImage>,
    cap: usize,
    /// Bundles merged into a non-empty queue.
    pub coalesced: u64,
    /// Incoming frames dropped past the cap.
    pub dropped_frames: u64,
    pub cap_tripped: bool,
}

impl Default for BundleQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl BundleQueue {
    pub fn new() -> Self {
        Self::with_cap(BUNDLE_QUEUE_MAX_FRAMES)
    }

    pub fn with_cap(cap: usize) -> Self {
        Self {
            frames: Vec::new(),
            cap,
            coalesced: 0,
            dropped_frames: 0,
            cap_tripped: false,
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Add one drained bundle, coalescing onto whatever waits.
    pub fn push(&mut self, bundle: Vec<GrayImage>) {
        if bundle.is_empty() {
            return;
        }
        if !self.frames.is_empty() {
            self.coalesced += 1;
        }
        let room = self.cap.saturating_sub(self.frames.len());
        let take = room.min(bundle.len());
        if take < bundle.len() {
            self.dropped_frames += (bundle.len() - take) as u64;
            self.cap_tripped = true;
        }
        self.frames.extend(bundle.into_iter().take(take));
    }

    /// Take up to `max` frames as the next stitch job.
    pub fn take_job(&mut self, max: usize) -> Vec<GrayImage> {
        let n = max.min(self.frames.len());
        self.frames.drain(..n).collect()
    }
}

/// One stitch bundle's owned inputs (pass 36): everything
/// [`stitch_parts_session`] needs, with no borrows, so the job
/// can cross to the stitch thread.
struct StitchJob {
    frames: Vec<GrayImage>,
    layout: Layout,
    universe: Universe,
    opts: StitchOptions,
}

/// The background stitch thread (pass 36, brief scope 3). One
/// thread, one job in flight: it owns a DEDICATED engine
/// instance because the frame engine's cache lock is held for
/// the whole duration of a batched read (forensic finding) — a
/// background stitch sharing that engine would serialise
/// against header/relic reads for the bundle's whole runtime.
/// Reports come back in job order (single thread, FIFO), so
/// merges on the worker thread are deterministic.
struct StitchDispatcher {
    job_tx: std::sync::mpsc::Sender<StitchJob>,
    report_rx: std::sync::mpsc::Receiver<StitchReport>,
    handle: Option<std::thread::JoinHandle<()>>,
    busy: bool,
}

impl StitchDispatcher {
    fn start(engine: Box<dyn OcrEngine + Send>) -> Self {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<StitchJob>();
        let (report_tx, report_rx) = std::sync::mpsc::channel::<StitchReport>();
        let handle = std::thread::spawn(move || {
            while let Ok(job) = job_rx.recv() {
                let report = stitch_parts_session(
                    &job.frames,
                    &job.layout,
                    &*engine,
                    &job.opts,
                    &job.universe,
                );
                if report_tx.send(report).is_err() {
                    return;
                }
            }
        });
        Self {
            job_tx,
            report_rx,
            handle: Some(handle),
            busy: false,
        }
    }

    fn send(&mut self, job: StitchJob) {
        // A failed send means the thread died (a panic inside
        // the stitch); the report wait in finish_parts observes
        // the closed channel and stops waiting.
        let _ = self.job_tx.send(job);
        self.busy = true;
    }

    /// Close the job queue and join the thread. Callers ensure
    /// no job is in flight (every report merged), so the thread
    /// is idle-blocked on its queue and exits at once — the
    /// bounded join the finalize contract needs.
    fn shutdown(self) {
        let StitchDispatcher {
            job_tx,
            handle,
            ..
        } = self;
        drop(job_tx);
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

/// The accumulating capture state. Folding a frame is a UNION over stack
/// identity: the same stack seen in ten frames counts once, never ten
/// times — this is what makes scroll capture dedupe work. The COUNT a
/// stack resolves to is the MODE of its per-frame observations (ties
/// go to the larger count), not the max (pass 27): a one-frame
/// phantom badge read — the census's Trumna Prime Stock, x1 seen as
/// x8 once — no longer outvotes every frame that saw the true count.
/// A stack observed only once keeps that single observation; voting
/// cannot fix a phantom nobody contradicted.
///
/// (No `Debug` derive since pass 36: the session can own a
/// background stitch thread and a boxed engine, neither of
/// which is `Debug`.)
#[derive(Default)]
pub struct CaptureSession {
    captured: StockState,
    /// Count observations per stack identity: count -> frames that
    /// observed it. `captured` is re-resolved from these votes after
    /// every frame (the maps are session-sized; the rebuild is cheap).
    part_votes: HashMap<String, HashMap<u32, u32>>,
    relic_votes: HashMap<(String, RefinementTier), HashMap<u32, u32>>,
    arcane_votes: HashMap<(String, u8), HashMap<u32, u32>>,
    saw_relics: bool,
    saw_parts: bool,
    saw_arcanes: bool,
    /// Parts frames awaiting the stitch (pass 33): parts frames
    /// are not read on arrival — they accumulate here and are
    /// stitched as a session by [`Self::finish_parts`].
    parts_pending: Vec<GrayImage>,
    /// The stitch reports this session produced (pass 33), one
    /// per `finish_parts` call — the coverage accounting record.
    stitch_reports: Vec<StitchReport>,
    /// Pass 35 × 36 integration: the session-level synthesis
    /// adjudication's summary, recorded when `finish_parts`
    /// runs the post-pass over the merged reports.
    synth_summary: Option<crate::synth::SynthSummary>,
    /// Pass 36: the content-aware parts intake (dedupe +
    /// progress accounting + the session frame cap).
    parts_intake: PartsIntake,
    /// Pass 36: drained bundles waiting for the stitch thread.
    bundle_queue: BundleQueue,
    /// Pass 36: the dedicated stitch engine, when the driver
    /// provided one ([`Self::set_stitch_engine`]). Without one,
    /// bundles are stitched synchronously on the capture thread
    /// with the frame engine (unit tests, stub sessions) —
    /// identical results, no backgrounding.
    stitch_engine: Option<Box<dyn OcrEngine + Send>>,
    /// Pass 36: the background stitch thread, once started.
    stitch: Option<StitchDispatcher>,
    /// Intake duplicates dropped past the per-position cap.
    parts_dupes_dropped: u64,
    /// Parts frames refused by the session frame cap.
    parts_frames_capped: u64,
    /// The session duration cap tripped (set by `run_capture`).
    duration_cap_tripped: bool,
    /// Evidence for a zero-collection diagnosis: the first few
    /// (class, header OCR text) pairs this session classified.
    pub header_samples: Vec<(ScreenClass, String)>,
    /// Evidence frames written to the debug dir, by file name.
    pub debug_files: Vec<String>,
    debug_dumped: std::collections::HashMap<&'static str, u32>,
    /// Pass 37: per-name debug-dump caps that stopped dumping
    /// this session (first-hit order). The finalize log names
    /// them — a capped dump can no longer masquerade as a
    /// capture that ended early (LIVE_DIAG2 §1: the v0.5.30
    /// run's folder "ending" at page 4 was this cap, silent).
    debug_capped: Vec<&'static str>,
    /// Pass 37: the intake's arrival-time source. Production
    /// leaves this unset and gets the real wall clock; the
    /// replay harness installs a modelled feed clock, because
    /// frame-arrival times are an INPUT to a capture (the live
    /// stream delivers at its own rate), not a policy knob —
    /// modelling them is how the time gate is exercised
    /// honestly in a gate that feeds files as fast as it can
    /// read them.
    feed_clock: Option<Box<dyn Fn() -> std::time::Instant + Send>>,
    /// Detached evidence-writer threads; joined at finalize so the
    /// logged file list is true by the time it is logged.
    debug_writers: Vec<std::thread::JoinHandle<()>>,
    last_sig: Option<u64>,
    last_class: Option<ScreenClass>,
    /// Header-strip cache (addendum #81): (crop hash, class, raw text)
    /// of the last classified header. The classifier's region/PSM are
    /// fixed for a run, so identical header pixels classify identically
    /// — a scroll step (frame changed, header did not) reuses this
    /// instead of paying another tesseract spawn per frame.
    last_header: Option<(u64, ScreenClass, String)>,
    idle: u64,
    /// Consecutive FRESH header classifications that came back
    /// Other (addendum #83). Idle only accrues once this reaches
    /// 3: a single OCR misread — now sticky under its header-hash
    /// cache entry — must not be able to end a capture that is
    /// still inside the inventory. Genuinely leaving produces
    /// fresh Other reads continuously and crosses 3 at once.
    fresh_other_streak: u64,
    frames_seen: u64,
    frames_read: u64,
    frames_skipped: u64,
    /// Pass 38: the owned-gate's session brightness reference —
    /// the running maximum icon highlight over this session's
    /// relic frames, handed to each frame's read so a catalogue
    /// page filled entirely with unowned tiles can still be
    /// judged dim (see ocr::relic_owned_gate).
    relic_bright_ref: Option<f64>,
    /// Pass 38: names the owned-gate excluded this session, in
    /// first-exclusion order, deduplicated — the finalize log's
    /// record of what a browse sync refused to import.
    relic_gate_excluded: Vec<String>,
    /// Pass 45: the session's refinement-disposition evidence —
    /// per relic stack, whether any appearance carried a count
    /// label and the maximum eye-window bright mass over its
    /// label-less appearances. Stock resolution consults it on
    /// every rebuild; the finalize line names what it excludes.
    relic_evidence: crate::ocr::RefinementEvidence,
    /// Pass 46: name-admission refusals this session, in
    /// first-refusal order, deduplicated on (raw, winner) —
    /// the finalize log's record of the reads whose resolved
    /// names their own evidence did not prove.
    relic_name_refused: Vec<crate::ocr::NameRefusal>,
}

impl CaptureSession {
    /// Per-name evidence-dump cap (addendum #82): enough frames
    /// for an OCR training sample, bounded disk.
    const MAX_DEBUG_PER_NAME: u32 = 60;

    pub fn new() -> Self {
        Self::default()
    }

    /// Provide the dedicated engine for the background stitch
    /// thread (pass 36). The driver (run_capture, the census
    /// harness) probes a second engine instance; sessions that
    /// never get one stitch synchronously with the frame engine.
    pub fn set_stitch_engine(&mut self, engine: Box<dyn OcrEngine + Send>) {
        self.stitch_engine = Some(engine);
    }

    /// Pass 37: install a modelled arrival-time source for the
    /// intake's time gate (see the `feed_clock` field). The
    /// production driver never calls this.
    pub fn set_feed_clock(&mut self, clock: Box<dyn Fn() -> std::time::Instant + Send>) {
        self.feed_clock = Some(clock);
    }

    /// The arrival time stamped on the next offered frame.
    fn feed_now(&self) -> std::time::Instant {
        match &self.feed_clock {
            Some(clock) => clock(),
            None => std::time::Instant::now(),
        }
    }

    /// Parts frames the intake accepted this session.
    pub fn parts_frames_accepted(&self) -> u64 {
        self.parts_intake.accepted_total()
    }

    /// Intake duplicates dropped past the per-position cap.
    pub fn parts_dupes_dropped(&self) -> u64 {
        self.parts_dupes_dropped
    }

    /// Pass 37: parts frames dropped by the settle gate.
    pub fn parts_off_lattice_dropped(&self) -> u64 {
        self.parts_intake.off_lattice_dropped()
    }

    /// Pass 37: parts frames dropped by the time gate.
    pub fn parts_interval_dropped(&self) -> u64 {
        self.parts_intake.interval_dropped()
    }

    /// Pass 37: parts frames dropped by the per-position latch.
    pub fn parts_latched_dropped(&self) -> u64 {
        self.parts_intake.latched_dropped()
    }

    /// Frames the bundle queue has dropped at its cap so far
    /// (pass 37: the resident logs increments as they happen,
    /// not only in the finalize failsafe summary).
    pub fn bundle_queue_dropped_frames(&self) -> u64 {
        self.bundle_queue.dropped_frames
    }

    /// Which stitch path this session's bundles take (pass 37:
    /// named in the finalize log — LIVE_DIAG2 §3 could not
    /// exclude the synchronous fallback from the v0.5.30
    /// evidence because no record named the path). A provided
    /// second engine starts the dispatcher at the first drain;
    /// a session that never drains a bundle reports the armed
    /// path, which is the path any bundle would have taken.
    pub fn stitch_path_label(&self) -> &'static str {
        if self.stitch.is_some() || self.stitch_engine.is_some() {
            "background stitch thread (dedicated second engine)"
        } else {
            "synchronous fallback (frame engine, capture thread)"
        }
    }

    /// Debug-dump names whose per-name cap stopped dumping
    /// this session (pass 37; see the `debug_capped` field).
    pub fn debug_capped_names(&self) -> &[&'static str] {
        &self.debug_capped
    }

    /// Parts frames refused by the session frame cap.
    pub fn parts_frames_capped(&self) -> u64 {
        self.parts_frames_capped
    }

    /// Whether the session duration cap tripped.
    pub fn duration_cap_tripped(&self) -> bool {
        self.duration_cap_tripped
    }

    pub(crate) fn note_duration_cap(&mut self) {
        self.duration_cap_tripped = true;
    }

    /// One finalize-visible line naming every failsafe that
    /// tripped this session (pass 36 addendum: caps trip LOUD,
    /// never silent), or `None` when none did. Coalescing
    /// without drops is routine back-pressure and is reported
    /// only alongside a trip.
    pub fn failsafe_summary(&self) -> Option<String> {
        let mut notes: Vec<String> = Vec::new();
        if self.parts_intake.cap_tripped() {
            notes.push(format!(
                "parts frame cap ({PARTS_SESSION_FRAME_CAP}) tripped: {} frames refused at intake",
                self.parts_frames_capped
            ));
        }
        if self.bundle_queue.cap_tripped {
            notes.push(format!(
                "bundle queue cap ({BUNDLE_QUEUE_MAX_FRAMES} frames) tripped: {} queued frames dropped, {} bundles coalesced",
                self.bundle_queue.dropped_frames, self.bundle_queue.coalesced
            ));
        }
        if self.duration_cap_tripped {
            notes.push(format!(
                "session duration cap ({}s) tripped: auto-finalized",
                SESSION_MAX_DURATION.as_secs()
            ));
        }
        if notes.is_empty() {
            None
        } else {
            Some(format!("capture failsafes: {}", notes.join("; ")))
        }
    }

    /// Pass 38: the owned-gate's finalize line — how many
    /// catalogue tiles were refused as unowned this session, and
    /// which. `None` when the gate excluded nothing (any
    /// owned-only session, where the gate never fires).
    pub fn relic_gate_summary(&self) -> Option<String> {
        if self.relic_gate_excluded.is_empty() {
            return None;
        }
        Some(format!(
            "relic owned-gate: excluded {} unowned catalogue tile(s): {}",
            self.relic_gate_excluded.len(),
            self.relic_gate_excluded.join(", ")
        ))
    }

    /// Pass 45: the refinement disposition's finalize line —
    /// which stacks the eye test excluded as unowned this
    /// session. `None` when nothing was excluded (any session
    /// that never read a refinement frame, and any refinement
    /// sync whose label-less tiles were all eye-free).
    pub fn relic_variant_summary(&self) -> Option<String> {
        let excluded = self.relic_evidence.excluded_stacks();
        if excluded.is_empty() {
            return None;
        }
        let mut names: Vec<&str> = Vec::new();
        for (name, _) in &excluded {
            if !names.contains(&name.as_str()) {
                names.push(name.as_str());
            }
        }
        Some(format!(
            "relic refinement rule: excluded {} unowned stack(s) (label-less refinement tile with the eye glyph): {}",
            excluded.len(),
            names.join(", ")
        ))
    }

    /// Pass 46: the name-admission finalize line — every read
    /// this session whose resolved name was refused because
    /// the read's own tokens did not witness it, with the raw
    /// read, the refused winner, the runner-up and the
    /// margin. `None` when nothing was refused.
    pub fn relic_name_summary(&self) -> Option<String> {
        if self.relic_name_refused.is_empty() {
            return None;
        }
        let parts: Vec<String> = self
            .relic_name_refused
            .iter()
            .map(|r| r.describe())
            .collect();
        Some(format!(
            "relic name admission: refused {} read(s) whose resolved name the read did not prove: {}",
            parts.len(),
            parts.join("; ")
        ))
    }

    /// The union of stacks captured so far (relics/parts/arcanes buckets).
    pub fn stock(&self) -> &StockState {
        &self.captured
    }

    pub fn counts(&self) -> CaptureCounts {
        CaptureCounts {
            relic_stacks: self.captured.relic_stack_count(),
            parts: self.captured.parts.len(),
            arcanes: self.captured.arcane_stack_count(),
        }
    }

    /// Did any frame classify into a real inventory screen?
    pub fn has_any(&self) -> bool {
        self.saw_relics || self.saw_parts || self.saw_arcanes
    }

    pub fn frames_seen(&self) -> u64 {
        self.frames_seen
    }
    pub fn frames_read(&self) -> u64 {
        self.frames_read
    }
    pub fn frames_skipped(&self) -> u64 {
        self.frames_skipped
    }
    pub fn idle_frames(&self) -> u64 {
        self.idle
    }

    /// One line for the tray: what has been collected so far.
    pub fn status_line(&self) -> String {
        let c = self.counts();
        format!(
            "Capture: {} relic stacks, {} parts, {} arcanes",
            c.relic_stacks, c.parts, c.arcanes
        )
    }

    /// Fold one frame: skip when it is unchanged from the previous frame,
    /// else classify it and OCR the matching layout into the union. An
    /// unclassifiable frame (menu/gameplay) pauses collection silently.
    pub fn process_frame(
        &mut self,
        ctx: &CaptureContext<'_>,
        engine: &dyn OcrEngine,
        frame: &GrayImage,
    ) {
        self.frames_seen += 1;
        if self.frames_seen == 1 {
            self.dump_debug_frame(ctx, frame, "first");
        }
        let sig = frame_signature(frame);
        if self.last_sig == Some(sig) {
            // Unchanged screen: same class as last time.
            self.frames_skipped += 1;
            if matches!(self.last_class, Some(c) if c != ScreenClass::Other) {
                self.idle = 0;
            } else if self.fresh_other_streak >= 3 {
                self.idle += 1;
            }
            return;
        }
        self.last_sig = Some(sig);

        let header_key = ocr::region_key(frame, ctx.classifier.header);
        let (class, header_text, header_fresh) = match &self.last_header {
            Some((k, class, text)) if *k == header_key => (*class, text.clone(), false),
            _ => {
                let (class, text) = ctx.classifier.classify_detailed(engine, frame);
                self.last_header = Some((header_key, class, text.clone()));
                (class, text, true)
            }
        };
        self.last_class = Some(class);
        if header_fresh {
            if class == ScreenClass::Other {
                self.fresh_other_streak += 1;
            } else {
                self.fresh_other_streak = 0;
            }
        }
        if header_fresh && self.header_samples.len() < 10 {
            let flat: String = header_text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(160)
                .collect();
            self.header_samples.push((class, flat));
        }
        if class != ScreenClass::Other {
            self.dump_debug_frame(ctx, frame, class.label());
        }
        let Some(layout) = class.layout_key().and_then(|k| ctx.layouts.get(k)) else {
            // No screen recognised — pause, do not read the wrong grid.
            self.frames_skipped += 1;
            if self.fresh_other_streak >= 3 {
                self.idle += 1;
            }
            return;
        };
        self.idle = 0;
        self.frames_read += 1;
        match class {
            ScreenClass::Relics => self.saw_relics = true,
            ScreenClass::Parts => self.saw_parts = true,
            ScreenClass::Arcanes => self.saw_arcanes = true,
            ScreenClass::Other => {}
        }

        // Pass 36: merges from any completed background bundle
        // land before this frame is folded, so the counts the
        // driver reads after process_frame returns climb during
        // the scroll (the pass-33 deferral made them read 0
        // until finalize).
        self.poll_stitch(ctx);

        // Pass 33/36: parts frames accumulate for the stitch —
        // through the content-aware intake, which drops
        // same-viewport duplicates past a small cap and drains
        // the pending bundle on panorama progress. A non-parts
        // frame drains the pending parts first, so a screen
        // change closes the stitch where the scroll ended.
        if class == ScreenClass::Parts {
            match self.parts_intake.offer_at(frame, layout, self.feed_now()) {
                IntakeVerdict::Accept { flush } => {
                    // A boundary verdict closes the pending
                    // bundle before this frame is pushed — the
                    // frame opens the next bundle (positions are
                    // atomic; see PartsIntake::offer_at).
                    if flush && self.parts_pending.len() >= BUNDLE_MIN_FRAMES {
                        self.drain_bundle(ctx, engine);
                    }
                    self.parts_pending.push(frame.clone());
                    if self.parts_pending.len() >= BUNDLE_MAX_FRAMES {
                        self.drain_bundle(ctx, engine);
                    }
                }
                IntakeVerdict::DropDuplicate => self.parts_dupes_dropped += 1,
                IntakeVerdict::DropSessionCap => self.parts_frames_capped += 1,
                // Pass 37: the gate drops are counted on the
                // intake itself and read back at finalize
                // (parts_off_lattice_dropped &c.).
                IntakeVerdict::DropOffLattice
                | IntakeVerdict::DropInterval
                | IntakeVerdict::DropLatched => {}
            }
            return;
        }
        if !self.parts_pending.is_empty() {
            self.drain_bundle(ctx, engine);
        }
        self.parts_intake.reset_position();

        // Pass 38: on relic frames, extend the session's
        // owned-brightness reference with this frame BEFORE the
        // read, and hand the running reference to the owned-gate
        // (a catalogue tail page can be uniformly dim; the bright
        // tiles of earlier pages are its yardstick).
        let bright_ref = if class == ScreenClass::Relics {
            let frame_bright = ocr::relic_frame_bright(frame, layout);
            self.relic_bright_ref = Some(
                self.relic_bright_ref
                    .unwrap_or(0.0)
                    .max(frame_bright.unwrap_or(0.0)),
            );
            self.relic_bright_ref
        } else {
            None
        };
        // Pass 43: the relic variant rides the header this
        // session already classified — the refinement screen
        // names itself in its title.
        let variant = if class == ScreenClass::Relics {
            ctx.classifier.relic_variant(&header_text)
        } else {
            RelicScreenVariant::Unknown
        };
        let results = read_frame(
            layout,
            frame,
            engine,
            ctx,
            ctx.universes.for_class(class),
            bright_ref,
            variant,
        );
        // Pass 38: record what the owned-gate refused, for the
        // finalize log (first-exclusion order, deduplicated).
        // Pass 45: fold this frame's refinement measurements
        // into the session's disposition evidence — the stock
        // rebuild below and the finalize line both read it.
        for r in &results {
            if r.gate_excluded {
                if let Some(name) = &r.matched {
                    if !self.relic_gate_excluded.contains(name) {
                        self.relic_gate_excluded.push(name.clone());
                    }
                }
            }
            // Pass 46: record name-admission refusals for the
            // finalize log (first-refusal order, deduplicated
            // on raw + winner — the same garbled tile read on
            // five frames is one refusal, stated once).
            if let Some(nr) = &r.name_refused {
                if !self
                    .relic_name_refused
                    .iter()
                    .any(|x| x.raw == nr.raw && x.winner == nr.winner)
                {
                    self.relic_name_refused.push(nr.clone());
                }
            }
        }
        if class == ScreenClass::Relics {
            self.relic_evidence.observe(&results, ctx.relic_names);
        }
        let frame_stock = StockState::from_results(&results, ctx.relic_names, ctx.arcane_names);
        self.observe_counts(&frame_stock);
        self.captured = resolve_voted_stock(
            &self.part_votes,
            &self.relic_votes,
            &self.arcane_votes,
            &self.relic_evidence,
        );
    }

    /// Drain the pending parts bundle (pass 36): hand it to the
    /// background stitch thread (or the bounded queue behind it)
    /// when a stitch engine was provided; stitch it synchronously
    /// with the frame engine otherwise. Bundle boundaries are
    /// the intake's progress flush, the bundle frame cap, a
    /// screen change, and `finish_parts` — never a raw
    /// accumulated-frame count on its own.
    fn drain_bundle(&mut self, ctx: &CaptureContext<'_>, engine: &dyn OcrEngine) {
        if self.parts_pending.is_empty() {
            return;
        }
        let frames = std::mem::take(&mut self.parts_pending);
        self.parts_intake.note_flush();
        if self.stitch.is_none() {
            match self.stitch_engine.take() {
                Some(e) => self.stitch = Some(StitchDispatcher::start(e)),
                None => {
                    self.stitch_sync(frames, ctx, engine);
                    return;
                }
            }
        }
        let busy = self.stitch.as_ref().map(|d| d.busy).unwrap_or(false);
        if busy {
            self.bundle_queue.push(frames);
            return;
        }
        let Some(layout) = ctx.layouts.get("parts") else {
            return;
        };
        let job = StitchJob {
            frames,
            layout: layout.clone(),
            universe: ctx.universes.for_class(ScreenClass::Parts).clone(),
            opts: ctx.stitch_options(),
        };
        if let Some(d) = &mut self.stitch {
            d.send(job);
        }
    }

    /// The pass-34 path, kept for sessions without a stitch
    /// engine: stitch a bundle on this thread, now.
    fn stitch_sync(
        &mut self,
        frames: Vec<GrayImage>,
        ctx: &CaptureContext<'_>,
        engine: &dyn OcrEngine,
    ) {
        let Some(layout) = ctx.layouts.get("parts") else {
            return;
        };
        let report = stitch_parts_session(
            &frames,
            layout,
            engine,
            &ctx.stitch_options(),
            ctx.universes.for_class(ScreenClass::Parts),
        );
        self.merge_report(report);
    }

    /// Dispatch the next queued bundle if the stitch thread is
    /// free. Returns whether a job was sent.
    fn dispatch_queued(&mut self, ctx: &CaptureContext<'_>) -> bool {
        let busy = match &self.stitch {
            Some(d) => d.busy,
            None => return false,
        };
        if busy || self.bundle_queue.is_empty() {
            return false;
        }
        let Some(layout) = ctx.layouts.get("parts") else {
            return false;
        };
        let frames = self.bundle_queue.take_job(BUNDLE_MAX_FRAMES);
        if frames.is_empty() {
            return false;
        }
        let job = StitchJob {
            frames,
            layout: layout.clone(),
            universe: ctx.universes.for_class(ScreenClass::Parts).clone(),
            opts: ctx.stitch_options(),
        };
        if let Some(d) = &mut self.stitch {
            d.send(job);
        }
        true
    }

    /// Merge every stitch report that has completed since the
    /// last poll (non-blocking), dispatching queued bundles as
    /// the thread frees. Runs on the capture worker thread, so
    /// `captured.parts` — and the tray counts read from it —
    /// update during the scroll.
    fn poll_stitch(&mut self, ctx: &CaptureContext<'_>) {
        loop {
            let report = self
                .stitch
                .as_ref()
                .and_then(|d| d.report_rx.try_recv().ok());
            match report {
                Some(report) => {
                    self.merge_report(report);
                    if let Some(d) = &mut self.stitch {
                        d.busy = false;
                    }
                    self.dispatch_queued(ctx);
                }
                None => break,
            }
        }
    }

    /// Finish the parts stitch (pass 33/36): drain the pending
    /// bundle, then collect every outstanding bundle — the
    /// bounded join. Outstanding work is bounded by construction
    /// (one job in flight plus the frame-capped queue), reports
    /// merge in dispatch order, and the stitch thread is retired
    /// before returning, so finalize = join + merge + (for the
    /// final partial bundle) one last stitch.
    pub fn finish_parts(&mut self, ctx: &CaptureContext<'_>, engine: &dyn OcrEngine) {
        self.drain_bundle(ctx, engine);
        if self.stitch.is_some() {
            loop {
                self.poll_stitch(ctx);
                let busy = self.stitch.as_ref().map(|d| d.busy).unwrap_or(false);
                if !busy && self.bundle_queue.is_empty() {
                    break;
                }
                if !busy && !self.dispatch_queued(ctx) {
                    // Not busy, queue non-empty, and nothing can
                    // be dispatched (no parts layout): abandon
                    // the queue rather than wait forever.
                    break;
                }
                let report = self
                    .stitch
                    .as_ref()
                    .and_then(|d| d.report_rx.recv().ok());
                match report {
                    Some(report) => {
                        self.merge_report(report);
                        if let Some(d) = &mut self.stitch {
                            d.busy = false;
                        }
                        self.dispatch_queued(ctx);
                    }
                    // The thread died (a panic in the stitch):
                    // stop waiting; what merged stands.
                    None => break,
                }
            }
            if let Some(d) = self.stitch.take() {
                d.shutdown();
            }
        }
        // Pass 35 × 36 integration: the synthesis post-pass runs
        // ONCE per session, here at finalize, over the merged
        // bundle reports — never per bundle. This preserves the
        // v0.5.29 whole-session semantics (one calibration over
        // the session's both-readers-held slots, one exclusion
        // set over the session's holds) that the per-bundle hook
        // inside `stitch_parts_session` could not express once
        // bundles moved to the background thread. The post-pass
        // consumes the canonical crops retained on the slots it
        // touches; its holds are voted exactly like reader holds
        // and stock is re-resolved.
        if ctx.synthesis && !self.stitch_reports.is_empty() {
            if let Some(layout) = ctx.layouts.get("parts") {
                let universe = ctx.universes.for_class(ScreenClass::Parts);
                let mut reports = std::mem::take(&mut self.stitch_reports);
                let summary = crate::synth::adjudicate_session(
                    &mut reports,
                    layout,
                    engine,
                    &ctx.read_options(),
                    universe,
                );
                let mut any = false;
                for report in &reports {
                    for slot in &report.slots {
                        if slot.coverage == SlotCoverage::Held
                            && slot.held_via == Some(crate::lineread::Via::Synthesis)
                        {
                            self.vote_held_slot(slot);
                            any = true;
                        }
                    }
                }
                if any {
                    self.captured = resolve_voted_stock(
                        &self.part_votes,
                        &self.relic_votes,
                        &self.arcane_votes,
                        &self.relic_evidence,
                    );
                }
                self.synth_summary = Some(summary);
                self.stitch_reports = reports;
            }
        }
    }

    /// Fold one stitch report into the session (pass 33 merge,
    /// pass 36 split out so background bundles merge identically
    /// whenever they land). Each held slot contributes one count
    /// vote per read instance — the stitched equivalent of the
    /// per-frame observations the vote was designed for (a ×1
    /// instance, its ring-gated read absent, votes 1).
    fn merge_report(&mut self, report: StitchReport) {
        for slot in &report.slots {
            self.vote_held_slot(slot);
        }
        self.captured = resolve_voted_stock(
            &self.part_votes,
            &self.relic_votes,
            &self.arcane_votes,
            &self.relic_evidence,
        );
        self.stitch_reports.push(report);
    }

    /// One held slot's session votes (the merge body, split out
    /// so the finalize-time synthesis holds vote identically to
    /// reader holds). Coverage is the authoritative verdict: a
    /// contested slot's stored result is the canonical read kept
    /// for diagnostics, and it must not vote.
    fn vote_held_slot(&mut self, slot: &SlotReport) {
        if slot.coverage != SlotCoverage::Held {
            return;
        }
        let Some(result) = &slot.result else { return };
        let Some(name) = &result.matched else { return };
        if slot.instance_counts.is_empty() {
            *self
                .part_votes
                .entry(name.clone())
                .or_default()
                .entry(result.count.unwrap_or(1))
                .or_insert(0) += 1;
        } else {
            for c in &slot.instance_counts {
                *self
                    .part_votes
                    .entry(name.clone())
                    .or_default()
                    .entry(c.unwrap_or(1))
                    .or_insert(0) += 1;
            }
        }
        // Pass 34: the built-copy glyph on a held blueprint
        // slot records its built sibling as held stock (one
        // ×1 vote — the twin's own count is not observed).
        if let Some(implied) = &slot.glyph_implied {
            *self
                .part_votes
                .entry(implied.clone())
                .or_default()
                .entry(1)
                .or_insert(0) += 1;
        }
    }

    /// Every stitch report this session produced (pass 33).
    pub fn stitch_reports(&self) -> &[StitchReport] {
        &self.stitch_reports
    }

    /// The session-level synthesis adjudication summary, when
    /// `finish_parts` ran the post-pass (pass 35 × 36).
    pub fn synth_summary(&self) -> Option<&crate::synth::SynthSummary> {
        self.synth_summary.as_ref()
    }

    /// Coverage accounting across all stitch reports (pass 33):
    /// held / seen-not-held / never-fully-visible slot counts plus
    /// disagreement flags — the finalize-visible answer to "what
    /// did the capture actually cover". `None` when no parts
    /// session was stitched.
    pub fn coverage_summary(&self) -> Option<String> {
        if self.stitch_reports.is_empty() {
            return None;
        }
        let (mut held, mut seen, mut never, mut flagged) = (0usize, 0usize, 0usize, 0usize);
        let mut implied = 0usize;
        let mut synth_held = 0usize;
        for r in &self.stitch_reports {
            for s in &r.slots {
                match s.coverage {
                    SlotCoverage::Held => held += 1,
                    SlotCoverage::SeenNotHeld => seen += 1,
                    SlotCoverage::NeverFullyVisible => never += 1,
                }
                if s.disagreement {
                    flagged += 1;
                }
                if s.glyph_implied.is_some() {
                    implied += 1;
                }
                if s.held_via == Some(crate::lineread::Via::Synthesis) {
                    synth_held += 1;
                }
            }
        }
        Some(format!(
            "stitch coverage: {held} held, {seen} seen-not-held, {never} never-fully-visible, {flagged} instance-disagreement flagged, {implied} glyph-implied built-form holds, {synth_held} synthesis holds ({} stitch report(s))",
            self.stitch_reports.len()
        ))
    }

    /// Record one frame's per-stack counts as votes (pass 27). A
    /// stack appears at most once per frame (one inventory slot), so
    /// each frame contributes exactly one observation per stack.
    fn observe_counts(&mut self, frame_stock: &StockState) {
        for (name, count) in &frame_stock.parts {
            *self
                .part_votes
                .entry(name.clone())
                .or_default()
                .entry(*count)
                .or_insert(0) += 1;
        }
        for (name, stacks) in &frame_stock.relics {
            for (tier, count) in &stacks.tiers {
                *self
                    .relic_votes
                    .entry((name.clone(), *tier))
                    .or_default()
                    .entry(*count)
                    .or_insert(0) += 1;
            }
        }
        for (name, stacks) in &frame_stock.arcanes {
            for (rank, count) in &stacks.ranks {
                *self
                    .arcane_votes
                    .entry((name.clone(), *rank))
                    .or_default()
                    .entry(*count)
                    .or_insert(0) += 1;
            }
        }
    }

    /// Write an evidence frame to the context's debug dir
    /// (best-effort — a failed dump never fails a capture). Up to
    /// [`Self::MAX_DEBUG_PER_NAME`] frames per name per session,
    /// named `frame-<name>-<idx>.png`: since addendum #82 the dumps
    /// double as OCR training data, so one frame per class was too
    /// thin a sample. The PNG write runs on a detached thread — a
    /// full-res encode on the reader thread would worsen the very
    /// frame starvation the evidence exists to diagnose.
    fn dump_debug_frame(&mut self, ctx: &CaptureContext<'_>, frame: &GrayImage, name: &'static str) {
        let Some(dir) = &ctx.debug_dir else { return };
        let idx = {
            let n = self.debug_dumped.entry(name).or_insert(0);
            if *n >= Self::MAX_DEBUG_PER_NAME {
                // Pass 37: the cap stopping is recorded (once
                // per name) for the finalize log — silently
                // stopping is what made a capped dump read as a
                // truncated capture (LIVE_DIAG2 §1).
                if !self.debug_capped.contains(&name) {
                    self.debug_capped.push(name);
                }
                return;
            }
            let idx = *n;
            *n += 1;
            idx
        };
        let file = format!("frame-{name}-{idx:02}.png");
        self.debug_files.push(file.clone());
        let dir = dir.clone();
        let frame = frame.clone();
        let handle = std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            let _ = frame.write_png(&dir.join(&file));
        });
        self.debug_writers.push(handle);
    }

    /// Join every outstanding evidence writer. Called at finalize:
    /// the dumps are few and small, and the evidence log should
    /// only name files that actually landed.
    pub fn flush_debug_dumps(&mut self) {
        for handle in self.debug_writers.drain(..) {
            let _ = handle.join();
        }
    }

    /// Finalise into an existing stock file: REPLACE only the buckets that
    /// saw classified frames. A bucket with zero classified frames is left
    /// untouched, never zeroed (partial captures are safe).
    ///
    /// The unverified (⍻) section is not a capture bucket: it is
    /// preserved verbatim across finalise, except that when the
    /// parts bucket is replaced, entries the fresh capture VERIFIES
    /// are pruned — the assertion is spent once capture confirms the
    /// part (thirtieth pass, addendum #93).
    pub fn finalize_into(&self, existing: &mut StockState) {
        if self.saw_relics {
            existing.relics = self.captured.relics.clone();
        }
        if self.saw_arcanes {
            existing.arcanes = self.captured.arcanes.clone();
        }
        if self.saw_parts {
            existing.parts = self.captured.parts.clone();
            existing
                .unverified
                .retain(|part, _| !part_verified_in_bucket(part, &self.captured.parts));
        }
    }
}

/// Is a roster part name verified by a captured parts bucket
/// (thirtieth pass prune rule)? The bucket holds raw screen names,
/// so a part is verified when the bucket carries the part's own
/// name OR its built-component form (the name minus a trailing
/// " Blueprint") at count >= 1 — the same two forms
/// `StockState::canonical_part_counts` sums onto the roster part
/// (#89). The gear's own "<Gear> Blueprint" strips to "<Gear>",
/// which is never a captured part name, so it cannot false-prune;
/// a parts bucket that never saw the part leaves the assertion
/// standing.
fn part_verified_in_bucket(
    part: &str,
    bucket: &std::collections::BTreeMap<String, u32>,
) -> bool {
    if bucket.get(part).copied().unwrap_or(0) >= 1 {
        return true;
    }
    match part.strip_suffix(" Blueprint") {
        Some(built) => bucket.get(built).copied().unwrap_or(0) >= 1,
        None => false,
    }
}

/// The winning count from one stack's vote map: the most-observed
/// count, ties broken toward the LARGER count (the pre-pass-27
/// behaviour for the tied case, so a stack seen once at x8 and once
/// at x1 still resolves x8 — voting only helps when observations
/// actually disagree in number).
fn voted_count(votes: &HashMap<u32, u32>) -> u32 {
    votes
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)))
        .map(|(count, _)| *count)
        .unwrap_or(1)
}

/// Rebuild the session stock from the count votes (pass 27).
/// Pass 45: a relic stack the refinement disposition calls
/// unowned (never labelled, eye glyph measured) is skipped
/// here, whatever its x1 votes say — the disposition is
/// consulted on every rebuild, so a late eyed appearance
/// retracts a stack an earlier frame voted in.
fn resolve_voted_stock(
    part_votes: &HashMap<String, HashMap<u32, u32>>,
    relic_votes: &HashMap<(String, RefinementTier), HashMap<u32, u32>>,
    arcane_votes: &HashMap<(String, u8), HashMap<u32, u32>>,
    relic_evidence: &crate::ocr::RefinementEvidence,
) -> StockState {
    let mut stock = StockState::default();
    for (name, votes) in part_votes {
        stock.parts.insert(name.clone(), voted_count(votes));
    }
    for ((name, tier), votes) in relic_votes {
        if relic_evidence.is_unowned(name, *tier) {
            continue;
        }
        stock
            .relics
            .entry(name.clone())
            .or_default()
            .merge(*tier, voted_count(votes));
    }
    for ((name, rank), votes) in arcane_votes {
        stock
            .arcanes
            .entry(name.clone())
            .or_default()
            .merge(*rank, voted_count(votes));
    }
    stock
}

/// Read every cell of one classified frame into validated results. Mirrors
/// [`ocr::ingest_folder`]'s per-page body: a partial cell at the viewport
/// edge is never read (never guessed). Public as the census-replay seam
/// (the pass-29 gate drives this exact function over the census frames).
pub fn read_frame(
    layout: &Layout,
    frame: &GrayImage,
    engine: &dyn OcrEngine,
    ctx: &CaptureContext<'_>,
    universe: &Universe,
    bright_ref: Option<f64>,
    variant: RelicScreenVariant,
) -> Vec<ocr::CellResult> {
    // One batched call for the whole page (addendum #81): the
    // tesseract engine caches by crop hash and batches cache misses
    // into a handful of processes, instead of 2–3 spawns per cell.
    let cells = layout.split(frame);
    let mut opts = ctx.read_options();
    opts.relic_bright_ref = bright_ref;
    opts.relic_variant = variant;
    read_cells(&cells, layout, engine, &opts, universe)
}

/// Validate + decode an already-cut set of cells (pass 33 seam):
/// everything [`read_frame`] does after the grid split — batched
/// OCR, margin validation under the shared hold predicate, and the
/// radius decoder's band + strip phases. The stitch path feeds
/// this the session's canonical (median-of-instances) crops so a
/// stitched session and a single frame are read by literally the
/// same code, with the same evidence bars.
pub fn read_cells(
    cells: &[Option<GrayImage>],
    layout: &Layout,
    engine: &dyn OcrEngine,
    opts: &ReadOptions,
    universe: &Universe,
) -> Vec<ocr::CellResult> {
    let reads = engine.ocr_cells_in(cells, layout);
    let mut results = Vec::with_capacity(reads.len());
    for (index, read) in reads.iter().enumerate() {
        // A `None` cell (partial, at the viewport edge) is never read.
        let read = read.clone().unwrap_or_else(ocr::OcrRead::empty);
        results.push(ocr::validate_read(
            &read,
            universe,
            opts.min_confidence,
            layout.min_score,
            index,
        ));
    }
    // Pass 29 (addendum #92): radius-decode the parts reads the
    // margin matcher refused. The decoder can only ADD holds — it
    // runs after validation, touches only unheld parts cells, and
    // its own evidence standard (verified class suffix + fragment
    // inside a derived radius) replaces the margin/score gates for
    // the cells it accepts; counts ride along exactly as on the
    // margin path. Relic and arcane frames never reach this block.
    if opts.radius_decoder && layout.key == "parts" {
        // Phase 1: band decodes — pure matcher work, no OCR.
        let mut strip_pending: Vec<usize> = Vec::new();
        for (index, result) in results.iter_mut().enumerate() {
            if result.held {
                continue;
            }
            let Some(read) = reads.get(index).and_then(|r| r.as_ref()) else {
                continue;
            };
            if read.text.trim().is_empty() {
                continue;
            }
            if let Some(name) =
                universe.radius_decode_budget(&read.text, opts.radius_tail_budget)
            {
                result.matched = Some(name);
                result.held = true;
                result.count = read.count;
            } else if opts.radius_strip {
                strip_pending.push(index);
            }
        }
        // Phase 2: line-1 strip reads for the cells the band
        // fragment could not decode, batched through the engine
        // (pass 29 re-gate — one batched read per frame instead of
        // one tesseract spawn per refused cell, which cost
        // +7.2 s/frame unbatched on the first gate). The strip is
        // the name band's top strip (band-relative y 0.00–0.40),
        // single-line mode: the strip the verification measured at
        // 27/39 with radius decoding (NEUROPTICS_BRIEF appendix).
        // Acceptance logic is unchanged by the batching: a strip
        // fragment decodes only inside the class the BAND read
        // nominated and tail-verified.
        if !strip_pending.is_empty() {
            let band = layout.name;
            let strip = [
                band[0],
                band[1],
                band[2],
                band[1] + 0.40 * (band[3] - band[1]),
            ];
            let pending: Vec<(usize, &GrayImage)> = strip_pending
                .iter()
                .filter_map(|&index| {
                    cells
                        .get(index)
                        .and_then(|c| c.as_ref())
                        .map(|cell| (index, cell))
                })
                .collect();
            // Pass 34 (brief §4): strip reads at a small fixed
            // set of vertical offsets around the fitted
            // placement, under a UNIFORM policy (never per-cell
            // tuning). Pass 33 measured the strip on fitted
            // cells to be extremely phase-sensitive — the same
            // cell reads clean at −8px and garbage at 0 — so a
            // single fixed placement strands the cells whose
            // fitted phase is the bad one. The offsets are page
            // px at the capture scale, expressed per cell as a
            // fraction of its height. Each offset's text decodes
            // through the same predicate-gated strip path; a
            // cell holds only when the offsets that decode AGREE
            // on one name (the agreement discipline the stitch
            // applies across sightings, applied across phases).
            let offsets_px: &[f32] = if opts.strip_offsets {
                &[-8.0, 0.0, 8.0]
            } else {
                &[0.0]
            };
            let mut requests: Vec<(&GrayImage, [f32; 4], u32, u32)> = Vec::new();
            let mut req_cell: Vec<usize> = Vec::new(); // pending position per request
            for (pi, (_, cell)) in pending.iter().enumerate() {
                let dy = |px: f32| px / (cell.height.max(1) as f32);
                for &off in offsets_px {
                    let region = [
                        strip[0],
                        (strip[1] + dy(off)).clamp(0.0, 1.0),
                        strip[2],
                        (strip[3] + dy(off)).clamp(0.0, 1.0),
                    ];
                    if region[3] <= region[1] {
                        continue;
                    }
                    requests.push((cell, region, 7, 4));
                    req_cell.push(pi);
                }
            }
            let texts = engine.read_text_batch(&requests);
            let mut decodes: Vec<Vec<String>> = vec![Vec::new(); pending.len()];
            for (ri, text) in texts.iter().enumerate() {
                if text.trim().is_empty() {
                    continue;
                }
                let pi = req_cell[ri];
                let (index, _) = pending[pi];
                let Some(read) = reads.get(index).and_then(|r| r.as_ref()) else {
                    continue;
                };
                if let Some(name) =
                    universe.radius_decode_strip(&read.text, text, opts.radius_tail_budget)
                {
                    if !decodes[pi].contains(&name) {
                        decodes[pi].push(name);
                    }
                }
            }
            for (pi, (index, _)) in pending.iter().enumerate() {
                if decodes[pi].len() != 1 {
                    continue;
                }
                let Some(read) = reads.get(*index).and_then(|r| r.as_ref()) else {
                    continue;
                };
                let result = &mut results[*index];
                result.matched = Some(decodes[pi][0].clone());
                result.held = true;
                result.count = read.count;
            }
        }
    }
    // Pass 38 (addendum #103): the relic owned-gate runs on
    // relic frames only — after validation, so it sees exactly the
    // tiles that would enter stock, and only ever REMOVES holds
    // (on catalogue frames, under the full evidence conjunction;
    // see ocr::relic_owned_gate). Parts and arcane frames never
    // reach it, and on owned-only relic frames it is inert.
    // Pass 46 (addendum #111): the name-admission gate runs
    // BEFORE it, on the same frames — the owned-gate sees only
    // tiles whose names were admitted.
    //
    // Pass 45 (addendum #110): the refinement measure runs
    // after the gate, on the same terms — on refinement frames
    // it records the eye-window bright mass of every held,
    // label-less tile. It refuses nothing itself; the session
    // dispositions each stack from the aggregated evidence
    // (ocr::RefinementEvidence): label = owned at the count,
    // eye seen = unowned, neither = owned x1.
    if layout.key == "relic" {
        // Pass 46 (addendum #111): the name-admission gate
        // runs first among the relic passes — a read whose
        // resolved name its own tokens do not witness is
        // refused here and feeds no other evidence (not the
        // owned-gate, not the refinement disposition).
        ocr::relic_name_admission(&mut results, universe);
        ocr::relic_owned_gate(cells, layout, &mut results, opts.relic_bright_ref);
        ocr::relic_refinement_measure(cells, layout, &mut results, opts.relic_variant);
    }
    results
}

/// How one panorama slot ended (pass 33 coverage accounting —
/// the finalize-visible record of what a capture covered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotCoverage {
    /// Read and held: the slot's item is in the captured stock.
    Held,
    /// Fully visible at least once, but no read held.
    SeenNotHeld,
    /// Covered by viewports only in partial sightings: no instance
    /// ever had adequate viewport margin, so no crop was ever cut.
    NeverFullyVisible,
}

/// One panorama slot's outcome in a stitched parts session.
#[derive(Debug, Clone)]
pub struct SlotReport {
    pub segment: usize,
    pub row: i32,
    pub col: u32,
    pub coverage: SlotCoverage,
    /// The winning read's validation result (the canonical read,
    /// kept for diagnostics, when the slot is contested or
    /// unheld; `None` when the slot was never fully visible).
    pub result: Option<ocr::CellResult>,
    /// Usable instances that fed the canonical crop.
    pub instances: usize,
    /// Partial sightings (clipped / viewport-edge) of this slot.
    pub partial_sightings: usize,
    /// Ring-gated count reads per usable instance (`None` = no
    /// badge detected = ×1). The session vote consumes these.
    pub instance_counts: Vec<Option<u32>>,
    /// A pooled instance disagreed with the canonical crop beyond
    /// the stated threshold — reported, not silently resolved.
    pub disagreement: bool,
    /// The canonical crop was a pixel-wise median (≥3 pooled).
    pub median: bool,
    /// The slot's segment was a relink fallback (started after a
    /// link failure or a post-solve ejection).
    pub fallback: bool,
    /// Pass 34: the band reader's agreement verdict for this
    /// slot (the ONE name its reads held), before arbitration;
    /// `None` when it held nothing or was internally contested.
    pub band_held: Option<String>,
    /// Pass 34: the line reader's agreement verdict, before
    /// form gating; `None` likewise, and whenever the line
    /// reader is off.
    pub line_held: Option<String>,
    /// Pass 34: the form channels' verdict for this slot
    /// (`Unknown` when the line reader is off or the slot was
    /// never fully visible).
    pub form_verdict: crate::lineread::FormVerdict,
    /// Pass 34: which reader(s) the arbitrated hold came from;
    /// `None` when the slot is not held.
    pub held_via: Option<crate::lineread::Via>,
    /// Pass 34: why arbitration refused a slot a reader wanted
    /// to hold (gate attribution); `None` otherwise.
    pub refuse_reason: Option<&'static str>,
    /// Pass 34: the built form implied held by the built-copy
    /// glyph on a slot held as its blueprint sibling — recorded
    /// as held stock by the session, and marked as implied in
    /// the coverage accounting.
    pub glyph_implied: Option<String>,
    /// Pass 34 diagnostics: the glyph template score and the
    /// slot's median per-instance mesh score.
    pub glyph_score: f32,
    pub mesh_score: f32,
    /// Pass 35: synthesis diagnostics for slots the synthesis
    /// post-pass adjudicated (candidates + scores + outcome);
    /// `None` for held slots and slots it never considered.
    pub synth: Option<crate::synth::SynthDiag>,
    /// Pass 36 integration: the slot's canonical crop, retained
    /// only for the two slot classes the finalize-time synthesis
    /// adjudication touches — held via both readers (its
    /// calibration pool) and seen-not-held (its eligible pool) —
    /// so the session-level post-pass can run after the bundle
    /// frames have been dropped. `None` for every other slot.
    pub canonical_crop: Option<GrayImage>,
}

/// One stitched parts session: the plan plus every slot's outcome.
#[derive(Debug, Clone)]
pub struct StitchReport {
    pub plan: crate::stitch::StitchPlan,
    pub slots: Vec<SlotReport>,
    /// Pass 35: the synthesis post-pass's calibration record
    /// for this session (fitted parameters, measured score
    /// distributions, chosen thresholds); `None` when the
    /// post-pass did not run or could not calibrate.
    pub synth_calib: Option<crate::synth::SynthCalibSummary>,
}

impl StitchReport {
    /// Distinct held item names in this report.
    pub fn held_names(&self) -> std::collections::BTreeSet<&str> {
        self.slots
            .iter()
            .filter(|s| s.coverage == SlotCoverage::Held)
            .filter_map(|s| s.result.as_ref().and_then(|r| r.matched.as_deref()))
            .collect()
    }
}

/// Pass 36: the most instance crops per slot the readers
/// consume, on top of the canonical crop (which is always read).
/// Set from census + duplicate-inflation evidence
/// (docs/thirty-sixth-pass.md): the censuses run ~2
/// sightings/slot, so the cap rarely binds there, while at live
/// density (~16 sightings/slot) it cuts the per-slot OCR volume
/// roughly fourfold against the uncapped pass-34 stack.
const READER_INSTANCE_CAP: usize = 3;

/// Stitch one accumulated parts session and read every slot
/// (pass 33): plan the session ([`crate::stitch::plan_session`]),
/// build each slot's canonical crop from its usable instances,
/// and read the slot through the same [`read_cells`] path a
/// single frame takes. Every covered slot produces a
/// [`SlotReport`] — including slots no instance ever fully
/// showed.
///
/// The slot's verdict follows an agreement rule (pass 33 gate
/// finding): the canonical crop AND every usable instance are
/// read; the slot holds a name only when it is the ONE name any
/// of its reads holds. Two different held names make the slot
/// contested — refused and flagged as a disagreement — because
/// a panorama slot is one physical item and its sightings must
/// not disagree about what it is. (A single canonical read was
/// measured to lose the union-of-draws advantage the per-frame
/// pipeline had: items at the OCR margin are held by one of
/// their several sightings, rarely by a designated one.)
///
/// Pass 34 adds the second reader: with the line reader on,
/// every crop is ALSO read by the line reader
/// ([`crate::lineread::read_cells_lines`]) under the same
/// agreement rule, the slot's form channels are measured, and
/// the two verdicts are arbitrated by
/// [`crate::lineread::arbitrate`] — hold on agreement, form
/// channels as the licence for a lone reader, refusal on any
/// active contradiction. With the line reader off the slot
/// verdict is pass 33's exactly.
pub fn stitch_parts_session(
    frames: &[GrayImage],
    layout: &Layout,
    engine: &dyn OcrEngine,
    opts: &StitchOptions,
    universe: &Universe,
) -> StitchReport {
    let refs: Vec<&GrayImage> = frames.iter().collect();
    let plan = crate::stitch::plan_session(&refs, layout);
    let mut slots_out: Vec<SlotReport> = Vec::new();
    for (si, seg) in plan.segments.iter().enumerate() {
        let slots = crate::stitch::slot_instances(&plan, si, &refs, layout);
        let cw = seg.pitch.col_pitch.round().max(1.0) as u32;
        let ch = seg.pitch.row_pitch.round().max(1.0) as u32;
        // Pass 34 ablation (§4): snap crop origins to detected
        // tile boundaries instead of the fitted grid.
        let slots = if opts.snap_cells {
            slots
                .iter()
                .map(|s| crate::stitch::snap_slot(s, &refs, cw, ch))
                .collect()
        } else {
            slots
        };
        // Flattened read list: every slot's canonical crop first
        // (slot order), then each multi-instance slot's usable
        // instance crops (slot order), all through one read_cells
        // call so the batched engine sees the whole segment.
        let mut canons: Vec<Option<crate::stitch::CanonicalCrop>> = Vec::with_capacity(slots.len());
        let mut cells: Vec<Option<GrayImage>> = Vec::with_capacity(slots.len());
        for slot in &slots {
            let cc = crate::stitch::canonical_crop(slot, &refs, cw, ch);
            cells.push(cc.as_ref().map(|c| c.image.clone()));
            canons.push(cc);
        }
        let mut inst_ranges: Vec<std::ops::Range<usize>> = Vec::with_capacity(slots.len());
        // Pass 36 reader instance cap: the readers consume the
        // canonical crop plus at most READER_INSTANCE_CAP
        // best-ranked usable instance crops per slot (ranked by
        // the canonical criteria — margin, gain agreement, link
        // residual). At census density (~2 sightings/slot) the
        // cap rarely binds; at live density (~16) the uncapped
        // list was the bundle cost driver. Counts and the mesh
        // channel read the same capped set.
        let mut read_sets: Vec<Vec<usize>> = Vec::with_capacity(slots.len());
        for slot in &slots {
            let usable_n = slot.instances.iter().filter(|i| i.usable()).count();
            let read_set =
                crate::stitch::rank_instances(slot, &refs, cw, ch, READER_INSTANCE_CAP);
            let start = cells.len();
            if usable_n > 1 {
                for &ii in &read_set {
                    let i = &slot.instances[ii];
                    cells.push(Some(crate::stitch::crop_integer(
                        &frames[i.frame],
                        i.fx, i.fy, cw, ch,
                    )));
                }
            }
            inst_ranges.push(start..cells.len());
            read_sets.push(read_set);
        }
        let results = read_cells(&cells, layout, engine, &opts.read, universe);
        // Pass 34: the line reader over the SAME crops (canonical
        // + instances, same order), when enabled.
        let line_outcomes = if opts.line_reader {
            Some(crate::lineread::read_cells_lines(
                &cells, layout, engine, &opts.read, universe,
            ))
        } else {
            None
        };
        for (idx, slot) in slots.iter().enumerate() {
            let partial = slot.instances.iter().filter(|i| !i.usable()).count();
            match &canons[idx] {
                Some(cc) => {
                    let usable: Vec<&crate::stitch::Instance> =
                        slot.instances.iter().filter(|i| i.usable()).collect();
                    // Counts vote from the reader-capped instance
                    // set (pass 36), not every usable sighting.
                    let icells: Vec<Option<GrayImage>> = read_sets[idx]
                        .iter()
                        .map(|&ii| {
                            let i = &slot.instances[ii];
                            Some(crate::stitch::crop_integer(
                                &frames[i.frame],
                                i.fx, i.fy, cw, ch,
                            ))
                        })
                        .collect();
                    let counts = engine.read_counts_in(&icells, layout);
                    // Band agreement rule (pass 33): distinct
                    // held names across the canonical read and
                    // the instance reads; exactly one name holds,
                    // more than one is an internal contest (the
                    // reader abstains, flagged as disagreement).
                    let canon_res = &results[idx];
                    let mut band_names: Vec<&str> = Vec::new();
                    if canon_res.held {
                        if let Some(m) = canon_res.matched.as_deref() {
                            band_names.push(m);
                        }
                    }
                    let mut band_winner: Option<&ocr::CellResult> =
                        if canon_res.held { Some(canon_res) } else { None };
                    for ri in inst_ranges[idx].clone() {
                        let r = &results[ri];
                        if r.held {
                            if let Some(m) = r.matched.as_deref() {
                                if !band_names.contains(&m) {
                                    band_names.push(m);
                                }
                                if band_winner.is_none() {
                                    band_winner = Some(r);
                                }
                            }
                        }
                    }
                    let band_contested = band_names.len() > 1;
                    let band_held: Option<String> = if band_names.len() == 1 {
                        Some(band_names[0].to_owned())
                    } else {
                        None
                    };
                    // Line agreement (pass 34): the same rule
                    // over the line reader's outcomes, plus the
                    // slot's form channels.
                    let mut line_held: Option<String> = None;
                    let mut line_contested = false;
                    let mut line_winner: Option<ocr::CellResult> = None;
                    let mut channels: Option<crate::lineread::FormChannels> = None;
                    if let Some(outcomes) = &line_outcomes {
                        let lo0 = &outcomes[idx];
                        let mut line_names: Vec<&str> = Vec::new();
                        if lo0.result.held {
                            if let Some(m) = lo0.result.matched.as_deref() {
                                line_names.push(m);
                                line_winner = Some(lo0.result.clone());
                            }
                        }
                        for ri in inst_ranges[idx].clone() {
                            let o = &outcomes[ri];
                            if o.result.held {
                                if let Some(m) = o.result.matched.as_deref() {
                                    if !line_names.contains(&m) {
                                        line_names.push(m);
                                    }
                                    if line_winner.is_none() {
                                        line_winner = Some(o.result.clone());
                                    }
                                }
                            }
                        }
                        line_contested = line_names.len() > 1;
                        if line_names.len() == 1 {
                            line_held = Some(line_names[0].to_owned());
                        }
                        // Channels: mesh per instance crop,
                        // median-aggregated (the canonical
                        // composite blurs the mesh; when the
                        // slot has no separate instance crops
                        // the canonical IS the best instance);
                        // glyph on the canonical crop; line
                        // evidence pooled over the slot's reads,
                        // certification from the canonical read.
                        let mut mesh_v: Vec<f32> = Vec::new();
                        let range = inst_ranges[idx].clone();
                        if range.is_empty() {
                            mesh_v.push(
                                crate::lineread::mesh_scores(&cc.image, layout).margin_grad,
                            );
                        } else {
                            for ri in range.clone() {
                                if let Some(img) = &cells[ri] {
                                    mesh_v.push(
                                        crate::lineread::mesh_scores(img, layout)
                                            .margin_grad,
                                    );
                                }
                            }
                        }
                        mesh_v.sort_by(|a, b| {
                            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let mesh_score = if mesh_v.is_empty() {
                            0.0
                        } else {
                            mesh_v[mesh_v.len() / 2]
                        };
                        let glyph_score = crate::lineread::glyph_score(&cc.image);
                        let mut bp_line = lo0.bp_line;
                        for ri in range {
                            bp_line = bp_line || outcomes[ri].bp_line;
                        }
                        channels = Some(crate::lineread::FormChannels {
                            mesh: crate::lineread::mesh_class(mesh_score),
                            mesh_score,
                            glyph: glyph_score >= crate::lineread::GLYPH_NCC_MIN,
                            glyph_score,
                            bp_line,
                            no_line_certified: lo0.certified,
                        });
                    }
                    // Arbitration (pass 34): with the line
                    // reader off, the band agreement decides
                    // exactly as pass 33 left it.
                    let (held_name, held_via, refuse_reason, form_verdict) =
                        match &channels {
                            Some(ch) => {
                                let v = ch.verdict();
                                match crate::lineread::arbitrate(
                                    band_held.as_deref(),
                                    line_held.as_deref(),
                                    ch,
                                    universe,
                                ) {
                                    crate::lineread::Arbitration::Hold { name, via } => {
                                        (Some(name), Some(via), None, v)
                                    }
                                    crate::lineread::Arbitration::Refuse { reason } => {
                                        (None, None, Some(reason), v)
                                    }
                                }
                            }
                            None => {
                                let h = band_held.clone();
                                let via = if h.is_some() {
                                    Some(crate::lineread::Via::Band)
                                } else {
                                    None
                                };
                                (h, via, None, crate::lineread::FormVerdict::Unknown)
                            }
                        };
                    let held = held_name.is_some();
                    let result = match (&held_name, held_via) {
                        (Some(_), Some(crate::lineread::Via::Line)) => line_winner
                            .clone()
                            .unwrap_or_else(|| canon_res.clone()),
                        (Some(_), _) => band_winner
                            .cloned()
                            .unwrap_or_else(|| canon_res.clone()),
                        _ => canon_res.clone(),
                    };
                    // The built-copy glyph on a slot held as a
                    // blueprint implies its built sibling as
                    // held stock (brief §2(c)).
                    let glyph_implied = match (&held_name, &channels) {
                        (Some(n), Some(ch))
                            if ch.glyph
                                && crate::lineread::form_of(universe, n)
                                    == Some(crate::lineread::Form::Blueprint) =>
                        {
                            universe.form_sibling(n)
                        }
                        _ => None,
                    };
                    slots_out.push(SlotReport {
                        segment: si,
                        row: slot.row,
                        col: slot.col,
                        coverage: if held {
                            SlotCoverage::Held
                        } else {
                            SlotCoverage::SeenNotHeld
                        },
                        result: Some(result),
                        instances: usable.len(),
                        partial_sightings: partial,
                        instance_counts: counts,
                        disagreement: cc.disagreement()
                            || band_contested
                            || line_contested
                            || refuse_reason == Some("name-disagreement"),
                        median: cc.median,
                        fallback: seg.relinked,
                        band_held,
                        line_held,
                        form_verdict,
                        held_via,
                        refuse_reason,
                        glyph_implied,
                        glyph_score: channels.map(|c| c.glyph_score).unwrap_or(0.0),
                        mesh_score: channels.map(|c| c.mesh_score).unwrap_or(0.0),
                        synth: None,
                        // Retain the canonical crop for exactly
                        // the classes the finalize-time synthesis
                        // adjudication consumes (see the field).
                        canonical_crop: if held_via == Some(crate::lineread::Via::Both)
                            || !held
                        {
                            Some(cc.image.clone())
                        } else {
                            None
                        },
                    });
                }
                _ => {
                    slots_out.push(SlotReport {
                        segment: si,
                        row: slot.row,
                        col: slot.col,
                        coverage: SlotCoverage::NeverFullyVisible,
                        result: None,
                        instances: 0,
                        partial_sightings: partial,
                        instance_counts: Vec::new(),
                        disagreement: false,
                        median: false,
                        fallback: seg.relinked,
                        band_held: None,
                        line_held: None,
                        form_verdict: crate::lineread::FormVerdict::Unknown,
                        held_via: None,
                        refuse_reason: None,
                        glyph_implied: None,
                        glyph_score: 0.0,
                        mesh_score: 0.0,
                        synth: None,
                        canonical_crop: None,
                    });
                }
            }
        }
    }
    // Pass 36 integration: no synthesis hook here. The post-pass
    // adjudicates ONCE per session over the merged bundle
    // reports at finalize (`CaptureSession::finish_parts` →
    // `crate::synth::adjudicate_session`), preserving the
    // v0.5.29 whole-session semantics; it consumes the canonical
    // crops retained on the slots it touches, so this per-bundle
    // function stays free of session state.
    StitchReport {
        plan,
        slots: slots_out,
        synth_calib: None,
    }
}

/// A coarse change signature: a 24x14 grid of block means, hashed. Two
/// consecutive frames with equal signatures are "the same screen" and are
/// skipped without OCR; a scroll (any cell moving) changes the signature.
fn frame_signature(img: &GrayImage) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let bx = 24u32.min(img.width.max(1));
    let by = 14u32.min(img.height.max(1));
    for byi in 0..by {
        for bxi in 0..bx {
            let x0 = bxi * img.width / bx;
            let x1 = (((bxi + 1) * img.width) / bx).max(x0 + 1).min(img.width);
            let y0 = byi * img.height / by;
            let y1 = (((byi + 1) * img.height) / by).max(y0 + 1).min(img.height);
            let mut sum = 0u64;
            let mut n = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += img.get(x, y) as u64;
                    n += 1;
                }
            }
            (if n == 0 { 0 } else { sum / n }).hash(&mut h);
        }
    }
    h.finish()
}

/// A source of frames, a few per second. The real implementation is the
/// ScreenCast/PipeWire source; tests and calibration use [`SequenceSource`].
pub trait FrameSource: Send {
    /// Block for the next frame, or return `None` when the source is done
    /// (finite sequence exhausted, or a shutdown request observed). An
    /// `Err` aborts the capture with a visible message.
    fn next_frame(&mut self) -> Result<Option<GrayImage>, String>;
}

/// A finite source over a fixed list of frames (calibration + the §2 gate).
pub struct SequenceSource {
    frames: Vec<GrayImage>,
    at: usize,
}

impl SequenceSource {
    pub fn new(frames: Vec<GrayImage>) -> Self {
        Self { frames, at: 0 }
    }
}

impl FrameSource for SequenceSource {
    fn next_frame(&mut self) -> Result<Option<GrayImage>, String> {
        if self.at >= self.frames.len() {
            return Ok(None);
        }
        let f = self.frames[self.at].clone();
        self.at += 1;
        Ok(Some(f))
    }
}

/// Drain a frame source into a [`CaptureSession`] until it ends, `stop` is
/// set, or the classifier has been idle for `idle_stop_frames` consecutive
/// frames after collecting something ("Seth left the inventory"). Returns
/// the finished session. Callers do the finalise + report.
pub fn run_capture(
    ctx: &CaptureContext<'_>,
    engine: &dyn OcrEngine,
    source: &mut dyn FrameSource,
    stop: &AtomicBool,
    idle_stop_frames: u64,
    on_progress: impl FnMut(&CaptureSession),
) -> Result<CaptureSession, String> {
    run_capture_limited(
        ctx,
        engine,
        source,
        stop,
        idle_stop_frames,
        SESSION_MAX_DURATION,
        on_progress,
    )
}

/// [`run_capture`] with the session duration cap as a parameter
/// (pass 36 addendum item 3 — a test needs the cap in
/// milliseconds, production gets [`SESSION_MAX_DURATION`]).
/// When the cap trips, draining stops and the session
/// auto-finalizes through the ordinary finish path; the trip
/// is recorded on the session and surfaces in the finalize
/// output via [`CaptureSession::failsafe_summary`].
pub fn run_capture_limited(
    ctx: &CaptureContext<'_>,
    engine: &dyn OcrEngine,
    source: &mut dyn FrameSource,
    stop: &AtomicBool,
    idle_stop_frames: u64,
    max_duration: std::time::Duration,
    mut on_progress: impl FnMut(&CaptureSession),
) -> Result<CaptureSession, String> {
    let mut session = CaptureSession::new();
    // Pass 36: when a tesseract engine can be probed for the
    // parts layout, the session's bundles stitch on a background
    // thread with that dedicated instance (the frame engine's
    // cache lock is held across whole batched reads, so sharing
    // it would serialise the scroll against the stitch — the
    // forensic cache-lock finding). No second engine (a stub
    // session, tests) keeps the synchronous pass-34 path.
    if let Some(parts_layout) = ctx.layouts.get("parts") {
        if let Some(e) = crate::ocr::TesseractCliEngine::probe(parts_layout.clone()) {
            session.set_stitch_engine(Box::new(e));
        }
    }
    let started = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if started.elapsed() >= max_duration {
            session.note_duration_cap();
            break;
        }
        match source.next_frame()? {
            Some(frame) => {
                session.process_frame(ctx, engine, &frame);
                on_progress(&session);
                if idle_stop_frames > 0
                    && session.idle >= idle_stop_frames
                    && session.has_any()
                {
                    break;
                }
            }
            None => break,
        }
    }
    // Pass 33: the parts stitch closes when the capture does.
    session.finish_parts(ctx, engine);
    Ok(session)
}

/// Buckets a capture touched, for a human summary.
pub fn captured_summary(session: &CaptureSession) -> String {
    let c = session.counts();
    let mut parts = Vec::new();
    if session.saw_relics {
        parts.push(format!("{} relic stacks", c.relic_stacks));
    }
    if session.saw_parts {
        parts.push(format!("{} parts", c.parts));
    }
    if session.saw_arcanes {
        parts.push(format!("{} arcanes", c.arcanes));
    }
    if parts.is_empty() {
        "nothing collected".to_owned()
    } else {
        format!("Capture: {}", parts.join(", "))
    }
}

// ---------------------------------------------------------------------------
// Capture lifecycle (ninth pass §1)
// ---------------------------------------------------------------------------

/// The capture lifecycle stages Seth sees in the tray. The whole flow is
/// one single-shot pipeline; the tray reflects exactly where it is, and
/// **every** failure lands in [`CaptureStage::Error`] naming the stage that
/// failed (never a silent stall — item 40a).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureStage {
    /// No capture running.
    Idle,
    /// The ScreenCast consent dialog is up (or the portal is being asked).
    ConsentPending,
    /// Consent granted; opening the PipeWire remote / connecting the stream.
    Connecting,
    /// Frames flowing; per-bucket counts accrue.
    Capturing,
    /// Scroll done; captured buckets are being written to stock + report.
    Finalizing,
    /// A stage failed. `stage` names it; `message` is the verbatim
    /// D-Bus/PipeWire error so nothing is swallowed.
    Error { stage: &'static str, message: String },
}

impl CaptureStage {
    /// Human label for the tray status line.
    pub fn label(&self) -> String {
        match self {
            CaptureStage::Idle => "idle".to_owned(),
            CaptureStage::ConsentPending => "consent pending".to_owned(),
            CaptureStage::Connecting => "connecting".to_owned(),
            CaptureStage::Capturing => "capturing".to_owned(),
            CaptureStage::Finalizing => "finalizing".to_owned(),
            CaptureStage::Error { stage, message } => format!("error ({stage}): {message}"),
        }
    }

    /// A capture is in flight (any stage except Idle/Error). The tray uses
    /// this to relabel the toggle "Stop capture".
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            CaptureStage::ConsentPending
                | CaptureStage::Connecting
                | CaptureStage::Capturing
                | CaptureStage::Finalizing
        )
    }

    pub fn is_error(&self) -> bool {
        matches!(self, CaptureStage::Error { .. })
    }
}

/// The live capture state: current stage plus the running per-bucket
/// counts. Pure — the resident owns one behind the status mutex, so the
/// transition rules and failure surfacing are unit-testable with a fake
/// frame source (ninth pass §1 gate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureLifecycle {
    stage: CaptureStage,
    counts: CaptureCounts,
}

impl Default for CaptureLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureLifecycle {
    pub fn new() -> Self {
        Self {
            stage: CaptureStage::Idle,
            counts: CaptureCounts::default(),
        }
    }

    pub fn stage(&self) -> &CaptureStage {
        &self.stage
    }

    pub fn counts(&self) -> CaptureCounts {
        self.counts
    }

    /// Move to `stage`, returning the stage we came from. Callers record
    /// the transition (the machine itself does no I/O, so it stays pure).
    pub fn set_stage(&mut self, stage: CaptureStage) -> CaptureStage {
        let previous = std::mem::replace(&mut self.stage, stage);
        if !matches!(self.stage, CaptureStage::Capturing) {
            // Counts only mean anything while capturing; clear on every
            // other transition so a stale count never shows.
            self.counts = CaptureCounts::default();
        }
        previous
    }

    /// Record a failure at `stage` (verbatim message). The stage becomes
    /// [`CaptureStage::Error`] naming it — the visible failure surface.
    pub fn fail(&mut self, stage: &'static str, message: impl Into<String>) {
        self.stage = CaptureStage::Error {
            stage,
            message: message.into(),
        };
        self.counts = CaptureCounts::default();
    }

    pub fn set_counts(&mut self, counts: CaptureCounts) {
        self.counts = counts;
    }

    /// One line for the tray: the stage, plus live counts while capturing.
    pub fn status_line(&self) -> String {
        match &self.stage {
            CaptureStage::Capturing => format!(
                "Capture: capturing — {} relic stacks, {} parts, {} arcanes",
                self.counts.relic_stacks, self.counts.parts, self.counts.arcanes
            ),
            other => format!("Capture: {}", other.label()),
        }
    }
}

/// Append-only capture log in the app data dir (ninth pass §1): every
/// lifecycle transition and every verbatim D-Bus/PipeWire failure lands
/// here, so a silent post-consent stall is impossible to miss. Cheap to
/// clone (shared handle) and safe to write from the PipeWire thread.
#[derive(Clone)]
pub struct CaptureLog {
    path: PathBuf,
    file: Arc<Mutex<Option<std::fs::File>>>,
}

impl CaptureLog {
    /// Open (append) `<data_dir>/capture.log`. Failure to open is not
    /// fatal: the log degrades to a no-op and the tray still shows state.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("capture.log");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        Self {
            path,
            file: Arc::new(Mutex::new(file)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one timestamped line: `<local time> [stage] message`.
    pub fn record(&self, stage: &str, message: &str) {
        let Ok(mut guard) = self.file.lock() else {
            return;
        };
        let Some(f) = guard.as_mut() else {
            return;
        };
        use std::io::Write;
        let stamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z");
        let _ = writeln!(f, "{stamp} [{stage}] {message}");
    }
}

/// A frame source that yields `good` frames, then fails with `msg` — the
/// fake failing source the §1 gate uses to prove a mid-capture frame
/// failure surfaces instead of stalling.
pub struct FailingSource {
    remaining: usize,
    msg: String,
}

impl FailingSource {
    pub fn after(good: usize, msg: impl Into<String>) -> Self {
        Self {
            remaining: good,
            msg: msg.into(),
        }
    }
}

impl FrameSource for FailingSource {
    fn next_frame(&mut self) -> Result<Option<GrayImage>, String> {
        if self.remaining == 0 {
            return Err(self.msg.clone());
        }
        self.remaining -= 1;
        Ok(Some(GrayImage::blank(4, 4, 7)))
    }
}

/// Drive one capture through the full lifecycle with injected stage
/// actions. The resident's real actions are portal/PipeWire I/O; the §1
/// gate injects fakes that fail at each stage. Every `Err` lands the
/// machine in [`CaptureStage::Error`] naming the stage and is returned
/// verbatim.
pub fn run_capture_flow<O, C, D, F>(
    lifecycle: &mut CaptureLifecycle,
    open_consent: O,
    connect: C,
    drain: D,
    finalize: F,
) -> Result<CaptureSession, String>
where
    O: FnOnce() -> Result<(), String>,
    C: FnOnce() -> Result<Box<dyn FrameSource>, String>,
    D: FnOnce(&mut dyn FrameSource, &mut CaptureLifecycle) -> Result<CaptureSession, String>,
    F: FnOnce(&CaptureSession) -> Result<(), String>,
{
    lifecycle.set_stage(CaptureStage::ConsentPending);
    if let Err(e) = open_consent() {
        lifecycle.fail("consent pending", e.clone());
        return Err(e);
    }
    lifecycle.set_stage(CaptureStage::Connecting);
    let mut source = match connect() {
        Ok(s) => s,
        Err(e) => {
            lifecycle.fail("connecting", e.clone());
            return Err(e);
        }
    };
    lifecycle.set_stage(CaptureStage::Capturing);
    let session = match drain(source.as_mut(), lifecycle) {
        Ok(s) => s,
        Err(e) => {
            lifecycle.fail("capturing", e.clone());
            return Err(e);
        }
    };
    lifecycle.set_stage(CaptureStage::Finalizing);
    if let Err(e) = finalize(&session) {
        lifecycle.fail("finalizing", e.clone());
        return Err(e);
    }
    lifecycle.set_stage(CaptureStage::Idle);
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::{RefinementTier, Universe};

    fn classifier() -> ScreenClassifier {
        ScreenClassifier::from_toml(
            r#"
            header = [0.0, 0.0, 1.0, 0.12]
            header_psm = 6
            header_upscale = 2
            idle_stop_frames = 5
            [relic]
            keywords = ["REFINEMENT", "VOID RELICS", "RADIANT"]
            [parts]
            keywords = ["INVENTORY", "SELL"]
            [arcanes]
            keywords = ["ARCANES", "UPGRADED"]
            "#,
        )
        .unwrap()
    }

    #[test]
    fn screens_toml_loads_and_scores_the_measured_headers() {
        // The exact header text measured from Seth's three screens.
        let c = classifier();
        assert_eq!(
            c.classify_text("VOID RELICS/REFINEMENT RADIANT 45/799"),
            ScreenClass::Relics
        );
        assert_eq!(c.classify_text("INVENTORY / SELL 47,554"), ScreenClass::Parts);
        assert_eq!(
            c.classify_text("ARCANES ALL COLLECTED 111/166 UPGRADED 35/166"),
            ScreenClass::Arcanes
        );
        // A menu/gameplay frame matches nothing -> Other (pause).
        assert_eq!(c.classify_text("Navigation  Star Chart"), ScreenClass::Other);
        assert_eq!(c.idle_stop_frames, 5);
        // Addendum #83: Seth's real header reads, 2026-10-24
        // (v0.5.20 run). The title font drops the "IN" ligature
        // in every read; the third also splits "SELL" and went
        // Other under the old exact-substring match — the
        // misclassification that auto-finalised a live capture.
        assert_eq!(
            c.classify_text("rene (SIN V ENT O R Y / SELL ny ee # | 53,212,704 @4,004"),
            ScreenClass::Parts
        );
        assert_eq!(
            c.classify_text("i _ SI VENTOR fi SELL toca & | 53,212,704 @4,004"),
            ScreenClass::Parts
        );
        assert_eq!(
            c.classify_text("ae VENTOR Y/ SEL L Bee. $i | 453,212,704 &4,004"),
            ScreenClass::Parts
        );
        // The shipped file parses and covers all three classes.
        let shipped = ScreenClassifier::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("data/capture/screens.toml"),
        )
        .expect("shipped screens.toml loads");
        for cls in [ScreenClass::Relics, ScreenClass::Parts, ScreenClass::Arcanes] {
            assert!(cls.layout_key().is_some());
        }
        assert!(!shipped.relic_keywords.is_empty());
    }

    #[test]
    fn relic_variant_reads_the_header_title() {
        // Pass 43: the refinement screen names itself; the
        // selection picker's header does not. Garbled reads of
        // the kind Seth's headers produce stay on their side of
        // the line (the fuzzy window forgives a dropped glyph).
        let c = classifier();
        assert_eq!(
            c.relic_variant("VOID RELICS/REFINEMENT ALL COLLECTED 528/800 RADIANT 46/800"),
            RelicScreenVariant::Refinement
        );
        assert_eq!(
            c.relic_variant("4 VOID RELICS/REFINEMENT oo (Cine SV ALL COLLECTED 528/800"),
            RelicScreenVariant::Refinement
        );
        assert_eq!(
            c.relic_variant("VOID RELICS ALL"),
            RelicScreenVariant::Selection
        );
        assert_eq!(
            c.relic_variant("+VOID RELICS [SEARCH"),
            RelicScreenVariant::Selection
        );
    }

    /// Engine that counts header reads and always reports a relic
    /// header — the seam for the addendum #81 header-cache test.
    struct CountingEngine {
        header_reads: std::sync::Mutex<usize>,
    }

    impl crate::ocr::OcrEngine for CountingEngine {
        fn name(&self) -> &str {
            "counting"
        }

        fn ocr_cell(&self, _cell: &GrayImage) -> crate::ocr::OcrRead {
            crate::ocr::OcrRead::empty()
        }

        fn read_text(
            &self,
            _image: &GrayImage,
            _region: [f32; 4],
            _psm: u32,
            _upscale: u32,
        ) -> String {
            *self.header_reads.lock().unwrap() += 1;
            "VOID RELICS REFINEMENT".to_owned()
        }
    }

    #[test]
    fn header_is_reread_only_when_its_pixels_change() {
        // Addendum #81: a scroll step changes the frame but not the
        // header strip — classification must reuse the cached header
        // read instead of spawning tesseract again. A changed header
        // is re-read.
        let engine = CountingEngine {
            header_reads: std::sync::Mutex::new(0),
        };
        let universes = UniverseSet::default();
        let layouts = HashMap::new();
        let relic_names = HashSet::new();
        let arcane_names = HashSet::new();
        let classifier = classifier();
        let ctx = CaptureContext {
            layouts: &layouts,
            universes: &universes,
            relic_names: &relic_names,
            arcane_names: &arcane_names,
            classifier: &classifier,
            min_confidence: 0.5,
            radius_decoder: true,
            radius_strip: true,
            radius_tail_budget: crate::ocr::RADIUS_TAIL_BUDGET,
            line_reader: false,
            strip_offsets: false,
            snap_cells: false,
            synthesis: false,
            debug_dir: None,
        };
        let mut session = CaptureSession::new();

        // 100x100 frames; the header strip is the top 12 rows.
        let frame = |header_v: u8, body_v: u8| {
            let mut img = GrayImage::blank(100, 100, body_v);
            for y in 0..12 {
                for x in 0..100 {
                    img.set(x, y, header_v);
                }
            }
            img
        };
        session.process_frame(&ctx, &engine, &frame(200, 10));
        assert_eq!(*engine.header_reads.lock().unwrap(), 1);
        // Body scrolls, header identical: no second header read.
        session.process_frame(&ctx, &engine, &frame(200, 90));
        session.process_frame(&ctx, &engine, &frame(200, 160));
        assert_eq!(*engine.header_reads.lock().unwrap(), 1);
        // Header changes: re-read.
        session.process_frame(&ctx, &engine, &frame(40, 160));
        assert_eq!(*engine.header_reads.lock().unwrap(), 2);
        // Only fresh classifications land in the evidence samples.
        assert_eq!(session.header_samples.len(), 2);
    }

    /// An engine whose header read returns scripted texts in
    /// order (repeating the last one when the script runs out).
    struct ScriptedEngine {
        texts: std::sync::Mutex<std::collections::VecDeque<String>>,
    }

    impl crate::ocr::OcrEngine for ScriptedEngine {
        fn name(&self) -> &str {
            "scripted"
        }

        fn ocr_cell(&self, _cell: &GrayImage) -> crate::ocr::OcrRead {
            crate::ocr::OcrRead::empty()
        }

        fn read_text(
            &self,
            _image: &GrayImage,
            _region: [f32; 4],
            _psm: u32,
            _upscale: u32,
        ) -> String {
            let mut q = self.texts.lock().unwrap();
            if q.len() > 1 {
                q.pop_front().unwrap()
            } else {
                q.front().cloned().unwrap_or_default()
            }
        }
    }

    #[test]
    fn idle_needs_three_consecutive_fresh_other_classifications() {
        // Addendum #83: one header misread must not be able to
        // end a capture. Idle accrues only from the third
        // consecutive FRESH Other classification on; a fresh real
        // classification resets the streak.
        let engine = ScriptedEngine {
            texts: std::sync::Mutex::new(
                [
                    "INVENTORY / SELL", // fresh Parts — streak 0
                    "unreadable banner", // fresh Other — streak 1
                    "unreadable banner", // fresh Other — streak 2
                    "unreadable banner", // fresh Other — streak 3: idle starts
                    "INVENTORY / SELL", // fresh Parts — streak resets
                    "unreadable banner", // fresh Other — streak 1 again
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ),
        };
        let universes = UniverseSet::default();
        let layouts = HashMap::new();
        let relic_names = HashSet::new();
        let arcane_names = HashSet::new();
        let classifier = classifier();
        let ctx = CaptureContext {
            layouts: &layouts,
            universes: &universes,
            relic_names: &relic_names,
            arcane_names: &arcane_names,
            classifier: &classifier,
            min_confidence: 0.5,
            radius_decoder: true,
            radius_strip: true,
            radius_tail_budget: crate::ocr::RADIUS_TAIL_BUDGET,
            line_reader: false,
            strip_offsets: false,
            snap_cells: false,
            synthesis: false,
            debug_dir: None,
        };
        let mut session = CaptureSession::new();
        // Header values >=16 apart => distinct header-cache keys
        // => every frame is a FRESH classification; body steps
        // move the coarse frame signature so no frame takes the
        // unchanged-screen shortcut.
        let frame = |header_v: u8, body_v: u8| {
            let mut img = GrayImage::blank(100, 100, body_v);
            for y in 0..12 {
                for x in 0..100 {
                    img.set(x, y, header_v);
                }
            }
            img
        };
        // No layouts in ctx: even Parts frames take the no-layout
        // branch, which is where idle used to accrue per frame.
        session.process_frame(&ctx, &engine, &frame(32, 10));
        assert_eq!(session.idle_frames(), 0);
        session.process_frame(&ctx, &engine, &frame(64, 60));
        assert_eq!(session.idle_frames(), 0, "first fresh Other must not accrue idle");
        session.process_frame(&ctx, &engine, &frame(96, 110));
        assert_eq!(session.idle_frames(), 0, "second fresh Other must not accrue idle");
        session.process_frame(&ctx, &engine, &frame(128, 160));
        assert_eq!(session.idle_frames(), 1, "third consecutive fresh Other accrues idle");
        session.process_frame(&ctx, &engine, &frame(160, 210));
        assert_eq!(session.idle_frames(), 1, "fresh Parts resets the streak, no accrual");
        session.process_frame(&ctx, &engine, &frame(192, 250));
        assert_eq!(session.idle_frames(), 1, "a lone fresh Other after reset does not accrue");
    }

    #[test]
    fn debug_dumps_land_in_the_given_dir_with_sequence_names() {
        // Addendum #82: dumps go to the context's debug dir (the
        // caller points it at the USER data dir — the install tree
        // is read-only in the Flatpak), sequence-named per class,
        // capped per session.
        let universes = UniverseSet::default();
        let layouts = HashMap::new();
        let relic_names = HashSet::new();
        let arcane_names = HashSet::new();
        let classifier = classifier();
        let dir = std::env::temp_dir().join(format!("fracker-dbg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = CaptureContext {
            layouts: &layouts,
            universes: &universes,
            relic_names: &relic_names,
            arcane_names: &arcane_names,
            classifier: &classifier,
            min_confidence: 0.5,
            radius_decoder: true,
            radius_strip: true,
            radius_tail_budget: crate::ocr::RADIUS_TAIL_BUDGET,
            line_reader: false,
            strip_offsets: false,
            snap_cells: false,
            synthesis: false,
            debug_dir: Some(dir.clone()),
        };
        let mut session = CaptureSession::new();
        let frame = GrayImage::blank(8, 8, 42);
        session.dump_debug_frame(&ctx, &frame, "parts");
        session.dump_debug_frame(&ctx, &frame, "parts");
        assert_eq!(
            session.debug_files,
            vec!["frame-parts-00.png".to_string(), "frame-parts-01.png".to_string()]
        );
        // Writes are off-thread; finalize joins them, and the
        // files must then be on disk — deterministically, no
        // polling (under full-suite load the writers can be
        // starved for seconds).
        session.flush_debug_dumps();
        for i in 0..2 {
            assert!(
                dir.join(format!("frame-parts-{i:02}.png")).exists(),
                "evidence frame {i} should be written"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An engine that names each cell positionally (test seam for
    /// the read path: validation sees an exact read per cell, so
    /// the owned-gate is the only thing that can unhold a tile).
    struct PositionalEngine {
        names: Vec<String>,
    }

    impl OcrEngine for PositionalEngine {
        fn name(&self) -> &str {
            "positional-test"
        }
        fn ocr_cell(&self, _cell: &GrayImage) -> ocr::OcrRead {
            ocr::OcrRead::empty()
        }
        fn ocr_cells_in(
            &self,
            cells: &[Option<GrayImage>],
            _layout: &Layout,
        ) -> Vec<Option<ocr::OcrRead>> {
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    c.as_ref().map(|_| ocr::OcrRead {
                        text: self.names[i].clone(),
                        confidence: 1.0,
                        count: None,
                        rank: None,
                    })
                })
                .collect()
        }
    }

    /// A synthetic relic cell (canonical 168x174): checkerboard
    /// icon at the bright (210) or faded (52) highlight level, and
    /// optionally the measured eye template at the badge window.
    /// The calibration is ocr's own tests' business; this builder
    /// exists so the read-path test can present a true eye.
    fn gate_cell(eye: bool, bright: bool) -> GrayImage {
        let mut cell = GrayImage::blank(168, 174, 6);
        let (hi, lo) = if bright { (210, 60) } else { (52, 15) };
        for y in 17..122 {
            for x in 25..143 {
                cell.set(x, y, if (x + y) % 2 == 0 { hi } else { lo });
            }
        }
        if eye {
            // The measured template itself (ocr tests own the
            // calibration; the read path only needs a true eye).
            let tpl = ocr::eye_template_for_test();
            for (ty, row) in tpl.iter().enumerate() {
                for (tx, &v) in row.iter().enumerate() {
                    cell.set(tx as u32, ty as u32, v);
                }
            }
        }
        cell
    }

    fn relic_layout_capture() -> Layout {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let layouts =
            Layout::load_file(&base.join("data/capture/layouts.toml")).unwrap();
        layouts.get("relic").unwrap().clone()
    }

    fn gate_read_options() -> ReadOptions {
        ReadOptions {
            min_confidence: 0.5,
            radius_decoder: false,
            radius_strip: false,
            radius_tail_budget: crate::ocr::RADIUS_TAIL_BUDGET,
            strip_offsets: false,
            relic_bright_ref: None,
            relic_variant: RelicScreenVariant::Unknown,
        }
    }

    #[test]
    fn read_cells_applies_the_owned_gate_to_relic_frames() {
        let layout = relic_layout_capture();
        let names: Vec<String> = [
            "Lith A1", "Meso B2", "Neo C3", "Axi D4", "Axi E5", "Axi F6",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let universe = Universe::from_names(&names);
        let engine = PositionalEngine { names: names.clone() };
        // 3 owned (bright) + 3 unowned (dim + eye): the gate inside
        // read_cells must unhold exactly the unowned three.
        let cells: Vec<Option<GrayImage>> = (0..6)
            .map(|i| Some(gate_cell(i >= 3, i < 3)))
            .collect();
        let results = read_cells(&cells, &layout, &engine, &gate_read_options(), &universe);
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r.held, i < 3, "cell {i} held");
            assert_eq!(r.gate_excluded, i >= 3, "cell {i} gate flag");
        }
    }

    #[test]
    fn browse_strip_gate_verdicts() {
        // The manual-verification gate of pass 38: run the
        // production read path over Seth's browse screenshot
        // (fracker-frames/fixtures/browse-strip.png, a
        // palette-clipped chat upload — verdicts, not thresholds,
        // are what this asserts). Skips when the strip is absent.
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let strip_path = root.join("../fracker-frames/fixtures/browse-strip.png");
        if !strip_path.exists() {
            eprintln!("skip browse-strip: fixture absent");
            return;
        }
        let img = GrayImage::from_bytes(&std::fs::read(&strip_path).unwrap()).unwrap();
        // The strip is the grid band of a 2560x1440 capture; its
        // measured grid (pass 38): cells 168x174 at x0 62, y0 15,
        // pitch (168.2, 174). Pad the 2px the upload clipped off
        // the right edge so the last column cuts whole.
        let mut canvas = GrayImage::blank(1912, 363, 0);
        for y in 0..img.height {
            for x in 0..img.width {
                canvas.set(x, y, img.get(x, y));
            }
        }
        let names: [[&str; 11]; 2] = [
            [
                "Neo F3", "Neo K9", "Neo O2", "Neo P4", "Neo P7", "Neo P9", "Neo T11",
                "Neo V7", "Neo V12", "Neo Y1", "Axi A2",
            ],
            [
                "Axi A3", "Axi A6", "Axi A7", "Axi A8", "Axi A12", "Axi A13", "Axi A15",
                "Axi A19", "Axi A23", "Axi B7", "Axi B9",
            ],
        ];
        let flat: Vec<String> = names
            .iter()
            .flatten()
            .map(|s| s.to_string())
            .collect();
        let universe = Universe::from_names(&flat);
        let engine = PositionalEngine { names: flat.clone() };
        let layout = relic_layout_capture();
        let mut cells: Vec<Option<GrayImage>> = Vec::new();
        for r in 0..2 {
            for c in 0..11 {
                let x = (62.0 + c as f64 * 168.2) as u32;
                let y = (15.0 + r as f64 * 174.0) as u32;
                cells.push(Some(canvas.crop(x, y, 168, 174)));
            }
        }
        let results = read_cells(&cells, &layout, &engine, &gate_read_options(), &universe);
        for (i, r) in results.iter().enumerate() {
            eprintln!(
                "strip verdict {:>9}: held={} gate_excluded={}",
                flat[i], r.held, r.gate_excluded
            );
            if i < 10 {
                assert!(r.held, "owned {} must be kept", flat[i]);
            } else {
                assert!(!r.held, "unowned {} must not enter stock", flat[i]);
            }
        }
    }

    #[test]
    fn refinement_corpus_replay_three_state_disposition() {
        // Pass 45 (addendum #110) corpus gate: replay Seth's 25
        // native refinement frames (the 2026-10-26 scrape whose
        // stock imported 43 wrong names) through the production
        // read path, folding each frame through the session's
        // own machinery — evidence.observe, count votes, and
        // resolve_voted_stock — with the variant taken from
        // each frame's header exactly as the session takes it.
        // The three-state disposition must land exactly:
        //
        // - the only genuinely unowned tiles, the 11 Axi of the
        //   catalogue's final row, are excluded (and named by
        //   the session's finalize line);
        // - every ground-truth owned label-less stack imports
        //   at x1: the stock-2 singleton pairs (three of them
        //   read labelled x2 in this corpus — Axi A21, Lith
        //   M7, Lith T14; the corpus is authoritative for the
        //   corpus and they are asserted with the labelled
        //   population), Meso V15 in both tiers, the Axi V1 /
        //   Lith A2 / Meso A8 Radiant singles, the 26 Meso/Neo
        //   x1s pass 43 wrongly listed as phantoms (they are
        //   Seth's; stock-2's scroll never reached them), and
        //   Axi K2, a bare x1 stock-2 never recorded;
        // - the labelled population is unchanged from pass 43
        //   (411 stacks at their counts);
        // - "Meso L1" has no tile: the reader names the Meso
        //   C11 [Radiant] cell "Meso L1" in both its settled
        //   appearances (raw "Mesoll Relic [Radiant]"). Pass 45
        //   pinned that false import as a known issue; pass 46
        //   refuses it at the name-admission gate — L1 does not
        //   import, the refusal is recorded, and the finalize
        //   line names it. Skips when the corpus is absent
        //   (the strip-test pattern).
        const UNOWNED_AXI: &[&str] = &[
            "Axi A12", "Axi A13", "Axi A15", "Axi A19", "Axi A2", "Axi A23", "Axi A3",
            "Axi A6", "Axi A7", "Axi A8", "Axi B7",
        ];
        const OWNED_X1: &[(&str, RefinementTier)] = &[
            // The 65 stock-2 singleton (name, tier) pairs.
            ("Axi A14", RefinementTier::Intact),
            ("Axi A20", RefinementTier::Intact),
            ("Axi A22", RefinementTier::Intact),
            ("Axi A9", RefinementTier::Intact),
            ("Axi B4", RefinementTier::Intact),
            ("Axi B8", RefinementTier::Intact),
            ("Axi C4", RefinementTier::Intact),
            ("Axi C6", RefinementTier::Intact),
            ("Axi F2", RefinementTier::Intact),
            ("Axi G5", RefinementTier::Intact),
            ("Axi H7", RefinementTier::Intact),
            ("Axi N3", RefinementTier::Intact),
            ("Axi N9", RefinementTier::Intact),
            ("Axi O3", RefinementTier::Intact),
            ("Axi P1", RefinementTier::Intact),
            ("Axi S16", RefinementTier::Intact),
            ("Axi S20", RefinementTier::Intact),
            ("Axi S8", RefinementTier::Intact),
            ("Axi T5", RefinementTier::Intact),
            ("Lith A12", RefinementTier::Intact),
            ("Lith A7", RefinementTier::Radiant),
            ("Lith B11", RefinementTier::Radiant),
            ("Lith C1", RefinementTier::Intact),
            ("Lith C11", RefinementTier::Radiant),
            ("Lith C12", RefinementTier::Radiant),
            ("Lith C14", RefinementTier::Radiant),
            ("Lith C2", RefinementTier::Radiant),
            ("Lith C7", RefinementTier::Intact),
            ("Lith C9", RefinementTier::Radiant),
            ("Lith D3", RefinementTier::Intact),
            ("Lith D7", RefinementTier::Intact),
            ("Lith G12", RefinementTier::Intact),
            ("Lith G14", RefinementTier::Radiant),
            ("Lith G6", RefinementTier::Radiant),
            ("Lith H2", RefinementTier::Radiant),
            ("Lith H4", RefinementTier::Radiant),
            ("Lith H7", RefinementTier::Radiant),
            ("Lith K12", RefinementTier::Radiant),
            ("Lith K7", RefinementTier::Intact),
            ("Lith L1", RefinementTier::Radiant),
            ("Lith L7", RefinementTier::Intact),
            ("Lith L8", RefinementTier::Radiant),
            ("Lith N15", RefinementTier::Radiant),
            ("Lith N16", RefinementTier::Intact),
            ("Lith N5", RefinementTier::Intact),
            ("Lith O4", RefinementTier::Intact),
            ("Lith O4", RefinementTier::Radiant),
            ("Lith P5", RefinementTier::Intact),
            ("Lith P6", RefinementTier::Radiant),
            ("Lith P8", RefinementTier::Intact),
            ("Lith P9", RefinementTier::Intact),
            ("Lith R4", RefinementTier::Radiant),
            ("Lith S10", RefinementTier::Intact),
            ("Lith S11", RefinementTier::Radiant),
            ("Lith S15", RefinementTier::Radiant),
            ("Lith S16", RefinementTier::Radiant),
            ("Lith S9", RefinementTier::Intact),
            ("Meso A1", RefinementTier::Intact),
            ("Meso A5", RefinementTier::Radiant),
            ("Meso D2", RefinementTier::Intact),
            ("Meso E5", RefinementTier::Intact),
            ("Meso F2", RefinementTier::Intact),
            // Seth's ground truth, absent from stock-2.
            ("Meso V15", RefinementTier::Intact),
            ("Meso V15", RefinementTier::Radiant),
            ("Axi V1", RefinementTier::Radiant),
            ("Lith A2", RefinementTier::Radiant),
            ("Meso A8", RefinementTier::Radiant),
            // The 26 Meso/Neo x1s the pass-43 diff mislabelled.
            ("Meso G7", RefinementTier::Intact),
            ("Meso H1", RefinementTier::Intact),
            ("Meso L3", RefinementTier::Intact),
            ("Meso L5", RefinementTier::Intact),
            ("Meso M1", RefinementTier::Intact),
            ("Meso N12", RefinementTier::Intact),
            ("Meso O3", RefinementTier::Intact),
            ("Meso P9", RefinementTier::Intact),
            ("Meso R2", RefinementTier::Intact),
            ("Meso T2", RefinementTier::Intact),
            ("Meso V5", RefinementTier::Intact),
            ("Meso V11", RefinementTier::Intact),
            ("Meso Z2", RefinementTier::Intact),
            ("Neo A12", RefinementTier::Intact),
            ("Neo B6", RefinementTier::Intact),
            ("Neo E2", RefinementTier::Intact),
            ("Neo F3", RefinementTier::Intact),
            ("Neo K9", RefinementTier::Intact),
            ("Neo O2", RefinementTier::Intact),
            ("Neo P4", RefinementTier::Intact),
            ("Neo P7", RefinementTier::Intact),
            ("Neo P9", RefinementTier::Intact),
            ("Neo T11", RefinementTier::Intact),
            ("Neo V7", RefinementTier::Intact),
            ("Neo V12", RefinementTier::Intact),
            ("Neo Y1", RefinementTier::Intact),
            // Bare in the corpus, absent from stock-2 (a gain
            // that evening or its scroll's coverage gap — the
            // three-state read is owned x1 either way).
            ("Axi K2", RefinementTier::Intact),
        ];
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let corpus = root.join("../fracker-frames/live-relic-sync");
        if !corpus.join("frame-relics-00.png").exists() {
            eprintln!("skip refinement corpus: absent");
            return;
        }
        let data = CaptureData::load(&root.join("data")).expect("capture data loads");
        let ctx = data.context();
        let layout = ctx.layouts.get("relic").expect("relic layout").clone();
        let Some(engine) = crate::ocr::TesseractCliEngine::probe(layout.clone()) else {
            eprintln!("skip refinement corpus: no tesseract");
            return;
        };
        let universe = ctx.universes.for_class(ScreenClass::Relics);
        let mut evidence = crate::ocr::RefinementEvidence::default();
        let mut name_refusals: Vec<crate::ocr::NameRefusal> = Vec::new();
        let mut relic_votes: HashMap<(String, RefinementTier), HashMap<u32, u32>> =
            HashMap::new();
        let mut labelled: HashMap<(String, RefinementTier), u32> = HashMap::new();
        for i in 0..25 {
            let path = corpus.join(format!("frame-relics-{i:02}.png"));
            let img = GrayImage::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
            let (class, header) = data.classifier.classify_detailed(&engine, &img);
            assert_eq!(class, ScreenClass::Relics, "frame {i} class");
            let variant = data.classifier.relic_variant(&header);
            assert_eq!(
                variant,
                RelicScreenVariant::Refinement,
                "frame {i} variant (header: {header:?})"
            );
            let results = read_frame(&layout, &img, &engine, &ctx, universe, None, variant);
            for r in &results {
                if let Some(nr) = &r.name_refused {
                    name_refusals.push(nr.clone());
                }
            }
            evidence.observe(&results, ctx.relic_names);
            for r in &results {
                if r.held {
                    if let Some(c) = r.count {
                        let Some(name) = &r.matched else { continue };
                        let e = labelled.entry((name.clone(), r.tier)).or_insert(0);
                        *e = (*e).max(c);
                    }
                }
            }
            let frame_stock =
                StockState::from_results(&results, ctx.relic_names, ctx.arcane_names);
            for (name, stacks) in &frame_stock.relics {
                for (tier, count) in &stacks.tiers {
                    *relic_votes
                        .entry((name.clone(), *tier))
                        .or_default()
                        .entry(*count)
                        .or_insert(0) += 1;
                }
            }
        }
        let stock = resolve_voted_stock(
            &HashMap::new(),
            &relic_votes,
            &HashMap::new(),
            &evidence,
        );
        let total_stacks: usize = stock.relics.values().map(|s| s.tiers.len()).sum();
        let total_items: u32 = stock.relics.values().map(|s| s.total()).sum();
        eprintln!(
            "corpus replay: {} stacks, {} items imported; {} labelled stacks; {} excluded",
            total_stacks,
            total_items,
            labelled.len(),
            evidence.excluded_stacks().len()
        );

        // The 11 unowned Axi are excluded, and the finalize
        // line names them.
        let session = {
            let mut s = CaptureSession::new();
            s.relic_evidence = evidence.clone();
            s
        };
        let summary = session.relic_variant_summary().expect("finalize line present");
        assert_eq!(evidence.excluded_stacks().len(), 11, "exactly the 11 Axi");
        for name in UNOWNED_AXI {
            assert!(
                !stock.relics.contains_key(*name),
                "unowned {name} imported"
            );
            assert!(summary.contains(name), "finalize line must name {name}: {summary}");
        }
        // Every ground-truth owned label-less stack imports x1.
        for (name, tier) in OWNED_X1 {
            assert_eq!(
                stock.relics.get(*name).map(|s| s.get(*tier)),
                Some(1),
                "owned x1 {name} {tier:?} must import at x1"
            );
        }
        // The labelled population is pass 43's: exactly the
        // same 411 stacks ever show a label, and each imports
        // at the count the (unchanged, pass-27) vote machinery
        // resolves from its per-frame reads — the mode of the
        // counts its frames voted, ties to the larger. Where
        // the label reads disagree across appearances (Lith
        // C9 reads 14 and 15), the mode is the arbiter, in
        // pass 43 and here alike; the max label is asserted
        // only where the reads agree.
        assert_eq!(labelled.len(), 411, "labelled stack count changed");
        for ((name, tier), max_label) in &labelled {
            let votes = &relic_votes[&(name.clone(), *tier)];
            let mode = votes
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)))
                .map(|(c, _)| *c)
                .expect("a labelled stack has votes");
            assert_eq!(
                stock.relics.get(name).map(|s| s.get(*tier)),
                Some(mode),
                "labelled {name} {tier:?} must import at its voted count"
            );
            if votes.len() == 1 {
                assert_eq!(mode, *max_label, "single-read label must agree");
            }
        }
        // The Lith M4 guard case: its x15 label never parses
        // (it sits at the count region's edge), so the tile
        // imports at the vote-resolved count — what matters
        // is that it imports at all: the eye disposition must
        // not exclude a physically labelled stack.
        assert!(
            stock.relics.contains_key("Lith M4"),
            "physically labelled Lith M4 must not be eye-excluded"
        );
        // Pass 46 (addendum #111): no "Meso L1" tile exists.
        // The reader names the owned Meso C11 [Radiant] cell
        // "Meso L1" in both settled appearances (raw "Mesoll
        // Relic [Radiant]" — the code token burned down and
        // fused with the era). Pass 45 pinned that false
        // import as a known issue; Seth's ruling reversed the
        // priority — a false name in stock is worse than a
        // missing relic — so the name-admission gate now
        // REFUSES the read: L1 does not import at all, the
        // C11 stack is missed (never misnamed), and the
        // refusal is recorded and named in the finalize line.
        assert!(
            !stock.relics.contains_key("Meso L1"),
            "the C11 garble must not import under the false name L1"
        );
        assert!(
            !stock.relics.contains_key("Meso C11")
                || stock.relics["Meso C11"].get(RefinementTier::Radiant) == 0,
            "the garbled appearance must not credit C11 Radiant either"
        );
        // Both settled appearances refused, one distinct
        // record (same raw, same winner), and the finalize
        // line names it with its runner-up and margin.
        assert_eq!(name_refusals.len(), 2, "both garble appearances refused");
        assert!(
            name_refusals
                .iter()
                .all(|nr| nr.winner == "Meso L1" && nr.raw == "Mesoll Relic [Radiant]"),
            "refusals are the C11 garble: {name_refusals:?}"
        );
        let name_session = {
            let mut s = CaptureSession::new();
            s.relic_name_refused = vec![name_refusals[0].clone()];
            s
        };
        let name_summary = name_session
            .relic_name_summary()
            .expect("name-admission finalize line present");
        assert!(
            name_summary.contains("Meso L1") && name_summary.contains("Mesoll"),
            "finalize line must name the refusal: {name_summary}"
        );
        // Totals reconcile: 411 labelled + the owned x1 list;
        // the garble pair is gone (505, down from 506).
        assert_eq!(
            total_stacks,
            411 + OWNED_X1.len(),
            "import composition changed"
        );
    }

    #[test]
    fn finalize_replaces_only_touched_buckets() {
        // A capture that only saw relics must not zero the arcane/part
        // buckets, and must replace the relic bucket wholesale.
        let mut session = CaptureSession::new();
        session.saw_relics = true;
        session
            .captured
            .relics
            .entry("Lith G6".to_owned())
            .or_default()
            .merge(RefinementTier::Intact, 36);

        let mut existing = StockState::default();
        existing.parts.insert("Forma Blueprint".to_owned(), 9);
        existing
            .arcanes
            .entry("Arcane Guardian".to_owned())
            .or_default()
            .merge(0, 14);
        existing
            .relics
            .entry("Axi A1".to_owned())
            .or_default()
            .merge(RefinementTier::Intact, 8);

        session.finalize_into(&mut existing);
        assert_eq!(existing.relics.len(), 1, "relic bucket replaced");
        assert_eq!(
            existing.relics.get("Lith G6").unwrap().get(RefinementTier::Intact),
            36
        );
        assert!(!existing.relics.contains_key("Axi A1"));
        assert_eq!(existing.parts.get("Forma Blueprint"), Some(&9), "parts untouched");
        assert!(existing.arcanes.contains_key("Arcane Guardian"), "arcanes untouched");
    }

    #[test]
    fn finalize_preserves_unverified_and_prunes_verified() {
        // Thirtieth pass (#93): a parts capture replaces the parts
        // bucket; the unverified (⍻) section survives EXCEPT the
        // entries the fresh capture verifies — by exact name or by
        // built-component form — whose assertions are spent.
        let mut session = CaptureSession::new();
        session.saw_parts = true;
        session
            .captured
            .parts
            .insert("Protea Prime Systems".to_owned(), 1); // built form
        session
            .captured
            .parts
            .insert("Khora Prime Chassis Blueprint".to_owned(), 2); // exact name

        let mut existing = StockState::default();
        existing.parts.insert("Stale Part".to_owned(), 7);
        existing
            .unverified
            .insert("Protea Prime Systems Blueprint".to_owned(), 1);
        existing
            .unverified
            .insert("Khora Prime Chassis Blueprint".to_owned(), 1);
        existing
            .unverified
            .insert("Wisp Prime Neuroptics Blueprint".to_owned(), 1);

        session.finalize_into(&mut existing);
        assert_eq!(
            existing.parts.get("Protea Prime Systems"),
            Some(&1),
            "parts bucket replaced"
        );
        assert!(!existing.parts.contains_key("Stale Part"));
        assert_eq!(
            existing.unverified.len(),
            1,
            "both verified assertions pruned (exact + built form)"
        );
        assert!(
            existing.unverified.contains_key("Wisp Prime Neuroptics Blueprint"),
            "the part the capture never saw keeps its assertion"
        );
    }

    #[test]
    fn finalize_without_parts_keeps_unverified_verbatim() {
        // A relic-only capture verifies nothing about parts, so no
        // unverified assertion is pruned (thirtieth pass).
        let mut session = CaptureSession::new();
        session.saw_relics = true;
        session
            .captured
            .relics
            .entry("Lith G6".to_owned())
            .or_default()
            .merge(RefinementTier::Intact, 3);
        let mut existing = StockState::default();
        existing
            .unverified
            .insert("Protea Prime Systems Blueprint".to_owned(), 1);
        session.finalize_into(&mut existing);
        assert_eq!(
            existing.unverified.get("Protea Prime Systems Blueprint"),
            Some(&1)
        );
    }

    #[test]
    fn voted_count_picks_the_mode_and_breaks_ties_toward_max() {
        let votes = |pairs: &[(u32, u32)]| pairs.iter().cloned().collect::<HashMap<u32, u32>>();
        assert_eq!(voted_count(&votes(&[(8, 1), (1, 2)])), 1, "mode wins");
        assert_eq!(voted_count(&votes(&[(8, 1), (1, 1)])), 8, "tie goes to max");
        assert_eq!(voted_count(&votes(&[(3, 1)])), 3, "single observation kept");
    }

    /// Engine whose cell reads are scripted (text + count), with a
    /// header that always classifies as the Parts screen.
    struct CountScriptEngine {
        reads: std::sync::Mutex<std::collections::VecDeque<crate::ocr::OcrRead>>,
    }

    impl crate::ocr::OcrEngine for CountScriptEngine {
        fn name(&self) -> &str {
            "count-script"
        }

        fn ocr_cell(&self, _cell: &GrayImage) -> crate::ocr::OcrRead {
            self.reads
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(crate::ocr::OcrRead::empty)
        }

        fn read_text(
            &self,
            _image: &GrayImage,
            _region: [f32; 4],
            _psm: u32,
            _upscale: u32,
        ) -> String {
            "INVENTORY / SELL".to_owned()
        }
    }

    #[test]
    fn session_count_votes_outvote_a_phantom_badge() {
        // Pass 27 (census finding 5): one frame's phantom badge read
        // (Trumna Prime Stock x1 recorded x8) must not survive when
        // later frames observe the true count — the session count is
        // the mode of the observations, not the max.
        let layouts = Layout::parse(
            r#"
            [parts]
            cols = 1
            rows = 1
            ref_width = 100
            ref_height = 100
            x0 = 0.0
            cell_w = 100.0
            y0 = 0.0
            cell_h = 100.0
            name = [0.0, 0.0, 1.0, 1.0]
            count = [0.0, 0.0, 0.3, 0.2]
            count_style = "badge"
            name_psm = 6
            count_psm = 7
            name_upscale = 1
            count_upscale = 1
            min_score = 0.5
            "#,
        )
        .unwrap();
        let universes = UniverseSet {
            parts: Universe::from_names(&["Trumna Prime Stock".to_owned()]),
            ..UniverseSet::default()
        };
        let relic_names = HashSet::new();
        let arcane_names = HashSet::new();
        let classifier = classifier();
        let ctx = CaptureContext {
            layouts: &layouts,
            universes: &universes,
            relic_names: &relic_names,
            arcane_names: &arcane_names,
            classifier: &classifier,
            min_confidence: 0.5,
            radius_decoder: true,
            radius_strip: true,
            radius_tail_budget: crate::ocr::RADIUS_TAIL_BUDGET,
            line_reader: false,
            strip_offsets: false,
            snap_cells: false,
            synthesis: false,
            debug_dir: None,
        };
        // Pass 33 mechanics: parts frames stitch at finish, and
        // the vote consumes PER-INSTANCE count reads — so the
        // script interleaves one name read (the canonical crop's)
        // with one count read per instance: [name, x8, name, x1,
        // name, x1] models frame 1 observing a phantom x8 and
        // frames 2–3 observing the true x1. (If frames 2–3 link
        // into one segment their slot still takes one name read
        // and two instance count reads; the script's fifth entry
        // is then consumed as a count read whose empty count votes
        // x1 — the assertions below hold under either stitch.)
        let name_read = || crate::ocr::OcrRead {
            text: "Trumna Prime Stock".to_owned(),
            confidence: 0.9,
            count: None,
            rank: None,
        };
        let count_read = |count: u32| crate::ocr::OcrRead {
            text: String::new(),
            confidence: 0.9,
            count: Some(count),
            rank: None,
        };
        let engine = CountScriptEngine {
            reads: std::sync::Mutex::new(
                vec![
                    name_read(),
                    count_read(8),
                    name_read(),
                    count_read(1),
                    name_read(),
                    count_read(1),
                ]
                .into(),
            ),
        };
        let mut session = CaptureSession::new();
        // Pass 37: the three frames model three separate scroll
        // observations, seconds apart in a real capture; the
        // session's feed clock spaces their arrivals past the
        // intake's time gate.
        let tick = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let tick2 = tick.clone();
        let clock_base = std::time::Instant::now();
        session.set_feed_clock(Box::new(move || {
            clock_base
                + std::time::Duration::from_secs(
                    3 * tick2.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                )
        }));
        // Distinct bodies (signatures), identical header strip.
        let frame = |body_v: u8| {
            let mut img = GrayImage::blank(100, 100, body_v);
            for y in 0..12 {
                for x in 0..100 {
                    img.set(x, y, 200);
                }
            }
            img
        };
        session.process_frame(&ctx, &engine, &frame(10));
        session.finish_parts(&ctx, &engine);
        assert_eq!(
            session.stock().parts.get("Trumna Prime Stock"),
            Some(&8),
            "a single observation stands until contradicted"
        );
        session.process_frame(&ctx, &engine, &frame(90));
        session.process_frame(&ctx, &engine, &frame(160));
        session.finish_parts(&ctx, &engine);
        assert_eq!(
            session.stock().parts.get("Trumna Prime Stock"),
            Some(&1),
            "two x1 observations outvote the one phantom x8"
        );
    }

    #[test]
    fn counts_and_summary_track_the_buckets() {
        let mut session = CaptureSession::new();
        session.saw_relics = true;
        session.saw_arcanes = true;
        session
            .captured
            .relics
            .entry("Lith G6".to_owned())
            .or_default()
            .merge(RefinementTier::Intact, 36);
        session
            .captured
            .relics
            .get_mut("Lith G6")
            .unwrap()
            .merge(RefinementTier::Radiant, 1);
        session
            .captured
            .arcanes
            .entry("Arcane Guardian".to_owned())
            .or_default()
            .merge(0, 14);
        assert_eq!(
            session.counts(),
            CaptureCounts { relic_stacks: 2, parts: 0, arcanes: 1 }
        );
        assert_eq!(session.status_line(), "Capture: 2 relic stacks, 0 parts, 1 arcanes");
        assert_eq!(captured_summary(&session), "Capture: 2 relic stacks, 1 arcanes");
    }

    #[test]
    fn sequence_source_ends_and_universe_round_trips() {
        // The frame source is finite; the universe handles the bucket routing.
        let mut src = SequenceSource::new(vec![
            GrayImage::blank(4, 4, 10),
            GrayImage::blank(4, 4, 20),
        ]);
        assert!(src.next_frame().unwrap().is_some());
        assert!(src.next_frame().unwrap().is_some());
        assert!(src.next_frame().unwrap().is_none());
        let u = Universe::from_names(&["Lith G6".to_owned()]);
        assert_eq!(u.len(), 1);
    }

    // ---- §1 capture lifecycle ----

    #[test]
    fn lifecycle_transitions_and_counts_render_in_the_status_line() {
        let mut lc = CaptureLifecycle::new();
        assert_eq!(*lc.stage(), CaptureStage::Idle);
        assert_eq!(lc.status_line(), "Capture: idle");
        assert!(!lc.stage().is_active());

        lc.set_stage(CaptureStage::ConsentPending);
        assert_eq!(lc.status_line(), "Capture: consent pending");
        assert!(lc.stage().is_active());

        lc.set_stage(CaptureStage::Connecting);
        assert_eq!(lc.status_line(), "Capture: connecting");

        lc.set_stage(CaptureStage::Capturing);
        lc.set_counts(CaptureCounts {
            relic_stacks: 12,
            parts: 3,
            arcanes: 1,
        });
        assert_eq!(
            lc.status_line(),
            "Capture: capturing — 12 relic stacks, 3 parts, 1 arcanes"
        );

        lc.set_stage(CaptureStage::Finalizing);
        // Counts do not leak past capturing.
        assert_eq!(lc.status_line(), "Capture: finalizing");
        assert_eq!(lc.counts(), CaptureCounts::default());

        lc.set_stage(CaptureStage::Idle);
        assert_eq!(lc.status_line(), "Capture: idle");
    }

    #[test]
    fn lifecycle_surfaces_a_failure_naming_the_stage() {
        let mut lc = CaptureLifecycle::new();
        lc.set_stage(CaptureStage::Connecting);
        lc.fail("connecting", "PipeWire: pw_stream_connect failed (-1)");
        assert!(lc.stage().is_error());
        assert!(!lc.stage().is_active());
        let line = lc.status_line();
        assert!(
            line.contains("error (connecting)") && line.contains("pw_stream_connect failed (-1)"),
            "the verbatim failure must survive into the status line: {line}"
        );
    }

    #[test]
    fn capture_flow_fails_at_each_stage_with_a_visible_stage_name() {
        // The §1 gate: a fake failure injected at each seam must land the
        // state machine in Error naming that stage — never a silent stall.
        let run = |which: &str| {
            let mut lc = CaptureLifecycle::new();
            let res = run_capture_flow(
                &mut lc,
                || {
                    if which == "consent" {
                        Err("portal: user cancelled".to_owned())
                    } else {
                        Ok(())
                    }
                },
                || {
                    if which == "connect" {
                        Err("PipeWire: pw_context_connect_fd failed".to_owned())
                    } else if which == "capture" {
                        // A fake source that yields one frame then fails.
                        Ok(Box::new(FailingSource::after(
                            1,
                            "PipeWire stream error: no first frame",
                        )) as Box<dyn FrameSource>)
                    } else {
                        Ok(Box::new(SequenceSource::new(vec![
                            GrayImage::blank(4, 4, 7),
                            GrayImage::blank(4, 4, 9),
                        ])) as Box<dyn FrameSource>)
                    }
                },
                |source, lifecycle| {
                    let universes = UniverseSet::default();
                    let layouts = HashMap::new();
                    let relic_names = HashSet::new();
                    let arcane_names = HashSet::new();
                    let classifier = ScreenClassifier::default();
                    let ctx = CaptureContext {
                        layouts: &layouts,
                        universes: &universes,
                        relic_names: &relic_names,
                        arcane_names: &arcane_names,
                        classifier: &classifier,
                        min_confidence: 0.5,
                        radius_decoder: true,
                        radius_strip: true,
                        radius_tail_budget: crate::ocr::RADIUS_TAIL_BUDGET,
                        line_reader: false,
                        strip_offsets: false,
                        snap_cells: false,
                        synthesis: false,
                        debug_dir: None,
                    };
                    let session = run_capture(
                        &ctx,
                        &crate::ocr::StubEngine,
                        source,
                        &AtomicBool::new(false),
                        0,
                        |_| {},
                    )?;
                    lifecycle.set_counts(session.counts());
                    Ok(session)
                },
                |_| {
                    if which == "finalize" {
                        Err("stock write: permission denied".to_owned())
                    } else {
                        Ok(())
                    }
                },
            );
            (lc, res)
        };

        for (which, stage) in [
            ("consent", "consent pending"),
            ("connect", "connecting"),
            ("capture", "capturing"),
            ("finalize", "finalizing"),
        ] {
            let (lc, res) = run(which);
            assert!(res.is_err(), "{which} must fail");
            match lc.stage() {
                CaptureStage::Error { stage: s, message } => {
                    assert_eq!(*s, stage, "failing stage name for {which}");
                    assert!(!message.is_empty());
                }
                other => panic!("{which} should land in Error, got {other:?}"),
            }
        }

        // The happy path still completes: Idle, never Error.
        let (lc, res) = run("ok");
        assert!(res.is_ok());
        assert_eq!(*lc.stage(), CaptureStage::Idle);
    }

    #[test]
    fn capture_log_appends_timestamped_lines() {
        let dir = std::env::temp_dir().join(format!("fracker-caplog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = CaptureLog::open(&dir);
        assert_eq!(log.path(), dir.join("capture.log"));
        log.record("connecting", "PipeWire: pw_stream_connect failed (-1)");
        log.record("capturing", "first frame after 42ms");
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("[connecting] PipeWire: pw_stream_connect failed (-1)"));
        assert!(text.contains("[capturing] first frame after 42ms"));
        // Clone shares the same file so a worker thread can append too.
        let clone = log.clone();
        clone.record("finalizing", "writing 2 relic stacks");
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("[finalizing] writing 2 relic stacks"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------------------------------------------------------------
    // Pass 36: intake dedupe, progress bundles, failsafes.
    // ---------------------------------------------------------------

    fn intake_layout() -> Layout {
        Layout {
            key: "parts".to_owned(),
            cols: 4,
            rows: 3,
            ref_width: 460,
            ref_height: 300,
            x0: 20.0,
            cell_w: 100.0,
            y0: 60.0,
            cell_h: 50.0,
            name: [0.0, 0.53, 1.0, 1.0],
            count: [0.06, 0.0, 0.32, 0.16],
            count_style: crate::ocr::CountStyle::Badge,
            name_psm: 11,
            count_psm: 7,
            name_upscale: 4,
            count_upscale: 8,
            name_upper: None,
            pips: None,
            min_score: 0.5,
        }
    }

    fn xorshift(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    /// One synthetic inventory frame at `seed`: every grid cell
    /// carries a 5x3 mosaic of seeded flat levels. The gradient
    /// lives at the level borders — a step map that is distinct
    /// per cell (per-pixel noise would average out of the
    /// block-mean thumbnails and make every cell look alike) —
    /// and the whole frame is scaled by `gain`, the
    /// animated-background proxy.
    ///
    /// Pass 37 fixture fidelity: real cells carry their
    /// dominant horizontal edge at the name band (frac ≈0.81
    /// of the cell — the fold calibration's FOLD_PEAK_FRAC),
    /// and the settle gate reads exactly that edge. The
    /// fixture paints it as a bright double row on a dark
    /// surround at the calibrated fraction of every cell;
    /// without it the fixture's fold phase is arbitrary and no
    /// synthetic frame could pass for settled.
    fn mosaic_frame(layout: &Layout, seed: u64, gain: f32) -> GrayImage {
        let (w, h) = (460u32, 300u32);
        let mut img = GrayImage::blank(w, h, 8);
        let rect = crate::stitch::GridRect::of(layout, &img);
        let cw = rect.w / layout.cols as f32;
        let ch = rect.h / layout.rows as f32;
        for r in 0..layout.rows {
            for c in 0..layout.cols {
                let x0 = (rect.x0 + c as f32 * cw) as u32;
                let y0 = (rect.y0 + r as f32 * ch) as u32;
                let x1 = (rect.x0 + (c + 1) as f32 * cw) as u32;
                let y1 = (rect.y0 + (r + 1) as f32 * ch) as u32;
                // Block-band boundaries are seeded per cell:
                // real item art puts its edges at varied
                // heights per cell, and the fixture must too —
                // borders at fixed fractions of every cell are
                // pitch-periodic, so they fold coherently and
                // fight the name band for the lattice fit, and
                // they comb the row profile the scroll
                // estimator reads (pass 37 correction).
                let mut bst = seed
                    ^ ((r as u64) << 24)
                    ^ ((c as u64) << 16)
                    ^ 0x2545_F491;
                let row_jit = [
                    (xorshift(&mut bst) % 11) as i32 - 5,
                    (xorshift(&mut bst) % 11) as i32 - 5,
                ];
                let col_jit = [
                    (xorshift(&mut bst) % 9) as i32 - 4,
                    (xorshift(&mut bst) % 9) as i32 - 4,
                    (xorshift(&mut bst) % 9) as i32 - 4,
                    (xorshift(&mut bst) % 9) as i32 - 4,
                ];
                let mut xb = vec![x0];
                for bx in 1..5u32 {
                    xb.push(
                        (x0 + bx * (x1 - x0) / 5)
                            .saturating_add_signed(col_jit[(bx - 1) as usize]),
                    );
                }
                xb.push(x1);
                let mut yb = vec![y0];
                for by in 1..3u32 {
                    yb.push(
                        (y0 + by * (y1 - y0) / 3)
                            .saturating_add_signed(row_jit[(by - 1) as usize]),
                    );
                }
                yb.push(y1);
                for by in 0..3usize {
                    for bx in 0..5usize {
                        let mut st = seed
                            ^ ((r as u64) << 24)
                            ^ ((c as u64) << 16)
                            ^ ((by as u64) << 8)
                            ^ bx as u64
                            ^ 0x9E37_79B9;
                        let level = 40.0 + (xorshift(&mut st) % 180) as f32;
                        let (sx0, sx1, sy0, sy1) = (xb[bx], xb[bx + 1], yb[by], yb[by + 1]);
                        for y in sy0..sy1.min(h) {
                            for x in sx0..sx1.min(w) {
                                let v = (level * gain).clamp(0.0, 255.0) as u8;
                                img.set(x, y, v);
                            }
                        }
                    }
                }
                // The name-band edge (see the doc comment): a
                // bright double row at the calibrated fraction
                // of the cell, dark rows flanking it. Like a
                // real name, its LEVEL and LENGTH are seeded
                // per cell — the fold needs the edge's POSITION
                // coherent across cells, and the thumbnails
                // need its content distinct per cell; a uniform
                // line would be neither (it would pull every
                // cell's thumbnail toward identity).
                let mut lst = seed
                    ^ ((r as u64) << 24)
                    ^ ((c as u64) << 16)
                    ^ 0x51AB_C793;
                // Sized like a real name, not a banner: with
                // the texture aperiodic (above), a mid-strength
                // line dominates the fold while leaving the
                // raw row profile's texture decisive for the
                // scroll-dy estimator.
                let line_level = 150.0 + (xorshift(&mut lst) % 80) as f32;
                let span0 = (xorshift(&mut lst) % 25) as u32;
                let span1 = 45 + (xorshift(&mut lst) % 40) as u32;
                let line_y = y0 + (ch * 0.8054) as u32;
                if line_y >= 1 && line_y + 2 < y1 {
                    for x in (x0 + span0)..(x0 + span1).min(x1) {
                        let v = (line_level * gain).clamp(0.0, 255.0) as u8;
                        img.set(x, line_y - 1, 8);
                        img.set(x, line_y, v);
                        img.set(x, line_y + 1, v);
                        img.set(x, line_y + 2, 8);
                    }
                }
            }
        }
        img
    }

    /// Pass 37: offers carry arrival times. Tests that mean to
    /// exercise the content policy space their offers 3 s
    /// apart — past the 2 s time gate — through this helper.
    fn offer_spaced(
        intake: &mut PartsIntake,
        frame: &GrayImage,
        layout: &Layout,
        t0: std::time::Instant,
        step: u64,
    ) -> IntakeVerdict {
        intake.offer_at(frame, layout, t0 + std::time::Duration::from_secs(3 * step))
    }

    #[test]
    fn intake_collapses_a_static_viewport_to_the_cap() {
        // The incident's shape: the same viewport offered over
        // and over (with gain jitter standing in for the
        // animated background) must collapse to at most
        // VIEWPORT_ACCEPT_CAP accepted frames; the rest are
        // counted drops.
        let layout = intake_layout();
        let base = mosaic_frame(&layout, 7, 1.0);
        let mut intake = PartsIntake::new();
        let t0 = std::time::Instant::now();
        let (mut accepted, mut dropped) = (0usize, 0usize);
        for i in 0..16u64 {
            let gain = 1.0 + ((i % 5) as f32 - 2.0) * 0.015;
            let frame = if i == 0 {
                base.clone()
            } else {
                // Same pixels, global gain — viewport_print's
                // soft normalisation must see through it.
                let mut f = base.clone();
                for p in &mut f.pixels {
                    *p = ((*p as f32) * gain).clamp(0.0, 255.0) as u8;
                }
                f
            };
            match offer_spaced(&mut intake, &frame, &layout, t0, i) {
                IntakeVerdict::Accept { .. } => accepted += 1,
                IntakeVerdict::DropDuplicate => dropped += 1,
                other => panic!("unexpected verdict: {other:?}"),
            }
        }
        assert_eq!(accepted, VIEWPORT_ACCEPT_CAP, "static viewport collapses to the cap");
        assert_eq!(dropped, 16 - VIEWPORT_ACCEPT_CAP);
        assert_eq!(intake.accepted_total() as usize, VIEWPORT_ACCEPT_CAP);
        // A different page is a new position, accepted at once.
        let other = mosaic_frame(&layout, 99, 1.0);
        assert!(matches!(
            offer_spaced(&mut intake, &other, &layout, t0, 16),
            IntakeVerdict::Accept { .. }
        ));
    }

    #[test]
    fn intake_flushes_after_a_viewport_of_progress() {
        // A panorama of 9 textured rows; frames are 3-row
        // windows stepped one row (one pitch) at a time. A flush
        // must be requested once a viewport (3 pitches) of
        // progress has accumulated.
        let layout = intake_layout();
        // 10 rows under a 60px header, so a window cropped at a
        // whole-pitch step has its grid rows exactly on pano
        // rows (pass 37: the settle gate reads that alignment;
        // the pre-37 fixture painted rows from y 0 and every
        // window sat ~11 px off the layout lattice). The last
        // window (step 4) crops y 200..500. (Boundary semantics
        // fire the flush on the first frame of the position
        // AFTER the crossing, one offer later than the pre-36
        // verdict timing this fixture was sized for — the crop
        // at step 4 must exist.)
        let pano_rows = 10u32;
        let pano_h = 60 + pano_rows * 50;
        let mut pano = GrayImage::blank(460, pano_h, 8);
        for r in 0..pano_rows {
            // Paint row r of the panorama with the mosaic frame
            // seeded by r: rows are mutually distinct content.
            let f = mosaic_frame(&layout, 1000 + r as u64, 1.0);
            for y in 0..50u32 {
                for x in 0..460u32 {
                    let v = f.get(x, 60 + y);
                    pano.set(x, 60 + r * 50 + y, v);
                }
            }
        }
        let mut intake = PartsIntake::new();
        let t0 = std::time::Instant::now();
        let mut flush_at = None;
        for k in 0..5u32 {
            let frame = pano.crop(0, k * 50, 460, 300);
            match offer_spaced(&mut intake, &frame, &layout, t0, k as u64) {
                IntakeVerdict::Accept { flush: true } => {
                    flush_at = Some(k);
                    break;
                }
                IntakeVerdict::Accept { flush: false } => {}
                other => panic!("scroll frames are never dropped: {other:?}"),
            }
        }
        let k = flush_at.expect("a viewport of progress must request a flush");
        assert!(k <= 4, "flush within a viewport + one step (got step {k})");
    }

    #[test]
    fn intake_session_cap_trips_and_is_reported() {
        let layout = intake_layout();
        let mut intake = PartsIntake::with_session_cap(5);
        let t0 = std::time::Instant::now();
        let (mut accepted, mut capped) = (0usize, 0usize);
        for i in 0..9u64 {
            // Distinct position per offer (distinct mosaic seed).
            let frame = mosaic_frame(&layout, 500 + i * 17, 1.0);
            match offer_spaced(&mut intake, &frame, &layout, t0, i) {
                IntakeVerdict::Accept { .. } => accepted += 1,
                IntakeVerdict::DropSessionCap => capped += 1,
                other => panic!("distinct positions never cap/drop otherwise: {other:?}"),
            }
        }
        assert_eq!(accepted, 5);
        assert_eq!(capped, 4);
        assert!(intake.cap_tripped(), "the trip is recorded, not silent");
    }

    /// Shift a frame's content down by `dy` px (vacated rows
    /// take the background level) — a synthetic scroll
    /// transient for the settle gate.
    fn shift_down(frame: &GrayImage, dy: u32) -> GrayImage {
        let mut out = GrayImage::blank(frame.width, frame.height, 8);
        for y in dy..frame.height {
            for x in 0..frame.width {
                out.set(x, y, frame.get(x, y - dy));
            }
        }
        out
    }

    #[test]
    fn settle_gate_accepts_on_lattice_and_drops_displaced() {
        // Pass 37: the gate reads the fixture's own fold phase
        // — asserted in-band before the policy's verdict is.
        let layout = intake_layout();
        let settled = mosaic_frame(&layout, 7, 1.0);
        let phase = crate::stitch::lattice_phase(&settled, &layout)
            .expect("the fixture carries a measurable lattice");
        assert!(phase.abs() <= SETTLED_PHASE_MAX_PX, "fixture phase {phase}");
        let mut intake = PartsIntake::new();
        let t0 = std::time::Instant::now();
        assert!(matches!(
            offer_spaced(&mut intake, &settled, &layout, t0, 0),
            IntakeVerdict::Accept { .. }
        ));
        // The same content displaced 20 px (0.4 of the test
        // pitch — a mid-scroll transient at this scale) is
        // measurably off-lattice and never becomes a candidate.
        let displaced = shift_down(&settled, 20);
        let phase = crate::stitch::lattice_phase(&displaced, &layout)
            .expect("displaced content still carries a lattice");
        assert!(phase.abs() > SETTLED_PHASE_MAX_PX, "displaced phase {phase}");
        assert_eq!(
            offer_spaced(&mut intake, &displaced, &layout, t0, 1),
            IntakeVerdict::DropOffLattice
        );
        assert_eq!(intake.off_lattice_dropped(), 1);
        assert_eq!(intake.accepted_total(), 1, "the transient was not pooled");
        // An unmeasurable frame (blank — no lattice fit) is
        // NOT charged to the settle gate: the content policy
        // judges it, and on a fresh intake it anchors.
        let blank = GrayImage::blank(460, 300, 8);
        assert!(crate::stitch::lattice_phase(&blank, &layout).is_none());
        let mut fresh = PartsIntake::new();
        assert!(matches!(
            fresh.offer_at(&blank, &layout, t0),
            IntakeVerdict::Accept { .. }
        ));
        assert_eq!(fresh.off_lattice_dropped(), 0);
    }

    #[test]
    fn time_gate_drops_offers_inside_the_interval() {
        // Pass 37: at most one accepted viewport per
        // MIN_ACCEPT_INTERVAL, measured from the last ACCEPT —
        // drops do not move the clock.
        let layout = intake_layout();
        let a = mosaic_frame(&layout, 7, 1.0);
        let b = mosaic_frame(&layout, 99, 1.0);
        let mut intake = PartsIntake::new();
        let t0 = std::time::Instant::now();
        let at = |ms: u64| t0 + std::time::Duration::from_millis(ms);
        assert!(matches!(
            intake.offer_at(&a, &layout, at(0)),
            IntakeVerdict::Accept { .. }
        ));
        // Inside the interval the gate fires before the
        // content policy — for the same frame (which the
        // policy would call a duplicate) and for a new
        // position alike.
        assert_eq!(intake.offer_at(&a, &layout, at(500)), IntakeVerdict::DropInterval);
        assert_eq!(intake.offer_at(&b, &layout, at(1_500)), IntakeVerdict::DropInterval);
        assert_eq!(intake.interval_dropped(), 2);
        assert_eq!(intake.accepted_total(), 1);
        // 2.5 s after the accept at t0 (not after the drops),
        // offers flow again.
        assert!(matches!(
            intake.offer_at(&b, &layout, at(2_500)),
            IntakeVerdict::Accept { .. }
        ));
        assert_eq!(intake.accepted_total(), 2);
    }

    #[test]
    fn latch_drops_the_immediate_return_within_a_bundle() {
        // Pass 37: A→B→A inside one bundle refuses the return;
        // a further displacement re-admits A; a flush clears
        // the latch for the next bundle.
        let layout = intake_layout();
        let a = mosaic_frame(&layout, 7, 1.0);
        let b = mosaic_frame(&layout, 99, 1.0);
        let c = mosaic_frame(&layout, 517, 1.0);
        let mut intake = PartsIntake::new();
        let t0 = std::time::Instant::now();
        assert!(matches!(
            offer_spaced(&mut intake, &a, &layout, t0, 0),
            IntakeVerdict::Accept { .. }
        ));
        assert!(matches!(
            offer_spaced(&mut intake, &b, &layout, t0, 1),
            IntakeVerdict::Accept { .. }
        ));
        assert_eq!(
            offer_spaced(&mut intake, &a, &layout, t0, 2),
            IntakeVerdict::DropLatched
        );
        assert_eq!(
            offer_spaced(&mut intake, &a, &layout, t0, 3),
            IntakeVerdict::DropLatched
        );
        assert_eq!(intake.latched_dropped(), 2);
        // C's acceptance moves the latch to B; A is fresh again.
        assert!(matches!(
            offer_spaced(&mut intake, &c, &layout, t0, 4),
            IntakeVerdict::Accept { .. }
        ));
        assert!(matches!(
            offer_spaced(&mut intake, &a, &layout, t0, 5),
            IntakeVerdict::Accept { .. }
        ));
        // A's acceptance latched C; the flush clears it, so C
        // may open the next bundle.
        intake.note_flush();
        assert!(matches!(
            offer_spaced(&mut intake, &c, &layout, t0, 6),
            IntakeVerdict::Accept { .. }
        ));
    }

    #[test]
    fn gate_counters_account_every_drop_kind() {
        // Pass 37: one scripted sequence hitting each gate;
        // the counters the finalize log prints must account
        // for every drop.
        let layout = intake_layout();
        let a = mosaic_frame(&layout, 7, 1.0);
        let b = mosaic_frame(&layout, 99, 1.0);
        let displaced = shift_down(&a, 20);
        let mut intake = PartsIntake::new();
        let t0 = std::time::Instant::now();
        let at = |ms: u64| t0 + std::time::Duration::from_millis(ms);
        assert!(matches!(
            intake.offer_at(&a, &layout, at(0)),
            IntakeVerdict::Accept { .. }
        ));
        assert_eq!(
            intake.offer_at(&displaced, &layout, at(3_000)),
            IntakeVerdict::DropOffLattice
        );
        assert_eq!(
            intake.offer_at(&b, &layout, at(500)),
            IntakeVerdict::DropInterval
        );
        assert!(matches!(
            intake.offer_at(&b, &layout, at(3_500)),
            IntakeVerdict::Accept { .. }
        ));
        assert_eq!(
            intake.offer_at(&a, &layout, at(7_000)),
            IntakeVerdict::DropLatched
        );
        assert_eq!(intake.accepted_total(), 2);
        assert_eq!(intake.off_lattice_dropped(), 1);
        assert_eq!(intake.interval_dropped(), 1);
        assert_eq!(intake.latched_dropped(), 1);
    }

    #[test]
    fn session_reports_intake_counters_and_stitch_path() {
        // Pass 37: the finalize log's sources — gate counters
        // delegate to the intake; the stitch-path label names
        // the configured path.
        let mut session = CaptureSession::new();
        assert_eq!(session.parts_off_lattice_dropped(), 0);
        assert_eq!(session.parts_interval_dropped(), 0);
        assert_eq!(session.parts_latched_dropped(), 0);
        assert_eq!(session.bundle_queue_dropped_frames(), 0);
        assert_eq!(
            session.stitch_path_label(),
            "synchronous fallback (frame engine, capture thread)"
        );
        session.set_stitch_engine(Box::new(crate::ocr::StubEngine));
        assert_eq!(
            session.stitch_path_label(),
            "background stitch thread (dedicated second engine)"
        );
    }

    #[test]
    fn debug_dump_cap_is_recorded_by_name() {
        // Pass 37 (LIVE_DIAG2 §1): when the per-name dump cap
        // stops dumping, the session records the name for the
        // finalize log — the v0.5.30 run's dump folder ended
        // at the cap in silence and read as a truncated
        // capture.
        let universes = UniverseSet::default();
        let layouts = HashMap::new();
        let relic_names = HashSet::new();
        let arcane_names = HashSet::new();
        let classifier = classifier();
        let dir = std::env::temp_dir().join(format!("fracker-dbgcap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = CaptureContext {
            layouts: &layouts,
            universes: &universes,
            relic_names: &relic_names,
            arcane_names: &arcane_names,
            classifier: &classifier,
            min_confidence: 0.5,
            radius_decoder: true,
            radius_strip: true,
            radius_tail_budget: crate::ocr::RADIUS_TAIL_BUDGET,
            line_reader: false,
            strip_offsets: false,
            snap_cells: false,
            synthesis: false,
            debug_dir: Some(dir.clone()),
        };
        let mut session = CaptureSession::new();
        let frame = GrayImage::blank(8, 8, 42);
        for _ in 0..(CaptureSession::MAX_DEBUG_PER_NAME + 5) {
            session.dump_debug_frame(&ctx, &frame, "parts");
        }
        assert_eq!(
            session.debug_files.len(),
            CaptureSession::MAX_DEBUG_PER_NAME as usize,
            "dumping stops at the cap"
        );
        assert_eq!(session.debug_capped_names(), ["parts"]);
        session.flush_debug_dumps();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_queue_coalesces_and_drops_loudly() {
        let frame = || GrayImage::blank(4, 4, 9);
        let mut q = BundleQueue::with_cap(10);
        q.push(vec![frame(), frame(), frame(), frame()]);
        assert_eq!(q.len(), 4);
        assert_eq!(q.coalesced, 0, "the first bundle queues, nothing to merge into");
        // Second bundle coalesces onto the waiting first; the
        // queue cap (10) drops the overflow of the third.
        q.push(vec![frame(), frame(), frame(), frame()]);
        assert_eq!(q.coalesced, 1);
        assert_eq!(q.len(), 8);
        q.push(vec![frame(), frame(), frame(), frame()]);
        assert_eq!(q.len(), 10, "the queue is frame-capped");
        assert_eq!(q.dropped_frames, 2);
        assert!(q.cap_tripped, "the drop is recorded, not silent");
        // Jobs drain in bounded chunks.
        assert_eq!(q.take_job(4).len(), 4);
        assert_eq!(q.take_job(48).len(), 6);
        assert!(q.is_empty());
    }

    struct SlowSource {
        remaining: usize,
        delay: std::time::Duration,
    }

    impl FrameSource for SlowSource {
        fn next_frame(&mut self) -> Result<Option<GrayImage>, String> {
            if self.remaining == 0 {
                return Ok(None);
            }
            std::thread::sleep(self.delay);
            self.remaining -= 1;
            Ok(Some(GrayImage::blank(4, 4, 7)))
        }
    }

    #[test]
    fn run_capture_duration_cap_auto_finalizes() {
        let universes = UniverseSet::default();
        let layouts = HashMap::new();
        let relic_names = HashSet::new();
        let arcane_names = HashSet::new();
        let classifier = classifier();
        let ctx = CaptureContext {
            layouts: &layouts,
            universes: &universes,
            relic_names: &relic_names,
            arcane_names: &arcane_names,
            classifier: &classifier,
            min_confidence: 0.5,
            radius_decoder: true,
            radius_strip: true,
            radius_tail_budget: crate::ocr::RADIUS_TAIL_BUDGET,
            line_reader: false,
            strip_offsets: false,
            snap_cells: false,
            synthesis: false,
            debug_dir: None,
        };
        let stop = AtomicBool::new(false);
        let mut src = SlowSource {
            remaining: 10_000,
            delay: std::time::Duration::from_millis(10),
        };
        let started = std::time::Instant::now();
        let session = run_capture_limited(
            &ctx,
            &crate::ocr::StubEngine,
            &mut src,
            &stop,
            0,
            std::time::Duration::from_millis(55),
            |_| {},
        )
        .unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the cap ends the drain promptly (took {:?})",
            started.elapsed()
        );
        assert!(session.duration_cap_tripped(), "the trip is recorded");
        let summary = session
            .failsafe_summary()
            .expect("the trip surfaces in the finalize output");
        assert!(summary.contains("duration cap"), "summary names the cap: {summary}");
    }
}
