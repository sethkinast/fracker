//! Pass 35 — analysis-by-synthesis, the third voice.
//!
//! The two readers ask what a slot's pixels SAY (a general
//! recogniser over presentations of the text). Synthesis asks
//! the closed question instead: of the handful of items this
//! slot could still be, which one's name — rendered in the
//! game's own font, the 2013 Roboto cut vendored under
//! `assets/fonts/` — PRODUCES these pixels? It runs only over
//! slots the shipped arbitration leaves unheld but that carry
//! candidate evidence (a refused hold, a sibling tie, a decoder
//! nomination, a radius refusal). It never overrules a reader
//! hold, never breaks a hold-vs-hold name disagreement (those
//! stay refused by the arbitration contract), and never
//! decides form: the form channels' verdict gates its
//! candidate set, and under an abstaining verdict a
//! blueprint-form winner must still prove its Blueprint line
//! in rendered pixels.
//!
//! Calibration is per session, from the session's own
//! both-readers-agreement slots (their truth is known without
//! ground truth): the rasteriser's size/coverage parameters
//! are fitted to maximise the true lines' masked NCC, then the
//! acceptance floor and margin are chosen from the measured
//! distributions of true-candidate vs best-wrong-candidate
//! scores on those same slots. No constant in this module was
//! tuned against a census truth label; the pass doc states the
//! distributions and the plateau the thresholds sit in.
//!
//! Integration is deliberately a post-pass: [`adjudicate_session`]
//! runs once per session at finalize over the merged stitch
//! reports (pass 36 integration), observes the canonical crops
//! retained on the slots it touches when each bundle was
//! stitched, re-reads those crops for evidence, and mutates
//! only unheld slots' verdicts. The session pipeline's
//! structure is untouched.

use std::collections::{HashMap, HashSet};

use crate::capture::{ReadOptions, SlotCoverage, SlotReport, StitchReport};
use crate::lineread::{self, FormVerdict, LineDetection, Via};
use crate::ocr::{CellResult, GrayImage, Layout, OcrEngine, Universe};
use crate::ttf::TtfFont;

// ---------------------------------------------------------------------------
// Name structure
// ---------------------------------------------------------------------------

/// Split a roster name into its SEMANTIC lines:
/// `"<Gear> Prime"`, the part words, and `"Blueprint"` when the
/// name is a blueprint form (a main blueprint's part line IS
/// "Blueprint"). This is the structure the form rules and the
/// exemplar keys reason about. The game's DISPLAY breaking is
/// a width-driven word wrap of the full name (a short name
/// renders on one line: "Glaive Prime Blade"; a long one wraps
/// at word boundaries), which is a different function — see
/// [`Scorer::wrapped_lines`], fitted per session.
pub fn name_lines(name: &str) -> Vec<String> {
    let (base, blueprint) = match name.strip_suffix(" Blueprint") {
        Some(b) => (b, true),
        None => (name, false),
    };
    let mut lines = Vec::new();
    match base.find(" Prime") {
        Some(pos) => {
            lines.push(base[..pos + 6].to_string());
            let part = base[pos + 6..].trim();
            if !part.is_empty() {
                lines.push(part.to_string());
            }
        }
        None => lines.push(base.to_string()),
    }
    if blueprint {
        lines.push("Blueprint".to_string());
    }
    lines
}

/// The gear portion of a roster name (everything before
/// " Prime"; the whole first token when there is no " Prime").
pub fn gear_of(name: &str) -> String {
    let base = name.strip_suffix(" Blueprint").unwrap_or(name);
    match base.find(" Prime") {
        Some(pos) => base[..pos].to_string(),
        None => base.split(' ').next().unwrap_or(base).to_string(),
    }
}

/// The part portion (after " Prime", minus a trailing
/// "Blueprint"); empty for a main blueprint.
pub fn part_of(name: &str) -> String {
    let base = name.strip_suffix(" Blueprint").unwrap_or(name);
    match base.find(" Prime") {
        Some(pos) => base[pos + 6..].trim().to_string(),
        None => String::new(),
    }
}

/// The session sort key (regrid finding, refined against both
/// censuses at 260/261): gear ascending; within a gear the MAIN
/// blueprint sorts last; then part ascending; within a part,
/// blueprint before built; then the full name.
pub fn sort_key(name: &str) -> (String, u8, String, u8, String) {
    let gear = gear_of(name).to_lowercase();
    let part = part_of(name);
    let blueprint = name.ends_with(" Blueprint");
    let main_bp = blueprint && part.is_empty();
    (
        gear,
        if main_bp { 1 } else { 0 },
        part.to_lowercase(),
        if blueprint { 0 } else { 1 },
        name.to_lowercase(),
    )
}

// ---------------------------------------------------------------------------
// Observed lines and masked NCC
// ---------------------------------------------------------------------------

/// One observed text line of a slot: the top-hat ink map of the
/// detected line box (original pixels), its ink mass and ink
/// extent, and a dilated ink mask for NCC support.
#[derive(Debug, Clone)]
pub struct LineObs {
    pub ink: GrayImage,
    pub mass: f32,
    pub ink_x0: u32,
    pub ink_x1: u32,
    /// Dilated ink mask (255 = in support), same size as `ink`.
    pub mask: GrayImage,
}

impl LineObs {
    pub fn ink_width(&self) -> u32 {
        self.ink_x1.saturating_sub(self.ink_x0)
    }
}

/// A slot's observed lines (canonical crop), in order.
#[derive(Debug, Clone)]
pub struct SlotObs {
    pub lines: Vec<LineObs>,
    /// Top-hat mass per line (count-penalty accounting).
    pub total_mass: f32,
    /// The name band's width in px — the wrap model expresses
    /// the game's line-breaking width as a fraction of it.
    pub band_w: u32,
}

fn observe_line(band: &GrayImage, det: &LineDetection, idx: usize) -> Option<LineObs> {
    let b = det.boxes.get(idx)?;
    let crop = band.crop(b.x0, b.y0, (b.x1 - b.x0).max(1), (b.y1 - b.y0).max(1));
    let ink = lineread::top_hat(&crop);
    let (w, h) = (ink.width, ink.height);
    if w == 0 || h == 0 {
        return None;
    }
    let mut mass = 0.0f32;
    let mut max_v = 0.0f32;
    for &p in &ink.pixels {
        mass += p as f32;
        max_v = max_v.max(p as f32);
    }
    let thr = (0.12 * max_v).max(8.0);
    let (mut x0, mut x1) = (w, 0u32);
    for x in 0..w {
        let mut col = 0.0f32;
        for y in 0..h {
            col += ink.get(x, y) as f32;
        }
        if col > thr * h as f32 * 0.10 {
            x0 = x0.min(x);
            x1 = x1.max(x + 1);
        }
    }
    if x1 == 0 {
        x0 = 0;
        x1 = w;
    }
    // Dilated ink mask (1px) for NCC support.
    let mut mask = GrayImage::blank(w, h, 0);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut on = false;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx >= 0 && yy >= 0 && xx < w as i32 && yy < h as i32 {
                        if ink.get(xx as u32, yy as u32) as f32 > thr {
                            on = true;
                        }
                    }
                }
            }
            if on {
                mask.set(x as u32, y as u32, 255);
            }
        }
    }
    Some(LineObs {
        ink,
        mass,
        ink_x0: x0,
        ink_x1: x1,
        mask,
    })
}

