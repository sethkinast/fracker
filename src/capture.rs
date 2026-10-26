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

use crate::ocr::{self, GrayImage, Layout, OcrEngine, RefinementTier, StockState, Universe};

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
        }
    }

    pub fn accepted_total(&self) -> u64 {
        self.accepted_total
    }

    pub fn cap_tripped(&self) -> bool {
        self.cap_tripped
    }

    /// The screen changed away from Parts: the next parts frame
    /// is a new position wherever it lands. (The session total
    /// and the cap are session-scoped and persist.)
    pub fn reset_position(&mut self) {
        self.anchor = None;
        self.run = 0;
        self.progress = 0.0;
    }

    /// A bundle drained: progress restarts.
    pub fn note_flush(&mut self) {
        self.progress = 0.0;
    }

    /// Offer one parts-classified frame.
    pub fn offer(&mut self, frame: &GrayImage, layout: &Layout) -> IntakeVerdict {
        if self.accepted_total >= self.session_cap {
            self.cap_tripped = true;
            return IntakeVerdict::DropSessionCap;
        }
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
        self.anchor = Some(print);
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

    /// Parts frames the intake accepted this session.
    pub fn parts_frames_accepted(&self) -> u64 {
        self.parts_intake.accepted_total()
    }

    /// Intake duplicates dropped past the per-position cap.
    pub fn parts_dupes_dropped(&self) -> u64 {
        self.parts_dupes_dropped
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
            match self.parts_intake.offer(frame, layout) {
                IntakeVerdict::Accept { flush } => {
                    // A boundary verdict closes the pending
                    // bundle before this frame is pushed — the
                    // frame opens the next bundle (positions are
                    // atomic; see PartsIntake::offer).
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
            }
            return;
        }
        if !self.parts_pending.is_empty() {
            self.drain_bundle(ctx, engine);
        }
        self.parts_intake.reset_position();

        let results = read_frame(layout, frame, engine, ctx, ctx.universes.for_class(class));
        let frame_stock = StockState::from_results(&results, ctx.relic_names, ctx.arcane_names);
        self.observe_counts(&frame_stock);
        self.captured = resolve_voted_stock(&self.part_votes, &self.relic_votes, &self.arcane_votes);
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
    }

    /// Fold one stitch report into the session (pass 33 merge,
    /// pass 36 split out so background bundles merge identically
    /// whenever they land). Each held slot contributes one count
    /// vote per read instance — the stitched equivalent of the
    /// per-frame observations the vote was designed for (a ×1
    /// instance, its ring-gated read absent, votes 1).
    fn merge_report(&mut self, report: StitchReport) {
        for slot in &report.slots {
            // Coverage is the authoritative verdict: a contested
            // slot's stored result is the canonical read kept for
            // diagnostics, and it must not vote.
            if slot.coverage != SlotCoverage::Held {
                continue;
            }
            let Some(result) = &slot.result else { continue };
            let Some(name) = &result.matched else { continue };
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
        self.captured =
            resolve_voted_stock(&self.part_votes, &self.relic_votes, &self.arcane_votes);
        self.stitch_reports.push(report);
    }

    /// Every stitch report this session produced (pass 33).
    pub fn stitch_reports(&self) -> &[StitchReport] {
        &self.stitch_reports
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
            }
        }
        Some(format!(
            "stitch coverage: {held} held, {seen} seen-not-held, {never} never-fully-visible, {flagged} instance-disagreement flagged, {implied} glyph-implied built-form holds ({} stitch report(s))",
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
fn resolve_voted_stock(
    part_votes: &HashMap<String, HashMap<u32, u32>>,
    relic_votes: &HashMap<(String, RefinementTier), HashMap<u32, u32>>,
    arcane_votes: &HashMap<(String, u8), HashMap<u32, u32>>,
) -> StockState {
    let mut stock = StockState::default();
    for (name, votes) in part_votes {
        stock.parts.insert(name.clone(), voted_count(votes));
    }
    for ((name, tier), votes) in relic_votes {
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
) -> Vec<ocr::CellResult> {
    // One batched call for the whole page (addendum #81): the
    // tesseract engine caches by crop hash and batches cache misses
    // into a handful of processes, instead of 2–3 spawns per cell.
    let cells = layout.split(frame);
    read_cells(&cells, layout, engine, &ctx.read_options(), universe)
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
}

/// One stitched parts session: the plan plus every slot's outcome.
#[derive(Debug, Clone)]
pub struct StitchReport {
    pub plan: crate::stitch::StitchPlan,
    pub slots: Vec<SlotReport>,
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
                    });
                }
            }
        }
    }
    StitchReport {
        plan,
        slots: slots_out,
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
                for by in 0..3u32 {
                    for bx in 0..5u32 {
                        let mut st = seed
                            ^ ((r as u64) << 24)
                            ^ ((c as u64) << 16)
                            ^ ((by as u64) << 8)
                            ^ bx as u64
                            ^ 0x9E37_79B9;
                        let level = 40.0 + (xorshift(&mut st) % 180) as f32;
                        let sx0 = x0 + bx * (x1 - x0) / 5;
                        let sx1 = x0 + (bx + 1) * (x1 - x0) / 5;
                        let sy0 = y0 + by * (y1 - y0) / 3;
                        let sy1 = y0 + (by + 1) * (y1 - y0) / 3;
                        for y in sy0..sy1.min(h) {
                            for x in sx0..sx1.min(w) {
                                let v = (level * gain).clamp(0.0, 255.0) as u8;
                                img.set(x, y, v);
                            }
                        }
                    }
                }
            }
        }
        img
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
        let (mut accepted, mut dropped) = (0usize, 0usize);
        for i in 0..16 {
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
            match intake.offer(&frame, &layout) {
                IntakeVerdict::Accept { .. } => accepted += 1,
                IntakeVerdict::DropDuplicate => dropped += 1,
                IntakeVerdict::DropSessionCap => panic!("session cap must not trip here"),
            }
        }
        assert_eq!(accepted, VIEWPORT_ACCEPT_CAP, "static viewport collapses to the cap");
        assert_eq!(dropped, 16 - VIEWPORT_ACCEPT_CAP);
        assert_eq!(intake.accepted_total() as usize, VIEWPORT_ACCEPT_CAP);
        // A different page is a new position, accepted at once.
        let other = mosaic_frame(&layout, 99, 1.0);
        assert!(matches!(
            intake.offer(&other, &layout),
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
        // 10 rows: the last window (step 4) crops y 200..500.
        // (Boundary semantics fire the flush on the first frame
        // of the position AFTER the crossing, one offer later
        // than the pre-36 verdict timing this fixture was sized
        // for — the crop at step 4 must exist.)
        let pano_rows = 10u32;
        let pano_h = pano_rows * 50;
        let mut pano = GrayImage::blank(460, pano_h, 8);
        for r in 0..pano_rows {
            // Paint row r of the panorama with the mosaic frame
            // seeded by r: rows are mutually distinct content.
            let f = mosaic_frame(&layout, 1000 + r as u64, 1.0);
            for y in 0..50u32 {
                for x in 0..460u32 {
                    let v = f.get(x, 60 + y);
                    pano.set(x, r * 50 + y, v);
                }
            }
        }
        let mut intake = PartsIntake::new();
        let mut flush_at = None;
        for k in 0..5u32 {
            let frame = pano.crop(0, k * 50, 460, 300);
            match intake.offer(&frame, &layout) {
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
        let (mut accepted, mut capped) = (0usize, 0usize);
        for i in 0..9u64 {
            // Distinct position per offer (distinct mosaic seed).
            let frame = mosaic_frame(&layout, 500 + i * 17, 1.0);
            match intake.offer(&frame, &layout) {
                IntakeVerdict::Accept { .. } => accepted += 1,
                IntakeVerdict::DropSessionCap => capped += 1,
                IntakeVerdict::DropDuplicate => panic!("distinct positions never duplicate-drop"),
            }
        }
        assert_eq!(accepted, 5);
        assert_eq!(capped, 4);
        assert!(intake.cap_tripped(), "the trip is recorded, not silent");
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
