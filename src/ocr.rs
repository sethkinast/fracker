//! Inventory bootstrap OCR (BRIEF §8.1, addendum #21: OCR is the plan).
//!
//! End-to-end ingestion EXCEPT the live capture session (that happens
//! later, with the game on screen):
//! - input: a folder of inventory-grid screenshots (PGM/PPM files);
//! - grid segmentation + per-cell OCR behind a pluggable [`OcrEngine`]
//!   trait (a `tesseract` CLI engine when the binary exists on the box,
//!   otherwise a [`StubEngine`] that reports zero confidence —
//!   see [`engine_status`]);
//! - every read validated against the item universe (fuzzy match to a
//!   known name); low-confidence or unmatched reads are recorded as
//!   NOT held, never guessed;
//! - output: a stock state file (loose parts + relic counts, JSON) in
//!   the shape the planner consumes ([`StockState::to_relic_stock`]).
//!
//! Error asymmetry rule: an uncertain cell defaults to NOT held (a
//! redundant crack suggestion is cheap; wrongly pruning a needed part
//! is not).
//!
//! Image handling is dependency-free on purpose (offline Flatpak
//! build): 8-bit PGM/PPM only, which is all the pipeline needs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Gray image (8-bit PGM P5 / PPM P6, maxval 255)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl GrayImage {
    pub fn blank(width: u32, height: u32, value: u8) -> Self {
        Self {
            width,
            height,
            pixels: vec![value; (width * height) as usize],
        }
    }

    pub fn get(&self, x: u32, y: u32) -> u8 {
        self.pixels[(y * self.width + x) as usize]
    }

    pub fn set(&mut self, x: u32, y: u32, v: u8) {
        let i = (y * self.width + x) as usize;
        if i < self.pixels.len() {
            self.pixels[i] = v;
        }
    }

    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Self {
        let mut out = Self::blank(w, h, 0);
        for dy in 0..h {
            for dx in 0..w {
                out.set(dx, dy, self.get(x + dx, y + dy));
            }
        }
        out
    }

    pub fn mean(&self) -> f64 {
        if self.pixels.is_empty() {
            return 0.0;
        }
        self.pixels.iter().map(|&p| p as f64).sum::<f64>() / self.pixels.len() as f64
    }

    /// Nearest-neighbor integer downscale (synthetic/template path).
    pub fn downscale(&self, factor: u32) -> Self {
        assert!(factor >= 1);
        let w = self.width / factor;
        let h = self.height / factor;
        let mut out = Self::blank(w.max(1), h.max(1), 0);
        for y in 0..out.height {
            for x in 0..out.width {
                out.set(x, y, self.get(x * factor, y * factor));
            }
        }
        out
    }

    pub fn to_pgm_bytes(&self) -> Vec<u8> {
        let mut out = format!("P5\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.extend_from_slice(&self.pixels);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let (magic, rest) = split_token(bytes).ok_or("image: empty")?;
        match magic.as_slice() {
            b"P5" => Self::parse_gray(rest, 1),
            b"P6" => Self::parse_gray(rest, 3),
            _ => Err("image: only P5/P6 with maxval 255".to_owned()),
        }
    }

    fn parse_gray(mut rest: &[u8], channels: usize) -> Result<Self, String> {
        let (w, r1) = split_token(rest).ok_or("image: width")?;
        let (h, r2) = split_token(r1).ok_or("image: height")?;
        let (m, r3) = split_token(r2).ok_or("image: maxval")?;
        rest = r3;
        let width: u32 = std::str::from_utf8(&w)
            .map_err(|_| "image: width".to_owned())?
            .parse()
            .map_err(|_| "image: width".to_owned())?;
        let height: u32 = std::str::from_utf8(&h)
            .map_err(|_| "image: height".to_owned())?
            .parse()
            .map_err(|_| "image: height".to_owned())?;
        if m != b"255" {
            return Err("image: maxval must be 255".to_owned());
        }
        // Single whitespace byte separates header from raster.
        if rest.first().map(|b| b.is_ascii_whitespace()).unwrap_or(false) {
            rest = &rest[1..];
        }
        let need = (width as usize) * (height as usize) * channels;
        if rest.len() < need {
            return Err(format!(
                "image: short raster {} < {need}",
                rest.len()
            ));
        }
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize));
        if channels == 1 {
            pixels.extend_from_slice(&rest[..need]);
        } else {
            // Luma from RGB (integer approx).
            for px in rest[..need].chunks_exact(3) {
                let y = (px[0] as u32 * 30 + px[1] as u32 * 59 + px[2] as u32 * 11) / 100;
                pixels.push(y as u8);
            }
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }
}

