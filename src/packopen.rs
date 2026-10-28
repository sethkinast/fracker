//! Pack-open detection (fifty-third pass, Seth's criteria
//! of 2026-10-28; loop stage 1's inflow).
//!
//! Granted relics from opened packs are named nowhere in
//! EE.log (content-blind, settled 2026-10-26 and reconfirmed
//! on Seth's live snippet): a pack reaches stock today only
//! via a later manual refinement capture. But the purchase
//! dialog IS named (`ThemedDetailedPurchaseDialog.lua:
//! PopulateInfo->…/BoosterPacks/…` — see `elog`), and each
//! reveal screen ("PACK n OF 3") prints the three granted
//! relic NAMES as captions under large era icons. So: arm
//! on the dialog line, grab one portal frame per second,
//! classify each frame structurally as a pack screen, OCR
//! the three captions against the relic universe under the
//! pass-46 admission discipline, and — only when ALL THREE
//! captions prove — credit +1 Intact each. Anything less
//! is a miss, never a guess (the never-fabricate rule) and
//! never a partial credit (Seth: atomic per screen).
//!
//! The chime is the human half of the loop (Seth): a proven
//! screen appends a `pack_open` event whose beacon plays
//! the pass-51 chime on the page, so he sits on the screen
//! until he hears it. No chime = not captured.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ocr::{GrayImage, OcrEngine, RefinementTier, StockState, Universe, MATCH_MARGIN};

// ---------------------------------------------------------------------------
// Session timing (Seth's criteria)
// ---------------------------------------------------------------------------

/// Grab cadence while armed: one portal frame per second.
pub const GRAB_INTERVAL: Duration = Duration::from_secs(1);
/// Disarm after this long with no pack-classifying frame:
/// buying happens as a unit (pick N packs, click buy, the
/// unpack screens follow each Yes at sub-second gaps), so
/// 15 s of nothing means the session is over.
pub const NO_SCREEN_DISARM: Duration = Duration::from_secs(15);
/// Hard cap on one armed session, whichever ends first.
pub const TOTAL_ARM_CAP: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Screen geometry, measured on Seth's fixtures
// (fixtures/pack-open/, 2560x1440; PACK_OPEN_DETECTION_QUEUE.md)
// ---------------------------------------------------------------------------

/// Card column left edges, fractions of frame width
/// (867 / 1155 / 1442 px at 2560).
const COL_LEFTS: [f32; 3] = [0.3387, 0.4512, 0.5637];
/// Card width, fraction of frame width (256 px).
const COL_W: f32 = 0.1000;
/// Card top, fraction of frame height (372 px at 1440) —
/// recorded for the geometry of record; the classifier
/// keys on the zones below.
#[allow(dead_code)]
const CARD_TOP: f32 = 0.2583;

/// The caption band of card `slot`, as a fraction-of-image
/// region for [`OcrEngine::read_text`]. Measured: the
/// caption glyphs sit at y 0.420–0.429 of H (core rows
/// 605–617 at 1440) under the era icon, with a dim rule at
/// ~0.439. (The brief's nominal 0.437–0.458 band was
/// calibrated against the fixtures during this pass and
/// holds only the rule and the drop table's diamond — no
/// caption glyphs; the band here is the measured one.)
pub fn caption_region(fw: u32, fh: u32, slot: usize) -> [f32; 4] {
    let _ = (fw, fh);
    [
        COL_LEFTS[slot],
        0.412,
        COL_LEFTS[slot] + COL_W,
        0.440,
    ]
}

// ---------------------------------------------------------------------------
// Screen classification (structural; no OCR)
// ---------------------------------------------------------------------------

