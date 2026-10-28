//! Reward-screen pick detection + stock credit (pass 39, loop
//! stages 4–5).
//!
//! EE.log cannot name the reward Seth PICKED on the fissure
//! reward-choice screen — it names only his own relic's roll,
//! and the other rewards not at all (QUEUE_REVIEW.md, EE-2.log
//! analysis). But the log timestamps the moment his reward is
//! granted (`Event::RelicReward`, the gets-reward line — pass
//! 49's rekeyed trigger), and on the choice
//! screen the selected card carries a light frame border. So:
//! grab a short burst of frames from the already-open stream
//! (T+8/10/12/14), classify each positively as the reward
//! screen, find the bordered card, OCR its name against the
//! roster parts universe, and credit +1 into stock.json —
//! from the LATEST burst frame that clears every gate.
//!
//! Every failure is a silent MISS (a log line, no credit),
//! never a wrong credit: the frame must classify, the border
//! winner must clear an absolute floor AND a margin over the
//! runner-up, and the name must match under the standard
//! score + margin discipline. The consumed relic is NOT
//! decremented — the screen does not show which relic
//! produced a card (established from Seth's screencap).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::ocr::{GrayImage, OcrEngine, Universe, MATCH_MARGIN};

/// Burst grab offsets from trigger detection (seconds;
/// pass 43 amendment, Seth's live evidence 2026-10-26
/// evening). Pass 39's single T+14 grab assumed the choice
/// screen always lives its full 15 s — true in
/// extraction-style missions (two of tonight's frames show
/// the countdown at 2 at grab time), false in ENDLESS
/// rotations, where the screen closes as soon as picks +
/// the continue/extract vote complete, often before T+14:
/// three of tonight's four T+14 grabs landed on non-reward
/// screens entirely (the next rotation's SELECT RELIC
/// screen; mid-wave Defense gameplay), and the classifier
/// correctly refused them — the grab was simply too late.
/// A second measurement killed the first burst draft
/// (anchored at T+8): the EE.log trigger itself can trail
/// the screen's appearance by ~10 s — tailing the log
/// live, Seth saw the "armed" line with only 5 s left on
/// the reward timer, so an armed+8 sample can land at
/// screen+18, after the close. The burst therefore
/// samples from T+0 at arming, roughly every 2 s through
/// T+14 — the stream is already open at T+0; sample it,
/// don't wait. Late-armed episodes are caught by the
/// first samples. Every frame that classifies is scored
/// under the unchanged per-frame gates, and the credit
/// comes from the LATEST frame that clears them — a
/// requirement, not a preference: the screen opens with
/// the player's OWN rolled reward pre-selected, so a
/// border is present from screen-open, before any click,
/// and a first-clearing rule would credit that default
/// even when the player then picks another card. Only
/// the latest clearing frame reflects the actual choice
/// (if the player never changes it, the default IS the
/// pick and an early clear is correct). A border cannot
/// un-happen, so an early-clearing frame still credits
/// when the screen closes early. Accepted residual
/// (Seth, confirming the design on these terms): a pick
/// switched in the final ~second, after the last
/// readable frame, credits the earlier selection.
pub const BURST_OFFSETS_SECS: [u64; 8] = [0, 2, 4, 6, 8, 10, 12, 14];

// ---------------------------------------------------------------------------
// Card geometry, measured on fixtures/reward-screen.png
// (1769x996; Seth's screencap of a 4-player reward screen).
// Fractions of the frame, so real 2560x1440 captures scale.
// Card 1's frame rect was measured directly (its light border
// is the selection marker): x 605..742, y 315..463. The row is
// centred, pitch 141 px, cards 137 px wide with ~4 px gaps.
//
// Pass 49: with fewer than four squad members the row keeps
// its card size, top, and CENTRE, so a row of n cards starts
// (SLOTS - n) * pitch / 2 to the right of ROW_LEFT (measured
// on Seth's 3-player burst, THREE_PERSON_REWARD_GEOMETRY.md:
// the production 4-grid scores the bordered card 0.00 there,
// while the same scorer at the +pitch/2 origin reads it
// 0.995). The geometry functions take the origin as a slot-
// unit offset; the 4-grid origin (0.0) path is the original
// arithmetic, unchanged.
// ---------------------------------------------------------------------------

/// Left edge of card slot 0, fraction of frame width.
const ROW_LEFT: f32 = 0.34200;
/// Slot pitch, fraction of frame width.
const CARD_PITCH: f32 = 0.07971;
/// Card width, fraction of frame width.
const CARD_W: f32 = 0.07744;
/// Card top, fraction of frame height.
const ROW_TOP: f32 = 0.31627;
/// Card height, fraction of frame height. (Top frame line at
/// y=315, bottom at y=451.5 on the fixture; an earlier taller
/// estimate put the bottom edge on the rarity bar below the
/// panel and diluted the border signal.)
const CARD_H: f32 = 0.13705;
/// Number of card slots on a full-squad screen.
pub const SLOTS: usize = 4;

/// Perimeter samples at or above this gray level count as
/// border-present. The selected frame line measures ~110–160
/// on the fixture (light grey, 1–2 px); scene content rarely
/// sustains it around a whole rectangle.
const BORDER_BRIGHT: u8 = 100;
/// Absolute floor for the winner's border continuity.
const BORDER_FLOOR: f32 = 0.55;
/// Required continuity margin over the runner-up slot.
const BORDER_MARGIN: f32 = 0.30;
/// Minimum fuzzy score for the bordered card's name read
/// (the relic layout's floor: large, clean text).
const NAME_MIN_SCORE: f32 = 0.80;

/// Row origin for a centred row of `cards` cards, as a
/// slot-unit offset from [`ROW_LEFT`]: each missing card
/// shifts the row right by half a pitch (pass 49).
fn row_origin(cards: usize) -> f32 {
    (SLOTS - cards) as f32 * 0.5
}

/// Pixel rect `(x, y, w, h)` of card `slot` in a `fw`x`fh` frame
/// on the full 4-card grid.
pub fn slot_rect(fw: u32, fh: u32, slot: usize) -> (u32, u32, u32, u32) {
    slot_rect_at(fw, fh, 0.0, slot)
}

/// [`slot_rect`] on a row whose origin is shifted `origin`
/// slot-units right of [`ROW_LEFT`] (pass 49's centred
/// fewer-card rows: 0.5 / 1.0 / 1.5 for 3 / 2 / 1 cards).
pub fn slot_rect_at(fw: u32, fh: u32, origin: f32, slot: usize) -> (u32, u32, u32, u32) {
    let x = ((ROW_LEFT + (origin + slot as f32) * CARD_PITCH) * fw as f32).round() as u32;
    let y = (ROW_TOP * fh as f32).round() as u32;
    let w = (CARD_W * fw as f32).round() as u32;
    let h = (CARD_H * fh as f32).round() as u32;
    (x, y, w, h)
}

/// The bordered card's name band as a fraction-of-image region
/// for [`OcrEngine::read_text`]: the card's lower portion,
/// where the item name renders (one or two centred lines,
/// bottom-anchored). Measured text zone on the fixture:
/// y 420..450 of the 315..451.5 card — fractions 0.70..1.0.
pub fn name_band_region(fw: u32, fh: u32, slot: usize) -> [f32; 4] {
    name_band_region_at(fw, fh, 0.0, slot)
}

/// [`name_band_region`] on a row shifted `origin` slot-units
/// right (pass 49).
pub fn name_band_region_at(fw: u32, fh: u32, origin: f32, slot: usize) -> [f32; 4] {
    let (x, y, w, h) = slot_rect_at(fw, fh, origin, slot);
    let fwf = fw as f32;
    let fhf = fh as f32;
    [
        (x as f32 + 0.02 * w as f32) / fwf,
        (y as f32 + 0.70 * h as f32) / fhf,
        (x as f32 + 0.98 * w as f32) / fwf,
        (y as f32 + h as f32) / fhf,
    ]
}