/// Next whitespace-delimited header token, skipping `#` comments.
fn split_token(mut b: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    loop {
        while b.first().map(|c| c.is_ascii_whitespace()).unwrap_or(false) {
            b = &b[1..];
        }
        if b.first() == Some(&b'#') {
            while !b.is_empty() && b[0] != b'\n' {
                b = &b[1..];
            }
            continue;
        }
        break;
    }
    if b.is_empty() {
        return None;
    }
    let mut i = 0;
    while i < b.len() && !b[i].is_ascii_whitespace() {
        i += 1;
    }
    Some((b[..i].to_vec(), &b[i..]))
}

// ---------------------------------------------------------------------------
// Grid segmentation
// ---------------------------------------------------------------------------

/// Even grid split (the capture UI lays cells out uniformly; the live
/// session calibrates rows/cols once, then every page splits the same).
#[derive(Debug, Clone, Copy)]
pub struct GridSpec {
    pub rows: u32,
    pub cols: u32,
}

impl GridSpec {
    pub fn parse(s: &str) -> Result<Self, String> {
        let (c, r) = s
            .split_once('x')
            .ok_or("grid: want COLSxROWS, e.g. 5x4")?;
        Ok(Self {
            cols: c.parse().map_err(|_| "grid: cols")?,
            rows: r.parse().map_err(|_| "grid: rows")?,
        })
    }
}

