//! Session stitch for the parts screen (pass 33, addendum #96).
//!
//! The production grid was calibrated at a row pitch of 180.65 px
//! (1440p) while the game renders rows at 185.0 px — a 4.35 px/row
//! error that accumulates with depth, so a frame's lower rows are
//! cut progressively further from their tiles, and a frame caught
//! mid-scroll (phase offset by tens of px) is cut across two items
//! at once and reads nothing (the census's dead frames). Pass 33
//! stops reading parts frames one at a time: a capture session's
//! parts frames are accumulated, FIT as a session (row and column
//! pitch separately), LINKED into a panorama by measured content
//! displacement, and read once per panorama slot from a canonical
//! crop built across the slot's instances.
//!
//! The pieces, in the order [`plan_session`] runs them:
//!
//! 1. **Axis fits.** Each frame's gradient profile is folded at
//!    candidate pitches; the pitch maximising fold concentration
//!    (sum of squared bin mass) wins. Measured on the censuses: the
//!    concentration at the true 185.0 px is ~10% above the value at
//!    the layout's 180.65. A frame whose own fit deviates from the
//!    running segment fit by more than [`SEGMENT_PITCH_BREAK`]
//!    closes the segment (a UI-scale/resize event) and starts a new
//!    one with its own fit.
//! 2. **Absolute phase.** The folded profile's dominant peak is
//!    the name text's main gradient line, at cell fraction
//!    [`FOLD_PEAK_FRAC`] — measured at bin 149/185 on standard and
//!    HC census frames alike. A frame's phase is its peak's offset
//!    from that fraction, wrapped to (−P/2, P/2]. This is what
//!    makes a displaced frame (parts-11: +48.3 px) cuttable at all.
//! 3. **Linking.** Consecutive frames link by content: the row
//!    profiles nominate integer-row displacements, a 2-D NCC over
//!    the grid overlap verifies and refines each to sub-pixel.
//!    Candidate displacements within [`LINK_ALIAS_NCC`] of the best
//!    are aliases (±1 pitch); the scrollbar thumb — an independent
//!    absolute reference — picks among aliases when it can.
//! 4. **Loop closure.** Wherever two non-adjacent frames share
//!    rows, the direct link must match the chained displacement
//!    within [`LOOP_CLOSE_TOL`] or the segment's link graph goes
//!    to a global least-squares solve; a frame whose incident
//!    residuals stay above tolerance after the solve is ejected
//!    into a segment of its own rather than placed by force.
//! 5. **Slots.** A frame's lattice row r sits at panorama row
//!    `round((g − φ_anchor)/P) + r`, where g = D + φ is the
//!    frame's lattice offset. Every (segment, row, col) the
//!    session's viewports presented is a slot — including the
//!    partial lattice row a positive phase pushes into the
//!    viewport's top edge; [`slot_instances`] enumerates them all,
//!    usable instances separated from partial sightings — the
//!    enumeration the coverage accounting is built on.
//!
//! Nothing here OCRs or validates; `capture` drives the canonical
//! crops through the same `read_cells` path a single frame takes.

use crate::ocr::{GrayImage, Layout};
use std::collections::BTreeMap;

/// Phase bins in a fold. Fixed so the bin grid is independent of
/// the pitch being tested.
const FOLD_BINS: usize = 256;
/// Cell fraction of the folded profile's dominant peak (the name
/// text's main gradient line): bin 149 at pitch 185 on the census
/// frames of both themes.
const FOLD_PEAK_FRAC: f32 = 0.8054;
/// A frame whose fitted row pitch deviates from its segment's fit
/// by more than this (page px) starts a new segment (brief §2.1).
const SEGMENT_PITCH_BREAK: f32 = 0.3;
/// NCC floor for accepting a content link between two frames.
/// Twin content at the right displacement measures 0.98+ on full
/// cells in the prototype; a whole-grid strip (background and tile
/// texture included) sits lower, and unrelated frames far below.
const LINK_NCC_MIN: f32 = 0.55;
/// Two candidate displacements whose verified NCCs sit within this
/// band are aliases of each other; the scrollbar prior decides.
const LINK_ALIAS_NCC: f32 = 0.02;
/// Loop-closure tolerance (px): chained vs direct displacement.
const LOOP_CLOSE_TOL: f32 = 1.0;
/// An instance is usable for a canonical crop only with at least
/// this much viewport margin (px) on every side.
const MIN_INSTANCE_MARGIN: f32 = 2.0;
/// Gain-check band (pass 33, reviewer Q2): an instance whose mean
/// luma sits outside this ratio of the pool's median mean is a
/// transient (hover glow, mid-fade opacity) and leaves the pool.
const GAIN_LO: f32 = 0.82;
const GAIN_HI: f32 = 1.22;
/// Instance disagreement threshold: mean absolute difference from
/// the canonical crop above this flags the slot (reported, never
/// silently resolved).
const DISAGREE_MAD: f32 = 22.0;

/// Fitted pitches for one segment, in page px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisFit {
    pub row_pitch: f32,
    pub col_pitch: f32,
}

/// One frame's placement inside its segment.
#[derive(Debug, Clone)]
pub struct FramePlacement {
    /// Index into the session's frame list.
    pub frame: usize,
    /// Absolute phase φ (page px, signed, wrapped to (−P/2, P/2]):
    /// the frame's row lattice sits at `grid_y0 + φ + r·P` in its
    /// own page.
    pub phase: f32,
    /// The frame's lattice offset in panorama coordinates:
    /// `g = D + φ`, where D is the frame's chained scroll
    /// displacement from the segment anchor (D = 0 at the anchor,
    /// so the anchor's g is its φ). The invariant the slot mapping
    /// relies on: `g − φ_anchor` is an integer multiple of the
    /// pitch (up to measurement error), because scroll deltas and
    /// wrapped-phase deltas compose into whole rows.
    pub g: f32,
    /// Worst incident link residual after any solve (px).
    pub residual: f32,
    /// Scrollbar thumb fraction for this frame, when readable.
    pub thumb: Option<f32>,
}

/// One stitch segment: frames linked under one axis fit.
#[derive(Debug, Clone)]
pub struct Segment {
    pub pitch: AxisFit,
    pub frames: Vec<FramePlacement>,
    /// True when this segment was started because the previous
    /// frame would not link (or was ejected): its frames are read
    /// on their own fit — the brief's per-frame fallback, flagged.
    pub relinked: bool,
}

/// The session's stitch plan plus the diagnostics the coverage
/// accounting and the pass gate report.
#[derive(Debug, Clone, Default)]
pub struct StitchPlan {
    pub segments: Vec<Segment>,
    /// Session row pitch (median of the per-frame fits).
    pub session_row_pitch: f32,
    /// Session column pitch (joint fit over all frames).
    pub session_col_pitch: f32,
    /// Per-frame fitted row pitches, parallel to the frame list.
    pub frame_pitches: Vec<f32>,
    /// Per-frame: did the fold produce a real fit? A frame with no
    /// measurable periodic signal (blank screen, synthetic test
    /// frame) is unfitted: it keeps the layout pitch and phase 0,
    /// so degenerate input degrades to the pre-stitch geometry
    /// exactly instead of to a confident wrong fit.
    pub frame_fitted: Vec<bool>,
    /// Non-adjacent pairs whose direct link contradicted the chain
    /// by more than [`LOOP_CLOSE_TOL`] (each ran the graph solve).
    pub loop_closure_failures: usize,
    /// Whether the global least-squares solve ran for any segment.
    pub solve_ran: bool,
    /// Frames ejected from their segment after the solve.
    pub ejected: usize,
    /// Scrollbar affine-fit outliers (|residual| > pitch/2) —
    /// diagnostic only; recorded, never silently acted on.
    pub scrollbar_outliers: usize,
}