/// A perimeter side contributes to a slot's continuity only
/// when some single offset in the sampling window carries a
/// real line on that side: the side's best single-offset
/// continuity must clear this bar. Measured on the two real
/// frames (pass 43, addendum #108): true border sides bottom
/// at 0.77 (the fixture's bottom line straddles two rows:
/// 0.57 / 0.77), while scene content on unbordered sides
/// peaks at 0.42 — the per-card progress bar under an
/// unpicked card, picker-name text, panel glow; none of it
/// sustains a line. 0.60 sits between the two populations,
/// so a side with no line contributes zero instead of its
/// scattered noise (under the pass-39 aggregation that noise
/// summed to 0.14–0.23 on unbordered slots).
const SIDE_LINE_MIN: f32 = 0.60;

/// One perimeter side's contribution to a slot's continuity:
/// `(bright, total)` sample counts, where a sample position
/// is bright when any offset in `offs` reads at or above
/// [`BORDER_BRIGHT`] there — unless no single offset carries
/// a line ([`SIDE_LINE_MIN`]), in which case the side
/// contributes zero bright samples. `fixed` is the edge
/// coordinate the offsets displace (the row for horizontal
/// sides, the column for vertical ones); samples run over
/// `from..=to` along the edge.
fn border_side(
    frame: &GrayImage,
    horizontal: bool,
    fixed: i32,
    from: i32,
    to: i32,
    offs: &[i32],
) -> (u32, u32) {
    let (fw, fh) = (frame.width as i32, frame.height as i32);
    let at = |x: i32, y: i32| -> u8 {
        if x < 0 || y < 0 || x >= fw || y >= fh {
            0
        } else {
            frame.get(x as u32, y as u32)
        }
    };
    let sample = |p: i32, o: i32| -> u8 {
        if horizontal {
            at(p, fixed + o)
        } else {
            at(fixed + o, p)
        }
    };
    // The side's best single-offset continuity: does any one
    // offset carry a line along the whole edge?
    let mut best = 0.0f32;
    for &o in offs {
        let (mut b, mut n) = (0u32, 0u32);
        for p in from..=to {
            n += 1;
            if sample(p, o) >= BORDER_BRIGHT {
                b += 1;
            }
        }
        if n > 0 {
            best = best.max(b as f32 / n as f32);
        }
    }
    let (mut bright, mut total) = (0u32, 0u32);
    for p in from..=to {
        total += 1;
        if offs.iter().map(|&o| sample(p, o)).max().unwrap_or(0) >= BORDER_BRIGHT {
            bright += 1;
        }
    }
    if best >= SIDE_LINE_MIN {
        (bright, total)
    } else {
        (0, total)
    }
}

/// Border continuity per slot: for each perimeter side
/// (top/bottom/left/right), take the max over a window
/// straddling the card edge (the frame line is 1–2 px and our
/// rect can sit a pixel off), then count the fraction of
/// samples at or above [`BORDER_BRIGHT`]. The selected card's
/// frame is continuous; an unselected card has no frame at
/// all, so its "perimeter" is scene content — bright in
/// patches, never all the way around.
///
/// Pass 43 (addendum #108): the window is CONTIGUOUS in frame
/// pixels — every integer offset in [-3s, +3s], the same on
/// all four sides, where s scales the fixture width to the
/// frame's. The pass-39 sets were four measured offsets per
/// side that scaling + dedup made sparse ([-1, 0, 1, 3] /
/// [-3, -1, 0, 1] at 2560 wide): on Seth's 2026-10-26 live
/// frame the border sits at top +1 / bottom +2 / left -2 /
/// right -2 from the assumed rect, and only the top side's
/// set contained its line — the bordered card read 0.31 and
/// the pick missed. A side now also has to carry a real line
/// somewhere in the window ([`SIDE_LINE_MIN`]) before its
/// samples count, which keeps the wider window from
/// inflating unbordered slots on scene content (the live
/// frame's runner-up reads 0.00; it read 0.14–0.22 under
/// window-union alone).
pub fn border_scores(frame: &GrayImage) -> [f32; SLOTS] {
    border_scores_at(frame, 0.0, SLOTS)
        .try_into()
        .expect("the 4-grid geometry scores four slots")
}

/// Border continuity for `cards` slots on a row shifted
/// `origin` slot-units right (pass 49). Identical scoring to
/// [`border_scores`] — same window, same side-line gate —
/// only the sampled rects move.
pub fn border_scores_at(frame: &GrayImage, origin: f32, cards: usize) -> Vec<f32> {
    let (fw, fh) = (frame.width, frame.height);
    let mut out = vec![0.0f32; cards];
    let s = fw as f32 / 1769.0;
    let half = (3.0 * s).round() as i32;
    let offs: Vec<i32> = ((-half)..=half).collect();
    for slot in 0..cards {
        let (x, y, w, h) = slot_rect_at(fw, fh, origin, slot);
        let (x0, y0) = (x as i32, y as i32);
        let (x1, y1) = (x0 + w as i32 - 1, y0 + h as i32 - 1);
        let (mut bright, mut total) = (0u32, 0u32);
        for (b, t) in [
            border_side(frame, true, y0, x0, x1, &offs),
            border_side(frame, true, y1, x0, x1, &offs),
            border_side(frame, false, x0, y0, y1, &offs),
            border_side(frame, false, x1, y0, y1, &offs),
        ] {
            bright += b;
            total += t;
        }
        out[slot] = if total == 0 {
            0.0
        } else {
            bright as f32 / total as f32
        };
    }
    out
}

/// The winning bordered slot, or `None` when no slot clears
/// the floor or the winner's margin over the runner-up is too
/// small (ambiguity is a miss, per the design of record).
pub fn border_winner(scores: &[f32; SLOTS]) -> Option<usize> {
    border_winner_in(scores)
}

/// [`border_winner`] over any slot count (pass 49's
/// fewer-card geometries score 3 / 2 / 1 slots).
pub fn border_winner_in(scores: &[f32]) -> Option<usize> {
    if scores.is_empty() {
        return None;
    }
    let mut best = 0usize;
    for i in 1..scores.len() {
        if scores[i] > scores[best] {
            best = i;
        }
    }
    let mut second = 0.0f32;
    for (i, s) in scores.iter().enumerate() {
        if i != best && *s > second {
            second = *s;
        }
    }
    if scores[best] >= BORDER_FLOOR && scores[best] - second >= BORDER_MARGIN {
        Some(best)
    } else {
        None
    }
}

/// Bright-pixel floor for a card's name band, at the
/// fixture's pixel area (scaled by frame area). A present
/// card always carries its item name: measured 227–483
/// bright (>=130) text pixels per band on the fixture.
const BAND_BRIGHT_MIN: f32 = 120.0;
/// A present card's panel interior is dark and translucent
/// (fixture means 52–71); bright scene content fails this.
const INTERIOR_MAX: f32 = 130.0;
/// Fixture pixel area the bright floor was measured at.
const FIXTURE_AREA: f32 = 1769.0 * 996.0;

/// Does `slot` hold a card? Structural presence: a dark
/// panel interior plus real text in the name band.
pub fn card_present(frame: &GrayImage, slot: usize) -> bool {
    card_present_at(frame, 0.0, slot)
}