/// Bright level for "ink" (icon art, glyph strokes).
const BRIGHT: u8 = 100;
/// Bright level for caption glyph cores (they render a
/// touch brighter than the icon art's midtones).
const CAPTION_BRIGHT: u8 = 110;
/// An era icon fills its zone with bright art: measured
/// bright fractions 0.156–0.295 across the fixtures'
/// columns. A column below this carries no icon.
const ICON_BRIGHT_MIN: f32 = 0.10;
/// Caption glyph density in the caption rows: fixtures
/// measure 0.047–0.056 of the band's pixels at >= 110.
const CAPTION_DENSITY_MIN: f32 = 0.02;
/// The drop table under each card is a column of small
/// text lines: the fraction of rows (y 0.46–0.95) carrying
/// at least a short line's worth of bright pixels measures
/// 0.109–0.160 on the fixtures. The band's ends exclude
/// both sparse screens (a reward screen's emptiest pack
/// column: 0.035) and busy scene columns (a gameplay
/// frame's: 0.93).
const DROP_ROW_FRAC_MIN: f32 = 0.06;
const DROP_ROW_FRAC_MAX: f32 = 0.45;
/// The gaps between the pack cards show the dark vendor
/// backdrop (means <= 25 on the fixtures at every sampled
/// row); on reward and gameplay frames the same windows
/// land on card art or scene (means >= 53). Every sampled
/// gap window must stay under this mean.
const GAP_MEAN_MAX: f32 = 40.0;

/// Pixel rect of column `slot`'s zone between vertical
/// fractions `y0..y1`, inset horizontally by `inset`
/// (fraction of the column width on each side).
fn zone(frame: &GrayImage, slot: usize, inset: f32, y0: f32, y1: f32) -> (u32, u32, u32, u32) {
    let (fw, fh) = (frame.width as f32, frame.height as f32);
    let x = ((COL_LEFTS[slot] + inset * COL_W) * fw).round() as u32;
    let w = (((1.0 - 2.0 * inset) * COL_W) * fw).round() as u32;
    let y = (y0 * fh).round() as u32;
    let h = ((y1 - y0) * fh).round() as u32;
    (x.min(frame.width), y.min(frame.height), w, h)
}

/// Bright-pixel fraction of a rect (sampled every 2 px —
/// a ratio, not a count).
fn bright_fraction(frame: &GrayImage, rect: (u32, u32, u32, u32), level: u8) -> f32 {
    let (x, y, w, h) = rect;
    let (mut bright, mut total) = (0u32, 0u32);
    let mut py = y;
    while py < (y + h).min(frame.height) {
        let mut px = x;
        while px < (x + w).min(frame.width) {
            total += 1;
            if frame.get(px, py) >= level {
                bright += 1;
            }
            px += 2;
        }
        py += 2;
    }
    if total == 0 {
        0.0
    } else {
        bright as f32 / total as f32
    }
}

/// One column's share of the structural conjunction.
fn column_present(frame: &GrayImage, slot: usize) -> bool {
    // Era icon: bright art in the icon zone.
    let icon = zone(frame, slot, 0.15, 0.30, 0.40);
    if bright_fraction(frame, icon, BRIGHT) < ICON_BRIGHT_MIN {
        return false;
    }
    // Caption: real glyph density in the caption rows.
    let cap = zone(frame, slot, 0.0, 0.415, 0.435);
    if bright_fraction(frame, cap, CAPTION_BRIGHT) < CAPTION_DENSITY_MIN {
        return false;
    }
    // Drop table: a text column's worth of populated rows
    // below the card, but not a scene's worth.
    let (x, _, w, _) = zone(frame, slot, 0.0, 0.0, 0.0);
    let y0 = (0.46 * frame.height as f32).round() as u32;
    let y1 = (0.95 * frame.height as f32).round() as u32;
    let min_line = ((15.0 * frame.width as f32 / 2560.0).round() as u32).max(4);
    let (mut rows, mut populated) = (0u32, 0u32);
    for py in y0..y1.min(frame.height) {
        rows += 1;
        let mut bright = 0u32;
        for px in x..(x + w).min(frame.width) {
            if frame.get(px, py) >= BRIGHT {
                bright += 1;
            }
        }
        if bright >= min_line {
            populated += 1;
        }
    }
    if rows == 0 {
        return false;
    }
    let frac = populated as f32 / rows as f32;
    if !(DROP_ROW_FRAC_MIN..=DROP_ROW_FRAC_MAX).contains(&frac) {
        return false;
    }
    true
}

