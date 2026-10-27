//! Pass 34 (Pass B) — the line reader and the form channels.
//!
//! Pass 33's stitch reads each panorama slot once, as a whole
//! name band (PSM 11). That presentation loses a measurable class
//! of cells: the band read garbles or ties on names the pixels
//! render cleanly line by line (the regrid prototype's stage 2:
//! 180/178 distinct per census against the band reader's 159/157,
//! with the union of the two readers at 216/217). The prototype's
//! line reader was nevertheless unbuildable as specified: fed
//! through the unchanged matcher it MINTED forms — on Warframe
//! component-blueprint cells its line assembly dropped the
//! `Blueprint` line, the assembled read became the built form's
//! exact name, and the matcher held the built sibling at full
//! score (24 false cells, one mechanism). Text evidence alone
//! cannot separate the forms — a token-subset read of one form
//! can BE the other form's clean name (the property battery's
//! forced exception, pass 33) — so this pass builds the reader
//! and, alongside it, the three independent form channels the
//! reviewer's Q3 required, with positive evidence in both
//! directions:
//!
//! * **(a) Line evidence** — this module's line detector either
//!   witnesses a `Blueprint` line in the cell, or CERTIFIES that
//!   no line went undetected (every significant profile run is
//!   inside a detected line box, and no line-like mass hides in
//!   the shadow below the text block). A dropped line is exactly
//!   what the certification exists to refuse to certify.
//! * **(b) Mesh** — blueprint tiles carry a grid mesh in the name
//!   band's background; built tiles render a smooth gradient.
//!   [`mesh_scores`] measures the band margins' gradient energy
//!   (text is centred, so the margins are mesh-or-smooth only).
//! * **(c) Built-copy glyph** — the figure marker at the cell's
//!   top-left means "this cell is a blueprint AND a built copy
//!   is owned" (confirmed by Seth). One-directional:
//!   [`glyph_score`] presence asserts a blueprint (and implies
//!   the built twin as held stock); absence asserts nothing.
//!
//! The channels combine into a [`FormVerdict`] per slot; the
//! arbitration that consumes it lives with the stitch
//! (`capture::stitch_parts_session`), because it arbitrates
//! between this reader and the band reader. The line reader's
//! own holds go through the SAME matching stack as the band
//! reader — [`crate::ocr::validate_read`] plus the radius
//! decoder's band and strip phases, gated by the SAME shared
//! hold predicate (`Universe::hold_permitted`, enforced inside
//! those paths) — the predicate gains a caller, not a variant,
//! and pass 32's evidence floor stays where it was.

use crate::capture::ReadOptions;
use crate::ocr::{
    self, CellResult, GrayImage, Layout, OcrEngine, OcrRead, Universe,
};

// ---------------------------------------------------------------------------
// White top-hat + line detection
// ---------------------------------------------------------------------------

/// Disk footprint offsets for the radius-3 opening (the
/// prototype's `grey_opening` structure element: a 7x7 disk,
/// `(i-3)^2 + (j-3)^2 <= 9`).
fn disk_offsets() -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for dy in -3..=3i32 {
        for dx in -3..=3i32 {
            if dx * dx + dy * dy <= 9 {
                out.push((dx, dy));
            }
        }
    }
    out
}

/// White top-hat: the image minus its grey opening at radius 3.
/// Thin bright structures (text strokes, mesh lines) survive;
/// smooth background (tile gradient, icon bodies) is removed.
/// Detection runs on the top-hat; the pixels OCR'd are always
/// the original band's (the prototype's rule).
pub fn top_hat(img: &GrayImage) -> GrayImage {
    if img.width == 0 || img.height == 0 {
        return img.clone();
    }
    let offs = disk_offsets();
    let (w, h) = (img.width as i32, img.height as i32);
    let sample = |img: &GrayImage, x: i32, y: i32| -> u8 {
        img.get(
            x.clamp(0, w - 1) as u32,
            y.clamp(0, h - 1) as u32,
        )
    };
    // Erosion (min over the disk), then dilation (max) of the
    // eroded image = the opening.
    let mut eroded = GrayImage::blank(img.width, img.height, 0);
    for y in 0..h {
        for x in 0..w {
            let mut m = u8::MAX;
            for &(dx, dy) in &offs {
                m = m.min(sample(img, x + dx, y + dy));
            }
            eroded.set(x as u32, y as u32, m);
        }
    }
    let mut out = GrayImage::blank(img.width, img.height, 0);
    for y in 0..h {
        for x in 0..w {
            let mut m = 0u8;
            for &(dx, dy) in &offs {
                m = m.max(sample(&eroded, x + dx, y + dy));
            }
            let orig = img.get(x as u32, y as u32);
            out.set(x as u32, y as u32, orig.saturating_sub(m));
        }
    }
    out
}

/// One detected text line's box, in name-band coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineBox {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

/// The line detector's read of one name band.
#[derive(Debug, Clone)]
pub struct LineDetection {
    /// Line boxes, top to bottom (padded for OCR; the CORE
    /// bounds used for mass accounting are derivable from the
    /// block and the split, so what is stored is the OCR box).
    pub boxes: Vec<LineBox>,
    /// The text block's vertical extent (first to last
    /// significant profile run), when any text was found.
    pub block: Option<(u32, u32)>,
    /// Total top-hat profile mass in the band.
    pub total_mass: f32,
    /// Mass of significant profile runs that fall OUTSIDE the
    /// block span, as a fraction of the total — an undetected
    /// line's signature.
    pub leftover_run_frac: f32,
    /// Line-like mass in the shadow zone below the block (where
    /// a dropped final line — the `Blueprint` line — would sit),
    /// as a fraction of the total.
    pub shadow_below_frac: f32,
}

impl LineDetection {
    /// Certify that no text line went undetected: every
    /// significant profile run is inside the block the boxes
    /// cover, and the shadow below the block carries no
    /// line-like mass. Certification is what lets a form
    /// channel assert "there is no third line" — the prototype's
    /// form-minting failure was an UNCERTIFIED absence treated
    /// as a certified one. Thresholds are census-calibrated
    /// (docs/thirty-fourth-pass.md): a genuine dropped line
    /// leaves an order of magnitude more mass than the noise
    /// floor these sit above.
    pub fn certifies_complete(&self) -> bool {
        !self.boxes.is_empty()
            && self.leftover_run_frac <= CERT_LEFTOVER_MAX
            && self.shadow_below_frac <= CERT_SHADOW_MAX
    }
}