/// [`card_present`] on a row shifted `origin` slot-units
/// right (pass 49).
pub fn card_present_at(frame: &GrayImage, origin: f32, slot: usize) -> bool {
    let (x, y, w, h) = slot_rect_at(frame.width, frame.height, origin, slot);
    // Panel interior: inset on all sides, above the name band.
    let ix0 = x + (0.06 * w as f32) as u32;
    let iy0 = y + (0.06 * h as f32) as u32;
    let ix1 = (x + (0.94 * w as f32) as u32).min(frame.width);
    let iy1 = (y + (0.78 * h as f32) as u32).min(frame.height);
    let mut sum = 0u64;
    let mut n = 0u64;
    for py in iy0..iy1 {
        for px in ix0..ix1 {
            sum += frame.get(px, py) as u64;
            n += 1;
        }
    }
    if n == 0 || (sum as f32 / n as f32) > INTERIOR_MAX {
        return false;
    }
    // Name-band text: bright glyph pixels.
    let region = name_band_region_at(frame.width, frame.height, origin, slot);
    let bx0 = (region[0] * frame.width as f32) as u32;
    let by0 = (region[1] * frame.height as f32) as u32;
    let bx1 = ((region[2] * frame.width as f32) as u32).min(frame.width);
    let by1 = ((region[3] * frame.height as f32) as u32).min(frame.height);
    let mut bright = 0u32;
    for py in by0..by1 {
        for px in bx0..bx1 {
            if frame.get(px, py) >= 130 {
                bright += 1;
            }
        }
    }
    let scale = (frame.width as f32 * frame.height as f32) / FIXTURE_AREA;
    bright as f32 >= BAND_BRIGHT_MIN * scale
}

/// Classification gate: the frame must positively show the
/// reward screen's card-row structure — at least two present
/// cards in the measured centred row. (Banner OCR was
/// measured and rejected as the gate: the "VOID FISSURE"
/// glyphs are thin and letterspaced and do not read reliably
/// even at capture resolution, while the card row is
/// unmistakable.) Gameplay and extraction frames do not
/// carry the row; the border and name gates downstream
/// protect the credit regardless.
pub fn is_reward_screen(frame: &GrayImage) -> bool {
    is_reward_screen_at(frame, 0.0, SLOTS)
}

/// [`is_reward_screen`] for a centred row of `cards` cards
/// (pass 49): at least two present cards — or the lone card
/// itself when `cards` is 1 (a solo screen has exactly one
/// card; requiring two would exclude it by construction).
pub fn is_reward_screen_at(frame: &GrayImage, origin: f32, cards: usize) -> bool {
    let need = if cards <= 1 { 1 } else { 2 };
    (0..cards)
        .filter(|&s| card_present_at(frame, origin, s))
        .count()
        >= need
}

/// Why a grab produced no credit. Every variant is a silent
/// miss: logged, never surfaced as an error, never a credit.
#[derive(Debug, Clone, PartialEq)]
pub enum Miss {
    /// The frame did not classify as the reward screen.
    NotRewardScreen,
    /// No card border cleared the floor / margin. The scores
    /// are the 4-GRID continuities even when pass 49's
    /// alternate geometries were also tried and missed: the
    /// miss shape (and the burst logic reading these) is
    /// unchanged from the 4-card design.
    NoBorderWinner { scores: [f32; SLOTS] },
    /// The bordered card's name reads matched nothing under
    /// the standard discipline — or the two reads held
    /// different names (an active contradiction, refused per
    /// the house arbitration doctrine).
    NameNoMatch { detail: String },
}

impl Miss {
    /// One-line reason for the resident log.
    pub fn describe(&self) -> String {
        match self {
            Miss::NotRewardScreen => "frame is not the reward screen".to_owned(),
            Miss::NoBorderWinner { scores } => format!(
                "no clear bordered card (continuities {:.2}/{:.2}/{:.2}/{:.2})",
                scores[0], scores[1], scores[2], scores[3]
            ),
            Miss::NameNoMatch { detail } => detail.clone(),
        }
    }
}

/// Full detection on one grabbed frame: classify, find the
/// bordered card, OCR its name, match against the roster
/// parts universe. `Ok` carries the canonical part name to
/// credit.
///
/// The 4-person path runs first and exactly as before
/// (pass 49). Only when the 4-grid border gate misses are
/// the centred 3-, 2-, and 1-card geometries tried, in
/// that order, each under the identical presence, border,
/// and name gates: a squad-size row is centred, so with
/// fewer players the cards sit half-pitches right of the
/// 4-grid and the 4-grid rects straddle card edges. The
/// first geometry that yields a border winner owns the
/// frame — its name verdict (credit or refusal) is final,
/// exactly as a 4-grid winner's is. When no geometry
/// yields a winner the miss is the 4-grid's own
/// `NoBorderWinner`, scores unchanged.
///
/// One refusal is never overridden: when a geometry DOES
/// see a border-strength slot (continuity at or above
/// [`BORDER_FLOOR`]) but no unique winner, the frame is
/// ambiguous ON THAT GEOMETRY and detection stops — a
/// shifted geometry must not manufacture a unique winner
/// by subsetting an ambiguous row (on a 4-person frame
/// with two bordered cards, the 2-card geometry would see
/// exactly one of them and "resolve" an ambiguity the
/// design of record refuses). Genuine fewer-card frames
/// never trip this: their 4-grid scores are structurally
/// low (a straddled card contributes at most one gated
/// side; Seth's real trio frames measure ≤ 0.192).
pub fn detect_pick(
    engine: &dyn OcrEngine,
    frame: &GrayImage,
    parts: &Universe,
) -> Result<String, Miss> {
    if !is_reward_screen(frame) {
        return Err(Miss::NotRewardScreen);
    }
    let scores = border_scores(frame);
    if let Some(slot) = border_winner(&scores) {
        let region = name_band_region(frame.width, frame.height, slot);
        return read_bordered_name(engine, frame, parts, region);
    }
    if scores.iter().any(|s| *s >= BORDER_FLOOR) {
        return Err(Miss::NoBorderWinner { scores });
    }
    for cards in [3usize, 2, 1] {
        let origin = row_origin(cards);
        if !is_reward_screen_at(frame, origin, cards) {
            continue;
        }
        let alt_scores = border_scores_at(frame, origin, cards);
        if let Some(slot) = border_winner_in(&alt_scores) {
            let region = name_band_region_at(frame.width, frame.height, origin, slot);
            return read_bordered_name(engine, frame, parts, region);
        }
        if alt_scores.iter().any(|s| *s >= BORDER_FLOOR) {
            return Err(Miss::NoBorderWinner { scores });
        }
    }
    Err(Miss::NoBorderWinner { scores })
}

/// Read the bordered card's name band and apply the hold
/// discipline. Shared by every geometry (pass 49): the
/// region is the only geometry-dependent input.
fn read_bordered_name(
    engine: &dyn OcrEngine,
    frame: &GrayImage,
    parts: &Universe,
    region: [f32; 4],
) -> Result<String, Miss> {
    // Two reads of the bordered card's band (PSM 6 and PSM
    // 11): measured on the fixture and on synthetic screens,
    // neither mode dominates — each drops or garbles a line
    // the other reads cleanly. Arbitration mirrors the grid
    // paths: a name is credited when a read holds it under
    // the standard discipline and no read holds a DIFFERENT
    // name; two holds that disagree are an active
    // contradiction and refuse.
    let mut held: Vec<String> = Vec::new();
    let mut detail = String::new();
    for psm in [6, 11] {
        let read = engine.read_text(frame, region, psm, 4);
        match parts.best_match_margin(&read) {
            Some((name, score, second))
                if score >= NAME_MIN_SCORE && score - second >= MATCH_MARGIN =>
            {
                if !held.iter().any(|h| h == name) {
                    held.push(name.to_owned());
                }
            }
            other => {
                if !detail.is_empty() {
                    detail.push_str("; ");
                }
                detail.push_str(&match other {
                    Some((name, score, second)) => format!(
                        "psm{psm} read {read:?} matched {name} at {score:.2} (runner-up {second:.2}), below the hold discipline"
                    ),
                    None => format!("psm{psm} read {read:?} matched nothing"),
                });
            }
        }
    }
    match held.len() {
        1 => Ok(held.pop().unwrap()),
        0 => Err(Miss::NameNoMatch { detail }),
        _ => Err(Miss::NameNoMatch {
            detail: format!("reads disagree: {} vs {}", held[0], held[1]),
        }),
    }
}

