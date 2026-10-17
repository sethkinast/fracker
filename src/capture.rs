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
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;

use crate::ocr::{self, GrayImage, Layout, OcrEngine, StockState, Universe};

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
        let up = text.to_ascii_uppercase();
        let score = |kws: &[String]| {
            kws.iter()
                .filter(|k| up.contains(&k.to_ascii_uppercase()))
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
        let text = engine.read_text(frame, self.header, self.header_psm, self.header_upscale);
        self.classify_text(&text)
    }
}

/// The static inputs a capture run needs (loaded once from `data/`). Built
/// by [`CaptureData::load`]; the resident reuses it for every capture.
pub struct CaptureData {
    pub layouts: HashMap<String, Layout>,
    pub classifier: ScreenClassifier,
    pub universe: Universe,
    pub relic_names: HashSet<String>,
    pub arcane_names: HashSet<String>,
}

impl CaptureData {
    /// Load the layouts + classifier + item universe from a `data/`
    /// directory. The universe is the same scrubbed set the one-shot
    /// `--ocr-ingest` path builds (roster + relic names + the arcane
    /// universe), so live capture and the debug path agree.
    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let layouts = Layout::load_file(&data_dir.join("capture/layouts.toml"))?;
        let classifier = ScreenClassifier::load(&data_dir.join("capture/screens.toml"))?;
        let (universe, relic_names, arcane_names) = load_universe(data_dir)?;
        Ok(Self {
            layouts,
            classifier,
            universe,
            relic_names,
            arcane_names,
        })
    }

    /// Borrow the per-frame capture context.
    pub fn context(&self) -> CaptureContext<'_> {
        CaptureContext {
            layouts: &self.layouts,
            universe: &self.universe,
            relic_names: &self.relic_names,
            arcane_names: &self.arcane_names,
            classifier: &self.classifier,
            min_confidence: 0.5,
        }
    }
}

/// Build the OCR item universe exactly as the one-shot `--ocr-ingest`
/// path does (roster + relic names + the arcane universe, scrubbed). Kept
/// as a free function so both the live capture and the debug ingest share
/// one definition — they can never drift.
pub fn load_universe(
    data_dir: &Path,
) -> Result<(Universe, HashSet<String>, HashSet<String>), String> {
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

    let mut names: Vec<String> = Vec::new();
    for item in &roster.items {
        names.push(item.name.clone());
        for p in &item.parts {
            names.push(p.name.clone());
        }
    }
    for name in relics.relics.keys() {
        names.push(name.clone());
    }
    names.extend(arcanes.names.iter().cloned());
    let mut names = scrub.scrub_names(&names);
    names.sort();
    names.dedup();

    Ok((
        Universe::from_names(&names),
        relics.relics.keys().cloned().collect(),
        arcanes.name_set(),
    ))
}

/// Per-frame, borrow-only inputs to the pipeline.
pub struct CaptureContext<'a> {
    pub layouts: &'a HashMap<String, Layout>,
    pub universe: &'a Universe,
    pub relic_names: &'a HashSet<String>,
    pub arcane_names: &'a HashSet<String>,
    pub classifier: &'a ScreenClassifier,
    pub min_confidence: f32,
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

/// The accumulating capture state. Folding a frame is a UNION over stack
/// identity: the same stack seen in ten frames counts once (the max
/// observed count), never ten times — this is what makes scroll capture
/// dedupe work (see [`StockState::merge_from`]).
#[derive(Debug, Default)]
pub struct CaptureSession {
    captured: StockState,
    saw_relics: bool,
    saw_parts: bool,
    saw_arcanes: bool,
    last_sig: Option<u64>,
    last_class: Option<ScreenClass>,
    idle: u64,
    frames_seen: u64,
    frames_read: u64,
    frames_skipped: u64,
}

impl CaptureSession {
    pub fn new() -> Self {
        Self::default()
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
        let sig = frame_signature(frame);
        if self.last_sig == Some(sig) {
            // Unchanged screen: same class as last time.
            self.frames_skipped += 1;
            if matches!(self.last_class, Some(c) if c != ScreenClass::Other) {
                self.idle = 0;
            } else {
                self.idle += 1;
            }
            return;
        }
        self.last_sig = Some(sig);

        let class = ctx.classifier.classify(engine, frame);
        self.last_class = Some(class);
        let Some(layout) = class.layout_key().and_then(|k| ctx.layouts.get(k)) else {
            // No screen recognised — pause, do not read the wrong grid.
            self.frames_skipped += 1;
            self.idle += 1;
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

        let results = read_frame(layout, frame, engine, ctx);
        let frame_stock = StockState::from_results(&results, ctx.relic_names, ctx.arcane_names);
        self.captured.merge_from(&frame_stock);
    }

    /// Finalise into an existing stock file: REPLACE only the buckets that
    /// saw classified frames. A bucket with zero classified frames is left
    /// untouched, never zeroed (partial captures are safe).
    pub fn finalize_into(&self, existing: &mut StockState) {
        if self.saw_relics {
            existing.relics = self.captured.relics.clone();
        }
        if self.saw_arcanes {
            existing.arcanes = self.captured.arcanes.clone();
        }
        if self.saw_parts {
            existing.parts = self.captured.parts.clone();
        }
    }
}

/// Read every cell of one classified frame into validated results. Mirrors
/// [`ocr::ingest_folder`]'s per-page body: a partial cell at the viewport
/// edge is never read (never guessed).
fn read_frame(
    layout: &Layout,
    frame: &GrayImage,
    engine: &dyn OcrEngine,
    ctx: &CaptureContext<'_>,
) -> Vec<ocr::CellResult> {
    let mut results = Vec::new();
    for cell in layout.split(frame) {
        let index = results.len();
        match cell {
            Some(c) => {
                let read = engine.ocr_cell_in(&c, layout);
                results.push(ocr::validate_read(
                    &read,
                    ctx.universe,
                    ctx.min_confidence,
                    layout.min_score,
                    index,
                ));
            }
            None => results.push(ocr::validate_read(
                &ocr::OcrRead::empty(),
                ctx.universe,
                ctx.min_confidence,
                layout.min_score,
                index,
            )),
        }
    }
    results
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
    mut on_progress: impl FnMut(&CaptureSession),
) -> Result<CaptureSession, String> {
    let mut session = CaptureSession::new();
    while !stop.load(Ordering::Relaxed) {
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
}