/// Observe a slot's lines from its canonical cell crop.
/// Detection boxes whose ink touches BOTH band edges are not
/// text lines (text is centred with margins; an icon's bottom
/// edge bleeding into the band spans the full width) and are
/// dropped — the line detector's block logic counts such a
/// run when it carries mass (the census's Akstiletto slot
/// detects 3 boxes for a 2-line name), and a junk box would
/// break the 1:1 line comparison synthesis depends on.
pub fn observe_slot(cell: &GrayImage, layout: &Layout) -> (SlotObs, LineDetection) {
    let band = lineread::name_band(cell, layout);
    let det = lineread::detect_lines(&band);
    let mut lines = Vec::with_capacity(det.boxes.len());
    for i in 0..det.boxes.len() {
        if let Some(l) = observe_line(&band, &det, i) {
            let touches_both = l.ink_x0 <= 1 && l.ink_x1 >= band.width.saturating_sub(1);
            if !touches_both {
                lines.push(l);
            }
        }
    }
    let total_mass = lines.iter().map(|l| l.mass).sum();
    (
        SlotObs {
            lines,
            total_mass,
            band_w: band.width,
        },
        det,
    )
}

/// Ink bounding box (x0, y0) of a coverage/ink image.
fn ink_origin(img: &GrayImage, thr: u8) -> Option<(u32, u32)> {
    let (mut x0, mut y0) = (u32::MAX, u32::MAX);
    for y in 0..img.height {
        for x in 0..img.width {
            if img.get(x, y) > thr {
                x0 = x0.min(x);
                y0 = y0.min(y);
            }
        }
    }
    if x0 == u32::MAX {
        None
    } else {
        Some((x0, y0))
    }
}

/// Masked NCC between an observed ink line and another ink
/// image (a render, or an exemplar's ink), anchored by ink
/// bounding-box origins and searched over shifts. The support
/// is the union of the observed dilated mask and the guest's
/// ink at the sampled position. Returns 0 when the support is
/// too small to mean anything.
///
/// The shift range is deliberately asymmetric and wide in Y
/// (and moderately wide in X): the first line's detected box
/// merges the item icon's lower rim with the text (their
/// top-hat row profiles touch), so the observed ink origin
/// can sit several px above — and, for wide icons, several px
/// beside — the text's own origin. A ±2px search around the
/// ink origins scored icon-overprinted lines at ~0 against
/// their TRUE renders (measured on the census: Neuroptics
/// blueprint lines were the worst), while unobstructed lines
/// scored 0.55–0.78. The search IS the alignment; the width
/// pre-filter and the candidate margins carry the
/// discrimination a wide search gives up.
pub fn masked_ncc(obs: &LineObs, guest: &GrayImage) -> f32 {
    let (Some((ox, oy)), Some((gx, gy))) = (
        ink_origin(&obs.ink, 24),
        ink_origin(guest, 24),
    ) else {
        return 0.0;
    };
    let mut best = f32::MIN;
    for dy in -3i32..=10 {
        for dx in -6i32..=6 {
            // Guest coordinate for observed (x, y):
            // align ink origins, then shift.
            let offx = gx as i32 - ox as i32 + dx;
            let offy = gy as i32 - oy as i32 + dy;
            let (mut n, mut sa, mut sb, mut saa, mut sbb, mut sab) =
                (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for y in 0..obs.ink.height as i32 {
                for x in 0..obs.ink.width as i32 {
                    let (gxx, gyy) = (x + offx, y + offy);
                    let gv = if gxx >= 0
                        && gyy >= 0
                        && gxx < guest.width as i32
                        && gyy < guest.height as i32
                    {
                        guest.get(gxx as u32, gyy as u32) as f32
                    } else {
                        0.0
                    };
                    let in_mask = obs.mask.get(x as u32, y as u32) > 0 || gv > 24.0;
                    if !in_mask {
                        continue;
                    }
                    let a = obs.ink.get(x as u32, y as u32) as f32;
                    n += 1.0;
                    sa += a;
                    sb += gv;
                    saa += a * a;
                    sbb += gv * gv;
                    sab += a * gv;
                }
            }
            if n < 40.0 {
                continue;
            }
            let cov = sab - sa * sb / n;
            let va = saa - sa * sa / n;
            let vb = sbb - sb * sb / n;
            if va <= 1.0 || vb <= 1.0 {
                continue;
            }
            let ncc = cov / (va * vb).sqrt();
            if ncc > best {
                best = ncc;
            }
        }
    }
    if best == f32::MIN {
        0.0
    } else {
        best
    }
}

// ---------------------------------------------------------------------------
// The scorer: renders under a fitted coverage curve
// ---------------------------------------------------------------------------

/// Rasteriser parameters, fitted per session by calibration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitParams {
    /// Em size in output px.
    pub px_size: f32,
    pub blur: f32,
    pub level: f32,
    pub gamma: f32,
    /// The game's word-wrap width as a fraction of the name
    /// band's width (fitted jointly with the size: the wrap is
    /// only reproduced at the size the game rendered).
    pub wrap_frac: f32,
}

impl Default for FitParams {
    fn default() -> Self {
        Self {
            px_size: 17.0,
            blur: 0.7,
            level: 0.05,
            gamma: 1.15,
            wrap_frac: 0.94,
        }
    }
}

pub struct Scorer {
    font: TtfFont,
    fit: FitParams,
    cache: HashMap<String, GrayImage>,
}

impl Scorer {
    pub fn new(font: TtfFont, fit: FitParams) -> Self {
        Self {
            font,
            fit,
            cache: HashMap::new(),
        }
    }

    pub fn fit(&self) -> FitParams {
        self.fit
    }

    pub fn set_fit(&mut self, fit: FitParams) {
        if fit != self.fit {
            self.fit = fit;
            self.cache.clear();
        }
    }

    /// Render one line's ink under the current fit (cached by
    /// text; the fit is fixed between set_fit calls).
    pub fn render(&mut self, text: &str) -> GrayImage {
        if let Some(img) = self.cache.get(text) {
            return img.clone();
        }
        let img = self.font.render_line(
            text,
            self.fit.px_size,
            self.fit.blur,
            self.fit.level,
            self.fit.gamma,
            3,
        );
        self.cache.insert(text.to_string(), img.clone());
        img
    }