/// Leftover-run ceiling for line-absence certification.
/// Census calibration (standard census slots, pass 34): built
/// cells' leftover mass sits at median 0, 95th percentile
/// 0.036; a dropped line's run carries far more.
const CERT_LEFTOVER_MAX: f32 = 0.05;
/// Below-block shadow-mass ceiling for certification. The
/// shadow zone always carries a benign floor (tile bottom
/// edge, glyph tails: median ~0.085, 95th percentile ~0.10 on
/// cells with nothing dropped), while a genuinely dropped
/// line — a full line of text mass — puts a quarter or more of
/// the band's mass there. At 0.20 the standard census certifies
/// 153/155 built cells and commits ZERO dangerous
/// certifications (certified + no `Blueprint` witness on a
/// blueprint-truth cell); see docs/thirty-fourth-pass.md.
const CERT_SHADOW_MAX: f32 = 0.20;

/// Detect the text lines of one name band (the prototype's
/// stage-2 detector, hardened):
///
/// 1. White top-hat, row projection, box-smoothed.
/// 2. Significant runs: profile above 30% of peak, gaps of up
///    to 6px bridged, runs under 3px dropped.
/// 3. The text block spans every run carrying at least 12% of
///    the largest run's mass — the prototype took only the
///    max-mass run, which is how a `Blueprint` line separated
///    by a wider valley was dropped from the block entirely;
///    a span that grows past 4.3 line pitches falls back to
///    the max-mass run (a stray mass far below the name must
///    not stretch the block).
/// 4. Line count = round(block height / line pitch), clamped
///    1..=4 (the game's name-line pitch in the band measures
///    19px at the 1440p reference the band is ~87px tall at;
///    both scale with the band).
/// 5. Split at profile minima nearest the equal-split points;
///    boxes padded (outer ±5px, internal ±2px, scaled) and
///    x-trimmed by column mass.
pub fn detect_lines(band: &GrayImage) -> LineDetection {
    let empty = LineDetection {
        boxes: Vec::new(),
        block: None,
        total_mass: 0.0,
        leftover_run_frac: 0.0,
        shadow_below_frac: 0.0,
    };
    if band.width == 0 || band.height == 0 {
        return empty;
    }
    let th = top_hat(band);
    let h = band.height as usize;
    // Row profile of the top-hat, box-smoothed over 3 rows.
    let mut prof = vec![0.0f32; h];
    for y in 0..h {
        let mut s = 0.0f32;
        for x in 0..band.width {
            s += th.get(x, y as u32) as f32;
        }
        prof[y] = s;
    }
    let sm: Vec<f32> = (0..h)
        .map(|y| {
            let mut s = 0.0f32;
            let mut n = 0.0f32;
            for dy in -1i32..=1 {
                let yy = y as i32 + dy;
                if yy >= 0 && (yy as usize) < h {
                    s += prof[yy as usize];
                    n += 1.0;
                }
            }
            s / n.max(1.0)
        })
        .collect();
    let total_mass: f32 = sm.iter().sum();
    let peak = sm.iter().copied().fold(0.0f32, f32::max);
    if peak <= 0.0 || total_mass <= 0.0 {
        return LineDetection {
            boxes: Vec::new(),
            block: None,
            total_mass,
            leftover_run_frac: 0.0,
            shadow_below_frac: 0.0,
        };
    }
    let scale = band.height as f32 / 87.0;
    let line_pitch = 19.0 * scale;
    let bridge = (6.0 * scale).round().max(1.0) as usize;
    let thr = 0.30 * peak;
    // Significant runs above the threshold, gaps bridged.
    let mut runs: Vec<(usize, usize)> = Vec::new(); // [start, end)
    let mut y = 0usize;
    while y < h {
        if sm[y] > thr {
            let start = y;
            let mut last_above = y;
            let mut yy = y;
            while yy < h {
                if sm[yy] > thr {
                    last_above = yy;
                } else if yy - last_above > bridge {
                    break;
                }
                yy += 1;
            }
            runs.push((start, last_above + 1));
            y = last_above + 1;
        } else {
            y += 1;
        }
    }
    runs.retain(|&(a, b)| b - a >= 3);
    if runs.is_empty() {
        return LineDetection {
            boxes: Vec::new(),
            block: None,
            total_mass,
            leftover_run_frac: 0.0,
            shadow_below_frac: 0.0,
        };
    }
    let run_mass: Vec<f32> = runs
        .iter()
        .map(|&(a, b)| sm[a..b].iter().sum())
        .collect();
    let max_mass = run_mass.iter().copied().fold(0.0f32, f32::max);
    // Block: span of the runs carrying >= 12% of the max run's
    // mass, with the over-stretch fallback to the max-mass run.
    let (mut bs, mut be) = (usize::MAX, 0usize);
    for (i, &(a, b)) in runs.iter().enumerate() {
        if run_mass[i] >= 0.12 * max_mass {
            bs = bs.min(a);
            be = be.max(b);
        }
    }
    if (be - bs) as f32 > 4.3 * line_pitch {
        let i = run_mass
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);
        bs = runs[i].0;
        be = runs[i].1;
    }
    // Certification accounting: significant runs outside the
    // block span, and line-like mass in the shadow below it.
    let mut leftover = 0.0f32;
    for (i, &(a, b)) in runs.iter().enumerate() {
        if b <= bs || a >= be {
            leftover += run_mass[i];
        }
    }
    let shadow_end = ((be as f32 + 1.2 * line_pitch) as usize).min(h);
    let shadow_below: f32 = if shadow_end > be {
        sm[be..shadow_end].iter().sum()
    } else {
        0.0
    };
    // Line count and split points (profile minima nearest the
    // equal-split targets).
    let block_h = (be - bs) as f32;
    let n = ((block_h / line_pitch).round() as usize).clamp(1, 4);
    let mut bounds: Vec<usize> = vec![bs];
    for k in 1..n {
        let target = bs as f32 + k as f32 * block_h / n as f32;
        let win = (6.0 * scale).round() as usize;
        let lo = ((target as usize).saturating_sub(win)).max(bs + 3).min(be.saturating_sub(3));
        let hi = ((target as usize) + win + 1).min(be.saturating_sub(3)).max(lo + 1);
        let mut argmin = lo;
        let mut best = f32::MAX;
        for yy in lo..hi.min(h) {
            if sm[yy] < best {
                best = sm[yy];
                argmin = yy;
            }
        }
        // Bounds stay strictly increasing and inside the block.
        let prev = *bounds.last().unwrap_or(&bs);
        let b = argmin.max(prev + 1);
        bounds.push(if b < be { b } else { prev + 1 });
    }
    bounds.push(be);
    // Boxes: padded splits, x-trimmed by the top-hat column
    // mass of each line's core rows.
    let pad_outer = (5.0 * scale).round() as u32;
    let pad_inner = (2.0 * scale).round() as u32;
    let mut boxes = Vec::with_capacity(n);
    for k in 0..n {
        let core0 = bounds[k];
        let core1 = bounds[k + 1];
        let y0 = if k == 0 {
            core0.saturating_sub(pad_outer as usize)
        } else {
            core0.saturating_sub(pad_inner as usize)
        };
        let y1 = if k == n - 1 {
            (core1 + pad_outer as usize).min(h)
        } else {
            (core1 + pad_inner as usize).min(h)
        };
        // Column mass over the core rows.
        let mut colmax = 0.0f32;
        let mut cols = vec![0.0f32; band.width as usize];
        for yy in core0..core1.min(h) {
            for x in 0..band.width {
                let v = th.get(x, yy as u32) as f32;
                cols[x as usize] += v;
            }
        }
        for &c in &cols {
            colmax = colmax.max(c);
        }
        let (x0, x1) = if colmax <= 0.0 {
            (0u32, band.width)
        } else {
            let nz: Vec<usize> = (0..cols.len())
                .filter(|&x| cols[x] > 0.04 * colmax)
                .collect();
            let first = *nz.first().unwrap_or(&0);
            let last = *nz.last().unwrap_or(&(cols.len() - 1));
            (
                (first.saturating_sub(3)) as u32,
                ((last + 4).min(cols.len())) as u32,
            )
        };
        boxes.push(LineBox {
            x0,
            y0: y0 as u32,
            x1,
            y1: y1 as u32,
        });
    }
    LineDetection {
        boxes,
        block: Some((bs as u32, be as u32)),
        total_mass,
        leftover_run_frac: leftover / total_mass,
        shadow_below_frac: shadow_below / total_mass,
    }
}