/// The inter-card gaps must show the dark vendor backdrop
/// at every sampled row.
fn gaps_dark(frame: &GrayImage) -> bool {
    for &gx in &[0.4450f32, 0.5575] {
        for &gy in &[0.35f32, 0.38, 0.42] {
            let w = (0.008 * frame.width as f32).round() as u32;
            let x0 = ((gx * frame.width as f32).round() as u32).saturating_sub(w / 2);
            let y = (gy * frame.height as f32).round() as u32;
            let (mut sum, mut n) = (0u64, 0u64);
            for px in x0..(x0 + w).min(frame.width) {
                if y < frame.height {
                    sum += frame.get(px, y) as u64;
                    n += 1;
                }
            }
            if n == 0 || (sum as f32 / n as f32) >= GAP_MEAN_MAX {
                return false;
            }
        }
    }
    true
}

/// Classification gate: the frame positively shows a pack
/// reveal screen — all three measured card columns carry
/// an era icon, a caption, and a drop-table column, and
/// the gaps between the cards are the dark backdrop.
/// Reward screens fail it (their cards sit on a different
/// pitch, so the pack columns straddle card art and the
/// gap windows land on panels); gameplay fails it (scene
/// in the gaps, scene density in the drop zone).
pub fn is_pack_screen(frame: &GrayImage) -> bool {
    (0..3).all(|slot| column_present(frame, slot)) && gaps_dark(frame)
}

// ---------------------------------------------------------------------------
// Caption admission + per-frame detection
// ---------------------------------------------------------------------------

/// Minimum fuzzy score for a caption read to hold its name
/// (the relic layout's floor; the reward name gate's too).
const CAPTION_MIN_SCORE: f32 = 0.80;

/// Does this caption read PROVE `universe`'s best name?
/// The pass-46 admission discipline, applied to a single
/// read: the resolver's score + margin bars, the shared
/// hold predicate, and the token witness — every token of
/// the resolved name witnessed by a DISTINCT read token,
/// in order, within one confusable edit (the caption's
/// trailing "Relic" is furniture: both the matcher and the
/// witness drop it before judging). Returns the proven
/// name; anything less is unresolved, never a guess.
fn admit_caption(read: &str, universe: &Universe) -> Option<String> {
    let (name, score, second) = universe.best_match_margin(read)?;
    if score < CAPTION_MIN_SCORE || (score - second) < MATCH_MARGIN {
        return None;
    }
    if !universe.hold_permitted(read, name) {
        return None;
    }
    if !crate::ocr::relic_name_witnessed(read, name) {
        return None;
    }
    Some(name.to_owned())
}

/// Resolve one caption slot from its reads. Two PSM reads
/// (6 and 7), mirroring the reward name gate's arbitration:
/// a name is proven when a read holds it under the
/// discipline and no read holds a DIFFERENT name; two
/// holds that disagree are an active contradiction and
/// refuse. Returns the proven name and a representative
/// raw read (the longest, for diagnostics).
fn resolve_caption(
    engine: &dyn OcrEngine,
    frame: &GrayImage,
    slot: usize,
    relics: &Universe,
) -> (Option<String>, String) {
    let region = caption_region(frame.width, frame.height, slot);
    let mut held: Vec<String> = Vec::new();
    let mut raw = String::new();
    for psm in [6, 7] {
        let read = engine.read_text(frame, region, psm, 4);
        if read.trim().len() > raw.trim().len() {
            raw = read.clone();
        }
        if let Some(name) = admit_caption(&read, relics) {
            if !held.iter().any(|h| h == &name) {
                held.push(name);
            }
        }
    }
    if held.len() == 1 {
        (Some(held.pop().unwrap()), raw)
    } else {
        (None, raw)
    }
}

/// One pack screen's per-slot verdicts.
#[derive(Debug, Clone, PartialEq)]
pub struct PackRead {
    /// Per-slot proven relic name (`None` = unproven).
    pub resolved: [Option<String>; 3],
    /// Per-slot representative raw caption read (diagnostics).
    pub raw: [String; 3],
}

impl PackRead {
    /// All three captions proven: the screen's trio, in
    /// display order (a multiset — duplicates kept).
    pub fn trio(&self) -> Option<[String; 3]> {
        let [a, b, c] = &self.resolved;
        match (a, b, c) {
            (Some(a), Some(b), Some(c)) => Some([a.clone(), b.clone(), c.clone()]),
            _ => None,
        }
    }
}