    /// The ink-width pre-filter: a candidate line whose
    /// rendered ink width disagrees with the observed line's
    /// by more than the tolerance is not the same string, at
    /// any correlation — centred lines make width a hard
    /// measurement, not a soft feature. On the FIRST line the
    /// rule is one-sided: the item icon overprints that line
    /// and can only ADD observed ink (widening the observed
    /// extent past the text's), so a render narrower than the
    /// observation is not evidence against the candidate
    /// there; a render WIDER than the observation remains
    /// impossible on every line.
    fn width_rejects(&mut self, obs: &LineObs, text: &str, first_line: bool) -> bool {
        let rend = self.render(text);
        let Some((rx0, _)) = ink_origin(&rend, 24) else {
            return true;
        };
        let mut rx1 = 0u32;
        for x in 0..rend.width {
            for y in 0..rend.height {
                if rend.get(x, y) > 24 {
                    rx1 = rx1.max(x + 1);
                }
            }
        }
        let rw = rx1.saturating_sub(rx0) as f32;
        let ow = obs.ink_width() as f32;
        let tol = (0.22 * ow).max(5.0);
        if first_line {
            rw - ow > tol
        } else {
            (rw - ow).abs() > tol
        }
    }

    /// Score one observed line against one candidate line's
    /// render. `None` = rejected by the width pre-filter.
    pub fn line_score(&mut self, obs: &LineObs, text: &str, first_line: bool) -> Option<f32> {
        if self.width_rejects(obs, text, first_line) {
            return None;
        }
        let rend = self.render(text);
        Some(masked_ncc(obs, &rend))
    }