/// The name band of a cell, as both readers see it.
pub fn name_band(cell: &GrayImage, layout: &Layout) -> GrayImage {
    ocr::crop_region(cell, layout.name)
}

// ---------------------------------------------------------------------------
// Mesh channel
// ---------------------------------------------------------------------------

/// Mesh measurements of one cell's name band. Blueprint tiles
/// carry a grid mesh in the band background; built tiles are a
/// smooth gradient. Text is horizontally centred, so the band's
/// side margins carry mesh-or-smooth only — both scores are
/// margin statistics, immune to the name itself.
#[derive(Debug, Clone, Copy)]
pub struct MeshScores {
    /// Mean gradient magnitude (|dx| + |dy|) over the margins
    /// of the raw band.
    pub margin_grad: f32,
    /// Mean top-hat response over the margins (thin-structure
    /// energy: the mesh lines are thin; a smooth gradient
    /// leaves nothing).
    pub margin_tophat: f32,
}

pub fn mesh_scores(cell: &GrayImage, layout: &Layout) -> MeshScores {
    let band = name_band(cell, layout);
    let (w, h) = (band.width, band.height);
    if w < 8 || h < 4 {
        return MeshScores {
            margin_grad: 0.0,
            margin_tophat: 0.0,
        };
    }
    let mx = (w as f32 * 0.10) as u32; // margin width on each side
    let in_margin = |x: u32| x < mx || x >= w - mx;
    let mut grad_sum = 0.0f32;
    let mut n = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            if !in_margin(x) {
                continue;
            }
            let v = band.get(x, y) as i32;
            let dx = if x + 1 < w {
                (band.get(x + 1, y) as i32 - v).abs()
            } else {
                0
            };
            let dy = if y + 1 < h {
                (band.get(x, y + 1) as i32 - v).abs()
            } else {
                0
            };
            grad_sum += (dx + dy) as f32;
            n += 1.0;
        }
    }
    let th = top_hat(&band);
    let mut th_sum = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            if in_margin(x) {
                th_sum += th.get(x, y) as f32;
            }
        }
    }
    MeshScores {
        margin_grad: if n > 0.0 { grad_sum / n } else { 0.0 },
        margin_tophat: if n > 0.0 { th_sum / n } else { 0.0 },
    }
}

/// The mesh channel's reading of a score. The production score
/// is [`MeshScores::margin_grad`] (a forward-difference margin
/// gradient), measured PER INSTANCE crop and aggregated by
/// median (the arbitration's job): the stitch's median
/// composite blurs the 1px mesh lines across instances'
/// sub-pixel phases and measurably dilutes the statistic.
/// Slot-median distributions on the standard census: blueprint
/// median 6.07, built median 5.78, with long tails both ways —
/// a weak separator alone, which is why the thresholds below
/// make it assert only in its tails and why channel (a), not
/// the mesh, carries the ordinary form assertions. A score in
/// the dead zone asserts nothing, in either direction.
/// Calibration record: docs/thirty-fourth-pass.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshClass {
    Blueprint,
    Built,
    Unknown,
}