/// Row-major cells; trailing pixels are dropped when the image does not
/// divide evenly.
pub fn split_flat_grid(img: &GrayImage, spec: GridSpec) -> Vec<GrayImage> {
    let mut out = Vec::new();
    if spec.rows == 0 || spec.cols == 0 {
        return out;
    }
    let cw = img.width / spec.cols;
    let ch = img.height / spec.rows;
    if cw == 0 || ch == 0 {
        return out;
    }
    for r in 0..spec.rows {
        for c in 0..spec.cols {
            out.push(img.crop(c * cw, r * ch, cw, ch));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// OCR engine trait + implementations
// ---------------------------------------------------------------------------

/// One cell read: raw text plus 0..1 confidence.
#[derive(Debug, Clone)]
pub struct OcrRead {
    pub text: String,
    pub confidence: f32,
}

/// Pluggable OCR engine. Implementations must be conservative:
/// anything uncertain comes back low-confidence (the pipeline records
/// those cells as NOT held, never guessed).
pub trait OcrEngine {
    fn name(&self) -> &str;
    fn ocr_cell(&self, cell: &GrayImage) -> OcrRead;
}

/// No engine available: every read is empty at zero confidence, so the
/// pipeline outputs empty stock rather than guesses.
pub struct StubEngine;

impl OcrEngine for StubEngine {
    fn name(&self) -> &str {
        "stub"
    }

    fn ocr_cell(&self, _cell: &GrayImage) -> OcrRead {
        OcrRead {
            text: String::new(),
            confidence: 0.0,
        }
    }
}

/// `tesseract` CLI engine (`tesseract <pgm> stdout --psm 8`): present
/// only when the binary is on PATH. [`TesseractCliEngine::probe`]
/// reports availability; the pipeline falls back to [`StubEngine`]
/// when it is absent.
pub struct TesseractCliEngine {
    pub binary: String,
}

impl TesseractCliEngine {
    pub fn probe() -> Option<Self> {
        let out = std::process::Command::new("tesseract")
            .arg("--version")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(Self {
            binary: "tesseract".to_owned(),
        })
    }

    pub fn version(&self) -> String {
        std::process::Command::new(&self.binary)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(str::to_owned))
            .unwrap_or_default()
    }
}

impl OcrEngine for TesseractCliEngine {
    fn name(&self) -> &str {
        "tesseract-cli"
    }

    fn ocr_cell(&self, cell: &GrayImage) -> OcrRead {
        let dir = std::env::temp_dir().join(format!("fracker-ocr-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cell.pgm");
        if std::fs::write(&path, cell.to_pgm_bytes()).is_err() {
            return OcrRead {
                text: String::new(),
                confidence: 0.0,
            };
        }
        let out = std::process::Command::new(&self.binary)
            .arg(&path)
            .arg("stdout")
            .arg("--psm")
            .arg("8")
            .output();
        let _ = std::fs::remove_file(&path);
        match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                // CLI tesseract reports no per-word confidence here;
                // non-empty reads are trusted, empties are not held.
                let conf = if text.is_empty() { 0.0 } else { 0.75 };
                OcrRead {
                    text,
                    confidence: conf,
                }
            }
            _ => OcrRead {
                text: String::new(),
                confidence: 0.0,
            },
        }
    }
}

/// Box status line for the pass summary.
pub fn engine_status() -> String {
    match TesseractCliEngine::probe() {
        Some(e) => format!("tesseract available ({})", e.version()),
        None => "tesseract NOT installed on this box (no root to install it); "
            .to_owned()
            + "shipping the OcrEngine trait + StubEngine; live reads come from the capture session",
    }
}

// ---------------------------------------------------------------------------
// Synthetic bitmap path (tests + fixture generation, no engine needed)
// ---------------------------------------------------------------------------

/// 5x7 bitmap font shared by the synthetic renderer and the template
/// recognizer. Patterns only need to be mutually consistent.
pub mod bitmap_font {
    pub const ADVANCE: u32 = 6;
    pub const GLYPH_W: u32 = 5;
    pub const GLYPH_H: u32 = 7;
    pub const PAD: u32 = 2;

    fn g(rows: [&str; 7]) -> [u8; 35] {
        let mut out = [0u8; 35];
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().take(5).enumerate() {
                out[y * 5 + x] = u8::from(ch == '#');
            }
        }
        out
    }

    pub fn glyph(ch: char) -> Option<[u8; 35]> {
        Some(match ch {
            'A' => g([".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"]),
            'B' => g(["####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."]),
            'C' => g([".####", "#....", "#....", "#....", "#....", "#....", ".####"]),
            'D' => g(["####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####."]),
            'E' => g(["#####", "#....", "#....", "####.", "#....", "#....", "#####"]),
            'F' => g(["#####", "#....", "#....", "####.", "#....", "#....", "#...."]),
            'G' => g([".####", "#....", "#....", "#.###", "#...#", "#...#", ".###."]),
            'H' => g(["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"]),
            'I' => g(["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "#####"]),
            'J' => g(["..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##.."]),
            'K' => g(["#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"]),
            'L' => g(["#....", "#....", "#....", "#....", "#....", "#....", "#####"]),
            'M' => g(["#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"]),
            'N' => g(["#...#", "##..#", "##..#", "#.#.#", "#..##", "#..##", "#...#"]),
            'O' => g([".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
            'P' => g(["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."]),
            'Q' => g([".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#"]),
            'R' => g(["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"]),
            'S' => g([".####", "#....", "#....", ".###.", "....#", "....#", "####."]),
            'T' => g(["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."]),
            'U' => g(["#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
            'V' => g(["#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#.."]),
            'W' => g(["#...#", "#...#", "#...#", "#.#.#", "#.#.#", "##.##", "#...#"]),
            'X' => g(["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"]),
            'Y' => g(["#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#.."]),
            'Z' => g(["#####", "....#", "...#.", "..#..", ".#...", "#....", "#####"]),
            '0' => g([".###.", "#..##", "#.#.#", "#.#.#", "##..#", "#...#", ".###."]),
            '1' => g(["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."]),
            '2' => g([".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"]),
            '3' => g(["#####", "...#.", "..#..", "...#.", "....#", "#...#", ".###."]),
            '4' => g(["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."]),
            '5' => g(["#####", "#....", "####.", "....#", "....#", "#...#", ".###."]),
            '6' => g([".###.", "#....", "#....", "####.", "#...#", "#...#", ".###."]),
            '7' => g(["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."]),
            '8' => g([".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."]),
            '9' => g([".###.", "#...#", "#...#", ".####", "....#", "....#", ".###."]),
            ' ' => g([".....", ".....", ".....", ".....", ".....", ".....", "....."]),
            '-' => g([".....", ".....", ".....", "#####", ".....", ".....", "....."]),
            '.' => g([".....", ".....", ".....", ".....", ".....", ".##..", ".##.."]),
            '\'' => g(["..#..", "..#..", ".....", ".....", ".....", ".....", "....."]),
            '(' => g(["...#.", "..#..", ".#...", ".#...", ".#...", "..#..", "...#."]),
            ')' => g([".#...", "..#..", "...#.", "...#.", "...#.", "..#..", ".#..."]),
            _ => return None,
        })
    }

    pub const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -.()'";

    /// Dark UI background, light label text (like the capture surface).
    pub fn render_text(text: &str, scale: u32) -> super::GrayImage {
        let upper: String = text.to_uppercase();
        let chars: Vec<char> = upper.chars().collect();
        let w = PAD * 2 + ADVANCE * scale * chars.len().max(1) as u32;
        let h = PAD * 2 + GLYPH_H * scale;
        let mut img = super::GrayImage::blank(w, h, 18);
        for (i, ch) in chars.iter().enumerate() {
            if let Some(bits) = glyph(*ch) {
                for gy in 0..GLYPH_H {
                    for gx in 0..GLYPH_W {
                        if bits[(gy * GLYPH_W + gx) as usize] == 1 {
                            for sy in 0..scale {
                                for sx in 0..scale {
                                    img.set(
                                        PAD + (i as u32) * ADVANCE * scale + gx * scale + sx,
                                        PAD + gy * scale + sy,
                                        232,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        img
    }

    /// Uniform grid of labeled cells (one synthetic screenshot page).
    pub fn render_grid(labels: &[&str], cols: u32, cell_w: u32, cell_h: u32) -> super::GrayImage {
        let rows = labels.len().div_ceil(cols.max(1) as usize) as u32;
        let mut page = super::GrayImage::blank(cell_w * cols.max(1), cell_h * rows.max(1), 12);
        for (i, label) in labels.iter().enumerate() {
            let cell = render_text(label, 1);
            let ox = (i as u32 % cols.max(1)) * cell_w + 3;
            let oy = (i as u32 / cols.max(1)) * cell_h + (cell_h.saturating_sub(cell.height)) / 2;
            for y in 0..cell.height {
                for x in 0..cell.width {
                    if ox + x < page.width && oy + y < page.height {
                        page.set(ox + x, oy + y, cell.get(x, y));
                    }
                }
            }
        }
        page
    }
}

/// Template recognizer over the synthetic bitmap font: an [`OcrEngine`]
/// that genuinely extracts text from [`bitmap_font`] renderings (the
/// end-to-end path when tesseract is unavailable). Anything that does
/// not match the font — real screenshots, noise, blank cells — comes
/// back below confidence and is NOT held.
pub struct TemplateEngine {
    pub min_confidence: f32,
}

impl TemplateEngine {
    pub fn new() -> Self {
        Self {
            min_confidence: 0.85,
        }
    }

    fn read_slot(slot: &GrayImage) -> (char, f32) {
        let mut best = ('?', 0.0f32);
        for ch in bitmap_font::ALPHABET.chars() {
            let bits = match bitmap_font::glyph(ch) {
                Some(b) => b,
                None => continue,
            };
            let total = (bitmap_font::GLYPH_W * bitmap_font::GLYPH_H) as f32;
            let mut agree = 0.0;
            for gy in 0..bitmap_font::GLYPH_H {
                for gx in 0..bitmap_font::GLYPH_W {
                    let ink_template = bits[(gy * bitmap_font::GLYPH_W + gx) as usize] == 1;
                    // Slot origin: PAD offset inside the cell image.
                    let px = slot.get(
                        (bitmap_font::PAD + gx).min(slot.width - 1),
                        (bitmap_font::PAD + gy).min(slot.height - 1),
                    );
                    let ink = px > 128;
                    if ink == ink_template {
                        agree += 1.0;
                    }
                }
            }
            let score = agree / total;
            if score > best.1 {
                best = (ch, score);
            }
        }
        best
    }
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl OcrEngine for TemplateEngine {
    fn name(&self) -> &str {
        "bitmap-template"
    }

    fn ocr_cell(&self, cell: &GrayImage) -> OcrRead {
        let inner_w = cell.width.saturating_sub(bitmap_font::PAD * 2);
        let slots = (inner_w / bitmap_font::ADVANCE).max(1);
        let slot_w = bitmap_font::PAD * 2 + bitmap_font::GLYPH_W;
        let slot_h = bitmap_font::PAD * 2 + bitmap_font::GLYPH_H;
        let mut text = String::new();
        let mut sum = 0.0;
        let mut n = 0;
        for s in 0..slots {
            let x = s * bitmap_font::ADVANCE;
            if x + slot_w > cell.width {
                break;
            }
            let slot = cell.crop(x, 0, slot_w, slot_h.min(cell.height));
            let (ch, score) = Self::read_slot(&slot);
            // Blank tail: spaces score perfectly; stop at trailing space.
            text.push(ch);
            sum += score;
            n += 1;
        }
        let conf = if n == 0 { 0.0 } else { sum / n as f32 };
        let text = text.trim_end().to_owned();
        // A uniform cell (blank/noise) matches the space glyph well but
        // carries no ink — reject inkless cells outright.
        let inked = cell.pixels.iter().filter(|&&p| p > 128).count();
        if inked < 8 || conf < self.min_confidence {
            return OcrRead {
                text: String::new(),
                confidence: (conf * 0.5).min(0.4),
            };
        }
        OcrRead {
            text,
            confidence: conf,
        }
    }
}

/// Canned reads in call order (validation-logic tests).
#[cfg(test)]
pub struct FakeEngine {
    pub reads: Vec<OcrRead>,
}

#[cfg(test)]
impl OcrEngine for FakeEngine {
    fn name(&self) -> &str {
        "fake"
    }

    fn ocr_cell(&self, _cell: &GrayImage) -> OcrRead {
        // Order-based reads need interior mutability; tests use
        // single-cell ingestion, so the first read suffices.
        self.reads.first().cloned().unwrap_or(OcrRead {
            text: String::new(),
            confidence: 0.0,
        })
    }
}

// ---------------------------------------------------------------------------
// Universe validation (fuzzy match to a known name, never a guess)
// ---------------------------------------------------------------------------

/// Known item names (parts, relics, gear) the pipeline may output.
#[derive(Debug, Clone, Default)]
pub struct Universe {
    names: Vec<String>,
    folded: Vec<String>,
}

impl Universe {
    pub fn from_names(names: &[String]) -> Self {
        Self {
            names: names.to_vec(),
            folded: names.iter().map(|n| fold(n)).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Best fuzzy match for a raw read. Returns the canonical name and
    /// a 0..1 score. Matching is case/space-insensitive; the threshold
    /// lives with the caller so tests can pin it.
    pub fn best_match(&self, read: &str) -> Option<(&str, f32)> {
        let f = fold(read);
        if f.is_empty() {
            return None;
        }
        let mut best: Option<(&str, f32)> = None;
        for (name, folded) in self.names.iter().zip(self.folded.iter()) {
            let d = levenshtein(&f, folded);
            let denom = f.len().max(folded.len()).max(1) as f32;
            let score = 1.0 - d as f32 / denom;
            let better = match best {
                None => true,
                Some((_, s)) => score > s,
            };
            if better {
                best = Some((name.as_str(), score));
            }
        }
        best
    }
}

fn fold(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

// ---------------------------------------------------------------------------
// Pipeline: cells -> validated results -> stock file
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CellResult {
    pub index: usize,
    pub raw_text: String,
    pub confidence: f32,
    pub matched: Option<String>,
    pub match_score: f32,
    /// Held only on confident reads that validate against the universe.
    pub held: bool,
}

/// Validate one engine read: confident + universe match, else NOT held.
pub fn validate_read(
    read: &OcrRead,
    universe: &Universe,
    min_confidence: f32,
    min_score: f32,
    index: usize,
) -> CellResult {
    let mut out = CellResult {
        index,
        raw_text: read.text.clone(),
        confidence: read.confidence,
        matched: None,
        match_score: 0.0,
        held: false,
    };
    if read.confidence < min_confidence {
        return out;
    }
    if let Some((name, score)) = universe.best_match(&read.text) {
        out.matched = Some(name.to_owned());
        out.match_score = score;
        out.held = score >= min_score;
    }
    out
}

/// Stock state file: loose parts + relic counts, JSON.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StockState {
    #[serde(default)]
    pub relics: HashMap<String, u32>,
    #[serde(default)]
    pub parts: HashMap<String, u32>,
}

impl StockState {
    /// Aggregate validated results: each held match counts once;
    /// names in `relic_names` are relic stock, the rest loose parts.
    pub fn from_results(results: &[CellResult], relic_names: &HashSet<String>) -> Self {
        let mut stock = Self::default();
        for r in results {
            if !r.held {
                continue;
            }
            if let Some(name) = &r.matched {
                if relic_names.contains(name) {
                    *stock.relics.entry(name.clone()).or_insert(0) += 1;
                } else {
                    *stock.parts.entry(name.clone()).or_insert(0) += 1;
                }
            }
        }
        stock
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("stock encode: {e}"))
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("stock: {e}"))
    }

    pub fn write_to(&self, path: &Path) -> Result<(), String> {
        let text = self.to_json()?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("stock out dir: {e}"))?;
            }
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("stock write: {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("stock publish: {e}"))?;
        Ok(())
    }

    /// Planner shape: relic counts become known stock; parts feed the
    /// bootstrap baseline (consumed via `AppState.parts_held`).
    pub fn to_relic_stock(&self) -> crate::model::RelicStock {
        crate::model::RelicStock::Known(self.relics.clone())
    }

    pub fn held_parts(&self) -> &HashMap<String, u32> {
        &self.parts
    }
}

/// Ingest one folder of screenshot pages: every `*.pgm`/`*.ppm` file,
/// sorted, each split by `grid`, every cell OCR'd + validated.
pub fn ingest_folder(
    dir: &Path,
    engine: &dyn OcrEngine,
    grid: GridSpec,
    universe: &Universe,
    relic_names: &HashSet<String>,
    min_confidence: f32,
    min_score: f32,
) -> Result<(StockState, Vec<CellResult>), String> {
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("ocr input {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("pgm") || x.eq_ignore_ascii_case("ppm"))
                .unwrap_or(false)
        })
        .collect();
    pages.sort();
    if pages.is_empty() {
        return Err(format!("ocr input {}: no .pgm/.ppm pages", dir.display()));
    }
    let mut results = Vec::new();
    for page_path in &pages {
        let bytes =
            std::fs::read(page_path).map_err(|e| format!("ocr read {}: {e}", page_path.display()))?;
        let img = GrayImage::from_bytes(&bytes)?;
        for cell in split_flat_grid(&img, grid) {
            let read = engine.ocr_cell(&cell);
            let index = results.len();
            results.push(validate_read(&read, universe, min_confidence, min_score, index));
        }
    }
    let stock = StockState::from_results(&results, relic_names);
    Ok((stock, results))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tuning::Tuning;

    fn universe() -> Universe {
        Universe::from_names(
            &[
                "Neo C5",
                "Ivara Prime Chassis Blueprint",
                "Nautilus Prime Blueprint",
                "Forma Blueprint",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        )
    }

    fn relics() -> HashSet<String> {
        ["Neo C5".to_owned()].into_iter().collect()
    }

    #[test]
    fn segments_even_grid() {
        let img = GrayImage::blank(100, 60, 0);
        let cells = split_flat_grid(&img, GridSpec { rows: 3, cols: 2 });
        assert_eq!(cells.len(), 6);
        assert!(cells.iter().all(|c| c.width == 50 && c.height == 20));
    }

    #[test]
    fn template_engine_extracts_synthetic_labels() {
        let engine = TemplateEngine::new();
        for label in ["NEO C5", "IVARA PRIME CHASSIS BLUEPRINT", "FORMA BLUEPRINT"] {
            let cell = bitmap_font::render_text(label, 1);
            let read = engine.ocr_cell(&cell);
            assert!(
                read.confidence >= 0.85,
                "confidence for {label}: {}",
                read.confidence
            );
            assert_eq!(read.text, label, "extracted text");
        }
    }

    #[test]
    fn template_rejects_blank_and_noise() {
        let engine = TemplateEngine::new();
        let blank = GrayImage::blank(80, 11, 18);
        let read = engine.ocr_cell(&blank);
        assert!(!read.text.is_empty() || read.confidence < 0.85);
        assert!(read.confidence < 0.85, "blank stays low: {}", read.confidence);
    }

    #[test]
    fn validation_fuzzy_matches_and_never_guesses() {
        let u = universe();
        // Exact read validates.
        let exact = validate_read(&OcrRead {
                text: "Neo C5".into(),
                confidence: 0.95,
            }, &u, 0.5, 0.8, 0);
        assert!(exact.held);
        assert_eq!(exact.matched.as_deref(), Some("Neo C5"));
        // Typo still resolves (fuzzy), below-perfect score.
        let typo = validate_read(&OcrRead {
                text: "Ivara Prime Chassis Blueprlnt".into(),
                confidence: 0.9,
            }, &u, 0.5, 0.8, 1);
        assert!(typo.held, "typo resolves: {typo:?}");
        assert!(typo.match_score < 1.0);
        // Garbage never validates.
        let garbage = validate_read(&OcrRead {
                text: "zzz qqq www".into(),
                confidence: 0.9,
            }, &u, 0.5, 0.8, 2);
        assert!(!garbage.held);
        assert!(garbage.matched.is_some()); // best effort named, but NOT held
        // Low confidence never validates even when exact.
        let shy = validate_read(&OcrRead {
                text: "Neo C5".into(),
                confidence: 0.2,
            }, &u, 0.5, 0.8, 3);
        assert!(!shy.held);
    }

    #[test]
    fn stock_counts_relics_and_parts_in_planner_shape() {
        let u = universe();
        let rel = relics();
        let reads = vec![
            validate_read(&OcrRead {
                    text: "Neo C5".into(),
                    confidence: 0.9,
                }, &u, 0.5, 0.8, 0),
            validate_read(&OcrRead {
                    text: "Neo C5".into(),
                    confidence: 0.9,
                }, &u, 0.5, 0.8, 1),
            validate_read(&OcrRead {
                    text: "Forma Blueprint".into(),
                    confidence: 0.9,
                }, &u, 0.5, 0.8, 2),
            validate_read(&OcrRead {
                    text: "zzz".into(),
                    confidence: 0.9,
                }, &u, 0.5, 0.8, 3),
        ];
        let stock = StockState::from_results(&reads, &rel);
        assert_eq!(stock.relics.get("Neo C5"), Some(&2));
        assert_eq!(stock.parts.get("Forma Blueprint"), Some(&1));
        // Planner shape: known relic stock.
        match stock.to_relic_stock() {
            crate::model::RelicStock::Known(m) => assert_eq!(m.get("Neo C5"), Some(&2)),
            crate::model::RelicStock::Unknown => panic!("stock must be known"),
        }
        let json = stock.to_json().unwrap();
        let back = StockState::from_json(&json).unwrap();
        assert_eq!(back.relics.get("Neo C5"), Some(&2));
    }

    #[test]
    fn stub_engine_yields_empty_stock_never_guesses() {
        let u = universe();
        let rel = relics();
        let engine = StubEngine;
        let img = GrayImage::blank(60, 22, 18);
        let read = engine.ocr_cell(&img);
        let r = validate_read(&read, &u, 0.5, 0.8, 0);
        assert!(!r.held);
        let stock = StockState::from_results(&[r], &rel);
        assert!(stock.relics.is_empty() && stock.parts.is_empty());
    }

    #[test]
    fn tesseract_probe_matches_binary_presence() {
        let on_path = std::env::var_os("PATH")
            .map(|p| {
                std::env::split_paths(&p).any(|d| {
                    d.join("tesseract").exists() || d.join("tesseract.exe").exists()
                })
            })
            .unwrap_or(false);
        assert_eq!(TesseractCliEngine::probe().is_some(), on_path);
    }

    #[test]
    fn ingest_folder_reads_pages_sequentially() {
        let dir = std::env::temp_dir().join("fracker-ocr-ingest");
        let _ = std::fs::create_dir_all(&dir);
        let labels = ["NEO C5", "FORMA BLUEPRINT"];
        let page = bitmap_font::render_grid(&labels, 2, 200, 24);
        std::fs::write(dir.join("page-01.pgm"), page.to_pgm_bytes()).unwrap();
        let u = Universe::from_names(
            &labels.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        let rel: HashSet<String> = ["NEO C5".to_owned()].into_iter().collect();
        let (stock, results) = ingest_folder(
            &dir,
            &StubEngine,
            GridSpec { rows: 1, cols: 2 },
            &u,
            &rel,
            0.5,
            0.8,
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].index, 0);
        assert_eq!(results[1].index, 1);
        assert!(stock.relics.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fixture_system_font_grid_segments_and_validates() {
        // Committed PPM rendered with a system font (DejaVu Sans via
        // PIL — see fixtures/ocr/README.md). No engine on this box
        // reads those glyphs, so reads stand in for the capture
        // engine; the test pins parsing + segmentation + validation.
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/ocr/grid-2x2.ppm");
        let bytes = std::fs::read(&path).unwrap();
        let img = GrayImage::from_bytes(&bytes).unwrap();
        assert_eq!((img.width, img.height), (1280, 144));
        let cells = split_flat_grid(&img, GridSpec { rows: 2, cols: 2 });
        assert_eq!(cells.len(), 4);
        assert!(cells.iter().all(|c| c.width == 640 && c.height == 72));
        for cell in &cells {
            let m = cell.mean();
            assert!(m > 0.5 && m < 60.0, "inked dark cell: {m}");
        }
        let u = Universe::from_names(
            &[
                "Neo C5",
                "Ivara Prime Chassis Blueprint",
                "Forma Blueprint",
                "Lith K9",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        );
        for (i, label) in [
            "Neo C5",
            "Ivara Prime Chassis Blueprint",
            "Forma Blueprint",
            "Lith K9",
        ]
        .iter()
        .enumerate()
        {
            let r = validate_read(
                &OcrRead {
                    text: label.to_string(),
                    confidence: 0.9,
                },
                &u,
                0.5,
                0.8,
                i,
            );
            assert!(r.held, "known label validates: {label}");
        }
    }

    #[test]
    fn tuning_files_present_for_planner() {
        let t = Tuning::for_tests();
        assert!(!t.weeklies.is_empty());
    }
}