/// Full detection on one grabbed frame: classify, then
/// resolve the three captions. `None` when the frame is
/// not a pack screen at all.
pub fn detect_pack(engine: &dyn OcrEngine, frame: &GrayImage, relics: &Universe) -> Option<PackRead> {
    if !is_pack_screen(frame) {
        return None;
    }
    let mut resolved: [Option<String>; 3] = [None, None, None];
    let mut raw: [String; 3] = [String::new(), String::new(), String::new()];
    for slot in 0..3 {
        let (name, read) = resolve_caption(engine, frame, slot, relics);
        resolved[slot] = name;
        raw[slot] = read;
    }
    Some(PackRead { resolved, raw })
}

// ---------------------------------------------------------------------------
// Stock credit (Intact relics)
// ---------------------------------------------------------------------------

/// Serialises pack credits against each other (the reward
/// credit's gate is its own; whole-file read-modify-write
/// races between the two are the same class capture
/// finalize already accepts).
static CREDIT_GATE: Mutex<()> = Mutex::new(());

/// Credit +1 Intact of each name in `names` into the relic
/// bucket of the stock file at `stock_path` — exactly the
/// key form a capture-produced Intact relic takes
/// (`relics[name].tiers[Intact]`), so planner counts are
/// identical. A name twice in the list credits +2
/// (multiset). Durability as the reward credit's: lock,
/// read-modify-write, tmp+rename via
/// [`StockState::write_to`], refuse on an unparseable
/// stock file (never clobber); a missing file starts from
/// empty stock. Returns the new Intact count after each
/// credited increment, parallel to `names`.
pub fn credit_relics(stock_path: &Path, names: &[String]) -> Result<Vec<u32>, String> {
    let _gate = CREDIT_GATE.lock().unwrap_or_else(|p| p.into_inner());
    let mut stock = match std::fs::read_to_string(stock_path) {
        Ok(text) => StockState::from_json(&text)
            .map_err(|e| format!("stock unparseable, refusing to credit: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => StockState::default(),
        Err(e) => return Err(format!("stock unreadable, refusing to credit: {e}")),
    };
    let mut counts = Vec::with_capacity(names.len());
    for name in names {
        let stacks = stock.relics.entry(name.clone()).or_default();
        let entry = stacks.tiers.entry(RefinementTier::Intact).or_insert(0);
        *entry = entry.saturating_add(1);
        counts.push(*entry);
    }
    stock
        .write_to(stock_path)
        .map_err(|e| format!("stock write failed: {e}"))?;
    Ok(counts)
}

/// Save a pack screen's frame under `capture-debug/packs/`
/// for audit (proven and unproven alike — the unproven
/// frames are the evidence when a miss is questioned).
/// Returns the path on success.
pub fn save_pack_frame(frame: &GrayImage, debug_dir: &Path, stamp: &str) -> Result<PathBuf, String> {
    let dir = debug_dir.join("packs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("audit dir: {e}"))?;
    let path = dir.join(format!("pack-{stamp}.png"));
    frame.write_png(&path)?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Session state machine
// ---------------------------------------------------------------------------

/// Why an armed session ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisarmReason {
    /// 15 continuous seconds with no pack-classifying frame.
    NoScreen,
    /// 60 s total since arm.
    TotalCap,
}

impl DisarmReason {
    pub fn label(&self) -> &'static str {
        match self {
            DisarmReason::NoScreen => "15s with no pack screen",
            DisarmReason::TotalCap => "60s arm cap reached",
        }
    }
}

/// A screen-instance that has ended (an intervening
/// non-pack frame, a conflicting new screen, or disarm).
#[derive(Debug, Clone, PartialEq)]
pub struct EndedScreen {
    pub resolved: [Option<String>; 3],
    pub proven: bool,
}

impl EndedScreen {
    /// The `pack_miss` reason naming the unresolved slots
    /// (1-based, display order): "unresolved slot 2",
    /// "unresolved slots 1 and 3", …
    pub fn unresolved_reason(&self) -> String {
        unresolved_reason(&self.resolved)
    }
}

/// The `pack_miss` reason for a resolution state.
pub fn unresolved_reason(resolved: &[Option<String>; 3]) -> String {
    let slots: Vec<String> = resolved
        .iter()
        .enumerate()
        .filter(|(_, r)| r.is_none())
        .map(|(i, _)| (i + 1).to_string())
        .collect();
    match slots.len() {
        0 => "unresolved".to_owned(),
        1 => format!("unresolved slot {}", slots[0]),
        2 => format!("unresolved slots {} and {}", slots[0], slots[1]),
        _ => format!(
            "unresolved slots {}, {} and {}",
            slots[0], slots[1], slots[2]
        ),
    }
}

/// What one caption-resolution step did to the current
/// screen-instance.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolutionStep {
    /// Stored; the instance is not (newly) creditable —
    /// either still partial, or already proven and this
    /// read agrees with it.
    Pending,
    /// The instance just became fully proven: credit this
    /// trio (once — the caller then calls
    /// [`PackWatch::mark_proven`]).
    ReadyToCredit([String; 3]),
    /// The read conflicts with the stored instance (a slot
    /// proven both ways with different names): the stored
    /// instance ended — the returned [`EndedScreen`] — and
    /// the new read starts a fresh instance (whose own
    /// creditable trio, when the read alone proves one, is
    /// in `ready`).
    Replaced {
        ended: EndedScreen,
        ready: Option<[String; 3]>,
    },
}

#[derive(Debug, Clone, Default)]
struct ScreenInstance {
    resolved: [Option<String>; 3],
    proven: bool,
}

/// The armed pack watch: arm/disarm timing plus the
/// current screen-instance's exactly-once state. The
/// resident task drives it once per grab; the machine
/// itself is pure (instants in, decisions out) so the
/// Seth-pinned behaviours are unit-testable without
/// frames: cadence is the caller's, disarm at 15 s
/// no-screen and at the 60 s cap, the same screen credits
/// once, an identical trio after an intervening absence
/// (or after a conflicting screen) credits again.
#[derive(Debug, Default)]
pub struct PackWatch {
    armed_at: Option<Instant>,
    last_screen_at: Option<Instant>,
    current: Option<ScreenInstance>,
}

impl PackWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm (or re-arm) the watch on a purchase-dialog line.
    pub fn arm(&mut self, now: Instant) {
        self.armed_at = Some(now);
        self.last_screen_at = None;
        self.current = None;
    }

    pub fn disarm(&mut self) {
        self.armed_at = None;
        self.last_screen_at = None;
        self.current = None;
    }

    pub fn is_armed(&self) -> bool {
        self.armed_at.is_some()
    }

    /// The disarm that is due at `now`, if any — the
    /// 60 s total cap, or 15 continuous seconds since the
    /// arm / the last pack-classifying frame, whichever
    /// the clock reaches first.
    pub fn disarm_due(&self, now: Instant) -> Option<DisarmReason> {
        let armed_at = self.armed_at?;
        if now.duration_since(armed_at) >= TOTAL_ARM_CAP {
            return Some(DisarmReason::TotalCap);
        }
        let last = self.last_screen_at.unwrap_or(armed_at);
        if now.duration_since(last) >= NO_SCREEN_DISARM {
            return Some(DisarmReason::NoScreen);
        }
        None
    }

    /// Record one grab's classification. A pack frame
    /// starts (or continues) the current screen-instance
    /// and refreshes the no-screen clock; a non-pack frame
    /// ends the instance — the ended screen is returned
    /// exactly once, so the caller can log its miss when
    /// it left unproven.
    pub fn note_frame(&mut self, now: Instant, is_pack: bool) -> Option<EndedScreen> {
        if is_pack {
            self.last_screen_at = Some(now);
            if self.current.is_none() {
                self.current = Some(ScreenInstance::default());
            }
            None
        } else {
            self.current.take().map(|c| EndedScreen {
                resolved: c.resolved,
                proven: c.proven,
            })
        }
    }

    /// Fold one pack frame's caption verdicts into the
    /// current instance. Slots merge (a proven slot stays
    /// proven) unless a slot is proven both ways with
    /// different names — a conflict means the screen
    /// CHANGED under us (the next pack's reveal replaced
    /// this one between grabs, with no absent frame in
    /// between): the old instance ends and the new read
    /// starts a fresh one.
    pub fn note_resolution(&mut self, resolved: [Option<String>; 3]) -> ResolutionStep {
        if self.current.is_none() {
            self.current = Some(ScreenInstance::default());
        }
        let cur = self.current.as_mut().unwrap();
        let conflict = (0..3).any(|i| match (&cur.resolved[i], &resolved[i]) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        });
        if conflict {
            let ended = EndedScreen {
                resolved: cur.resolved.clone(),
                proven: cur.proven,
            };
            let ready = match (&resolved[0], &resolved[1], &resolved[2]) {
                (Some(a), Some(b), Some(c)) => {
                    Some([a.clone(), b.clone(), c.clone()])
                }
                _ => None,
            };
            self.current = Some(ScreenInstance {
                resolved,
                proven: false,
            });
            return ResolutionStep::Replaced { ended, ready };
        }
        for i in 0..3 {
            if resolved[i].is_some() {
                cur.resolved[i] = resolved[i].clone();
            }
        }
        if cur.proven {
            return ResolutionStep::Pending;
        }
        match (&cur.resolved[0], &cur.resolved[1], &cur.resolved[2]) {
            (Some(a), Some(b), Some(c)) => {
                ResolutionStep::ReadyToCredit([a.clone(), b.clone(), c.clone()])
            }
            _ => ResolutionStep::Pending,
        }
    }

    /// The current instance's trio was credited (call on
    /// [`ResolutionStep::ReadyToCredit`]): later reads of
    /// the same screen agree-and-do-nothing.
    pub fn mark_proven(&mut self) {
        if let Some(cur) = self.current.as_mut() {
            cur.proven = true;
        }
    }

    /// End the current instance at disarm time.
    pub fn finish_instance(&mut self) -> Option<EndedScreen> {
        self.current.take().map(|c| EndedScreen {
            resolved: c.resolved,
            proven: c.proven,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relic_universe() -> Universe {
        let text = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data/cache/relictables.json"),
        )
        .unwrap();
        let tables = crate::model::RelicTables::from_json(&text).unwrap();
        let mut names: Vec<String> = tables.relics.keys().cloned().collect();
        names.sort();
        Universe::from_names(&names)
    }

    fn fixture(rel: &str) -> Option<GrayImage> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
        let bytes = std::fs::read(&path).ok()?;
        GrayImage::from_bytes(&bytes).ok()
    }

    fn real_engine() -> Option<crate::ocr::TesseractCliEngine> {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let layout = crate::ocr::Layout::load_file(&base.join("data/capture/layouts.toml"))
            .ok()?
            .get("relic")?
            .clone();
        crate::ocr::TesseractCliEngine::probe(layout)
    }

    const GROUND_TRUTH: [[&str; 3]; 3] = [
        ["Axi P10", "Meso K9", "Lith S19"],
        ["Neo C11", "Lith V11", "Lith G14"],
        ["Axi V14", "Lith A13", "Meso V17"],
    ];

    #[test]
    fn fixture_screens_classify_and_resolve_the_ground_truth() {
        let Some(engine) = real_engine() else {
            eprintln!("skip pack fixtures: no tesseract in this environment");
            return;
        };
        let universe = relic_universe();
        for (i, truth) in GROUND_TRUTH.iter().enumerate() {
            let frame = fixture(&format!("fixtures/pack-open/pack-{}.png", i + 1))
                .unwrap_or_else(|| panic!("fixture pack-{} missing", i + 1));
            assert!(
                is_pack_screen(&frame),
                "pack-{} must classify as a pack screen",
                i + 1
            );
            let read = detect_pack(&engine, &frame, &universe)
                .unwrap_or_else(|| panic!("pack-{} must detect", i + 1));
            let trio = read.trio().unwrap_or_else(|| {
                panic!(
                    "pack-{} must prove all three captions; resolved {:?} from reads {:?}",
                    i + 1,
                    read.resolved,
                    read.raw
                )
            });
            assert_eq!(&trio, truth, "pack-{} trio", i + 1);
        }
    }

    #[test]
    fn fixture_trios_credit_exactly_those_multisets_as_intact() {
        let dir = std::env::temp_dir().join(format!(
            "fracker-pack-credit-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stock_path = dir.join("stock.json");
        // The three packs of Seth's batch, credited in order.
        for truth in GROUND_TRUTH {
            let names: Vec<String> = truth.iter().map(|s| s.to_string()).collect();
            let counts = credit_relics(&stock_path, &names).unwrap();
            assert_eq!(counts, vec![1, 1, 1]);
        }
        let stock = StockState::from_json(&std::fs::read_to_string(&stock_path).unwrap()).unwrap();
        assert_eq!(stock.relics.len(), 9);
        for truth in GROUND_TRUTH {
            for name in truth {
                let stacks = stock.relics.get(name).unwrap();
                assert_eq!(stacks.get(RefinementTier::Intact), 1, "{name}");
                assert_eq!(stacks.total(), 1, "{name} credited only as Intact");
            }
        }
        // A duplicate within one pack credits +2 (multiset).
        let dup = vec!["Lith S19".to_string(), "Lith S19".to_string(), "Axi P10".to_string()];
        let counts = credit_relics(&stock_path, &dup).unwrap();
        assert_eq!(counts, vec![2, 3, 2]);
        let stock = StockState::from_json(&std::fs::read_to_string(&stock_path).unwrap()).unwrap();
        assert_eq!(
            stock.relics.get("Lith S19").unwrap().get(RefinementTier::Intact),
            3
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_pack_frames_never_classify() {
        // A blank frame.
        assert!(!is_pack_screen(&GrayImage::blank(2560, 1440, 30)));
        // Seth's reward screens (the sibling-tree fixtures,
        // the reward tests' pattern): the 4-player screencap
        // and the 3-player burst frames.
        for rel in [
            "../fracker-frames/fixtures/reward-screen.png",
            "../fracker-frames/three-person/reward-1793159728-t0.png",
            "../fracker-frames/three-person/reward-1793159728-t14.png",
        ] {
            let Some(frame) = fixture(rel) else {
                eprintln!("skip non-pack fixture {rel}: absent");
                continue;
            };
            assert!(!is_pack_screen(&frame), "{rel} must not classify");
        }
    }

    #[test]
    fn one_unprovable_caption_yields_no_credit_and_names_its_slot() {
        let Some(engine) = real_engine() else {
            eprintln!("skip corrupted-caption gate: no tesseract");
            return;
        };
        let universe = relic_universe();
        let mut frame = fixture("fixtures/pack-open/pack-1.png").unwrap();
        // Corrupt slot 2's caption (index 1): mirror the
        // caption band's pixels. Glyph density (the
        // classifier's leg) is preserved exactly; the text
        // is unprovable — mirrored glyphs witness nothing.
        let (fw, fh) = (frame.width, frame.height);
        let region = caption_region(fw, fh, 1);
        let x0 = (region[0] * fw as f32) as u32;
        let x1 = (region[2] * fw as f32) as u32;
        let y0 = (region[1] * fh as f32) as u32;
        let y1 = (region[3] * fh as f32) as u32;
        for py in y0..y1 {
            let row: Vec<u8> = (x0..x1).map(|px| frame.get(px, py)).collect();
            for (j, px) in (x0..x1).enumerate() {
                frame.set(px, py, row[row.len() - 1 - j]);
            }
        }
        assert!(
            is_pack_screen(&frame),
            "the corrupted frame still classifies (density preserved)"
        );
        let read = detect_pack(&engine, &frame, &universe).expect("still a pack screen");
        assert_eq!(read.trio(), None, "no trio may be proven: {read:?}");
        assert!(read.resolved[0].is_some(), "slot 1 still proves");
        assert_eq!(read.resolved[1], None, "slot 2 is the unprovable one");
        assert!(read.resolved[2].is_some(), "slot 3 still proves");
        // Through the state machine: the screen ends
        // unproven, exactly once, naming slot 2 — and
        // nothing was credited (no ReadyToCredit ever).
        let mut watch = PackWatch::new();
        let t0 = Instant::now();
        watch.arm(t0);
        assert_eq!(watch.note_frame(t0, true), None);
        assert_eq!(
            watch.note_resolution(read.resolved.clone()),
            ResolutionStep::Pending
        );
        let ended = watch
            .note_frame(t0 + Duration::from_secs(2), false)
            .expect("the instance ends on absence");
        assert!(!ended.proven);
        assert_eq!(ended.unresolved_reason(), "unresolved slot 2");
    }

    // -- State machine pins --------------------------------

    fn full(names: [&str; 3]) -> [Option<String>; 3] {
        names.map(|n| Some(n.to_owned()))
    }

    #[test]
    fn same_screen_credits_once_identical_trio_after_absence_credits_again() {
        let mut watch = PackWatch::new();
        let t0 = Instant::now();
        let trio = full(["Axi P10", "Meso K9", "Lith S19"]);
        watch.arm(t0);
        assert_eq!(watch.note_frame(t0, true), None);
        assert_eq!(
            watch.note_resolution(trio.clone()),
            ResolutionStep::ReadyToCredit([
                "Axi P10".to_owned(),
                "Meso K9".to_owned(),
                "Lith S19".to_owned()
            ])
        );
        watch.mark_proven();
        // Repeated grabs of the SAME screen: never again.
        for s in 1..=3 {
            assert_eq!(
                watch.note_frame(t0 + Duration::from_secs(s), true),
                None
            );
            assert_eq!(watch.note_resolution(trio.clone()), ResolutionStep::Pending);
        }
        // The screen goes away (proven — no miss owed).
        let ended = watch
            .note_frame(t0 + Duration::from_secs(5), false)
            .unwrap();
        assert!(ended.proven);
        // A later pack shows the identical trio: a NEW
        // instance, credited again.
        assert_eq!(watch.note_frame(t0 + Duration::from_secs(6), true), None);
        assert!(matches!(
            watch.note_resolution(trio.clone()),
            ResolutionStep::ReadyToCredit(_)
        ));
    }

    #[test]
    fn disarm_at_fifteen_seconds_no_screen_and_at_the_sixty_second_cap() {
        let mut watch = PackWatch::new();
        let t0 = Instant::now();
        watch.arm(t0);
        assert_eq!(watch.disarm_due(t0 + Duration::from_secs(14)), None);
        assert_eq!(
            watch.disarm_due(t0 + Duration::from_secs(15)),
            Some(DisarmReason::NoScreen)
        );
        // A screen sighting restarts the quiet clock, but
        // the total cap still lands at 60 s.
        let mut watch = PackWatch::new();
        watch.arm(t0);
        assert_eq!(watch.note_frame(t0 + Duration::from_secs(50), true), None);
        assert_eq!(watch.disarm_due(t0 + Duration::from_secs(59)), None);
        assert_eq!(
            watch.disarm_due(t0 + Duration::from_secs(60)),
            Some(DisarmReason::TotalCap)
        );
    }

    #[test]
    fn conflicting_screen_replaces_the_instance_and_reports_the_old_one() {
        let mut watch = PackWatch::new();
        let t0 = Instant::now();
        watch.arm(t0);
        assert_eq!(watch.note_frame(t0, true), None);
        assert_eq!(
            watch.note_resolution([
                Some("Axi P10".to_owned()),
                None,
                Some("Lith S19".to_owned())
            ]),
            ResolutionStep::Pending
        );
        // The next pack's reveal lands with no absent frame
        // in between: slot 1 proves a different name.
        match watch.note_resolution(full(["Neo C11", "Lith V11", "Lith G14"])) {
            ResolutionStep::Replaced { ended, ready } => {
                assert!(!ended.proven);
                assert_eq!(ended.unresolved_reason(), "unresolved slot 2");
                assert_eq!(
                    ready,
                    Some([
                        "Neo C11".to_owned(),
                        "Lith V11".to_owned(),
                        "Lith G14".to_owned()
                    ])
                );
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn unresolved_reason_names_every_unproven_slot() {
        assert_eq!(
            unresolved_reason(&[None, Some("Meso K9".into()), None]),
            "unresolved slots 1 and 3"
        );
        assert_eq!(
            unresolved_reason(&[None, None, None]),
            "unresolved slots 1, 2 and 3"
        );
    }
}