/// Margin gradient at or above this reads as blueprint mesh.
/// Deliberately conservative: channel (a) carries the ordinary
/// blueprint assertion (it witnessed the Blueprint line on
/// 120/120 standard-census blueprint slots), so the mesh's
/// blueprint assertion is reserved for scores in the tail
/// where built cells essentially never reach — its distinctive
/// work is the built-side assertion and the contradiction
/// check. The arbitration outcome is flat across the whole
/// plausible plateau of both thresholds (standard census:
/// 230–231 distinct, 0 FA for bp-min in 6.55..=7.6 and
/// built-max in 5.7..=5.95), so these sit mid-plateau rather
/// than at any single slot's convenience.
pub const MESH_BP_MIN: f32 = 7.2;
/// Margin gradient at or below this reads as built smoothness.
pub const MESH_BUILT_MAX: f32 = 5.85;

pub fn mesh_class(score: f32) -> MeshClass {
    if score >= MESH_BP_MIN {
        MeshClass::Blueprint
    } else if score <= MESH_BUILT_MAX {
        MeshClass::Built
    } else {
        MeshClass::Unknown
    }
}

// ---------------------------------------------------------------------------
// Built-copy glyph channel
// ---------------------------------------------------------------------------

/// The built-copy glyph template: the figure marker's cluster,
/// averaged over its three standard-census sightings (Grendel
/// Prime Neuroptics Blueprint, frames 07/08/10) in the canonical
/// badge window (62x36, see [`crate::ocr::BADGE_WINDOW`]),
/// cropped to x 12..38, y 0..24 — the cluster's measured extent.
/// The badge (a ring + digit) scores <= 0.31 against it and an
/// empty corner <= 0.10 on the spot checks; the full census
/// distributions are in docs/thirty-fourth-pass.md.
const GLYPH_TEMPLATE: [[u8; 26]; 24] = [
    [5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7, 11, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 53, 159, 20, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 5, 5, 8, 159, 159, 157, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 5, 5, 45, 159, 150, 142, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 5, 8, 136, 159, 90, 145, 27, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 19, 46, 159, 159, 142, 67, 153, 28, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 12, 99, 159, 159, 159, 158, 103, 118, 139, 17, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 39, 32, 157, 159, 159, 159, 159, 159, 53, 125, 51, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 44, 156, 48, 113, 159, 159, 159, 159, 159, 157, 17, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 6, 48, 159, 159, 159, 48, 116, 159, 159, 159, 147, 142, 79, 43, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 8, 159, 159, 159, 159, 159, 51, 112, 159, 128, 128, 133, 159, 101, 20, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 9, 154, 159, 159, 159, 159, 158, 53, 109, 40, 153, 158, 109, 54, 88, 19, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 6, 144, 104, 159, 159, 159, 159, 98, 6, 5, 101, 104, 61, 151, 159, 54, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 90, 47, 104, 100, 130, 100, 5, 5, 5, 5, 22, 159, 159, 159, 66, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 89, 73, 68, 91, 5, 5, 5, 5, 5, 47, 159, 159, 159, 67, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 72, 98, 150, 148, 127, 72, 8, 5, 5, 5, 47, 159, 159, 159, 67, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 76, 159, 88, 77, 74, 88, 150, 80, 5, 5, 5, 86, 159, 159, 159, 66, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 159, 84, 5, 5, 5, 75, 93, 159, 17, 77, 13, 145, 159, 159, 159, 66, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 159, 6, 46, 5, 79, 124, 24, 159, 55, 131, 21, 159, 159, 159, 159, 114, 84, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 159, 5, 123, 85, 119, 49, 5, 159, 58, 131, 21, 159, 159, 159, 159, 150, 82, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 159, 92, 71, 119, 44, 5, 65, 159, 21, 117, 21, 159, 159, 159, 159, 152, 97, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 68, 159, 96, 95, 46, 94, 139, 81, 52, 94, 21, 159, 159, 159, 159, 159, 126, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 64, 131, 135, 127, 154, 65, 14, 35, 39, 11, 64, 64, 64, 64, 64, 43, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 44, 60, 60, 57, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
];

/// NCC floor for "the built-copy glyph is present". The census
/// calibration put genuine sightings and every confuser class
/// (badge cells, empty corners, icon arcs) on opposite sides of
/// a wide empty band; this sits inside it
/// (docs/thirty-fourth-pass.md).
pub const GLYPH_NCC_MIN: f32 = 0.55;

/// Best template NCC of the built-copy glyph over the cell's
/// badge window, searched over the offsets the calibration
/// exercised (x 7..=17, y 0..=5 around the template's nominal
/// window offset (12, 0) — the stitch's canonical crops sit at
/// the nominal offset; the range absorbs per-frame phase).
/// Mirrors [`crate::ocr::badge_ring_present`]'s structure: same
/// window, same canonical resample, so the two corner channels
/// judge identical geometry.
pub fn glyph_score(cell: &GrayImage) -> f32 {
    let win = ocr::crop_region(cell, ocr::BADGE_WINDOW);
    if win.width < 16 || win.height < 10 {
        return 0.0;
    }
    let win = ocr::resize_bilinear(&win, ocr::BADGE_WINDOW_W, ocr::BADGE_WINDOW_H);
    let (tw, th) = (26usize, 24usize);
    let tmean: f32 =
        GLYPH_TEMPLATE.iter().flatten().map(|&v| v as f32).sum::<f32>() / (tw * th) as f32;
    let tcent: Vec<f32> = GLYPH_TEMPLATE
        .iter()
        .flatten()
        .map(|&v| v as f32 - tmean)
        .collect();
    let tnorm: f32 = tcent.iter().map(|v| v * v).sum::<f32>().sqrt();
    if tnorm <= f32::EPSILON {
        return 0.0;
    }
    let (ww, wh) = (win.width as usize, win.height as usize);
    let mut best = 0.0f32;
    for oy in 0..=5usize {
        for ox in 7..=17usize {
            if ox + tw > ww || oy + th > wh {
                continue;
            }
            let mut sum = 0.0f32;
            for yy in 0..th {
                for xx in 0..tw {
                    sum += win.pixels[(oy + yy) * ww + ox + xx] as f32;
                }
            }
            let pmean = sum / (tw * th) as f32;
            let (mut num, mut pss) = (0.0f32, 0.0f32);
            for yy in 0..th {
                for xx in 0..tw {
                    let pc = win.pixels[(oy + yy) * ww + ox + xx] as f32 - pmean;
                    num += pc * tcent[yy * tw + xx];
                    pss += pc * pc;
                }
            }
            if pss <= f32::EPSILON {
                continue;
            }
            let ncc = num / (pss.sqrt() * tnorm);
            if ncc > best {
                best = ncc;
            }
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Form verdict
// ---------------------------------------------------------------------------

/// A form an item can be held in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Blueprint,
    Built,
}

/// The form channels' combined reading of one slot (pass 34,
/// brief §2). Positive evidence in both directions; a verdict
/// is only ever as strong as its channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormVerdict {
    /// At least one channel asserts blueprint and none asserts
    /// built.
    Blueprint,
    /// All three built conditions hold (mesh-negative,
    /// certified no extra line, no glyph) and no channel asserts
    /// blueprint.
    Built,
    /// Channels actively disagree (blueprint evidence AND
    /// built evidence): refuse the form, whichever reader
    /// wants it.
    Contested,
    /// No positive evidence either way: the channels abstain.
    Unknown,
}

/// The channel readings for one slot, before combination.
#[derive(Debug, Clone, Copy)]
pub struct FormChannels {
    pub mesh: MeshClass,
    /// The production mesh score (diagnostics).
    pub mesh_score: f32,
    /// The built-copy glyph is present (score >= the floor).
    pub glyph: bool,
    /// The glyph template score (diagnostics).
    pub glyph_score: f32,
    /// A detected line witnesses `Blueprint` (channel (a)
    /// positive).
    pub bp_line: bool,
    /// The line detector certifies no line went undetected
    /// (channel (a) negative, certified).
    pub no_line_certified: bool,
}

impl FormChannels {
    /// Any positive blueprint evidence.
    pub fn blueprint_evidence(&self) -> bool {
        self.bp_line || self.mesh == MeshClass::Blueprint || self.glyph
    }

    /// Full positive built evidence (brief §2: mesh-negative +
    /// certified no-third-line + no glyph). "Certified
    /// no-third-line" is the conjunction of the detector's
    /// completeness certification AND no line witnessing
    /// `Blueprint`: a cell whose Blueprint line was read and
    /// whose detection is complete is a blueprint cell with
    /// nothing missing, not a built cell — omitting the witness
    /// clause made exactly that cell read as Contested on the
    /// standard census (Wukong Prime Neuroptics Blueprint: line
    /// witnessed, detection certified, mesh in the built tail).
    pub fn built_evidence(&self) -> bool {
        self.no_line_certified
            && !self.bp_line
            && self.mesh == MeshClass::Built
            && !self.glyph
    }

    pub fn verdict(&self) -> FormVerdict {
        match (self.blueprint_evidence(), self.built_evidence()) {
            (true, true) => FormVerdict::Contested,
            (true, false) => FormVerdict::Blueprint,
            (false, true) => FormVerdict::Built,
            (false, false) => FormVerdict::Unknown,
        }
    }

    /// Does this verdict SATISFY the §2 hold rule for `form` —
    /// the condition a single-reader hold (and every line-reader
    /// hold) must meet?
    pub fn satisfies(&self, form: Form) -> bool {
        self.verdict()
            == match form {
                Form::Blueprint => FormVerdict::Blueprint,
                Form::Built => FormVerdict::Built,
            }
    }

    /// Does this verdict positively CONTRADICT `form` (the
    /// opposite form asserted, or the channels contested)? An
    /// abstention (Unknown) contradicts nothing.
    pub fn contradicts(&self, form: Form) -> bool {
        match self.verdict() {
            FormVerdict::Contested => true,
            FormVerdict::Blueprint => form == Form::Built,
            FormVerdict::Built => form == Form::Blueprint,
            FormVerdict::Unknown => false,
        }
    }
}

/// The form of a universe name, when it has a form sibling:
/// `"X Blueprint"` is the blueprint form, `"X"` with an
/// `"X Blueprint"` sibling is the built form. `None` when the
/// item has a single form in the universe (form evidence is
/// vacuous for it).
pub fn form_of(universe: &Universe, name: &str) -> Option<Form> {
    if name.ends_with(" Blueprint") && universe.form_sibling(name).is_some() {
        return Some(Form::Blueprint);
    }
    if universe.form_sibling(name).is_some() {
        return Some(Form::Built);
    }
    None
}

/// Which reader(s) an arbitrated hold came from (gate
/// attribution and the pass record).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Both readers held the item; the channels did not
    /// positively contradict its form.
    Both,
    /// Only the band reader held it, with §2 satisfied.
    Band,
    /// Only the line reader held it, with §2 satisfied.
    Line,
    /// Pass 35: neither reader held it; analysis-by-synthesis
    /// adjudicated the slot from its candidate evidence (see
    /// `crate::synth`). Synthesis never produces a hold where
    /// a reader held a different name.
    Synthesis,
}

/// The arbitration's outcome for one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arbitration {
    Hold { name: String, via: Via },
    /// No hold. `reason` is a stable label for gate attribution
    /// ("name-disagreement", "form-contradicted",
    /// "form-unproven", "line-form-unproven", "no-hold").
    Refuse { reason: &'static str },
}

/// Arbitrate one slot's two reader verdicts against its form
/// channels (pass 34, brief §3):
///
/// * Both readers hold the same item ⇒ hold, unless the
///   channels positively contradict its form (the dual-minting
///   guard: when both readers drop the same `Blueprint` line,
///   agreement is not form evidence — the mesh and the glyph
///   still get their say).
/// * The readers hold different items ⇒ refuse.
/// * Exactly one reader holds ⇒ hold only if the held form
///   SATISFIES the §2 channels (its verdict equals the form);
///   for the line reader this is also its §1 licence to hold
///   at all. An abstaining verdict (Unknown) proves nothing
///   for a lone reader, and a contradicting one refuses it.
/// * A form with no sibling in the universe is exempt: there
///   is no other form for evidence to confuse it with.
pub fn arbitrate(
    band_held: Option<&str>,
    line_held: Option<&str>,
    ch: &FormChannels,
    universe: &Universe,
) -> Arbitration {
    let satisfies = |n: &str| -> bool {
        match form_of(universe, n) {
            None => true,
            Some(f) => ch.satisfies(f),
        }
    };
    let contradicts = |n: &str| -> bool {
        match form_of(universe, n) {
            None => false,
            Some(f) => ch.contradicts(f),
        }
    };
    match (band_held, line_held) {
        (Some(b), Some(l)) if b != l => Arbitration::Refuse {
            reason: "name-disagreement",
        },
        (Some(b), Some(_)) => {
            if contradicts(b) {
                Arbitration::Refuse {
                    reason: "form-contradicted",
                }
            } else {
                Arbitration::Hold {
                    name: b.to_owned(),
                    via: Via::Both,
                }
            }
        }
        (Some(b), None) => {
            if satisfies(b) {
                Arbitration::Hold {
                    name: b.to_owned(),
                    via: Via::Band,
                }
            } else {
                Arbitration::Refuse {
                    reason: "form-unproven",
                }
            }
        }
        (None, Some(l)) => {
            if satisfies(l) {
                Arbitration::Hold {
                    name: l.to_owned(),
                    via: Via::Line,
                }
            } else {
                Arbitration::Refuse {
                    reason: "line-form-unproven",
                }
            }
        }
        (None, None) => Arbitration::Refuse { reason: "no-hold" },
    }
}

// ---------------------------------------------------------------------------
// The line reader
// ---------------------------------------------------------------------------

/// Does any line's text witness the word `Blueprint`? (Channel
/// (a), positive direction.) A line token within confusable
/// distance 2 of "blueprint" and at least 5 chars long — the
/// witnessed-token standard the radius decoder's tail alignment
/// uses, applied to a whole line: a 9-char word is not witnessed
/// by a 3-char smudge that happens to sit near a prefix of it.
pub fn blueprint_line_witnessed(lines: &[String]) -> bool {
    lines.iter().any(|line| {
        line.split_whitespace().any(|tok| {
            let t: String = tok
                .chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect();
            t.len() >= 5 && ocr::levenshtein_confusable(&t, "blueprint") <= 2
        })
    })
}

/// One cell's line-reader outcome: the validated result of the
/// assembled read plus the channel-(a) evidence its detection
/// produced. The result went through the shared matching stack
/// and hold predicate; form gating is NOT applied here — it is
/// the arbitration's job, because channel (a)'s other half
/// (certification) and channels (b)/(c) are slot-level.
#[derive(Debug, Clone)]
pub struct LineOutcome {
    pub result: CellResult,
    /// Per-line OCR texts, top to bottom.
    pub lines: Vec<String>,
    /// The lines joined as one read (what the matcher saw).
    pub assembled: String,
    /// A detected line witnesses `Blueprint`.
    pub bp_line: bool,
    /// The detector certifies no line went undetected.
    pub certified: bool,
    /// The detection itself (diagnostics).
    pub detection: LineDetection,
}

impl LineOutcome {
    pub fn empty(index: usize) -> Self {
        Self {
            result: CellResult {
                index,
                raw_text: String::new(),
                confidence: 0.0,
                matched: None,
                match_score: 0.0,
                held: false,
                count: None,
                tier: ocr::RefinementTier::Unknown,
                rank: None,
                gate_excluded: false,
                refine_excluded: false,
            },
            lines: Vec::new(),
            assembled: String::new(),
            bp_line: false,
            certified: false,
            detection: LineDetection {
                boxes: Vec::new(),
                block: None,
                total_mass: 0.0,
                leftover_run_frac: 0.0,
                shadow_below_frac: 0.0,
            },
        }
    }
}

/// Validate one assembled line read through the shared stack:
/// the margin matcher first (via [`ocr::validate_read`]), then
/// the radius decoder's band phase on the assembled text, then
/// its strip phase with line 1 in the strip's role (the line
/// reader's line 1 IS its strip — detected, not fixed-geometry,
/// which is the reader's answer to the strip's phase problem).
/// Every acceptance passes through
/// `Universe::hold_permitted` inside those paths, exactly as
/// the band reader's do. This is the line reader's text-level
/// hold path — the property battery exercises it directly.
#[allow(clippy::too_many_arguments)]
pub fn validate_line_read(
    read: &OcrRead,
    line1: &str,
    universe: &Universe,
    min_confidence: f32,
    min_score: f32,
    index: usize,
    radius_decoder: bool,
    radius_strip: bool,
    radius_tail_budget: usize,
) -> CellResult {
    let mut result = ocr::validate_read(read, universe, min_confidence, min_score, index);
    if result.held || !radius_decoder || read.text.trim().is_empty() {
        return result;
    }
    if let Some(name) = universe.radius_decode_budget(&read.text, radius_tail_budget) {
        result.matched = Some(name);
        result.held = true;
        result.count = read.count;
        return result;
    }
    if radius_strip && !line1.trim().is_empty() {
        if let Some(name) = universe.radius_decode_strip(&read.text, line1, radius_tail_budget) {
            result.matched = Some(name);
            result.held = true;
            result.count = read.count;
        }
    }
    result
}

/// Read a set of already-cut cells with the line reader: detect
/// each cell's lines on its name band, OCR the ORIGINAL band
/// pixels inside the line boxes (PSM 7, one batched engine
/// call), assemble, and validate through the shared stack.
/// `cells` are the same crops the band reader reads — in the
/// stitch path, the slot's canonical crop and its usable
/// instances — so the two readers judge identical pixels and
/// differ only in presentation.
pub fn read_cells_lines(
    cells: &[Option<GrayImage>],
    layout: &Layout,
    engine: &dyn OcrEngine,
    opts: &ReadOptions,
    universe: &Universe,
) -> Vec<LineOutcome> {
    // Detect lines per cell and gather every line crop into one
    // batch (the engine serves region reads batched + cached).
    // The bands are kept alive beside the requests because the
    // line crops are cut from them.
    let mut detections: Vec<Option<LineDetection>> = Vec::with_capacity(cells.len());
    let mut bands: Vec<Option<GrayImage>> = Vec::with_capacity(cells.len());
    let mut req_map: Vec<(usize, usize)> = Vec::new(); // (cell, line)
    for (ci, cell) in cells.iter().enumerate() {
        let Some(cell) = cell else {
            detections.push(None);
            bands.push(None);
            continue;
        };
        let band = name_band(cell, layout);
        let det = detect_lines(&band);
        for li in 0..det.boxes.len() {
            req_map.push((ci, li));
        }
        detections.push(Some(det));
        bands.push(Some(band));
    }
    // Build the batch against the stored bands.
    let mut crops: Vec<GrayImage> = Vec::with_capacity(req_map.len());
    for &(ci, li) in &req_map {
        let band = bands[ci].as_ref().expect("band for requested cell");
        let det = detections[ci].as_ref().expect("detection");
        let b = det.boxes[li];
        crops.push(band.crop(b.x0, b.y0, (b.x1 - b.x0).max(1), (b.y1 - b.y0).max(1)));
    }
    let reqs: Vec<(&GrayImage, [f32; 4], u32, u32)> = crops
        .iter()
        .map(|c| (c, [0.0, 0.0, 1.0, 1.0], 7, 4))
        .collect();
    let texts = engine.read_text_batch(&reqs);
    let mut line_texts: Vec<Vec<String>> = vec![Vec::new(); cells.len()];
    for (ri, &(ci, _)) in req_map.iter().enumerate() {
        line_texts[ci].push(texts.get(ri).cloned().unwrap_or_default());
    }
    let mut out = Vec::with_capacity(cells.len());
    for ci in 0..cells.len() {
        let Some(det) = detections[ci].take() else {
            out.push(LineOutcome::empty(ci));
            continue;
        };
        let lines: Vec<String> = line_texts[ci]
            .iter()
            .map(|t| t.trim().to_owned())
            .collect();
        let assembled = lines
            .iter()
            .filter(|t| !t.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        let read = OcrRead {
            confidence: if assembled.is_empty() { 0.0 } else { 0.85 },
            text: assembled.clone(),
            count: None,
            rank: None,
        };
        let line1 = lines.first().cloned().unwrap_or_default();
        let result = validate_line_read(
            &read,
            &line1,
            universe,
            opts.min_confidence,
            layout.min_score,
            ci,
            opts.radius_decoder,
            opts.radius_strip,
            opts.radius_tail_budget,
        );
        out.push(LineOutcome {
            bp_line: blueprint_line_witnessed(&lines),
            certified: det.certifies_complete(),
            result,
            lines,
            assembled,
            detection: det,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::{Layout, Universe};

    /// A synthetic name band (182 wide) with text-like lines at
    /// the given top rows on a dark ground. Each "line" is three
    /// 2px horizontal strokes (glyph strokes are thinner than
    /// the top-hat's r=3 disk; a solid block would survive the
    /// opening and vanish from the residual) with word gaps.
    fn band_with_bars(bars: &[(u32, u32)], height: u32) -> GrayImage {
        let mut img = GrayImage::blank(182, height, 18);
        for &(y0, _) in bars {
            for sy in [y0 + 1, y0 + 5, y0 + 9] {
                for y in sy..sy + 2 {
                    if y >= height {
                        break;
                    }
                    let mut x = 10u32;
                    while x < 168 {
                        for dx in 0..11u32 {
                            img.set(x + dx, y, 235);
                        }
                        x += 17;
                    }
                }
            }
        }
        img
    }

    #[test]
    fn top_hat_keeps_text_and_drops_ground() {
        let img = band_with_bars(&[(10, 22)], 85);
        let th = top_hat(&img);
        assert!(th.get(15, 12) > 150, "stroke survives: {}", th.get(15, 12));
        assert!(th.get(15, 60) < 30, "ground removed: {}", th.get(15, 60));
    }

    #[test]
    fn detect_lines_counts_and_certifies_clean_bands() {
        // Two lines at production pitch in an 85px band.
        let band = band_with_bars(&[(10, 22), (31, 43)], 85);
        let det = detect_lines(&band);
        assert_eq!(det.boxes.len(), 2, "boxes: {:?}", det.boxes);
        assert!(
            det.certifies_complete(),
            "clean 2-line band certifies (leftover {:.3}, shadow {:.3})",
            det.leftover_run_frac, det.shadow_below_frac
        );
        // Three lines.
        let band3 = band_with_bars(&[(4, 15), (23, 34), (42, 53)], 85);
        let det3 = detect_lines(&band3);
        assert_eq!(det3.boxes.len(), 3, "boxes: {:?}", det3.boxes);
        assert!(det3.certifies_complete());
        // One line.
        let band1 = band_with_bars(&[(20, 33)], 85);
        assert_eq!(detect_lines(&band1).boxes.len(), 1);
    }

    #[test]
    fn detect_lines_refuses_certification_when_a_run_is_unaccounted() {
        // Two lines high in the band and a third at the very
        // bottom: the span of all three exceeds the plausible
        // block height, the block falls back to the strongest
        // run, and the unaccounted runs must refuse
        // certification (the dropped-line guard).
        let band = band_with_bars(&[(2, 14), (21, 33), (73, 85)], 85);
        let det = detect_lines(&band);
        assert!(
            !det.certifies_complete(),
            "leftover {:.3} / shadow {:.3} must refuse certification",
            det.leftover_run_frac, det.shadow_below_frac
        );
        assert!(
            det.leftover_run_frac > 0.05 || det.shadow_below_frac > 0.20,
            "the unaccounted mass must be the reason (leftover {:.3}, shadow {:.3})",
            det.leftover_run_frac, det.shadow_below_frac
        );
    }

    #[test]
    fn mesh_score_separates_grid_from_smooth() {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let layouts = Layout::load_file(&base.join("data/capture/layouts.toml"))
            .expect("layouts load");
        let layout = layouts.get("parts").expect("parts layout");
        let mut grid = GrayImage::blank(182, 181, 90);
        let mut smooth = GrayImage::blank(182, 181, 90);
        // Grid texture inside the band margins only.
        for y in 100..178u32 {
            for x in 0..182u32 {
                if x < 17 || x >= 165 {
                    grid.set(x, y, if (x / 2 + y / 2) % 2 == 0 { 140 } else { 50 });
                    smooth.set(x, y, 95);
                }
            }
        }
        let g = mesh_scores(&grid, layout).margin_grad;
        let s = mesh_scores(&smooth, layout).margin_grad;
        assert!(g > s + 2.0, "grid {g:.2} must exceed smooth {s:.2}");
    }

    #[test]
    fn glyph_score_is_zero_on_a_blank_cell() {
        assert_eq!(glyph_score(&GrayImage::blank(182, 181, 40)), 0.0);
    }

    #[test]
    fn blueprint_line_witness_matching() {
        let w = |s: &str| vec![s.to_string()];
        assert!(blueprint_line_witnessed(&w("Blueprint")));
        assert!(blueprint_line_witnessed(&w("Prime Biueprint")));
        assert!(blueprint_line_witnessed(&w("Neuroptics Blueprint")));
        assert!(!blueprint_line_witnessed(&w("Neuroptics")));
        assert!(!blueprint_line_witnessed(&w("Blu")));
        assert!(!blueprint_line_witnessed(&w("Systems Chassis")));
    }

    fn test_universe() -> Universe {
        Universe::from_names(&[
            "Mag Prime Systems".to_string(),
            "Mag Prime Systems Blueprint".to_string(),
            "Loki Prime Systems".to_string(),
            "Loki Prime Systems Blueprint".to_string(),
            "Paris Prime String".to_string(),
        ])
    }

    fn channels(mesh: MeshClass, glyph: bool, bp_line: bool, certified: bool) -> FormChannels {
        FormChannels {
            mesh,
            mesh_score: 0.0,
            glyph,
            glyph_score: 0.0,
            bp_line,
            no_line_certified: certified,
        }
    }

    #[test]
    fn form_verdicts_follow_the_channel_rules() {
        // A witnessed Blueprint line is positive blueprint
        // evidence on its own.
        let ch = channels(MeshClass::Unknown, false, true, false);
        assert_eq!(ch.verdict(), FormVerdict::Blueprint);
        // Built needs the full conjunction: certified no third
        // line, mesh in the built tail, no glyph.
        let ch = channels(MeshClass::Built, false, false, true);
        assert_eq!(ch.verdict(), FormVerdict::Built);
        // Missing certification: no built verdict.
        let ch = channels(MeshClass::Built, false, false, false);
        assert_eq!(ch.verdict(), FormVerdict::Unknown);
        // The glyph defeats a built reading (it only appears on
        // blueprint tiles) and asserts blueprint itself.
        let ch = channels(MeshClass::Built, true, false, true);
        assert_eq!(ch.verdict(), FormVerdict::Blueprint);
        // A witnessed blueprint line defeats the built
        // conjunction even with certified detection and a
        // built-tail mesh (the Wukong calibration case).
        let ch = channels(MeshClass::Built, false, true, true);
        assert_eq!(ch.verdict(), FormVerdict::Blueprint);
    }

    #[test]
    fn arbitration_table() {
        let u = test_universe();
        let neutral = channels(MeshClass::Unknown, false, false, false);
        let built = channels(MeshClass::Built, false, false, true);
        let bp = channels(MeshClass::Unknown, false, true, false);
        // Both readers, same name, no contradiction: hold.
        assert!(matches!(
            arbitrate(Some("Mag Prime Systems"), Some("Mag Prime Systems"), &neutral, &u),
            Arbitration::Hold { via: Via::Both, .. }
        ));
        // Both agree on a blueprint the channels call built:
        // the form channel contradicts — refuse.
        assert!(matches!(
            arbitrate(
                Some("Mag Prime Systems Blueprint"),
                Some("Mag Prime Systems Blueprint"),
                &built,
                &u
            ),
            Arbitration::Refuse { reason: "form-contradicted" }
        ));
        // Different names: refuse (the Panthera mechanism).
        assert!(matches!(
            arbitrate(Some("Mag Prime Systems"), Some("Loki Prime Systems"), &neutral, &u),
            Arbitration::Refuse { reason: "name-disagreement" }
        ));
        // A lone reader with its form satisfied: hold.
        assert!(matches!(
            arbitrate(Some("Loki Prime Systems"), None, &built, &u),
            Arbitration::Hold { via: Via::Band, .. }
        ));
        assert!(matches!(
            arbitrate(None, Some("Loki Prime Systems Blueprint"), &bp, &u),
            Arbitration::Hold { via: Via::Line, .. }
        ));
        // A lone reader without form satisfaction: refuse.
        assert!(matches!(
            arbitrate(Some("Loki Prime Systems"), None, &neutral, &u),
            Arbitration::Refuse { reason: "form-unproven" }
        ));
        assert!(matches!(
            arbitrate(None, Some("Loki Prime Systems"), &neutral, &u),
            Arbitration::Refuse { reason: "line-form-unproven" }
        ));
        // Neither reader: refuse.
        assert!(matches!(
            arbitrate(None, None, &neutral, &u),
            Arbitration::Refuse { reason: "no-hold" }
        ));
        // A name with no form sibling is exempt from form rules.
        assert!(matches!(
            arbitrate(Some("Paris Prime String"), None, &neutral, &u),
            Arbitration::Hold { via: Via::Band, .. }
        ));
    }

    #[test]
    fn line_read_chain_holds_clean_names_and_refuses_fragments() {
        let u = test_universe();
        let clean = OcrRead {
            text: "Mag Prime Systems Blueprint".to_string(),
            confidence: 1.0,
            count: None,
            rank: None,
        };
        let r = validate_line_read(
            &clean,
            "Mag Prime Systems Blueprint",
            &u,
            0.5,
            0.5,
            0,
            true,
            true,
            crate::ocr::RADIUS_TAIL_BUDGET,
        );
        assert!(r.held && r.matched.as_deref() == Some("Mag Prime Systems Blueprint"));
        // A two-token fragment is below the evidence floor /
        // predicate: it must not hold anything.
        let frag = OcrRead {
            text: "Mag Prime".to_string(),
            confidence: 1.0,
            count: None,
            rank: None,
        };
        let r = validate_line_read(
            &frag,
            "Mag Prime",
            &u,
            0.5,
            0.5,
            0,
            true,
            true,
            crate::ocr::RADIUS_TAIL_BUDGET,
        );
        assert!(!r.held, "fragment held: {:?}", r.matched);
    }
}