// ---------------------------------------------------------------------------
// Burst selection (pass 43 amendment)
// ---------------------------------------------------------------------------

/// One burst frame's detection outcome: its grab offset and
/// the unchanged per-frame verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct BurstFrame {
    pub offset_secs: u64,
    pub outcome: Result<String, Miss>,
}

/// The burst's verdict. `Pick` credits the named part;
/// `Miss` carries the one-line reason for the resident log,
/// naming the closest attempt and the frame it came from.
#[derive(Debug, Clone, PartialEq)]
pub enum BurstVerdict {
    Pick { offset_secs: u64, name: String },
    Miss(String),
}

/// Choose the burst's outcome. The credit comes from the
/// LATEST frame that cleared every per-frame gate — the
/// screen opens with the player's own roll pre-selected
/// (a border from screen-open), so the latest clearing
/// frame is the only one certain to reflect the player's
/// actual choice rather than the default (and a border
/// cannot un-happen: an early frame that cleared still
/// credits when later frames show the screen already
/// closed).
/// Never-a-wrong-credit is untouched: selection only ever
/// picks among frames that individually cleared the
/// classifier, the border floor + margin, and the name
/// discipline.
///
/// When nothing clears, the miss names the CLOSEST
/// attempt: a frame that reached name matching outranks
/// one that only failed the border gate, which outranks
/// frames that never classified; within a tier, the frame
/// with the strongest best-side continuity (border tier)
/// resp. the latest frame wins.
pub fn select_burst_pick(frames: &[BurstFrame]) -> BurstVerdict {
    let mut pick: Option<&BurstFrame> = None;
    for f in frames {
        if f.outcome.is_ok() {
            pick = Some(f);
        }
    }
    if let Some(f) = pick {
        return BurstVerdict::Pick {
            offset_secs: f.offset_secs,
            name: f.outcome.clone().unwrap(),
        };
    }
    // Closest name-tier miss: the latest frame that had a
    // gate-clearing border but failed the name discipline.
    for f in frames.iter().rev() {
        if matches!(f.outcome, Err(Miss::NameNoMatch { .. })) {
            let why = f.outcome.as_ref().unwrap_err().describe();
            return BurstVerdict::Miss(format!(
                "burst of {} grab(s) cleared nothing; closest: T+{} frame — {why}",
                frames.len(),
                f.offset_secs
            ));
        }
    }
    // Closest border-tier miss: the strongest best-slot
    // continuity seen anywhere (ties go to the later frame).
    let mut best: Option<(&BurstFrame, f32)> = None;
    for f in frames {
        if let Err(Miss::NoBorderWinner { scores }) = &f.outcome {
            let m = scores.iter().copied().fold(0.0f32, f32::max);
            if best.is_none_or(|(_, b)| m >= b) {
                best = Some((f, m));
            }
        }
    }
    if let Some((f, _)) = best {
        let why = f.outcome.as_ref().unwrap_err().describe();
        return BurstVerdict::Miss(format!(
            "burst of {} grab(s) cleared nothing; closest: T+{} frame — {why}",
            frames.len(),
            f.offset_secs
        ));
    }
    BurstVerdict::Miss(format!(
        "burst of {} grab(s): no frame was the reward screen",
        frames.len()
    ))
}

/// The short plain reason class for a burst miss, for the
/// event log's `crack_miss` records (fifty-first pass). Keyed
/// on [`select_burst_pick`]'s own phrasing: the border tier's
/// string names the missing border, the fallback string is
/// the no-screen tier, and the remaining tier — a bordered
/// card was read but its name was not proven — is the
/// fallthrough.
pub fn miss_reason_class(why: &str) -> &'static str {
    if why.contains("no clear bordered card") {
        "no bordered card"
    } else if why.contains("no frame was the reward screen") {
        "no reward screen"
    } else {
        "name not proven"
    }
}

/// Run the per-frame detection over a whole burst and
/// select the outcome — the resident's one call.
pub fn detect_burst(
    engine: &dyn OcrEngine,
    frames: &[(u64, GrayImage)],
    parts: &Universe,
) -> BurstVerdict {
    let outcomes: Vec<BurstFrame> = frames
        .iter()
        .map(|(offset_secs, frame)| BurstFrame {
            offset_secs: *offset_secs,
            outcome: detect_pick(engine, frame, parts),
        })
        .collect();
    select_burst_pick(&outcomes)
}