/// The grid rectangle in page px for a frame of this size.
#[derive(Debug, Clone, Copy)]
pub struct GridRect {
    pub x0: f32,
    pub y0: f32,
    pub w: f32,
    pub h: f32,
    pub sx: f32,
    pub sy: f32,
}

impl GridRect {
    pub fn of(layout: &Layout, frame: &GrayImage) -> Self {
        let sx = frame.width as f32 / layout.ref_width.max(1) as f32;
        let sy = frame.height as f32 / layout.ref_height.max(1) as f32;
        Self {
            x0: layout.x0 * sx,
            y0: layout.y0 * sy,
            w: layout.cell_w * sx * layout.cols as f32,
            h: layout.cell_h * sy * layout.rows as f32,
            sx,
            sy,
        }
    }
    /// Layout row pitch in page px (the miscalibrated constant the
    /// session fit replaces).
    pub fn layout_row_pitch(&self, layout: &Layout) -> f32 {
        layout.cell_h * self.sy
    }
    pub fn layout_col_pitch(&self, layout: &Layout) -> f32 {
        layout.cell_w * self.sx
    }
}

/// Vertical-gradient row profile over the grid rect: one value per
/// page row (0 outside the rect). Horizontal edges — tile borders,
/// text lines — are what repeat at the row pitch.
fn row_profile(frame: &GrayImage, rect: &GridRect) -> Vec<f32> {
    let mut out = vec![0.0f32; frame.height as usize];
    let (x0, x1) = (rect.x0.max(0.0) as u32, ((rect.x0 + rect.w) as u32).min(frame.width));
    let (y0, y1) = (rect.y0.max(1.0) as u32, ((rect.y0 + rect.h) as u32).min(frame.height));
    if x1 <= x0 || y1 <= y0 {
        return out;
    }
    for y in y0..y1 {
        let mut sum = 0.0f32;
        for x in x0..x1 {
            // The gradient between rows y and y+1, stored at y:
            // the fold's phase origin then matches the calibration
            // measurement exactly (a diff-index profile), with no
            // one-row bias in every fitted phase.
            let below = if y + 1 < frame.height {
                frame.get(x, y + 1)
            } else {
                frame.get(x, y)
            };
            sum += (below as i32 - frame.get(x, y) as i32).unsigned_abs() as f32;
        }
        out[y as usize] = sum / (x1 - x0) as f32;
    }
    out
}

/// Horizontal-gradient column profile over the grid rect.
fn col_profile(frame: &GrayImage, rect: &GridRect) -> Vec<f32> {
    let mut out = vec![0.0f32; frame.width as usize];
    let (x0, x1) = (rect.x0.max(1.0) as u32, ((rect.x0 + rect.w) as u32).min(frame.width));
    let (y0, y1) = (rect.y0.max(0.0) as u32, ((rect.y0 + rect.h) as u32).min(frame.height));
    if x1 <= x0 || y1 <= y0 {
        return out;
    }
    for x in x0..x1 {
        let mut sum = 0.0f32;
        for y in y0..y1 {
            sum += (frame.get(x, y) as i32 - frame.get(x - 1, y) as i32).unsigned_abs() as f32;
        }
        out[x as usize] = sum / (y1 - y0) as f32;
    }
    out
}

/// Fold a profile segment into [`FOLD_BINS`] phase bins at
/// `pitch`, over an extent of `periods` pitches from `start`.
/// The extent matters (pass 33, measured): it stops at
/// (rows − 0.07) periods — just short of the final period's
/// bottom-edge zone (frac ≈0.97 of the cell) while including its
/// text line (frac ≈0.81). At a full-rows extent the bottom
/// edge earns as many fold contributions as the text line and
/// outvotes it on weak-text frames (frame-parts-11's phase came
/// out 19px low), because the calibration balance — three text
/// lines against two bottom edges — no longer holds.
fn fold(profile: &[f32], start: usize, pitch: f32, periods: f32) -> [f32; FOLD_BINS] {
    let mut bins = [0.0f32; FOLD_BINS];
    if pitch <= 0.0 {
        return bins;
    }
    let limit = start + (periods * pitch).round() as usize;
    for (i, &v) in profile.iter().enumerate().skip(start).take(limit.saturating_sub(start)) {
        if v == 0.0 {
            continue;
        }
        let pos = (i - start) as f32 / pitch;
        let frac = pos - pos.floor();
        let b = ((frac * FOLD_BINS as f32) as usize).min(FOLD_BINS - 1);
        bins[b] += v;
    }
    bins
}

/// Fold concentration: sum of squared bin mass. A correct pitch
/// stacks every row's edges into the same bins; a wrong one smears
/// them flat.
fn fold_concentration(profile: &[f32], start: usize, pitch: f32, periods: f32) -> f32 {
    fold(profile, start, pitch, periods).iter().map(|b| b * b).sum()
}

/// Fit a pitch near `nominal` (page px) for one profile by fold
/// concentration: coarse scan, then parabolic refinement.
fn fit_pitch(profile: &[f32], start: usize, nominal: f32, periods: f32) -> f32 {
    let (lo, hi) = (nominal * 0.94, nominal * 1.06);
    let mut best = (f32::MIN, nominal);
    let mut p = lo;
    while p <= hi {
        let c = fold_concentration(profile, start, p, periods);
        if c > best.0 {
            best = (c, p);
        }
        p += 0.2;
    }
    // Parabolic refinement on the coarse winner.
    let step = 0.2f32;
    let (c0, cm, cp) = (
        fold_concentration(profile, start, best.1, periods),
        fold_concentration(profile, start, best.1 - step, periods),
        fold_concentration(profile, start, best.1 + step, periods),
    );
    let denom = cm - 2.0 * c0 + cp;
    if denom.abs() > f32::EPSILON {
        let shift = 0.5 * (cm - cp) / denom;
        if shift.abs() <= 1.0 {
            return best.1 + shift * step;
        }
    }
    best.1
}