    /// The game's display breaking of a name under the
    /// current fit: greedy word wrap at the fitted wrap width
    /// (font advances at the fitted size — the wrap only
    /// reproduces at the size the game rendered). A word
    /// longer than the wrap width takes its own line.
    pub fn wrapped_lines(&self, name: &str, band_w: u32) -> Vec<String> {
        let wrap_px = self.fit.wrap_frac * band_w as f32;
        let scale = self.fit.px_size / self.font.units_per_em();
        let space = self
            .font
            .glyph_id(' ')
            .map(|g| self.font.advance_units(g) * scale)
            .unwrap_or(0.0);
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cur_w = 0.0f32;
        for word in name.split(' ') {
            let ww = self.font.text_advance_units(word) * scale;
            if cur.is_empty() {
                cur = word.to_string();
                cur_w = ww;
            } else if cur_w + space + ww <= wrap_px {
                cur.push(' ');
                cur.push_str(word);
                cur_w += space + ww;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = word.to_string();
                cur_w = ww;
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }

    /// Mean line score of a name's wrapped lines against a
    /// slot's observed lines (shared prefix only, no count
    /// penalty), less a count-mismatch term: the calibration
    /// objective. The wrap is part of the fit, so the
    /// objective must charge for predicting the wrong number
    /// of lines, not just for correlating the shared ones.
    pub fn lines_mean(&mut self, slot: &SlotObs, name: &str) -> f32 {
        let texts = self.wrapped_lines(name, slot.band_w);
        let k = texts.len().min(slot.lines.len());
        if k == 0 {
            return 0.0;
        }
        let mut sum = 0.0f32;
        for i in 0..k {
            sum += self.line_score(&slot.lines[i], &texts[i], i == 0).unwrap_or(0.0);
        }
        let count_err = (texts.len() as i32 - slot.lines.len() as i32).abs() as f32;
        sum / k as f32 - 0.08 * count_err
    }

    /// Full candidate score for a slot: mass-weighted line
    /// NCC over the shared lines, penalised for line-count
    /// mismatch in both directions (undetected candidate ink
    /// and unexplained observed ink both cost their mass
    /// share). `width_fail` reports a pre-filter kill on any
    /// shared line.
    pub fn candidate_score(&mut self, slot: &SlotObs, name: &str) -> CandScore {
        self.candidate_score_w(slot, name, None)
    }

    /// Candidate score with per-line weights derived from the
    /// slot's candidate SET: a line's weight is its share of
    /// the set whose text at that line index differs (the
    /// set-level discriminative fraction), times the line's
    /// observed mass. Lines every candidate shares — line 1,
    /// "Gear Prime", across a gear's siblings — carry almost
    /// no weight, so the score and the margins are decided by
    /// the lines that actually discriminate the field, the
    /// same principle as the shared hold predicate's
    /// discriminative-token rule for the OCR paths. The raw
    /// per-line scores are still reported unweighted.
    pub fn candidate_score_in_set(
        &mut self,
        slot: &SlotObs,
        name: &str,
        set: &[String],
    ) -> CandScore {
        if set.len() < 2 {
            return self.candidate_score(slot, name);
        }
        let wrapped: Vec<Vec<String>> = set
            .iter()
            .map(|n| self.wrapped_lines(n, slot.band_w))
            .collect();
        let max_len = wrapped.iter().map(|w| w.len()).max().unwrap_or(0);
        let mut weights = Vec::with_capacity(max_len);
        for i in 0..max_len {
            // Frequency of the most common line-i text
            // (absence counts as its own value).
            let mut counts: std::collections::HashMap<Option<&str>, usize> =
                std::collections::HashMap::new();
            for w in &wrapped {
                *counts.entry(w.get(i).map(|s| s.as_str())).or_default() += 1;
            }
            let modal = counts.values().copied().max().unwrap_or(0) as f32;
            weights.push(1.0 - modal / set.len() as f32);
        }
        self.candidate_score_w(slot, name, Some(&weights))
    }

    fn candidate_score_w(
        &mut self,
        slot: &SlotObs,
        name: &str,
        line_weights: Option<&[f32]>,
    ) -> CandScore {
        let texts = self.wrapped_lines(name, slot.band_w);
        let shared = texts.len().min(slot.lines.len());
        if shared == 0 || slot.total_mass <= 0.0 {
            return CandScore {
                score: 0.0,
                per_line: Vec::new(),
                width_fail: false,
            };
        }
        // Effective per-line weights: observed mass times the
        // set-discriminative weight. If the weights annihilate
        // every shared line (the field differs only in lines
        // beyond the shared prefix — e.g. a built form against
        // its blueprint sibling alone), fall back to raw mass:
        // the count penalties below then carry the decision.
        let weight_of = |i: usize| -> f32 {
            line_weights
                .and_then(|w| w.get(i).copied())
                .unwrap_or(1.0)
        };
        let weighted_total: f32 = (0..shared)
            .map(|i| slot.lines[i].mass * weight_of(i))
            .sum();
        let use_weights = weighted_total > 0.0;
        let mut per_line = Vec::with_capacity(shared);
        let mut width_fails = 0usize;
        let mut wsum = 0.0f32;
        let mut msum = 0.0f32;
        for i in 0..shared {
            let m = if use_weights {
                slot.lines[i].mass * weight_of(i)
            } else {
                slot.lines[i].mass
            };
            match self.line_score(&slot.lines[i], &texts[i], i == 0) {
                Some(s) => {
                    per_line.push(s);
                    wsum += s * m;
                    msum += m;
                }
                None => {
                    width_fails += 1;
                    per_line.push(0.0);
                    msum += m;
                }
            }
        }
        // The hard width kill fires only when EVERY shared
        // line is the wrong width: such a candidate cannot be
        // this cell's text under any breaking. A single
        // rejected line is likelier a wrap-model disagreement
        // (my fitted breaking vs the game's, or a detector
        // split/merge) than a wrong name, and costs only that
        // line's mass share above.
        let width_fail = shared > 0 && width_fails == shared;
        let mut score = if msum > 0.0 { wsum / msum } else { 0.0 };
        if slot.lines.len() > texts.len() {
            // Observed ink the candidate does not explain.
            let extra: f32 = slot.lines[texts.len()..].iter().map(|l| l.mass).sum();
            score *= 1.0 - (extra / slot.total_mass).min(0.9);
        }
        if texts.len() > slot.lines.len() {
            // Candidate ink the detector never saw: cost its
            // share of the candidate's rendered ink mass.
            let total: f32 = texts
                .iter()
                .map(|t| {
                    let r = self.render(t);
                    r.pixels.iter().map(|&p| p as f32).sum::<f32>()
                })
                .sum();
            let missing: f32 = texts[slot.lines.len()..]
                .iter()
                .map(|t| {
                    let r = self.render(t);
                    r.pixels.iter().map(|&p| p as f32).sum::<f32>()
                })
                .sum();
            if total > 0.0 {
                score *= 1.0 - (missing / total).min(0.9);
            }
        }
        CandScore {
            score,
            per_line,
            width_fail,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CandScore {
    pub score: f32,
    pub per_line: Vec<f32>,
    pub width_fail: bool,
}

// ---------------------------------------------------------------------------
// Calibration
// ---------------------------------------------------------------------------

/// One calibration slot: a both-readers-agreement slot whose
/// held name is its truth for fitting purposes.
pub struct CalibSlot {
    pub obs: SlotObs,
    pub truth: String,
    /// Candidate-evidence reads (band raw + line assembled),
    /// for best-wrong enumeration.
    pub reads: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CalibReport {
    pub fit: FitParams,
    pub slots: usize,
    /// True-candidate full scores, one per calibration slot.
    pub true_scores: Vec<f32>,
    /// Best wrong-candidate full scores.
    pub wrong_scores: Vec<f32>,
    /// Per-slot true-minus-wrong margins.
    pub margins: Vec<f32>,
    /// Exemplar scores of the true name's shared lines
    /// (leave-one-out), where exemplars existed.
    pub exemplar_scores: Vec<f32>,
    pub floor: f32,
    pub margin_thr: f32,
    pub grey_thr: f32,
    pub ex_floor: f32,
    /// [wrong p98, true p02] — the plateau the floor sits in;
    /// `lo > hi` means the distributions overlap there.
    pub plateau: (f32, f32),
}

fn quantile(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f32 * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn sorted(v: &[f32]) -> Vec<f32> {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// Wrong candidates for a calibration slot: the enumerated
/// evidence candidates, the truth's form sibling (THE critical
/// discrimination), and the truth's gear siblings.
fn wrong_candidates(
    slot: &CalibSlot,
    universe: &Universe,
    held_elsewhere: &HashSet<String>,
) -> Vec<String> {
    let mut ev = Evidence {
        band_held: None,
        line_held: None,
        names: Vec::new(),
        reads: slot.reads.clone(),
    };
    let mut cands = build_candidates(&mut ev, universe, held_elsewhere, None, None);
    if let Some(sib) = universe.form_sibling(&slot.truth) {
        if !cands.contains(&sib) {
            cands.push(sib);
        }
    }
    let gear = gear_of(&slot.truth);
    for n in universe.names() {
        if n != &slot.truth && gear_of(n) == gear && !cands.contains(n) {
            cands.push(n.clone());
        }
    }
    cands.retain(|c| c != &slot.truth);
    cands
}

/// Fit the rasteriser and choose the decision thresholds from
/// one session's calibration slots. Returns None when there
/// are too few slots to fit honestly.
pub fn calibrate(
    font: &TtfFont,
    slots: &[CalibSlot],
    universe: &Universe,
    held_elsewhere: &HashSet<String>,
    exemplar_bank: &ExemplarBank,
) -> Option<CalibReport> {
    if slots.len() < 12 {
        return None;
    }
    let mut scorer = Scorer::new(font.clone(), FitParams::default());
    let subset_a = &slots[..slots.len().min(48)];
    let objective = |scorer: &mut Scorer, subset: &[CalibSlot]| -> f32 {
        subset
            .iter()
            .map(|s| scorer.lines_mean(&s.obs, &s.truth))
            .sum::<f32>()
            / subset.len() as f32
    };
    // Stage A1: geometry first (size x wrap) at the default
    // coverage curve — the wrap only reproduces at the game's
    // size, so the two are fitted jointly, on the cheap axes.
    let mut best: Option<(f32, FitParams)> = None;
    for &px in &[15.0f32, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0] {
        for &wrap in &[0.88f32, 0.91, 0.94, 0.97, 1.0] {
            let fit = FitParams {
                px_size: px,
                wrap_frac: wrap,
                ..FitParams::default()
            };
            scorer.set_fit(fit);
            let mean = objective(&mut scorer, subset_a);
            if best.map_or(true, |(b, _)| mean > b) {
                best = Some((mean, fit));
            }
        }
    }
    let (_, mut fit) = best?;
    // Stage A2: the coverage curve at that geometry.
    let mut best_a2: Option<(f32, FitParams)> = None;
    for &gamma in &[0.7f32, 0.9, 1.15, 1.45] {
        for &blur in &[0.3f32, 0.7, 1.1] {
            for &level in &[0.0f32, 0.08, 0.16] {
                let cand = FitParams {
                    gamma,
                    blur,
                    level,
                    ..fit
                };
                scorer.set_fit(cand);
                let mean = objective(&mut scorer, subset_a);
                if best_a2.map_or(true, |(b, _)| mean > b) {
                    best_a2 = Some((mean, cand));
                }
            }
        }
    }
    if let Some((_, f)) = best_a2 {
        fit = f;
    }
    // Stage B: refine geometry around the winner on a wider
    // subset.
    let subset_b = &slots[..slots.len().min(96)];
    let mut best_b: Option<(f32, FitParams)> = None;
    for &dpx in &[-0.5f32, 0.0, 0.5] {
        for &dw in &[-0.015f32, 0.0, 0.015] {
            for &dg in &[-0.15f32, 0.0, 0.15] {
                let cand = FitParams {
                    px_size: (fit.px_size + dpx).clamp(13.0, 23.0),
                    wrap_frac: (fit.wrap_frac + dw).clamp(0.8, 1.05),
                    blur: fit.blur,
                    level: fit.level,
                    gamma: (fit.gamma + dg).clamp(0.5, 2.0),
                };
                scorer.set_fit(cand);
                let mean: f32 = subset_b
                    .iter()
                    .map(|s| scorer.lines_mean(&s.obs, &s.truth))
                    .sum::<f32>()
                    / subset_b.len() as f32;
                if best_b.map_or(true, |(b, _)| mean > b) {
                    best_b = Some((mean, cand));
                }
            }
        }
    }
    if let Some((_, f)) = best_b {
        fit = f;
    }
    scorer.set_fit(fit);
    // Distributions over ALL calibration slots at the fit.
    let mut true_scores = Vec::with_capacity(slots.len());
    let mut wrong_scores = Vec::with_capacity(slots.len());
    let mut margins = Vec::with_capacity(slots.len());
    let mut exemplar_scores = Vec::new();
    for (i, s) in slots.iter().enumerate() {
        let wrongs = wrong_candidates(s, universe, held_elsewhere);
        // The field the truth must beat at this slot: itself
        // plus its enumerated wrongs — scores are computed
        // in-set, so the distributions measure the same
        // discriminative quantity decide() thresholds.
        let mut field = wrongs.clone();
        field.push(s.truth.clone());
        // Same effective-score transform decide() applies:
        // a width-pre-filter kill is -1, never a soft score.
        let eff = |cs: &CandScore| if cs.width_fail { -1.0 } else { cs.score };
        let t = eff(&scorer.candidate_score_in_set(&s.obs, &s.truth, &field));
        let w = wrongs
            .iter()
            .map(|c| eff(&scorer.candidate_score_in_set(&s.obs, c, &field)))
            .fold(0.0f32, f32::max);
        true_scores.push(t);
        wrong_scores.push(w);
        margins.push(t - w);
        // Exemplar corroboration of the truth (leave-one-out:
        // slot i's own crops are excluded by index).
        if let Some(es) = exemplar_bank.score_slot(&s.obs, &s.truth, i) {
            exemplar_scores.push(es);
        }
    }
    let ts = sorted(&true_scores);
    let ws = sorted(&wrong_scores);
    let ms = sorted(&margins);
    let plateau = (quantile(&ws, 0.98), quantile(&ts, 0.02));
    let floor = if plateau.1 > plateau.0 {
        (plateau.0 + plateau.1) / 2.0
    } else {
        // Overlapping distributions: sit above the wrong mass
        // and let the margin + exemplar rules carry the rest.
        quantile(&ws, 0.995) + 0.01
    };
    let margin_thr = (quantile(&ms, 0.10) / 2.0).max(0.015);
    let grey_thr = quantile(&ms, 0.35).max(margin_thr + 0.01);
    let ex_floor = if exemplar_scores.len() >= 8 {
        let es = sorted(&exemplar_scores);
        (quantile(&es, 0.05) - 0.03).max(0.3)
    } else {
        0.55
    };
    Some(CalibReport {
        fit,
        slots: slots.len(),
        true_scores,
        wrong_scores,
        margins,
        exemplar_scores,
        floor,
        margin_thr,
        grey_thr,
        ex_floor,
        plateau,
    })
}

// ---------------------------------------------------------------------------
// Exemplars
// ---------------------------------------------------------------------------

/// Real line crops harvested from held cells: line 1 by gear,
/// part lines by part word. Compared in ink space, so no
/// rasteriser error enters this channel.
#[derive(Default)]
pub struct ExemplarBank {
    /// (calibration slot index, line ink image) per gear.
    line1: HashMap<String, Vec<(usize, GrayImage)>>,
    part: HashMap<String, Vec<(usize, GrayImage)>>,
}

impl ExemplarBank {
    pub fn add_slot(&mut self, idx: usize, name: &str, obs: &SlotObs) {
        if let Some(l) = obs.lines.first() {
            let v = self.line1.entry(gear_of(name)).or_default();
            if v.len() < 5 {
                v.push((idx, l.ink.clone()));
            }
        }
        let part = part_of(name);
        if !part.is_empty() {
            if let Some(l) = obs.lines.get(1) {
                let v = self.part.entry(part.to_lowercase()).or_default();
                if v.len() < 5 {
                    v.push((idx, l.ink.clone()));
                }
            }
        }
    }

    /// Exemplar agreement for a candidate name against a slot:
    /// the best ink-space NCC over the shared lines' exemplars
    /// (line 1 from the same gear's held siblings, the part
    /// line from any gear's). `exclude` drops one calibration
    /// slot's own crops (leave-one-out in calibration); pass
    /// usize::MAX in adjudication.
    pub fn score_slot(&self, obs: &SlotObs, name: &str, exclude: usize) -> Option<f32> {
        let mut best: Option<f32> = None;
        if let Some(l) = obs.lines.first() {
            if let Some(v) = self.line1.get(&gear_of(name)) {
                for (i, ink) in v {
                    if *i == exclude {
                        continue;
                    }
                    // Exemplar ink as a guest image: wrap it as
                    // a LineObs-free NCC via a temp LineObs.
                    let s = ncc_ink(l, ink);
                    best = Some(best.map_or(s, |b: f32| b.max(s)));
                }
            }
        }
        let part = part_of(name);
        if !part.is_empty() {
            if let Some(l) = obs.lines.get(1) {
                if let Some(v) = self.part.get(&part.to_lowercase()) {
                    for (i, ink) in v {
                        if *i == exclude {
                            continue;
                        }
                        let s = ncc_ink(l, ink);
                        best = Some(best.map_or(s, |b: f32| b.max(s)));
                    }
                }
            }
        }
        best
    }
}

/// Ink-vs-ink NCC (exemplar channel): the observed line's mask
/// support against another observed line's ink, origin-anchored
/// like the render comparison.
fn ncc_ink(obs: &LineObs, guest_ink: &GrayImage) -> f32 {
    masked_ncc(obs, guest_ink)
}

// ---------------------------------------------------------------------------
// Candidate generation
// ---------------------------------------------------------------------------

/// The evidence one unheld slot carries.
pub struct Evidence {
    pub band_held: Option<String>,
    pub line_held: Option<String>,
    /// Names a reader's reads contested over, or held then
    /// refused (internal evidence beyond the two verdicts).
    pub names: Vec<String>,
    /// Raw evidence reads (band canonical + instances, line
    /// assembled + lines) for nominations.
    pub reads: Vec<String>,
}

/// Build a slot's candidate set: refused/contested holds,
/// matcher nominations (top-4 per read), radius-decoder
/// nominations, form siblings of all of those, and the
/// sort-interval set between its held panorama neighbours.
/// Filters: items held at other slots, and (per the brief)
/// nothing else — the form gate applies at decision time.
pub fn build_candidates(
    ev: &mut Evidence,
    universe: &Universe,
    held_elsewhere: &HashSet<String>,
    prev_held: Option<&str>,
    next_held: Option<&str>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |out: &mut Vec<String>, n: &str| {
        if !n.is_empty() && !out.iter().any(|x| x == n) {
            out.push(n.to_string());
        }
    };
    if let Some(b) = &ev.band_held {
        push(&mut out, b);
    }
    if let Some(l) = &ev.line_held {
        push(&mut out, l);
    }
    for n in &ev.names {
        push(&mut out, n);
    }
    for read in &ev.reads {
        for (name, _score) in universe.top_matches(read, 4) {
            push(&mut out, name);
        }
        if let Some(n) = universe.radius_decode_budget(read, crate::ocr::RADIUS_TAIL_BUDGET) {
            push(&mut out, &n);
        }
    }
    // Form siblings of everything so far.
    let base = out.clone();
    for n in &base {
        if let Some(sib) = universe.form_sibling(n) {
            push(&mut out, &sib);
        }
    }
    // Sort interval between held neighbours.
    let lo = prev_held.map(sort_key);
    let hi = next_held.map(sort_key);
    if lo.is_some() || hi.is_some() {
        for n in universe.names() {
            let k = sort_key(n);
            let above = lo.as_ref().map_or(true, |l| k > *l);
            let below = hi.as_ref().map_or(true, |h| k < *h);
            if above && below {
                push(&mut out, n);
            }
        }
    }
    out.retain(|n| !held_elsewhere.contains(n));
    out
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// Per-slot synthesis diagnostics (gate attribution + the
/// residue census).
#[derive(Debug, Clone)]
pub struct SynthDiag {
    pub candidates: Vec<(String, f32)>,
    pub best: Option<String>,
    pub best_score: f32,
    pub margin: f32,
    pub exemplar: Option<f32>,
    /// Stable outcome label: "held", "below-floor",
    /// "below-margin", "grey-no-exemplar", "form-gated",
    /// "form-unproven", "name-disagreement", "contested-form",
    /// "no-candidates", "uncalibrated".
    pub outcome: &'static str,
}

pub struct Decision {
    pub hold: Option<String>,
    pub diag: SynthDiag,
}

/// Decide one eligible slot. `form_verdict` is the slot's
/// channel verdict; `glyph` its glyph presence (for the
/// Unknown-verdict blueprint-line rule, form proof comes from
/// the pixels, never from synthesis assertion alone).
#[allow(clippy::too_many_arguments)]
pub fn decide(
    scorer: &mut Scorer,
    obs: &SlotObs,
    candidates: &[String],
    form_verdict: FormVerdict,
    universe: &Universe,
    calib: &CalibReport,
    exemplars: &ExemplarBank,
) -> Decision {
    let refuse = |outcome: &'static str, scored: Vec<(String, f32)>| Decision {
        hold: None,
        diag: SynthDiag {
            best: scored.first().map(|(n, _)| n.clone()),
            best_score: scored.first().map(|(_, s)| *s).unwrap_or(0.0),
            margin: if scored.len() > 1 {
                scored[0].1 - scored[1].1
            } else {
                scored.first().map(|(_, s)| *s).unwrap_or(0.0)
            },
            exemplar: None,
            candidates: scored,
            outcome,
        },
    };
    if form_verdict == FormVerdict::Contested {
        return refuse("contested-form", Vec::new());
    }
    // Form gate on the candidate set (channels decide form).
    let gated: Vec<String> = candidates
        .iter()
        .filter(|n| match form_verdict {
            FormVerdict::Blueprint => {
                crate::lineread::form_of(universe, n) != Some(crate::lineread::Form::Built)
            }
            FormVerdict::Built => {
                crate::lineread::form_of(universe, n) != Some(crate::lineread::Form::Blueprint)
            }
            _ => true,
        })
        .cloned()
        .collect();
    if gated.is_empty() {
        return refuse("form-gated", Vec::new());
    }
    let mut full: Vec<(String, CandScore)> = gated
        .iter()
        .map(|n| (n.clone(), scorer.candidate_score_in_set(obs, n, &gated)))
        .collect();
    let eff = |cs: &CandScore| if cs.width_fail { -1.0 } else { cs.score };
    full.sort_by(|a, b| {
        eff(&b.1)
            .partial_cmp(&eff(&a.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let scored: Vec<(String, f32)> =
        full.iter().map(|(n, cs)| (n.clone(), eff(cs))).collect();
    let (best_name, best_score) = scored[0].clone();
    let margin = if scored.len() > 1 {
        best_score - scored[1].1
    } else {
        best_score
    };
    if best_score < calib.floor {
        return refuse("below-floor", scored);
    }
    if margin < calib.margin_thr {
        return refuse("below-margin", scored);
    }
    // Unknown-form corroboration: a blueprint-form winner under
    // an abstaining verdict must prove its Blueprint word in
    // the crop. In display terms: the winner's wrapped lines
    // must all be present among the detections, and the last
    // wrapped line — the one carrying the "Blueprint" token,
    // alone or shared with the part words — must itself clear
    // the floor scaled to a single line.
    if form_verdict == FormVerdict::Unknown {
        if let Some(f) = crate::lineread::form_of(universe, &best_name) {
            if f == crate::lineread::Form::Blueprint {
                let wrapped = scorer.wrapped_lines(&best_name, obs.band_w);
                let per_line = &full[0].1.per_line;
                let ok = !wrapped.is_empty()
                    && wrapped.len() <= obs.lines.len()
                    && per_line.len() == wrapped.len()
                    && per_line[wrapped.len() - 1] >= calib.floor * 0.92;
                if !ok {
                    return refuse("form-unproven", scored);
                }
            }
        }
    }
    // Grey zone: a thin render margin needs exemplar agreement.
    let mut ex_score = None;
    if margin < calib.grey_thr {
        ex_score = exemplars.score_slot(obs, &best_name, usize::MAX);
        match ex_score {
            Some(s) if s >= calib.ex_floor => {}
            _ => {
                let mut d = refuse("grey-no-exemplar", scored);
                d.diag.exemplar = ex_score;
                return d;
            }
        }
    }
    Decision {
        hold: Some(best_name.clone()),
        diag: SynthDiag {
            candidates: scored,
            best: Some(best_name),
            best_score,
            margin,
            exemplar: ex_score,
            outcome: "held",
        },
    }
}

// ---------------------------------------------------------------------------
// Session post-pass over the session's StitchReports
// ---------------------------------------------------------------------------

/// The calibration record of one session, in reportable form:
/// the fitted rasteriser parameters, the measured score
/// distributions (quantiles), and the thresholds chosen from
/// them. Surfaced on the stitch report for the finalize log
/// and the gate record.
#[derive(Debug, Clone)]
pub struct SynthCalibSummary {
    pub slots: usize,
    pub fit: FitParams,
    pub floor: f32,
    pub margin_thr: f32,
    pub grey_thr: f32,
    pub ex_floor: f32,
    pub plateau: (f32, f32),
    /// True-candidate score quantiles [p02, p25, p50, p75, p98].
    pub true_q: [f32; 5],
    /// Best-wrong-candidate score quantiles, same points.
    pub wrong_q: [f32; 5],
    /// True-margin quantiles [p05, p10, p35, p50, p90].
    pub margin_q: [f32; 5],
    pub exemplar_n: usize,
    /// Exemplar true-line score quantiles [p05, p50, p95].
    pub exemplar_q: [f32; 3],
}

impl From<&CalibReport> for SynthCalibSummary {
    fn from(c: &CalibReport) -> Self {
        let t = sorted(&c.true_scores);
        let w = sorted(&c.wrong_scores);
        let m = sorted(&c.margins);
        let e = sorted(&c.exemplar_scores);
        Self {
            slots: c.slots,
            fit: c.fit,
            floor: c.floor,
            margin_thr: c.margin_thr,
            grey_thr: c.grey_thr,
            ex_floor: c.ex_floor,
            plateau: c.plateau,
            true_q: [
                quantile(&t, 0.02),
                quantile(&t, 0.25),
                quantile(&t, 0.50),
                quantile(&t, 0.75),
                quantile(&t, 0.98),
            ],
            wrong_q: [
                quantile(&w, 0.02),
                quantile(&w, 0.25),
                quantile(&w, 0.50),
                quantile(&w, 0.75),
                quantile(&w, 0.98),
            ],
            margin_q: [
                quantile(&m, 0.05),
                quantile(&m, 0.10),
                quantile(&m, 0.35),
                quantile(&m, 0.50),
                quantile(&m, 0.90),
            ],
            exemplar_n: c.exemplar_scores.len(),
            exemplar_q: [
                quantile(&e, 0.05),
                quantile(&e, 0.50),
                quantile(&e, 0.95),
            ],
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct SynthSummary {
    pub calibrated: bool,
    pub calib_slots: usize,
    pub held: usize,
    pub refused: usize,
    pub name_disagreements: usize,
    pub eligible: usize,
}

/// Adjudicate one capture session's unheld slots, over the
/// session's merged stitch reports (pass 36 integration: the
/// post-pass runs once at finalize, NOT per bundle — one
/// calibration over the session's pooled both-readers-held
/// slots, one exclusion set over the session's pooled holds,
/// exactly the whole-session semantics v0.5.29 measured when a
/// session produced a single report). Mutates the reports in
/// place (held verdicts, `held_via = Synthesis`, per-slot
/// diagnostics, the session calibration record on each) and
/// returns the summary. A session that cannot calibrate (too
/// few agreement slots, font failure) is left untouched.
pub fn adjudicate_session(
    reports: &mut [StitchReport],
    layout: &Layout,
    engine: &dyn OcrEngine,
    read_opts: &ReadOptions,
    universe: &Universe,
) -> SynthSummary {
    let mut summary = SynthSummary::default();
    let font = match crate::ttf::embedded_font() {
        Ok(f) => f,
        Err(_) => return summary,
    };
    // Names held anywhere in the session (exclusion set for
    // candidates), including glyph-implied twins — pooled over
    // every bundle report.
    let mut held_elsewhere: HashSet<String> = HashSet::new();
    for report in reports.iter() {
        for s in &report.slots {
            if s.coverage == SlotCoverage::Held {
                if let Some(n) = s.result.as_ref().and_then(|r| r.matched.clone()) {
                    held_elsewhere.insert(n);
                }
            }
            if let Some(imp) = &s.glyph_implied {
                held_elsewhere.insert(imp.clone());
            }
        }
    }
    summary.name_disagreements = reports
        .iter()
        .flat_map(|r| r.slots.iter())
        .filter(|s| s.refuse_reason == Some("name-disagreement"))
        .count();

    // Collect the slots this post-pass touches: calibration
    // (held via Both) and eligible (seen-not-held), pooled over
    // all bundle reports in session order. The canonical crops
    // were retained on exactly these slots when each bundle was
    // stitched (the bundle frames themselves are gone by
    // finalize), so no plan re-derivation is needed here; the
    // canonical crop is the synthesis subject by design, and
    // instance crops are never re-derived.
    struct Work {
        report_i: usize,
        slot_i: usize,
        cell: GrayImage,
        obs: SlotObs,
    }
    let mut calib_work: Vec<Work> = Vec::new();
    let mut elig_work: Vec<Work> = Vec::new();
    for (ri, report) in reports.iter().enumerate() {
        for (si, rep) in report.slots.iter().enumerate() {
            let want_calib =
                rep.coverage == SlotCoverage::Held && rep.held_via == Some(Via::Both);
            let want_elig = rep.coverage == SlotCoverage::SeenNotHeld;
            if !want_calib && !want_elig {
                continue;
            }
            let Some(cell) = rep.canonical_crop.clone() else {
                continue;
            };
            let (obs, _det) = observe_slot(&cell, layout);
            let w = Work {
                report_i: ri,
                slot_i: si,
                cell,
                obs,
            };
            if want_calib {
                calib_work.push(w);
            } else {
                elig_work.push(w);
            }
        }
    }
    if calib_work.len() < 12 {
        return summary;
    }
    summary.calib_slots = calib_work.len();

    // Evidence re-reads of the canonical crops (band + line),
    // for calibration wrong-enumeration and eligible
    // nominations alike.
    let read_evidence = |cells: &[GrayImage]| -> Vec<Vec<String>> {
        let opt_cells: Vec<Option<GrayImage>> = cells.iter().cloned().map(Some).collect();
        let band = crate::capture::read_cells(&opt_cells, layout, engine, read_opts, universe);
        let line = lineread::read_cells_lines(&opt_cells, layout, engine, read_opts, universe);
        band.iter()
            .zip(line.iter())
            .map(|(b, l)| {
                let mut v = vec![b.raw_text.clone(), l.assembled.clone()];
                v.extend(l.lines.iter().cloned());
                v.retain(|s| !s.trim().is_empty());
                v
            })
            .collect()
    };
    let calib_cells: Vec<GrayImage> = calib_work.iter().map(|w| w.cell.clone()).collect();
    let calib_reads = read_evidence(&calib_cells);
    let elig_cells: Vec<GrayImage> = elig_work.iter().map(|w| w.cell.clone()).collect();
    let elig_reads = read_evidence(&elig_cells);

    // Exemplar bank from the calibration slots.
    let mut bank = ExemplarBank::default();
    let mut calib_slots: Vec<CalibSlot> = Vec::with_capacity(calib_work.len());
    for (i, w) in calib_work.iter().enumerate() {
        let truth = reports[w.report_i].slots[w.slot_i]
            .result
            .as_ref()
            .and_then(|r| r.matched.clone())
            .unwrap_or_default();
        bank.add_slot(i, &truth, &w.obs);
        calib_slots.push(CalibSlot {
            obs: w.obs.clone(),
            truth,
            reads: calib_reads[i].clone(),
        });
    }
    let Some(calib) = calibrate(&font, &calib_slots, universe, &held_elsewhere, &bank) else {
        return summary;
    };
    summary.calibrated = true;
    // One session calibration, recorded on every bundle report
    // so any consumer of the merged reports sees the same record.
    let calib_summary = SynthCalibSummary::from(&calib);
    for report in reports.iter_mut() {
        report.synth_calib = Some(calib_summary.clone());
    }
    let mut scorer = Scorer::new(font.clone(), calib.fit);

    // Held panorama neighbours per segment (sort-interval
    // bounds), in report order within each segment — computed
    // per owning bundle report: a slot's sort interval is
    // bounded by the held slots of its own bundle's panorama.
    let held_by_seg: Vec<HashMap<usize, Vec<(usize, String)>>> = reports
        .iter()
        .map(|report| {
            let mut m: HashMap<usize, Vec<(usize, String)>> = HashMap::new();
            for s in &report.slots {
                if s.coverage == SlotCoverage::Held {
                    if let Some(n) = s.result.as_ref().and_then(|r| r.matched.clone()) {
                        m.entry(s.segment).or_default().push((s_col_key(s), n));
                    }
                }
            }
            m
        })
        .collect();

    summary.eligible = elig_work.len();
    for (wi, w) in elig_work.iter().enumerate() {
        let rep = &reports[w.report_i].slots[w.slot_i];
        // Hold-vs-hold disagreements stay refused (contract).
        if rep.band_held.is_some() && rep.line_held.is_some() {
            continue;
        }
        // Panorama neighbours: nearest held slots before/after
        // this one in its segment's slot order.
        let (prev_held, next_held) = neighbours(&held_by_seg[w.report_i], rep);
        let mut ev = Evidence {
            band_held: rep.band_held.clone(),
            line_held: rep.line_held.clone(),
            names: Vec::new(),
            reads: elig_reads[wi].clone(),
        };
        let candidates = build_candidates(
            &mut ev,
            universe,
            &held_elsewhere,
            prev_held.as_deref(),
            next_held.as_deref(),
        );
        if candidates.is_empty() {
            reports[w.report_i].slots[w.slot_i].synth = Some(SynthDiag {
                candidates: Vec::new(),
                best: None,
                best_score: 0.0,
                margin: 0.0,
                exemplar: None,
                outcome: "no-candidates",
            });
            summary.refused += 1;
            continue;
        }
        let decision = decide(
            &mut scorer,
            &w.obs,
            &candidates,
            rep.form_verdict,
            universe,
            &calib,
            &bank,
        );
        let diag = decision.diag;
        if let Some(name) = decision.hold {
            let slot = &mut reports[w.report_i].slots[w.slot_i];
            let mut result = slot.result.clone().unwrap_or(CellResult {
                index: 0,
                raw_text: String::new(),
                confidence: 0.0,
                matched: None,
                match_score: 0.0,
                held: false,
                count: None,
                tier: crate::ocr::RefinementTier::Unknown,
                rank: None,
                gate_excluded: false,
            });
            result.matched = Some(name.clone());
            result.held = true;
            slot.result = Some(result);
            slot.coverage = SlotCoverage::Held;
            slot.held_via = Some(Via::Synthesis);
            // The glyph implication applies to a synthesis-held
            // blueprint exactly as to a reader-held one.
            if slot.glyph_score >= lineread::GLYPH_NCC_MIN
                && lineread::form_of(universe, &name) == Some(lineread::Form::Blueprint)
            {
                slot.glyph_implied = universe.form_sibling(&name);
            }
            slot.synth = Some(diag);
            held_elsewhere.insert(name);
            summary.held += 1;
        } else {
            reports[w.report_i].slots[w.slot_i].synth = Some(diag);
            summary.refused += 1;
        }
    }
    summary
}

fn s_col_key(s: &SlotReport) -> usize {
    // Panorama order within a segment is (row, col) ascending;
    // a monotone key suffices for neighbour search.
    ((s.row as i64 + 4096) as usize) * 64 + s.col as usize
}

fn neighbours(
    held_by_seg: &HashMap<usize, Vec<(usize, String)>>,
    rep: &SlotReport,
) -> (Option<String>, Option<String>) {
    let Some(v) = held_by_seg.get(&rep.segment) else {
        return (None, None);
    };
    let key = s_col_key(rep);
    let mut prev: Option<&(usize, String)> = None;
    let mut next: Option<&(usize, String)> = None;
    for e in v {
        if e.0 < key && prev.map_or(true, |p| e.0 > p.0) {
            prev = Some(e);
        }
        if e.0 > key && next.map_or(true, |n| e.0 < n.0) {
            next = Some(e);
        }
    }
    (
        prev.map(|e| e.1.clone()),
        next.map(|e| e.1.clone()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_lines_break_as_the_game_breaks() {
        assert_eq!(
            name_lines("Nezha Prime Systems Blueprint"),
            vec!["Nezha Prime", "Systems", "Blueprint"]
        );
        assert_eq!(
            name_lines("Nezha Prime Systems"),
            vec!["Nezha Prime", "Systems"]
        );
        assert_eq!(
            name_lines("Acceltra Prime Blueprint"),
            vec!["Acceltra Prime", "Blueprint"]
        );
        assert_eq!(
            name_lines("Wisp Prime Harness Blueprint"),
            vec!["Wisp Prime", "Harness", "Blueprint"]
        );
        // A bare gear name (the parts universe carries them)
        // has no part: one semantic line, empty part.
        assert_eq!(name_lines("Revenant Prime"), vec!["Revenant Prime"]);
        assert_eq!(part_of("Revenant Prime"), "");
        assert_eq!(part_of("Revenant Prime Blueprint"), "");
        assert_eq!(gear_of("Revenant Prime"), "Revenant");
    }

    #[test]
    fn sort_key_orders_gear_components_then_main_blueprint() {
        let a = sort_key("Nezha Prime Systems Blueprint");
        let b = sort_key("Nezha Prime Systems");
        assert!(a < b, "blueprint before built within a part");
        let main = sort_key("Nezha Prime Blueprint");
        assert!(main > a && main > b, "main blueprint last in gear");
        let other = sort_key("Nova Prime Systems");
        assert!(other > main, "gear dominates");
    }

    #[test]
    fn self_render_scores_high_and_wrong_render_low() {
        let font = crate::ttf::embedded_font().unwrap();
        let mut scorer = Scorer::new(font, FitParams::default());
        // A synthetic observed line: render text, then present
        // the coverage AS the observed ink (perfect world).
        let rend = scorer.render("Rubico Prime");
        let obs = LineObs {
            ink: rend.clone(),
            mass: rend.pixels.iter().map(|&p| p as f32).sum(),
            ink_x0: 3,
            ink_x1: rend.width - 3,
            mask: {
                let mut m = GrayImage::blank(rend.width, rend.height, 0);
                for (i, &p) in rend.pixels.iter().enumerate() {
                    if p > 24 {
                        m.pixels[i] = 255;
                    }
                }
                m
            },
        };
        let s = scorer.line_score(&obs, "Rubico Prime", true);
        assert!(s.map_or(false, |v| v > 0.9), "self score: {s:?}");
        let wrong = scorer.line_score(&obs, "Rubico Prime Systems", true);
        assert!(
            wrong.map_or(true, |v| v < 0.9),
            "a longer string must not win on the gear line alone: {wrong:?}"
        );
    }
}