/// Resolve a gets-reward uniqueName path to a display name via
/// the item-names data, for the arm log line (pass 49). Tries
/// the path verbatim, its `StoreItems`-stripped dataset form,
/// and the `StoreItems`-inserted form (the dataset mixes both
/// shapes — the Varzia manifest resolution at plan.rs is the
/// precedent). None when the data does not name it: the caller
/// logs the raw path, never a guess. Diagnostic only — the
/// roll is never a credit input.
pub fn roll_display_name(
    roll_path: &str,
    item_names: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if let Some(name) = item_names.get(roll_path) {
        return Some(name.clone());
    }
    let stripped = roll_path.replace("/StoreItems/", "/");
    if stripped != roll_path {
        if let Some(name) = item_names.get(&stripped) {
            return Some(name.clone());
        }
    } else if let Some(rest) = roll_path.strip_prefix("/Lotus/") {
        let inserted = format!("/Lotus/StoreItems/{rest}");
        if let Some(name) = item_names.get(&inserted) {
            return Some(name.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Stock credit (stage 5)
// ---------------------------------------------------------------------------

/// Serialises reward credits against each other. (The
/// /unverified toggle carries its own gate; whole-file
/// read-modify-write races between the two are the same
/// class capture finalize already accepts.)
static CREDIT_GATE: Mutex<()> = Mutex::new(());

/// Credit +1 of `name` into the verified parts of the stock
/// file at `stock_path`, with the /unverified mutation's
/// durability discipline: lock, read-modify-write,
/// tmp+rename via [`StockState::write_to`], and refuse on an
/// unparseable stock file (never clobber). A missing file
/// starts from empty stock. A verified credit also prunes
/// the part's unverified entry — finalize does the same when
/// a capture verifies an asserted part. Returns the new
/// verified count.
pub fn credit_part(stock_path: &Path, name: &str) -> Result<u32, String> {
    let _gate = CREDIT_GATE.lock().unwrap_or_else(|p| p.into_inner());
    let mut stock = match std::fs::read_to_string(stock_path) {
        Ok(text) => crate::ocr::StockState::from_json(&text)
            .map_err(|e| format!("stock unparseable, refusing to credit: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => crate::ocr::StockState::default(),
        Err(e) => return Err(format!("stock unreadable, refusing to credit: {e}")),
    };
    let count = {
        let entry = stock.parts.entry(name.to_owned()).or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    };
    stock.unverified.remove(name);
    stock
        .write_to(stock_path)
        .map_err(|e| format!("stock write failed: {e}"))?;
    Ok(count)
}

/// Save a grabbed frame under `capture-debug/rewards/` for
/// audit (successes and misses alike — the miss frames are
/// the evidence when detection is questioned). Returns the
/// path on success.
pub fn save_audit_frame(frame: &GrayImage, debug_dir: &Path, stamp: &str) -> Result<PathBuf, String> {
    let dir = debug_dir.join("rewards");
    std::fs::create_dir_all(&dir).map_err(|e| format!("audit dir: {e}"))?;
    let path = dir.join(format!("reward-{stamp}.png"));
    frame.write_png(&path)?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Episode arming + live-capture frame tap (resident wiring)
// ---------------------------------------------------------------------------

/// Coalescing state for reward episodes. One pending grab
/// per episode, re-armed only after the grab resolves or the
/// screen shuts down. (Pass 39's paired "initialized"/"Got
/// rewards" lines motivated this; pass 49's gets-reward
/// trigger fires once per screen, and the coalescing remains
/// as the guard against any repeat within an open episode.)
#[derive(Debug, Default)]
pub struct RewardArming {
    pending: bool,
}

impl RewardArming {
    pub fn new() -> Self {
        Self::default()
    }

    /// A reward screen appeared. Returns true when the caller
    /// should start a grab (no episode was pending).
    pub fn on_screen(&mut self) -> bool {
        if self.pending {
            false
        } else {
            self.pending = true;
            true
        }
    }

    /// The grab task finished (credit or miss).
    pub fn on_resolved(&mut self) {
        self.pending = false;
    }

    /// The reward screen shut down (RelicCrack event). The
    /// episode is over whether or not the grab has resolved;
    /// a still-running grab keeps its own frame and resolves
    /// independently.
    pub fn on_closed(&mut self) {
        self.pending = false;
    }

    pub fn pending(&self) -> bool {
        self.pending
    }
}

/// The freshest frame of a live inventory-capture stream,
/// tapped for the reward grab so an armed episode does not
/// open a second portal stream. Frames are only retained
/// while armed (one clone per frame during the ~14 s burst
/// window), each stamped with its arrival time: if the
/// capture session stops mid-episode, the burst must not
/// mistake a stale frame for a current grab.
#[derive(Default)]
pub struct FrameTap {
    armed: AtomicBool,
    latest: Mutex<Option<(GrayImage, std::time::Instant)>>,
}

impl FrameTap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn arm(&self) {
        self.armed.store(true, Ordering::Relaxed);
    }

    pub fn disarm(&self) {
        self.armed.store(false, Ordering::Relaxed);
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    /// Called by the capture stream's frame source for every
    /// frame; retains a clone only while armed.
    pub fn offer(&self, frame: &GrayImage) {
        if self.is_armed() {
            if let Ok(mut g) = self.latest.lock() {
                *g = Some((frame.clone(), std::time::Instant::now()));
            }
        }
    }

    /// The freshest tapped frame, provided it arrived within
    /// `max_age` of now — a live stream delivers continuously,
    /// so a frame older than that means the stream stopped and
    /// its last frame predates the final pick.
    pub fn latest_fresh(&self, max_age: std::time::Duration) -> Option<GrayImage> {
        let g = self.latest.lock().ok()?;
        let (frame, at) = g.as_ref()?;
        if at.elapsed() <= max_age {
            Some(frame.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::OcrRead;

    /// Engine scripted by region: a slot's name band returns
    /// that slot's text; anything else reads empty.
    struct ScriptedEngine {
        bands: [String; SLOTS],
    }

    impl OcrEngine for ScriptedEngine {
        fn name(&self) -> &str {
            "scripted"
        }
        fn ocr_cell(&self, _cell: &GrayImage) -> OcrRead {
            OcrRead::empty()
        }
        fn read_text(
            &self,
            image: &GrayImage,
            region: [f32; 4],
            _psm: u32,
            _upscale: u32,
        ) -> String {
            for slot in 0..SLOTS {
                if region == name_band_region(image.width, image.height, slot) {
                    return self.bands[slot].clone();
                }
            }
            String::new()
        }
    }

    fn parts_universe() -> Universe {
        Universe::from_names(&[
            "Dual Kamas Prime Blueprint".to_owned(),
            "Alternox Prime Receiver".to_owned(),
            "Kompressa Prime Receiver".to_owned(),
            "Cedo Prime Stock".to_owned(),
            "Harrow Prime Neuroptics Blueprint".to_owned(),
        ])
    }

    /// A dark frame with the card row drawn in: panels on all
    /// four slots and white (255) 4 px borders on the given
    /// slots. `text_bars` adds bright name-band content to
    /// every slot (structural presence for the classifier);
    /// the real-engine test renders real names instead and
    /// passes false.
    fn synthetic_frame(bordered: &[usize], text_bars: bool) -> GrayImage {
        synthetic_frame_at(0.0, SLOTS, bordered, text_bars)
    }

    /// [`synthetic_frame`] for a centred row of `cards` cards
    /// at row origin `origin` (pass 49's fewer-card rows).
    fn synthetic_frame_at(
        origin: f32,
        cards: usize,
        bordered: &[usize],
        text_bars: bool,
    ) -> GrayImage {
        let (fw, fh) = (2560, 1440);
        let mut frame = GrayImage::blank(fw, fh, 35);
        for slot in 0..cards {
            let (x, y, w, h) = slot_rect_at(fw, fh, origin, slot);
            for py in y..y + h {
                for px in x..x + w {
                    frame.set(px, py, 55);
                }
            }
            if text_bars {
                for py in (y + (0.80 * h as f32) as u32)..(y + (0.87 * h as f32) as u32) {
                    for px in (x + (0.10 * w as f32) as u32)..(x + (0.90 * w as f32) as u32) {
                        frame.set(px, py, 200);
                    }
                }
            }
        }
        for &slot in bordered {
            let (x, y, w, h) = slot_rect_at(fw, fh, origin, slot);
            for t in 0..4u32 {
                for px in x..x + w {
                    frame.set(px, y + t, 255);
                    frame.set(px, y + h - 1 - t, 255);
                }
                for py in y..y + h {
                    frame.set(x + t, py, 255);
                    frame.set(x + w - 1 - t, py, 255);
                }
            }
        }
        frame
    }

    /// Engine scripted by (origin, slot): a band on a shifted
    /// row returns that row's text (pass 49 geometry tests).
    struct GeoEngine {
        /// (origin, per-slot band text for that row's cards).
        rows: Vec<(f32, Vec<String>)>,
    }

    impl OcrEngine for GeoEngine {
        fn name(&self) -> &str {
            "geo-scripted"
        }
        fn ocr_cell(&self, _cell: &GrayImage) -> OcrRead {
            OcrRead::empty()
        }
        fn read_text(
            &self,
            image: &GrayImage,
            region: [f32; 4],
            _psm: u32,
            _upscale: u32,
        ) -> String {
            for (origin, bands) in &self.rows {
                for (slot, text) in bands.iter().enumerate() {
                    if region == name_band_region_at(image.width, image.height, *origin, slot) {
                        return text.clone();
                    }
                }
            }
            String::new()
        }
    }

    fn sevagoth_universe() -> Universe {
        Universe::from_names(&[
            "Sevagoth Prime Systems Blueprint".to_owned(),
            "Sevagoth Prime Blueprint".to_owned(),
            "Sevagoth Prime Chassis Blueprint".to_owned(),
            "Burston Prime Blueprint".to_owned(),
            "Burston Prime Barrel".to_owned(),
        ])
    }

    fn scripted() -> ScriptedEngine {
        ScriptedEngine {
            bands: [
                "Dual Kamas Prime Blueprint".to_owned(),
                "Alternox Prime Receiver".to_owned(),
                "Kompressa Prime Receiver".to_owned(),
                "Cedo Prime Stock".to_owned(),
            ],
        }
    }

    #[test]
    fn synthetic_screen_detects_and_credits_end_to_end() {
        let frame = synthetic_frame(&[0], true);
        let engine = scripted();
        let name = detect_pick(&engine, &frame, &parts_universe())
            .expect("the bordered card's part must be detected");
        assert_eq!(name, "Dual Kamas Prime Blueprint");

        // Credit against a temp stock file.
        let dir = std::env::temp_dir().join(format!(
            "fracker-reward-e2e-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stock_path = dir.join("stock.json");
        std::fs::write(
            &stock_path,
            r#"{"parts":{"Cedo Prime Stock":2},"unverified":{"Dual Kamas Prime Blueprint":1}}"#,
        )
        .unwrap();
        let count = credit_part(&stock_path, &name).unwrap();
        assert_eq!(count, 1);
        let text = std::fs::read_to_string(&stock_path).unwrap();
        let stock = crate::ocr::StockState::from_json(&text).unwrap();
        assert_eq!(stock.parts.get("Dual Kamas Prime Blueprint"), Some(&1));
        assert_eq!(stock.parts.get("Cedo Prime Stock"), Some(&2));
        assert!(
            !stock.unverified.contains_key("Dual Kamas Prime Blueprint"),
            "a verified credit prunes the unverified entry"
        );
        // A second credit increments.
        assert_eq!(credit_part(&stock_path, &name).unwrap(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Load a sibling-tree fixture image, or `None` when the
    /// fracker-frames tree is absent (the strip-test pattern:
    /// verdicts on real pixels where they exist, a skip where
    /// they do not).
    fn sibling_image(rel: &str) -> Option<GrayImage> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
        let bytes = std::fs::read(&path).ok()?;
        GrayImage::from_bytes(&bytes).ok()
    }

    #[test]
    fn fixture_border_winner_stays_in_the_nineties() {
        // Pass 39's fixture (Seth's screencap, 1769x996): the
        // bordered slot 0 must still win, with continuity in
        // the 0.9s, under the pass-43 contiguous window.
        let Some(frame) = sibling_image("../fracker-frames/fixtures/reward-screen.png") else {
            eprintln!("skip reward fixture: absent");
            return;
        };
        let scores = border_scores(&frame);
        assert_eq!(border_winner(&scores), Some(0), "scores: {scores:?}");
        assert!(
            scores[0] >= 0.90,
            "fixture winner continuity {:.3} left the 0.9s",
            scores[0]
        );
    }

    #[test]
    fn live_frame_border_offsets_resolve_the_pick() {
        // Pass 43's live regression: Seth's 2026-10-26 T+14
        // frame (2560x1440) whose bordered card (slot 2,
        // Steflos Prime Receiver) the pass-39 sparse offsets
        // read at 0.31 and missed. The contiguous window must
        // resolve it: winner slot 2 at >= 0.90, every other
        // slot <= 0.10.
        let Some(frame) =
            sibling_image("../fracker-frames/live-relic-sync/reward-1793062149.png")
        else {
            eprintln!("skip live reward frame: absent");
            return;
        };
        let scores = border_scores(&frame);
        assert_eq!(border_winner(&scores), Some(2), "scores: {scores:?}");
        assert!(
            scores[2] >= 0.90,
            "live winner continuity {:.3} below 0.90",
            scores[2]
        );
        for (i, s) in scores.iter().enumerate() {
            if i != 2 {
                assert!(*s <= 0.10, "live slot {i} continuity {s:.3} above 0.10");
            }
        }
    }

    // -- Fewer-card geometries (pass 49) ------------------------

    #[test]
    fn three_person_row_detects_via_the_centred_origin() {
        // A centred trio: the bordered card straddles two
        // 4-grid slots, so the 4-grid border gate cannot win
        // (precondition), and detection must come from the
        // +pitch/2 geometry under the identical gates.
        let frame = synthetic_frame_at(row_origin(3), 3, &[0], true);
        let scores = border_scores(&frame);
        assert_eq!(
            border_winner(&scores),
            None,
            "precondition: the 4-grid must miss the trio: {scores:?}"
        );
        let engine = GeoEngine {
            rows: vec![(
                row_origin(3),
                vec![
                    "Sevagoth Prime Systems Blueprint".to_owned(),
                    "Burston Prime Barrel".to_owned(),
                    "Burston Prime Blueprint".to_owned(),
                ],
            )],
        };
        let name = detect_pick(&engine, &frame, &sevagoth_universe())
            .expect("the trio's bordered card must be detected");
        assert_eq!(name, "Sevagoth Prime Systems Blueprint");
    }

    #[test]
    fn one_and_two_person_rows_detect() {
        // Duo (origin +1 pitch — its cards coincide with
        // 4-grid slots 1/2, so this pins the end-to-end result
        // rather than a specific path).
        let duo = synthetic_frame_at(row_origin(2), 2, &[1], true);
        let engine = GeoEngine {
            rows: vec![
                (
                    row_origin(2),
                    vec![
                        "Burston Prime Barrel".to_owned(),
                        "Sevagoth Prime Systems Blueprint".to_owned(),
                    ],
                ),
                // The same bands as the 4-grid slots they coincide
                // with, so whichever path reads, the text agrees.
                (
                    0.0,
                    vec![
                        String::new(),
                        "Burston Prime Barrel".to_owned(),
                        "Sevagoth Prime Systems Blueprint".to_owned(),
                        String::new(),
                    ],
                ),
            ],
        };
        assert_eq!(
            detect_pick(&engine, &duo, &sevagoth_universe()),
            Ok("Sevagoth Prime Systems Blueprint".to_owned())
        );
        // Solo (origin +1.5 pitch — the lone card straddles
        // 4-grid slots 1/2; only the 1-card geometry aligns).
        let solo = synthetic_frame_at(row_origin(1), 1, &[0], true);
        let scores = border_scores(&solo);
        assert_eq!(
            border_winner(&scores),
            None,
            "precondition: the 4-grid must miss the solo card: {scores:?}"
        );
        let engine = GeoEngine {
            rows: vec![(
                row_origin(1),
                vec!["Sevagoth Prime Systems Blueprint".to_owned()],
            )],
        };
        assert_eq!(
            detect_pick(&engine, &solo, &sevagoth_universe()),
            Ok("Sevagoth Prime Systems Blueprint".to_owned())
        );
    }

    #[test]
    fn borderless_trio_misses_with_the_four_grid_scores() {
        // No border on any card: every geometry misses, and
        // the miss is the 4-grid's own NoBorderWinner — shape
        // and scores unchanged (burst reporting reads these).
        let frame = synthetic_frame_at(row_origin(3), 3, &[], true);
        let engine = GeoEngine { rows: vec![] };
        match detect_pick(&engine, &frame, &sevagoth_universe()) {
            Err(Miss::NoBorderWinner { scores }) => {
                assert_eq!(scores, border_scores(&frame));
                assert!(
                    scores.iter().all(|s| *s < BORDER_FLOOR),
                    "scores: {scores:?}"
                );
            }
            other => panic!("expected NoBorderWinner, got {other:?}"),
        }
    }

    #[test]
    fn three_person_real_frames_resolve_at_the_centred_origin() {
        // Seth's 2026-10-27 23:56 burst (3-player, client;
        // THREE_PERSON_REWARD_GEOMETRY.md). Before pass 49:
        // the production 4-grid border-scores these frames
        // [0, 0.192, 0, 0] / all-zero — a guaranteed miss.
        // After: the centred 3-card origin reads his bordered
        // card (slot 0, Sevagoth Prime Systems Blueprint) in
        // the high 0.9s.
        for rel in [
            "../fracker-frames/three-person/reward-1793159728-t0.png",
            "../fracker-frames/three-person/reward-1793159728-t14.png",
        ] {
            let Some(frame) = sibling_image(rel) else {
                eprintln!("skip 3-person frame {rel}: absent");
                return;
            };
            let scores4 = border_scores(&frame);
            assert_eq!(
                border_winner(&scores4),
                None,
                "{rel}: the 4-grid verdict must stay a miss: {scores4:?}"
            );
            let alt = border_scores_at(&frame, row_origin(3), 3);
            assert_eq!(
                border_winner_in(&alt),
                Some(0),
                "{rel}: trio scores: {alt:?}"
            );
            assert!(
                alt[0] >= 0.90,
                "{rel}: trio winner continuity {:.3} below 0.90",
                alt[0]
            );
        }
    }

    #[test]
    fn three_person_t14_detects_sevagoth_with_the_real_engine() {
        // The pass-49 acceptance frames end to end: real OCR
        // over the burst's first and last frames names his
        // pick (his roll, pre-selected from screen-open, is
        // the bordered card in both).
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let Some(layout) = crate::ocr::Layout::load_file(&base.join("data/capture/layouts.toml"))
            .ok()
            .and_then(|l| l.get("relic").cloned())
        else {
            return; // layouts unavailable in this environment
        };
        let Some(engine) = crate::ocr::TesseractCliEngine::probe(layout) else {
            return; // no tesseract in this environment
        };
        for rel in [
            "../fracker-frames/three-person/reward-1793159728-t0.png",
            "../fracker-frames/three-person/reward-1793159728-t14.png",
        ] {
            let Some(frame) = sibling_image(rel) else {
                eprintln!("skip 3-person frame {rel}: absent");
                return;
            };
            match detect_pick(&engine, &frame, &sevagoth_universe()) {
                Ok(name) => assert_eq!(name, "Sevagoth Prime Systems Blueprint", "{rel}"),
                Err(miss) => panic!("{rel} detection missed: {}", miss.describe()),
            }
        }
    }

    // -- Burst selection (pass 43 amendment) -------------------

    fn bf(offset_secs: u64, outcome: Result<String, Miss>) -> BurstFrame {
        BurstFrame {
            offset_secs,
            outcome,
        }
    }

    #[test]
    fn burst_latest_clearing_frame_wins() {
        // The pick changed mid-window: T+8 cleared on one
        // card, T+12 on another — the LATEST clearing frame
        // is the final pick. Non-classifying frames around
        // them change nothing.
        let frames = vec![
            bf(8, Ok("Baza Prime Stock".to_owned())),
            bf(10, Err(Miss::NotRewardScreen)),
            bf(12, Ok("Steflos Prime Receiver".to_owned())),
            bf(14, Err(Miss::NotRewardScreen)),
        ];
        assert_eq!(
            select_burst_pick(&frames),
            BurstVerdict::Pick {
                offset_secs: 12,
                name: "Steflos Prime Receiver".to_owned()
            }
        );
    }

    #[test]
    fn burst_early_close_keeps_the_early_pick() {
        // Endless rotation: the screen closed right after
        // T+8; every later grab landed elsewhere. The early
        // clearing frame still credits (a border cannot
        // un-happen).
        let frames = vec![
            bf(8, Ok("Baza Prime Stock".to_owned())),
            bf(10, Err(Miss::NotRewardScreen)),
            bf(12, Err(Miss::NotRewardScreen)),
            bf(14, Err(Miss::NotRewardScreen)),
        ];
        assert_eq!(
            select_burst_pick(&frames),
            BurstVerdict::Pick {
                offset_secs: 8,
                name: "Baza Prime Stock".to_owned()
            }
        );
    }

    #[test]
    fn burst_no_border_anywhere_reports_the_best_frame() {
        // Every classifying frame failed the border gate:
        // the miss names the strongest attempt and its frame.
        let frames = vec![
            bf(8, Err(Miss::NoBorderWinner {
                scores: [0.20, 0.05, 0.11, 0.02],
            })),
            bf(10, Err(Miss::NotRewardScreen)),
            bf(12, Err(Miss::NoBorderWinner {
                scores: [0.04, 0.31, 0.09, 0.00],
            })),
            bf(14, Err(Miss::NotRewardScreen)),
        ];
        match select_burst_pick(&frames) {
            BurstVerdict::Miss(line) => {
                assert!(line.contains("T+12"), "line: {line}");
                assert!(line.contains("0.31"), "line: {line}");
            }
            other => panic!("expected a miss, got {other:?}"),
        }
    }

    #[test]
    fn burst_no_reward_screen_at_all_is_a_plain_miss() {
        let frames = vec![
            bf(8, Err(Miss::NotRewardScreen)),
            bf(14, Err(Miss::NotRewardScreen)),
        ];
        match select_burst_pick(&frames) {
            BurstVerdict::Miss(line) => {
                assert!(line.contains("no frame was the reward screen"), "line: {line}");
            }
            other => panic!("expected a miss, got {other:?}"),
        }
    }

    #[test]
    fn miss_reason_class_names_each_burst_tier() {
        // Pass 51: the event log's crack-miss classes, pinned
        // against the real select_burst_pick outputs.
        let border = vec![bf(8, Err(Miss::NoBorderWinner {
            scores: [0.20, 0.05, 0.11, 0.02],
        }))];
        let BurstVerdict::Miss(why) = select_burst_pick(&border) else {
            panic!("expected a miss");
        };
        assert_eq!(miss_reason_class(&why), "no bordered card");

        let name = vec![bf(8, Err(Miss::NameNoMatch {
            detail: "psm6 read \"X\" matched nothing".to_owned(),
        }))];
        let BurstVerdict::Miss(why) = select_burst_pick(&name) else {
            panic!("expected a miss");
        };
        assert_eq!(miss_reason_class(&why), "name not proven");

        let screen = vec![bf(8, Err(Miss::NotRewardScreen))];
        let BurstVerdict::Miss(why) = select_burst_pick(&screen) else {
            panic!("expected a miss");
        };
        assert_eq!(miss_reason_class(&why), "no reward screen");
    }

    #[test]
    fn burst_detect_late_pick() {
        // Image level: the border only exists on the last
        // grab (the pick landed late in the window).
        let engine = scripted();
        let frames = vec![
            (8, synthetic_frame(&[], true)),
            (10, synthetic_frame(&[], true)),
            (14, synthetic_frame(&[2], true)),
        ];
        assert_eq!(
            detect_burst(&engine, &frames, &parts_universe()),
            BurstVerdict::Pick {
                offset_secs: 14,
                name: "Kompressa Prime Receiver".to_owned()
            }
        );
    }

    #[test]
    fn burst_detect_early_screen_close() {
        // Image level: only the first grab is the reward
        // screen; the rest are post-close frames (blank —
        // the next rotation / gameplay).
        let engine = scripted();
        let blank = GrayImage::blank(2560, 1440, 35);
        let frames = vec![
            (8, synthetic_frame(&[0], true)),
            (10, blank.clone()),
            (12, blank.clone()),
            (14, blank),
        ];
        assert_eq!(
            detect_burst(&engine, &frames, &parts_universe()),
            BurstVerdict::Pick {
                offset_secs: 8,
                name: "Dual Kamas Prime Blueprint".to_owned()
            }
        );
    }

    #[test]
    fn burst_late_armed_caught_by_the_first_samples() {
        // The EE.log trigger trailed the screen by ~10 s:
        // at arming, only ~5 s of the window remain. The
        // FIRST samples already show the final bordered
        // pick; everything after is the closed screen. Why
        // the burst starts at T+0 instead of waiting.
        let frames = vec![
            bf(0, Ok("Steflos Prime Receiver".to_owned())),
            bf(2, Err(Miss::NoBorderWinner {
                scores: [0.12, 0.03, 0.44, 0.01],
            })),
            bf(4, Err(Miss::NotRewardScreen)),
            bf(6, Err(Miss::NotRewardScreen)),
        ];
        assert_eq!(
            select_burst_pick(&frames),
            BurstVerdict::Pick {
                offset_secs: 0,
                name: "Steflos Prime Receiver".to_owned()
            }
        );
    }

    #[test]
    fn burst_detect_late_armed() {
        // Image level, same shape: the bordered screen
        // exists only in the earliest grabs.
        let engine = scripted();
        let blank = GrayImage::blank(2560, 1440, 35);
        let frames = vec![
            (0, synthetic_frame(&[3], true)),
            (2, blank.clone()),
            (4, blank),
        ];
        assert_eq!(
            detect_burst(&engine, &frames, &parts_universe()),
            BurstVerdict::Pick {
                offset_secs: 0,
                name: "Cedo Prime Stock".to_owned()
            }
        );
    }

    #[test]
    fn burst_detect_no_border_anywhere_misses() {
        // Image level: reward screens throughout, no border
        // on any — a miss, never a guess.
        let engine = scripted();
        let frames = vec![
            (8, synthetic_frame(&[], true)),
            (12, synthetic_frame(&[], true)),
        ];
        match detect_burst(&engine, &frames, &parts_universe()) {
            BurstVerdict::Miss(line) => {
                assert!(line.contains("no clear bordered card"), "line: {line}");
            }
            other => panic!("expected a miss, got {other:?}"),
        }
    }

    #[test]
    fn gameplay_frame_credits_nothing() {
        // A bright scene frame: no dark panels, no card row —
        // classification refuses before borders or names are
        // even considered.
        let frame = GrayImage::blank(2560, 1440, 180);
        let engine = scripted();
        let err = detect_pick(&engine, &frame, &parts_universe()).unwrap_err();
        assert_eq!(err, Miss::NotRewardScreen);
    }

    #[test]
    fn ambiguous_border_credits_nothing() {
        // Two equally bordered cards: no clear winner.
        let frame = synthetic_frame(&[1, 3], true);
        let engine = scripted();
        let err = detect_pick(&engine, &frame, &parts_universe()).unwrap_err();
        assert!(
            matches!(err, Miss::NoBorderWinner { .. }),
            "two borders must miss, got {err:?}"
        );
        // And no border at all also misses.
        let frame = synthetic_frame(&[], true);
        let err = detect_pick(&engine, &frame, &parts_universe()).unwrap_err();
        assert!(matches!(err, Miss::NoBorderWinner { .. }));
    }

    #[test]
    fn double_trigger_arms_exactly_one_grab() {
        let mut arming = RewardArming::new();
        assert!(arming.on_screen(), "first screen event arms");
        assert!(
            !arming.on_screen(),
            "a second screen event inside the episode must not re-arm"
        );
        assert!(arming.pending());
        arming.on_resolved();
        assert!(arming.on_screen(), "a resolved episode re-arms");
        arming.on_closed();
        assert!(!arming.pending());
        assert!(arming.on_screen(), "a closed episode re-arms");
    }

    #[test]
    fn roll_display_name_resolves_dataset_shapes() {
        let mut names = std::collections::HashMap::new();
        names.insert(
            "/Lotus/Types/Recipes/WarframeRecipes/HarrowPrimeHelmetBlueprint".to_owned(),
            "Harrow Prime Neuroptics Blueprint".to_owned(),
        );
        names.insert(
            "/Lotus/Types/StoreItems/AvatarImages/ImagePriest".to_owned(),
            "Harrow Glyph".to_owned(),
        );
        // The grant path is the StoreItems variant of the dataset key.
        assert_eq!(
            roll_display_name(
                "/Lotus/StoreItems/Types/Recipes/WarframeRecipes/HarrowPrimeHelmetBlueprint",
                &names
            ),
            Some("Harrow Prime Neuroptics Blueprint".to_owned())
        );
        // Verbatim hit.
        assert_eq!(
            roll_display_name("/Lotus/Types/StoreItems/AvatarImages/ImagePriest", &names),
            Some("Harrow Glyph".to_owned())
        );
        // Unresolvable: None (the caller logs the raw path).
        assert_eq!(
            roll_display_name("/Lotus/Types/Recipes/Nope", &names),
            None
        );
    }

    #[test]
    fn credit_refuses_unparseable_stock() {
        let dir = std::env::temp_dir().join(format!(
            "fracker-reward-bad-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stock_path = dir.join("stock.json");
        std::fs::write(&stock_path, "{not json").unwrap();
        assert!(credit_part(&stock_path, "Cedo Prime Stock").is_err());
        // The unparseable file is untouched.
        assert_eq!(std::fs::read_to_string(&stock_path).unwrap(), "{not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real tesseract engine over a synthetic screen whose
    /// names are rendered with the vendored game font
    /// (crate::ttf): the full pipeline — header read, border,
    /// band OCR, universe match — with no scripted answers.
    #[test]
    fn real_engine_reads_rendered_screen() {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let Some(layout) = crate::ocr::Layout::load_file(&base.join("data/capture/layouts.toml"))
            .ok()
            .and_then(|l| l.get("relic").cloned())
        else {
            return; // layouts unavailable in this environment
        };
        let Some(engine) = crate::ocr::TesseractCliEngine::probe(layout) else {
            return; // no tesseract in this environment
        };
        let Ok(font) = crate::ttf::embedded_font() else {
            return;
        };
        let (fw, fh) = (2560u32, 1440u32);
        let mut frame = synthetic_frame(&[2], false);
        // Names on all four cards (two lines for the long one),
        // sized to fit the band like the game's bottom-anchored
        // wrap.
        let names = [
            "Dual Kamas Prime Blueprint",
            "Alternox Prime Receiver",
            "Kompressa Prime Receiver",
            "Cedo Prime Stock",
        ];
        for (slot, name) in names.iter().enumerate() {
            let region = name_band_region(fw, fh, slot);
            let bx = (region[0] * fw as f32) as u32;
            let by = (region[1] * fh as f32) as u32;
            let bw = ((region[2] - region[0]) * fw as f32) as u32;
            let lines = wrap_two(name);
            for (i, line) in lines.iter().enumerate() {
                let img = font.render_line(line, 24.0, 0.6, 0.0, 1.0, 2);
                let x = bx + bw.saturating_sub(img.width) / 2;
                paste_ink(&mut frame, &img, x, by + i as u32 * 26);
            }
        }
        let name = match detect_pick(&engine, &frame, &parts_universe()) {
            Ok(n) => n,
            Err(miss) => panic!("real-engine detection missed: {}", miss.describe()),
        };
        assert_eq!(name, "Kompressa Prime Receiver");
    }

    /// Paste a rendered line's ink into `frame` at (x, y),
    /// brightening (max-blend), clipped to the frame.
    fn paste_ink(frame: &mut GrayImage, ink: &GrayImage, x: u32, y: u32) {
        for iy in 0..ink.height {
            for ix in 0..ink.width {
                let (fx, fy) = (x + ix, y + iy);
                if fx < frame.width && fy < frame.height {
                    let v = ink.get(ix, iy);
                    if v > frame.get(fx, fy) {
                        frame.set(fx, fy, v);
                    }
                }
            }
        }
    }

    /// Split a part name into at most two lines at the middle
    /// space, like the game's centred wrap.
    fn wrap_two(name: &str) -> Vec<String> {
        let words: Vec<&str> = name.split(' ').collect();
        if words.len() < 3 {
            return vec![name.to_owned()];
        }
        let mid = words.len() / 2;
        vec![
            words[..mid].join(" "),
            words[mid..].join(" "),
        ]
    }
}