/// A profile's phase at `pitch`: the folded peak's offset from
/// [`FOLD_PEAK_FRAC`], wrapped to (−P/2, P/2]. Sub-bin parabolic.
fn fit_phase(profile: &[f32], start: usize, pitch: f32, periods: f32) -> f32 {
    let bins = fold(profile, start, pitch, periods);
    // Raw argmax + parabolic sub-bin refinement — the exact
    // estimator the FOLD_PEAK_FRAC calibration measured (bin
    // 149/185 on the census frames). No smoothing: the fold sums
    // hundreds of profile rows, and smoothing a two-spike feature
    // biases the peak the calibration relies on.
    let at = |i: isize| -> f32 {
        let n = FOLD_BINS as isize;
        bins[(((i % n) + n) % n) as usize]
    };
    let mut arg = 0isize;
    let mut top = f32::MIN;
    for i in 0..FOLD_BINS as isize {
        let v = at(i);
        if v > top {
            top = v;
            arg = i;
        }
    }
    let (ym, y0, yp) = (at(arg - 1), at(arg), at(arg + 1));
    let denom = ym - 2.0 * y0 + yp;
    let sub = if denom.abs() > f32::EPSILON {
        (0.5 * (ym - yp) / denom).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let peak_px = ((arg as f32 + sub) / FOLD_BINS as f32) * pitch;
    let mut phase = peak_px - FOLD_PEAK_FRAC * pitch;
    while phase > pitch / 2.0 {
        phase -= pitch;
    }
    while phase <= -pitch / 2.0 {
        phase += pitch;
    }
    phase
}

/// Scrollbar thumb fraction (0..1 down the track) for one frame,
/// or `None` when no plausible thumb is present. The scrollbar
/// sits just right of the grid; the thumb is the brightest
/// contiguous run in that band. A weak, independent signal — used
/// only to prefer among alias link candidates and reported as a
/// diagnostic, never a placement input on its own.
fn thumb_fraction(frame: &GrayImage, rect: &GridRect) -> Option<f32> {
    let x0 = ((rect.x0 + rect.w + 4.0 * rect.sx) as u32).min(frame.width);
    let x1 = ((rect.x0 + rect.w + 44.0 * rect.sx) as u32).min(frame.width);
    let y0 = (rect.y0.max(0.0) as u32).min(frame.height);
    let y1 = ((rect.y0 + rect.h) as u32).min(frame.height);
    if x1 <= x0 || y1 <= y0 + 8 {
        return None;
    }
    let rows = (y1 - y0) as usize;
    let mut prof = vec![0.0f32; rows];
    for (i, y) in (y0..y1).enumerate() {
        let mut m = 0u32;
        for x in x0..x1 {
            m = m.max(frame.get(x, y) as u32);
        }
        prof[i] = m as f32;
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &v in &prof {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if hi - lo < 24.0 {
        return None;
    }
    let thr = lo + 0.55 * (hi - lo);
    // Longest run above threshold; its centre is the thumb centre.
    let (mut best, mut run) = ((0usize, 0usize), 0usize);
    for i in 0..rows {
        if prof[i] >= thr {
            run += 1;
            if run > best.1 - best.0 {
                best = (i + 1 - run, i + 1);
            }
        } else {
            run = 0;
        }
    }
    let len = best.1 - best.0;
    if len < 6 || len > rows / 2 {
        return None;
    }
    Some((best.0 + best.1) as f32 / 2.0 / rows as f32)
}

/// NCC between frame `a`'s grid rect and frame `b`'s grid rect
/// under the hypothesis that b's content is a's content displaced
/// by `dy` panorama px (b(y) ≈ a(y − dy)); stride-2 sampling over
/// the overlap. Returns 0 when the overlap is too small to judge.
fn grid_ncc(a: &GrayImage, b: &GrayImage, rect: &GridRect, dy: f32) -> f32 {
    grid_ncc_strided(a, b, rect, dy, 2)
}

fn grid_ncc_strided(a: &GrayImage, b: &GrayImage, rect: &GridRect, dy: f32, stride: usize) -> f32 {
    let x0 = rect.x0.max(0.0) as i32;
    let x1 = ((rect.x0 + rect.w) as i32).min(a.width as i32).min(b.width as i32);
    let ya0 = rect.y0.max(0.0) as i32;
    let ya1 = ((rect.y0 + rect.h) as i32).min(a.height as i32);
    // Paired rows: y in a, y-dy in b, both inside b's grid span.
    let yb_lo = rect.y0.max(0.0) as i32;
    let yb_hi = ((rect.y0 + rect.h) as i32).min(b.height as i32);
    let lo = ya0.max(yb_lo + dy.round() as i32);
    let hi = ya1.min(yb_hi + dy.round() as i32);
    if x1 - x0 < 16 || hi - lo < 24 {
        return 0.0;
    }
    let dyi = dy.round() as i32;
    let stride = stride.max(1) as i32;
    let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut y = lo;
    while y < hi {
        let mut x = x0;
        while x < x1 {
            let va = a.get(x as u32, y as u32) as f64;
            let vb = b.get(x as u32, (y - dyi) as u32) as f64;
            sa += va;
            sb += vb;
            saa += va * va;
            sbb += vb * vb;
            sab += va * vb;
            n += 1.0;
            x += stride;
        }
        y += stride;
    }
    if n < 64.0 {
        return 0.0;
    }
    let cov = sab - sa * sb / n;
    let va = saa - sa * sa / n;
    let vb = sbb - sb * sb / n;
    if va <= 0.0 || vb <= 0.0 {
        return 0.0;
    }
    (cov / (va.sqrt() * vb.sqrt())) as f32
}

/// A verified content link between two frames.
#[derive(Debug, Clone, Copy)]
pub struct Link {
    /// Panorama displacement: `g_b − g_a` (px).
    pub dy: f32,
    pub ncc: f32,
}

/// Measure the content displacement from frame `a` to frame `b`.
/// Candidates come from a coarse grid-NCC scan (stride 4, 2px
/// steps over ±4 pitches): the row-profile correlation an
/// earlier version pre-filtered with cannot rank candidates —
/// gradient profiles are periodic by construction (every row has
/// the same edge structure), so the true displacement routinely
/// ranked below the top-3 cut and links failed on ordinary
/// one-row scrolls (pass 33 gate: 11 segments for 15 census
/// frames). NCC runs on gradient-magnitude images, not raw
/// pixels: uniform tile backgrounds dominate raw-pixel NCC and
/// are periodic at the row pitch, so on the high-contrast theme
/// a wrong-row alignment verified on tile structure alone (HC
/// gate: a link aliased by three rows). In the gradient image
/// the uniform areas vanish and text/icon edges — the content
/// that distinguishes one row from another — carry the
/// correlation. Peaks are verified at stride 2 and refined to
/// sub-pixel by parabola. Among verified candidates within
/// [`LINK_ALIAS_NCC`] of the best, the one closest to
/// `scroll_hint` (the scrollbar prediction, when the caller has
/// one) wins — smallest-|offset| consistency is the tie-break
/// after that (brief §2.1).
fn measure_link(
    a: &GrayImage,
    b: &GrayImage,
    rect: &GridRect,
    _pa: &[f32],
    _pb: &[f32],
    _phase_a: f32,
    _phase_b: f32,
    pitch: f32,
    scroll_hint: Option<f32>,
) -> Option<Link> {
    // Coarse scan: local maxima of stride-4 NCC, best first,
    // separated by at least 8px so one peak yields one candidate.
    let mut scan: Vec<(f32, f32)> = Vec::new(); // (dy, ncc4)
    let mut d = -4.0 * pitch;
    while d <= 4.0 * pitch {
        scan.push((d, grid_ncc_strided(a, b, rect, d, 4)));
        d += 2.0;
    }
    let mut peaks: Vec<(f32, f32)> = Vec::new();
    for i in 0..scan.len() {
        let (dy, v) = scan[i];
        if v < 0.30 {
            continue;
        }
        let left = if i > 0 { scan[i - 1].1 } else { f32::MIN };
        let right = if i + 1 < scan.len() { scan[i + 1].1 } else { f32::MIN };
        if v >= left && v >= right {
            peaks.push((dy, v));
        }
    }
    peaks.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut cands: Vec<(f32, f32)> = Vec::new();
    for p in peaks {
        if cands.iter().all(|(cd, _)| (cd - p.0).abs() >= 8.0) {
            cands.push(p);
        }
        if cands.len() >= 4 {
            break;
        }
    }
    if cands.is_empty() {
        return None;
    }
    // Verify each candidate at integer offsets around it; keep the
    // best NCC per candidate.
    let mut verified: Vec<Link> = Vec::new();
    for (dy, _) in cands {
        let mut best = (f32::MIN, dy);
        let mut d = dy - 3.0;
        while d <= dy + 3.0 {
            let n = grid_ncc(a, b, rect, d);
            if n > best.0 {
                best = (n, d);
            }
            d += 1.0;
        }
        // Sub-pixel parabola on the winner's neighbourhood.
        let (n0, d0) = best;
        let (nm, np) = (grid_ncc(a, b, rect, d0 - 1.0), grid_ncc(a, b, rect, d0 + 1.0));
        let denom = nm - 2.0 * n0 + np;
        let shift = if denom.abs() > f32::EPSILON {
            (0.5 * (nm - np) / denom).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        verified.push(Link {
            dy: d0 + shift,
            ncc: n0,
        });
    }
    verified.retain(|l| l.ncc >= LINK_NCC_MIN);
    if verified.is_empty() {
        return None;
    }
    verified.sort_by(|x, y| y.ncc.partial_cmp(&x.ncc).unwrap_or(std::cmp::Ordering::Equal));
    let best_ncc = verified[0].ncc;
    let aliases: Vec<&Link> = verified
        .iter()
        .filter(|l| best_ncc - l.ncc <= LINK_ALIAS_NCC)
        .collect();
    if aliases.len() > 1 {
        if let Some(hint) = scroll_hint {
            let mut by_hint: Vec<&Link> = aliases.clone();
            by_hint.sort_by(|x, y| {
                (x.dy - hint)
                    .abs()
                    .partial_cmp(&(y.dy - hint).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            // The hint only decides when it clearly discriminates.
            if (by_hint[0].dy - hint).abs() + pitch / 2.0 < (by_hint[1].dy - hint).abs() {
                return Some(*by_hint[0]);
            }
        }
        // Smallest |offset| among near-ties.
        let mut by_mag: Vec<&Link> = aliases.clone();
        by_mag.sort_by(|x, y| {
            x.dy.abs()
                .partial_cmp(&y.dy.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        return Some(*by_mag[0]);
    }
    Some(verified[0])
}

/// Global least-squares over a segment's link graph (Gauss–Seidel
/// on the normal equations, anchor fixed at its chained value).
/// Edges: (frame slot i, frame slot j, measured g_j − g_i).
fn solve_graph(g: &mut [f32], edges: &[(usize, usize, f32)], anchor: usize) {
    for _ in 0..200 {
        let mut max_move = 0.0f32;
        for slot in 0..g.len() {
            if slot == anchor {
                continue;
            }
            let (mut num, mut den) = (0.0f32, 0.0f32);
            for &(i, j, d) in edges {
                if j == slot {
                    num += g[i] + d;
                    den += 1.0;
                } else if i == slot {
                    num += g[j] - d;
                    den += 1.0;
                }
            }
            if den > 0.0 {
                let new = num / den;
                max_move = max_move.max((new - g[slot]).abs());
                g[slot] = new;
            }
        }
        if max_move < 0.001 {
            break;
        }
    }
}

/// Plan a whole parts session's stitch. `frames` are the session's
/// parts frames in arrival order; all must share the layout's page
/// geometry (the caller guarantees one screen size per session).
/// Gradient-magnitude image (|dx| + |dy|), the domain link
/// correlation runs in (see measure_link).
fn gradient_image(img: &GrayImage) -> GrayImage {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut px = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let gx = if x + 1 < w {
                (img.pixels[i + 1] as i32 - img.pixels[i] as i32).abs()
            } else {
                0
            };
            let gy = if y + 1 < h {
                (img.pixels[i + w] as i32 - img.pixels[i] as i32).abs()
            } else {
                0
            };
            px[i] = (gx + gy).min(255) as u8;
        }
    }
    GrayImage {
        width: img.width,
        height: img.height,
        pixels: px,
    }
}

pub fn plan_session(frames: &[&GrayImage], layout: &Layout) -> StitchPlan {
    let mut plan = StitchPlan::default();
    if frames.is_empty() {
        return plan;
    }
    let grad: Vec<GrayImage> = frames.iter().map(|f| gradient_image(f)).collect();
    let grad_refs: Vec<&GrayImage> = grad.iter().collect();
    let rect = GridRect::of(layout, frames[0]);
    let start = rect.y0.max(0.0) as usize;
    let nominal_row = rect.layout_row_pitch(layout);
    let nominal_col = rect.layout_col_pitch(layout);

    let profiles: Vec<Vec<f32>> = frames.iter().map(|f| row_profile(f, &rect)).collect();
    let col_profiles: Vec<Vec<f32>> = frames.iter().map(|f| col_profile(f, &rect)).collect();

    // Per-frame row pitches + the session fits. A frame's fit is
    // real when its fold at the fitted pitch has a dominant peak
    // on a profile with real mass — the signature of a true
    // lattice. (An earlier version required the fit to BEAT the
    // layout pitch's concentration by 2%; that test inverted
    // once the layout prior became the measured pitch — pass 33:
    // agreeing fits were discarded as unfitted and their phases
    // forced to 0, which is how the two displaced census frames
    // lost their phases.) Otherwise the frame is unfitted
    // (layout pitch, and phase 0 wherever a phase is taken).
    let row_periods = layout.rows as f32 - 0.07;
    // The session pitch is a JOINT fit (fold concentrations summed
    // over all frames): one frame's fold can favour a wrong pitch
    // when its content is displaced and few periods fall inside
    // the fold window (frame-parts-11 scans to 191.95 alone), but
    // the summed landscape is unambiguous. A frame's own pitch is
    // adopted only when it deviates from the session pitch by
    // more than the segment-break threshold (0.3px) AND clearly
    // out-concentrates the session pitch on its own profile —
    // the signature of a genuinely different rendering segment,
    // not of a fragile fold.
    plan.session_row_pitch = {
        let (lo, hi) = (nominal_row * 0.94, nominal_row * 1.06);
        let mut best = (f32::MIN, nominal_row);
        let mut p = lo;
        while p <= hi {
            let c: f32 = profiles
                .iter()
                .map(|pr| fold_concentration(pr, start, p, row_periods))
                .sum();
            if c > best.0 {
                best = (c, p);
            }
            p += 0.2;
        }
        best.1
    };
    for p in &profiles {
        let own_pitch = fit_pitch(p, start, nominal_row, row_periods);
        let working = if (own_pitch - plan.session_row_pitch).abs() > 0.3
            && fold_concentration(p, start, own_pitch, row_periods)
                > 1.1 * fold_concentration(p, start, plan.session_row_pitch, row_periods)
        {
            own_pitch
        } else {
            plan.session_row_pitch
        };
        let mass: f32 = p[start..].iter().sum();
        let bins = fold(p, start, working, row_periods);
        let total: f32 = bins.iter().sum();
        let peak = bins.iter().copied().fold(0.0f32, f32::max);
        // The lattice must repeat: gradient mass spread over many
        // profile rows. A single edge (a solid synthetic frame, a
        // menu screenshot) folds into one bin at EVERY pitch, so
        // peak dominance alone would call it fitted with a garbage
        // pitch and phase.
        let pmax = p[start..].iter().copied().fold(0.0f32, f32::max);
        let active_rows = p[start..].iter().filter(|&&v| v > 0.1 * pmax).count();
        let real = mass > 1.0
            && total > 0.0
            && peak > 4.0 * (total / FOLD_BINS as f32)
            && active_rows >= 8;
                plan.frame_fitted.push(real);
        plan.frame_pitches.push(if real { working } else { nominal_row });
    }
    // Column pitch: joint fit — sum concentrations over frames.
    plan.session_col_pitch = {
        let xstart = rect.x0.max(0.0) as usize;
        let col_periods = layout.cols as f32 - 0.07;
        let (lo, hi) = (nominal_col * 0.94, nominal_col * 1.06);
        let mut best = (f32::MIN, nominal_col);
        let mut p = lo;
        while p <= hi {
            let c: f32 = col_profiles
                .iter()
                .map(|cp| fold_concentration(cp, xstart, p, col_periods))
                .sum();
            if c > best.0 {
                best = (c, p);
            }
            p += 0.2;
        }
        best.1
    };
    let thumbs: Vec<Option<f32>> = frames.iter().map(|f| thumb_fraction(f, &rect)).collect();
    // A frame's phase at a given pitch; unfitted frames sit at 0.
    let phase_of = |plan: &StitchPlan, i: usize, pitch: f32| -> f32 {
        if plan.frame_fitted[i] {
            fit_phase(&profiles[i], start, pitch, row_periods)
        } else {
            0.0
        }
    };

    // Segmentation + chaining. A segment under construction keeps
    // its member frame indices; its pitch is the members' median
    // fit, refined as members join.
    struct Building {
        members: Vec<usize>,
        /// Chained scroll displacements D from the segment anchor
        /// (the panorama lattice offset g = D + φ is derived at
        /// segment close, when the final phases are known).
        d: Vec<f32>,
        relinked: bool,
    }
    let mut done: Vec<Building> = Vec::new();
    let mut cur: Option<Building> = None;
    let seg_pitch = |b: &Building, plan: &StitchPlan| -> f32 {
        let mut ps: Vec<f32> = b.members.iter().map(|&i| plan.frame_pitches[i]).collect();
        ps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        ps[ps.len() / 2]
    };
    for (i, frame) in frames.iter().enumerate() {
        let Some(b) = cur.as_mut() else {
            cur = Some(Building {
                members: vec![i],
                d: vec![0.0],
                relinked: false,
            });
            continue;
        };
        let pitch = seg_pitch(b, &plan);
        if (plan.frame_pitches[i] - pitch).abs() > SEGMENT_PITCH_BREAK {
            done.push(cur.take().unwrap());
            cur = Some(Building {
                members: vec![i],
                d: vec![0.0],
                relinked: false,
            });
            continue;
        }
        let prev = *b.members.last().unwrap();
        let phase_prev = phase_of(&plan, prev, pitch);
        let phase_i = phase_of(&plan, i, pitch);
        // Scrollbar hint: thumb delta × the panorama span so far
        // (the placed extent plus one viewport).
        let hint = match (thumbs[prev], thumbs[i]) {
            (Some(tp), Some(ti)) => {
                let span = b.d.iter().fold(0.0f32, |m, &v| m.max(v)) + rect.h;
                Some((ti - tp) * span)
            }
            _ => None,
        };
        let link = measure_link(
            grad_refs[prev],
            grad_refs[i],
            &rect,
            &profiles[prev],
            &profiles[i],
            phase_prev,
            phase_i,
            pitch,
            hint,
        );
        match link {
            Some(l) => {
                let dprev = *b.d.last().unwrap();
                b.members.push(i);
                b.d.push(dprev + l.dy);
            }
            None => {
                done.push(cur.take().unwrap());
                cur = Some(Building {
                    members: vec![i],
                    d: vec![0.0],
                    relinked: true,
                });
            }
        }
    }
    if let Some(b) = cur.take() {
        done.push(b);
    }

    // Per segment: final pitch, phases, loop closure, solve, eject.
    for b in done {
        let mut ps: Vec<f32> = b.members.iter().map(|&i| plan.frame_pitches[i]).collect();
        ps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pitch = ps[ps.len() / 2];
        let phases: Vec<f32> = b
            .members
            .iter()
            .map(|&i| phase_of(&plan, i, pitch))
            .collect();
        // The link graph lives in D space (scroll displacements
        // from the anchor); placements convert at the end via
        // g = D + φ. Direct links measure D deltas, so the graph
        // is homogeneous.
        let mut d = b.d;
        let n = b.members.len();
        let mut ejected_slots: Vec<usize> = Vec::new();
        if n >= 3 {
            // Direct links for every overlapping non-adjacent pair.
            let mut edges: Vec<(usize, usize, f32)> = Vec::new();
            for s in 0..n - 1 {
                edges.push((s, s + 1, d[s + 1] - d[s]));
            }
            let mut closure_bad = false;
            for s in 0..n {
                for t in s + 2..n {
                    let chained = d[t] - d[s];
                    if chained.abs() >= pitch * layout.rows as f32 - pitch {
                        continue; // less than a row of overlap
                    }
                    if let Some(l) = measure_link(
                        grad_refs[b.members[s]],
                        grad_refs[b.members[t]],
                        &rect,
                        &profiles[b.members[s]],
                        &profiles[b.members[t]],
                        phases[s],
                        phases[t],
                        pitch,
                        None,
                    ) {
                        edges.push((s, t, l.dy));
                        if (l.dy - chained).abs() > LOOP_CLOSE_TOL {
                            closure_bad = true;
                        }
                    }
                }
            }
            if closure_bad {
                plan.loop_closure_failures += 1;
                plan.solve_ran = true;
                solve_graph(&mut d, &edges, 0);
                // Residual per slot from its incident edges.
                let mut resid = vec![0.0f32; n];
                for &(i, j, dd) in &edges {
                    let r = (d[j] - d[i] - dd).abs();
                    resid[i] = resid[i].max(r);
                    resid[j] = resid[j].max(r);
                }
                for s in 0..n {
                    if resid[s] > LOOP_CLOSE_TOL && s != 0 {
                        ejected_slots.push(s);
                    }
                }
                if !ejected_slots.is_empty() {
                    plan.ejected += ejected_slots.len();
                    // Re-solve without the ejected frames' edges.
                    let keep: Vec<(usize, usize, f32)> = edges
                        .iter()
                        .copied()
                        .filter(|(i, j, _)| {
                            !ejected_slots.contains(i) && !ejected_slots.contains(j)
                        })
                        .collect();
                    solve_graph(&mut d, &keep, 0);
                }
            }
            // Final residuals for the report: direct (non-adjacent)
            // edges only — consecutive residuals are definitional.
            let mut resid = vec![0.0f32; n];
            for &(i, j, dd) in &edges {
                if j > i + 1 {
                    let r = (d[j] - d[i] - dd).abs();
                    resid[i] = resid[i].max(r);
                    resid[j] = resid[j].max(r);
                }
            }
            let seg = Segment {
                pitch: AxisFit {
                    row_pitch: pitch,
                    col_pitch: plan.session_col_pitch,
                },
                frames: b
                    .members
                    .iter()
                    .enumerate()
                    .filter(|(s, _)| !ejected_slots.contains(s))
                    .map(|(s, &fi)| FramePlacement {
                        frame: fi,
                        phase: phases[s],
                        g: d[s] + phases[s],
                        residual: resid[s],
                        thumb: thumbs[fi],
                    })
                    .collect(),
                relinked: b.relinked,
            };
            plan.segments.push(seg);
            // Ejected frames become relinked singletons on their
            // own phase and the session pitch.
            for &s in &ejected_slots {
                let fi = b.members[s];
                plan.segments.push(Segment {
                    pitch: AxisFit {
                        row_pitch: pitch,
                        col_pitch: plan.session_col_pitch,
                    },
                    frames: vec![FramePlacement {
                        frame: fi,
                        phase: phases[s],
                        g: phases[s],
                        residual: 0.0,
                        thumb: thumbs[fi],
                    }],
                    relinked: true,
                });
            }
        } else {
            plan.segments.push(Segment {
                pitch: AxisFit {
                    row_pitch: pitch,
                    col_pitch: plan.session_col_pitch,
                },
                frames: b
                    .members
                    .iter()
                    .enumerate()
                    .map(|(s, &fi)| FramePlacement {
                        frame: fi,
                        phase: phases[s],
                        g: d[s] + phases[s],
                        residual: 0.0,
                        thumb: thumbs[fi],
                    })
                    .collect(),
                relinked: b.relinked,
            });
        }
    }

    // Scrollbar affine diagnostic: within each multi-frame
    // segment, scroll position (g − φ = D + φ_anchor) should be
    // affine in the thumb fraction.
    for seg in &plan.segments {
        let pts: Vec<(f32, f32)> = seg
            .frames
            .iter()
            .filter_map(|p| p.thumb.map(|t| (t, p.g - p.phase)))
            .collect();
        if pts.len() >= 4 {
            let (mt, mg) = (
                pts.iter().map(|p| p.0).sum::<f32>() / pts.len() as f32,
                pts.iter().map(|p| p.1).sum::<f32>() / pts.len() as f32,
            );
            let (mut num, mut den) = (0.0f32, 0.0f32);
            for &(t, gv) in &pts {
                num += (t - mt) * (gv - mg);
                den += (t - mt) * (t - mt);
            }
            if den > f32::EPSILON {
                let slope = num / den;
                let intercept = mg - slope * mt;
                for &(t, gv) in &pts {
                    if (gv - (intercept + slope * t)).abs() > seg.pitch.row_pitch / 2.0 {
                        plan.scrollbar_outliers += 1;
                    }
                }
            }
        }
    }
    plan
}

/// One sighting of a panorama slot in one frame.
#[derive(Debug, Clone)]
pub struct Instance {
    pub frame: usize,
    /// Crop origin in the frame's page px (fractional).
    pub fx: f32,
    pub fy: f32,
    /// Viewport margin of the crop box (px); negative when the box
    /// crosses the viewport edge or the page edge.
    pub margin: f32,
    /// The frame's link residual (canonical-crop ranking input).
    pub link_residual: f32,
}

impl Instance {
    /// Usable for a canonical crop: fully visible with margin.
    pub fn usable(&self) -> bool {
        self.margin >= MIN_INSTANCE_MARGIN
    }
}

/// A panorama slot: one (row, col) of one segment, with every
/// sighting the session recorded of it.
#[derive(Debug, Clone)]
pub struct Slot {
    pub segment: usize,
    pub row: i32,
    pub col: u32,
    pub instances: Vec<Instance>,
}

/// Enumerate every slot a segment's frames covered, with each
/// frame's sighting of it. Slots are keyed (row, col) in panorama
/// coordinates; sightings whose crop box leaves the page or the
/// list viewport are kept (as partial sightings) — the coverage
/// accounting counts on seeing them.
pub fn slot_instances(
    plan: &StitchPlan,
    seg_idx: usize,
    frames: &[&GrayImage],
    layout: &Layout,
) -> Vec<Slot> {
    let seg = &plan.segments[seg_idx];
    let rect = GridRect::of(layout, frames[0]);
    let (pr, pc) = (seg.pitch.row_pitch, seg.pitch.col_pitch);
    // The list viewport in page coords, in fitted units: the grid
    // top, extended by a quarter row of slack on both ends (rows
    // fade at the list edges; a crop crossing the viewport is
    // partial by construction).
    // Half a row of slack on both ends (pass 33 gate finding:
    // at a quarter row, the bottom row of a +48px-displaced frame
    // fell 1.5px short of usable while its name band is in fact
    // fully rendered — the stage-B mirror read those cells. The
    // list viewport's fade zone is wider than a quarter row;
    // rows beyond it still classify partial by margin.)
    let (vp_top, vp_bot) = (
        rect.y0 - 0.5 * pr,
        rect.y0 + layout.rows as f32 * pr + 0.5 * pr,
    );
    let anchor_phase = seg.frames.first().map(|p| p.phase).unwrap_or(0.0);
    let mut map: BTreeMap<(i32, u32), Vec<Instance>> = BTreeMap::new();
    for p in &seg.frames {
        // Panorama row of this frame's lattice row 0 (see the
        // invariant on FramePlacement::g).
        let scroll_rows = ((p.g - anchor_phase) / pr).round() as i32;
        // The viewport presents `rows` full lattice cells plus up
        // to one partial at each edge: with a positive phase a
        // partial row shows its bottom at the top edge (lattice
        // row -1), with a negative phase a partial row shows its
        // top at the bottom edge (lattice row `rows`). Enumerate
        // the full span and let the margin arithmetic classify
        // each instance usable or partial — enumerating exactly
        // `rows` rows from r0 silently drops a FULL row at the far
        // edge whenever a partial row appears at the near one.
        let r0: i32 = if p.phase > 0.0 { -1 } else { 0 };
        for r in r0..=r0 + layout.rows as i32 {
            let row = scroll_rows + r;
            let fy = rect.y0 + p.phase + r as f32 * pr;
            // Skip rows whose NAME BAND never intersects the
            // extended viewport: they are not sightings at all
            // (the far-edge row of the enumeration span), and
            // recording them would mint phantom slots.
            let band_top = fy + layout.name[1] * pr;
            let band_bot = fy + layout.name[3] * pr;
            if band_bot <= vp_top || band_top >= vp_bot {
                continue;
            }
            for c in 0..layout.cols {
                let fx = rect.x0 + c as f32 * pc;
                let frame = frames[p.frame];
                // Viewport margin on every side; a box that leaves
                // the PAGE is partial whatever its viewport margin
                // (its outer rows would be fabricated by the
                // resampler), so containment failure forces the
                // margin negative.
                let mut margin = [
                    fy - vp_top,
                    vp_bot - (fy + pr),
                    fx - (rect.x0 - 0.5 * pc),
                    (rect.x0 + rect.w) + 0.5 * pc - (fx + pc),
                ]
                .into_iter()
                .fold(f32::MAX, f32::min);
                if fx < 0.0
                    || fy < 0.0
                    || fx + pc > frame.width as f32
                    || fy + pr > frame.height as f32
                {
                    margin = margin.min(-1.0);
                }
                map.entry((row, c)).or_default().push(Instance {
                    frame: p.frame,
                    fx,
                    fy,
                    margin,
                    link_residual: p.residual,
                });
            }
        }
    }
    map.into_iter()
        .map(|((row, col), instances)| Slot {
            segment: seg_idx,
            row,
            col,
            instances,
        })
        .collect()
}

/// Bilinear crop at a fractional origin: the instance resampled
/// onto the canonical cell box (all boxes are one pitch in size,
/// so this is a pure fractional translation).
/// Cut a cell at the ROUNDED fitted offset — production's
/// integer-cut convention (see `canonical_crop`). The origin is
/// clamped into the page; usable instances are page-contained,
/// so the clamp only ever absorbs the rounding itself.
pub fn crop_integer(frame: &GrayImage, fx: f32, fy: f32, w: u32, h: u32) -> GrayImage {
    let x = (fx.round() as i64).clamp(0, (frame.width as i64 - w as i64).max(0)) as u32;
    let y = (fy.round() as i64).clamp(0, (frame.height as i64 - h as i64).max(0)) as u32;
    frame.crop(x, y, w, h)
}

pub fn crop_bilinear(frame: &GrayImage, fx: f32, fy: f32, w: u32, h: u32) -> GrayImage {
    let mut out = GrayImage::blank(w, h, 0);
    for dy in 0..h {
        let sy = fy + dy as f32;
        let y0 = sy.floor();
        let wy = sy - y0;
        for dx in 0..w {
            let sx = fx + dx as f32;
            let x0 = sx.floor();
            let wx = sx - x0;
            let px = |xx: f32, yy: f32| -> f32 {
                if xx < 0.0 || yy < 0.0 || xx >= frame.width as f32 || yy >= frame.height as f32 {
                    0.0
                } else {
                    frame.get(xx as u32, yy as u32) as f32
                }
            };
            let v = px(x0, y0) * (1.0 - wx) * (1.0 - wy)
                + px(x0 + 1.0, y0) * wx * (1.0 - wy)
                + px(x0, y0 + 1.0) * (1.0 - wx) * wy
                + px(x0 + 1.0, y0 + 1.0) * wx * wy;
            out.pixels[(dy * w + dx) as usize] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Snap one crop origin to the tile's detected boundaries
/// (pass 34, brief §4 ablation): search ±6px around the fitted
/// origin for the vertical offset whose top edge carries the
/// strongest horizontal gradient (the tile's top border) and
/// the horizontal offset whose left edge carries the strongest
/// vertical gradient. Ties prefer the smaller offset (the
/// fitted grid is the prior; the snap must earn its move).
fn snap_origin(frame: &GrayImage, fx: f32, fy: f32, w: u32, h: u32) -> (f32, f32) {
    let get = |x: i32, y: i32| -> i32 {
        frame.get(
            x.clamp(0, frame.width.saturating_sub(1) as i32) as u32,
            y.clamp(0, frame.height.saturating_sub(1) as i32) as u32,
        ) as i32
    };
    let (x0, y0) = (fx.round() as i32, fy.round() as i32);
    let mut best_dy = (f32::MIN, 0i32);
    for dy in -6..=6i32 {
        let mut s = 0.0f32;
        for x in x0..x0 + w as i32 {
            s += (get(x, y0 + dy) - get(x, y0 + dy - 1)).abs() as f32;
        }
        if s > best_dy.0 || (s == best_dy.0 && dy.abs() < best_dy.1.abs()) {
            best_dy = (s, dy);
        }
    }
    let mut best_dx = (f32::MIN, 0i32);
    for dx in -6..=6i32 {
        let mut s = 0.0f32;
        for y in y0..y0 + h as i32 {
            s += (get(x0 + dx, y) - get(x0 + dx - 1, y)).abs() as f32;
        }
        if s > best_dx.0 || (s == best_dx.0 && dx.abs() < best_dx.1.abs()) {
            best_dx = (s, dx);
        }
    }
    (fx + best_dx.1 as f32, fy + best_dy.1 as f32)
}

/// A copy of `slot` with every instance's crop origin snapped
/// to its tile's detected boundaries (see [`snap_origin`]).
/// Usability is re-derived: a snapped box that leaves the page
/// turns partial, exactly as the fitted enumeration classifies.
pub fn snap_slot(slot: &Slot, frames: &[&GrayImage], w: u32, h: u32) -> Slot {
    let mut out = slot.clone();
    for inst in &mut out.instances {
        let frame = frames[inst.frame];
        let (fx, fy) = snap_origin(frame, inst.fx, inst.fy, w, h);
        inst.fx = fx;
        inst.fy = fy;
        if fx < 0.0 || fy < 0.0 || fx + w as f32 > frame.width as f32 || fy + h as f32 > frame.height as f32 {
            inst.margin = inst.margin.min(-1.0);
        }
    }
    out
}

/// The built canonical crop for one slot, with its audit trail.
#[derive(Debug, Clone)]
pub struct CanonicalCrop {
    pub image: GrayImage,
    /// Instances that went into the pool (after the gain check).
    pub pooled: usize,
    /// Instances excluded by the gain check.
    pub gain_excluded: usize,
    /// True when built as a pixel-wise median (≥3 pooled); false
    /// when degraded to the best-ranked single instance.
    pub median: bool,
    /// Worst mean-absolute-difference of a pooled instance from
    /// the canonical crop; above [`DISAGREE_MAD`] flags the slot.
    pub worst_mad: f32,
}

impl CanonicalCrop {
    pub fn disagreement(&self) -> bool {
        self.worst_mad > DISAGREE_MAD
    }
}

fn image_mean(img: &GrayImage) -> f32 {
    if img.pixels.is_empty() {
        return 0.0;
    }
    img.pixels.iter().map(|&p| p as f32).sum::<f32>() / img.pixels.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::CountStyle;

    fn layout() -> Layout {
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
            count_style: CountStyle::Badge,
            name_psm: 11,
            count_psm: 7,
            name_upscale: 4,
            count_upscale: 8,
            name_upper: None,
            pips: None,
            min_score: 0.5,
        }
    }

    /// The renderer's true row pitch: 4% off the layout's 50.
    const TRUE_PITCH: f32 = 52.0;

    fn xorshift(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    /// A synthetic inventory list: each row has a dim top edge,
    /// row-unique texture, and a bright text line at the fold-peak
    /// fraction (the feature the phase fit anchors on).
    fn panorama(rows: usize) -> GrayImage {
        let w = 400u32;
        let h = (rows as f32 * TRUE_PITCH) as u32;
        let mut img = GrayImage::blank(w, h, 8);
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for r in 0..rows {
            let y0 = (r as f32 * TRUE_PITCH) as u32;
            for y in y0..y0 + 18 {
                for x in 0..w {
                    if (x + y) % 7 == 0 {
                        let v = 40 + (xorshift(&mut state) % 120) as u8;
                        img.set(x, y, v);
                    }
                }
            }
            for x in 0..w {
                img.set(x, y0, 60);
            }
            let ty = y0 + (FOLD_PEAK_FRAC * TRUE_PITCH) as u32;
            for x in (0..w).step_by(9) {
                for dx in 0..5u32 {
                    img.set(x + dx, ty, 240);
                }
            }
        }
        img
    }

    /// A page showing the panorama's list scrolled by `scroll` px.
    fn frame_at(pano: &GrayImage, scroll: f32) -> GrayImage {
        let mut img = GrayImage::blank(460, 300, 5);
        for y in 60..210u32 {
            for x in 20..420u32 {
                let py = (y - 60) as f32 + scroll;
                if py >= 0.0 && (py as u32) < pano.height {
                    img.set(x, y, pano.get(x - 20, py as u32));
                }
            }
        }
        img
    }

    #[test]
    fn plan_recovers_pitch_phase_and_slots() {
        let pano = panorama(6);
        let frames = [frame_at(&pano, 0.0), frame_at(&pano, 52.0), frame_at(&pano, 30.0)];
        let refs: Vec<&GrayImage> = frames.iter().collect();
        let plan = plan_session(&refs, &layout());
        assert!(
            (plan.session_row_pitch - TRUE_PITCH).abs() <= 0.4,
            "session pitch {} vs true {TRUE_PITCH}",
            plan.session_row_pitch
        );
        assert_eq!(plan.segments.len(), 1, "all three frames link into one segment");
        let seg = &plan.segments[0];
        assert_eq!(seg.frames.len(), 3);
        // Phases: scroll 0 and 52 sit at phase ~0; scroll 30 wraps
        // to +22 (30 - 52). Differences are the invariant.
        let (p0, p1, p2) = (seg.frames[0].phase, seg.frames[1].phase, seg.frames[2].phase);
        assert!(p0.abs() <= 2.5, "phase at scroll 0: {p0}");
        assert!(p1.abs() <= 2.5, "phase at scroll 52: {p1}");
        assert!((p2 - 22.0).abs() <= 2.5, "phase at scroll 30: {p2}");
        // Slots: panorama rows 0..=3, four columns each — plus
        // any partial edge rows the wider viewport admits (their
        // sightings are real; the content rows are the contract).
        let slots = slot_instances(&plan, 0, &refs, &layout());
        for r in 0..4 {
            for c in 0..4 {
                assert!(
                    slots.iter().any(|s| s.row == r && s.col == c),
                    "content slot r{r} c{c} exists"
                );
            }
        }
        let slot00 = slots.iter().find(|s| s.row == 0 && s.col == 0).unwrap();
        assert_eq!(
            slot00.instances.len(),
            2,
            "row 0 presented by frames 0 (full) and 2 (top partial)"
        );
        assert_eq!(slot00.instances.iter().filter(|i| i.usable()).count(), 1);
        let slot3 = slots.iter().find(|s| s.row == 3 && s.col == 0).unwrap();
        assert_eq!(
            slot3.instances.len(),
            3,
            "row 3: sightings in all three frames (full, margin-usable, and bottom-edge partial)"
        );
        assert_eq!(slot3.instances.iter().filter(|i| i.usable()).count(), 2);
        // The canonical crop of slot (0,0) is the panorama's own
        // first cell. The fixture's two-spike gradient feature
        // limits the synthetic phase estimate to ~1-2px (the
        // production fold's dominant line measured bin-consistent
        // across both censuses; the census gate arbitrates phase
        // accuracy there), so compare content at the best small
        // vertical alignment, with a loose absolute bound to catch
        // a wrong-row crop.
        let cc = canonical_crop(slot00, &refs, 100, 52).unwrap();
        let mad_at = |dy: f32| -> f32 {
            let truth = crop_bilinear(&frames[0], 20.0, 60.0 + dy, 100, 52);
            cc.image
                .pixels
                .iter()
                .zip(truth.pixels.iter())
                .map(|(&a, &b)| (a as f32 - b as f32).abs())
                .sum::<f32>()
                / cc.image.pixels.len() as f32
        };
        let absolute = mad_at(0.0);
        let aligned = (-6..=6)
            .map(|k| mad_at(k as f32 * 0.5))
            .fold(f32::MAX, f32::min);
        assert!(
            absolute < 20.0,
            "canonical crop is not the slot's cell at all: MAD {absolute}"
        );
        assert!(
            aligned < 4.0,
            "canonical crop content mismatch at best alignment: MAD {aligned}"
        );
    }

    #[test]
    fn canonical_median_rejects_a_transient() {
        let pano = panorama(4);
        let clean = frame_at(&pano, 0.0);
        let mut hover = clean.clone();
        // A small bright transient blob over the first cell (small
        // enough to stay inside the gain band — a bigger one is
        // excluded by the gain check instead, also by design).
        for y in 70..75u32 {
            for x in 30..35u32 {
                hover.set(x, y, 255);
            }
        }
        let frames = [clean.clone(), hover, clean];
        let refs: Vec<&GrayImage> = frames.iter().collect();
        let slot = Slot {
            segment: 0,
            row: 0,
            col: 0,
            instances: (0..3)
                .map(|f| Instance {
                    frame: f,
                    fx: 20.0,
                    fy: 60.0,
                    margin: 30.0,
                    link_residual: 0.0,
                })
                .collect(),
        };
        let cc = canonical_crop(&slot, &refs, 100, 52).unwrap();
        assert!(cc.median, "three pooled instances build a median");
        // At the blob's centre the median is the clean value, not 255.
        let centre = cc.image.get(12, 12);
        assert!(centre < 100, "median rejected the transient (got {centre})");
        assert!(!cc.disagreement(), "one small transient is not a disagreement");
    }

    #[test]
    fn unfitted_frames_keep_layout_geometry() {
        // Blank frames carry no periodic signal: no fit, layout
        // pitch, phase 0, and no links — each falls back to its own
        // segment, read on the layout geometry exactly as before.
        let frames = [
            GrayImage::blank(460, 300, 7),
            GrayImage::blank(460, 300, 9),
        ];
        let refs: Vec<&GrayImage> = frames.iter().collect();
        let plan = plan_session(&refs, &layout());
        assert!(plan.frame_fitted.iter().all(|f| !f));
        assert!(plan
            .frame_pitches
            .iter()
            .all(|&p| (p - 50.0).abs() < f32::EPSILON));
        assert_eq!(plan.segments.len(), 2, "unlinkable frames fall back singly");
        assert!(
            !plan.segments[0].relinked && plan.segments[1].relinked,
            "the first segment is the anchor; the second is the fallback"
        );
        assert!(plan
            .segments
            .iter()
            .all(|s| s.frames.iter().all(|p| p.phase == 0.0)));
    }
}

/// Build a slot's canonical crop from its usable instances:
/// agreement, never sharpness (reviewer Q2). Pool instances are
/// gain-checked against the pool's median mean; ≥3 pooled builds
/// the pixel-wise median, fewer degrade to the best-ranked single
/// (viewport margin, then agreement with the pool median, then
/// link residual).
pub fn canonical_crop(
    slot: &Slot,
    frames: &[&GrayImage],
    w: u32,
    h: u32,
) -> Option<CanonicalCrop> {
    let usable: Vec<&Instance> = slot.instances.iter().filter(|i| i.usable()).collect();
    if usable.is_empty() {
        return None;
    }
    // Instance cuts are INTEGER cuts at the rounded fitted
    // offset — the production split's own convention. A bilinear
    // resample at the fractional offset anti-aliases the small
    // name text and measurably degrades its OCR (pass 33 gate:
    // the same cell at the same geometry read "Harrow Pri" cut
    // integer and "Marrow Pr" cut resampled); a ≤0.5px rounding
    // offset is the error production has always carried.
    let crops: Vec<GrayImage> = usable
        .iter()
        .map(|i| crop_integer(frames[i.frame], i.fx, i.fy, w, h))
        .collect();
    let means: Vec<f32> = crops.iter().map(image_mean).collect();
    let mut sorted_means = means.clone();
    sorted_means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let med_mean = sorted_means[sorted_means.len() / 2].max(1.0);
    let inliers: Vec<usize> = (0..crops.len())
        .filter(|&i| means[i] >= GAIN_LO * med_mean && means[i] <= GAIN_HI * med_mean)
        .collect();
    let inliers = if inliers.is_empty() {
        (0..crops.len()).collect()
    } else {
        inliers
    };
    let gain_excluded = crops.len() - inliers.len();
    let mad = |a: &GrayImage, b: &GrayImage| -> f32 {
        a.pixels
            .iter()
            .zip(b.pixels.iter())
            .map(|(&x, &y)| (x as f32 - y as f32).abs())
            .sum::<f32>()
            / a.pixels.len().max(1) as f32
    };
    if inliers.len() >= 3 {
        let n = crops[0].pixels.len();
        let mut px: Vec<u8> = Vec::with_capacity(inliers.len());
        let mut out = GrayImage::blank(w, h, 0);
        for k in 0..n {
            px.clear();
            for &i in &inliers {
                px.push(crops[i].pixels[k]);
            }
            px.sort_unstable();
            out.pixels[k] = px[px.len() / 2];
        }
        let worst = inliers
            .iter()
            .map(|&i| mad(&crops[i], &out))
            .fold(0.0f32, f32::max);
        return Some(CanonicalCrop {
            image: out,
            pooled: inliers.len(),
            gain_excluded,
            median: true,
            worst_mad: worst,
        });
    }
    // Degrade to the best-ranked single instance.
    let mut ranked: Vec<usize> = inliers.clone();
    ranked.sort_by(|&a, &b| {
        usable[b]
            .margin
            .partial_cmp(&usable[a].margin)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                (means[a] - med_mean)
                    .abs()
                    .partial_cmp(&(means[b] - med_mean).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                usable[a]
                    .link_residual
                    .partial_cmp(&usable[b].link_residual)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    let best = ranked[0];
    let worst = if inliers.len() > 1 {
        inliers
            .iter()
            .filter(|&&i| i != best)
            .map(|&i| mad(&crops[i], &crops[best]))
            .fold(0.0f32, f32::max)
    } else {
        0.0
    };
    Some(CanonicalCrop {
        image: crops[best].clone(),
        pooled: inliers.len(),
        gain_excluded,
        median: false,
        worst_mad: worst,
    })
}
