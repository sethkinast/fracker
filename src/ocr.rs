//! Inventory bootstrap OCR (BRIEF §8.1, addendum #21: OCR is the plan).
//!
//! End-to-end ingestion EXCEPT the live capture session (that happens
//! later, with the game on screen):
//! - input: a folder of inventory-grid screenshots (PNG/PGM/PPM files);
//! - per-screen geometry (grid + per-cell text regions) loaded from
//!   `data/capture/layouts.toml` (addendum #20: content, not code);
//! - per-cell OCR behind a pluggable [`OcrEngine`] trait (a `tesseract`
//!   CLI engine probing the bundled sidecar first, then `$PATH`,
//!   otherwise a [`StubEngine`] that reports zero confidence — see
//!   [`engine_status`]);
//! - every read validated against the item universe (fuzzy match to a
//!   known name, with an ambiguity guard); low-confidence, unmatched or
//!   ambiguous reads are recorded as NOT held, never guessed;
//! - output: a stock state file (loose parts + relic counts, JSON) in
//!   the shape the planner consumes ([`StockState::to_relic_stock`]).
//!
//! Error asymmetry rule: an uncertain cell defaults to NOT held (a
//! redundant crack suggestion is cheap; wrongly pruning a needed part
//! is not).
//!
//! Image handling is streaming and offline-friendly: PNG via the pure
//! Rust `png` crate, plus 8-bit PGM/PPM for the synthetic test path, both
//! decoded to one gray raster.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Refinement tier (eighth pass §1)
// ---------------------------------------------------------------------------

/// Refinement tier of a relic stack. The refinement screen shows one cell
/// per (relic x tier); non-Intact tiers carry a bracketed label under the
/// name (`[Exceptional]`/`[Flawless]`/`[Radiant]`). Absence of a label is
/// the game's own Intact indicator, so an unlabeled cell is Intact; a
/// label we cannot parse is [`RefinementTier::Unknown`] — counted in
/// totals, excluded from tier-specific advice, never silently assigned.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum RefinementTier {
    Intact,
    Exceptional,
    Flawless,
    Radiant,
    Unknown,
}

impl RefinementTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            RefinementTier::Intact => "intact",
            RefinementTier::Exceptional => "exceptional",
            RefinementTier::Flawless => "flawless",
            RefinementTier::Radiant => "radiant",
            RefinementTier::Unknown => "unknown",
        }
    }

    /// The tier a bare (unbracketed) token names, if any.
    pub fn from_token(t: &str) -> Option<Self> {
        match t.trim().to_ascii_lowercase().as_str() {
            "intact" => Some(RefinementTier::Intact),
            "exceptional" => Some(RefinementTier::Exceptional),
            "flawless" => Some(RefinementTier::Flawless),
            "radiant" => Some(RefinementTier::Radiant),
            _ => None,
        }
    }

    /// Parse a tier label anywhere in a raw OCR read (`[Radiant]`, or a
    /// bare `RADIANT` line). Returns `Unknown` when a bracket survives
    /// but names no known tier.
    pub fn parse_label(read: &str) -> Self {
        let lower = read.to_ascii_lowercase();
        if let Some(start) = lower.find('[') {
            if let Some(end) = lower[start..].find(']') {
                let inner = &lower[start + 1..start + end];
                return Self::from_token(inner).unwrap_or(RefinementTier::Unknown);
            }
        }
        for tok in lower.split_whitespace() {
            if let Some(t) = Self::from_token(tok) {
                return t;
            }
        }
        RefinementTier::Intact
    }
}

/// Remove bracketed tier labels from a raw read so fuzzy name
/// matching sees the relic name alone. Only BALANCED `[...]`
/// spans are stripped (pass 27): an unmatched `[` used to delete
/// the rest of the read — the census case "[ Gauss Prime
/// Blueprint" lost the whole name to the stripper — so a stray
/// bracket now costs only the bracket character itself and the
/// text after it stays. A stray `]` is likewise dropped alone.
fn strip_tier_label(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                // Scan for the matching ']' (nesting counts). A
                // balanced span drops whole; an unmatched '[' drops
                // only itself and the scan resumes after it.
                let mut depth = 0usize;
                let mut j = i;
                let mut matched = false;
                while j < chars.len() {
                    match chars[j] {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                matched = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                i = if matched { j + 1 } else { i + 1 };
            }
            ']' => i += 1,
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// 8-byte PNG file signature.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

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

    /// Bilinear integer upscale. Small UI labels (11-16 px tall) are
    /// below what tesseract wants; a 2-8x upscale before OCR is the
    /// single biggest accuracy lever (pass 5). Bilinear rather than
    /// nearest: no block artefacts on antialiased glyph edges.
    pub fn upscale(&self, factor: u32) -> Self {
        if factor <= 1 || self.width == 0 || self.height == 0 {
            return self.clone();
        }
        let w = self.width * factor;
        let h = self.height * factor;
        let mut out = Self::blank(w, h, 0);
        for y in 0..h {
            // Sample position in source space (pixel centres).
            let sy = (y as f32 + 0.5) / factor as f32 - 0.5;
            let y0 = sy.floor().max(0.0) as u32;
            let y1 = (y0 + 1).min(self.height - 1);
            let fy = (sy - y0 as f32).clamp(0.0, 1.0);
            for x in 0..w {
                let sx = (x as f32 + 0.5) / factor as f32 - 0.5;
                let x0 = sx.floor().max(0.0) as u32;
                let x1 = (x0 + 1).min(self.width - 1);
                let fx = (sx - x0 as f32).clamp(0.0, 1.0);
                let top = self.get(x0, y0) as f32 * (1.0 - fx)
                    + self.get(x1, y0) as f32 * fx;
                let bot = self.get(x0, y1) as f32 * (1.0 - fx)
                    + self.get(x1, y1) as f32 * fx;
                out.set(x, y, (top * (1.0 - fy) + bot * fy).round() as u8);
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
        // Real capture screenshots are PNG (Seth's fixtures); the
        // synthetic/test path emits PGM/PPM. Both decode to one 8-bit
        // gray raster so the rest of the pipeline is format-agnostic.
        if bytes.starts_with(&PNG_SIGNATURE) {
            return Self::from_png(bytes);
        }
        let (magic, rest) = split_token(bytes).ok_or("image: empty")?;
        match magic.as_slice() {
            b"P5" => Self::parse_gray(rest, 1),
            b"P6" => Self::parse_gray(rest, 3),
            _ => Err("image: only PNG or P5/P6 with maxval 255".to_owned()),
        }
    }

    /// Decode a PNG (any colour type) to luma, compositing alpha over
    /// black (the UI screenshots have no transparency, but be safe).
    /// Write the image as an 8-bit grayscale PNG (the inverse of
    /// `from_png`). Used for capture debug dumps, so a human can look
    /// at exactly what the OCR was fed.
    pub fn write_png(&self, path: &std::path::Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("write_png: {e}"))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| format!("write_png: {e}"))?;
        writer.write_image_data(&self.pixels).map_err(|e| format!("write_png: {e}"))
    }

    fn from_png(bytes: &[u8]) -> Result<Self, String> {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder
            .read_info()
            .map_err(|e| format!("png: header: {e}"))?;
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| format!("png: frame: {e}"))?;
        let width = info.width;
        let height = info.height;
        let data = &buf[..info.buffer_size()];
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize));
        let luma = |r: u8, g: u8, b: u8| {
            ((r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100) as u8
        };
        match info.color_type {
            png::ColorType::Grayscale => pixels.extend_from_slice(data),
            png::ColorType::GrayscaleAlpha => {
                for px in data.chunks_exact(2) {
                    pixels.push((px[0] as u32 * px[1] as u32 / 255) as u8);
                }
            }
            png::ColorType::Rgb => {
                for px in data.chunks_exact(3) {
                    pixels.push(luma(px[0], px[1], px[2]));
                }
            }
            png::ColorType::Rgba => {
                for px in data.chunks_exact(4) {
                    let a = px[3] as u32;
                    let f = |c: u8| (c as u32 * a / 255) as u8;
                    pixels.push(luma(f(px[0]), f(px[1]), f(px[2])));
                }
            }
            other => return Err(format!("png: unsupported colour type {other:?}")),
        }
        if pixels.len() != (width as usize) * (height as usize) {
            return Err("png: raster size".to_owned());
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
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

/// One cell read: raw text plus 0..1 confidence, plus an optional
/// stack count when the capture UI labels one ("x8" on the relic
/// screen, a circled badge on the sell screen). `None` means the UI
/// showed no label, which for both screens means exactly one.
#[derive(Debug, Clone)]
pub struct OcrRead {
    pub text: String,
    pub confidence: f32,
    pub count: Option<u32>,
    /// Arcane rank read from the five rank pips (0..=5); `None` when the
    /// screen has no pip row (relics/parts).
    pub rank: Option<u8>,
}

impl OcrRead {
    pub fn empty() -> Self {
        Self {
            text: String::new(),
            confidence: 0.0,
            count: None,
            rank: None,
        }
    }
}

/// How the capture screen renders the per-cell stack count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountStyle {
    /// Literal `xN` text top-left (relic screen); absent when count == 1.
    XPrefix,
    /// Circled digit badge top-left (sell screen); absent when count == 1.
    Badge,
}

/// One capture-screen layout: grid geometry plus per-cell text regions.
/// Loaded from `data/capture/layouts.toml` (addendum #20 — geometry is
/// content, not code).
#[derive(Debug, Clone)]
pub struct Layout {
    pub key: String,
    pub cols: u32,
    pub rows: u32,
    pub ref_width: u32,
    pub ref_height: u32,
    pub x0: f32,
    pub cell_w: f32,
    pub y0: f32,
    pub cell_h: f32,
    pub name: [f32; 4],
    pub count: [f32; 4],
    pub count_style: CountStyle,
    pub name_psm: u32,
    pub count_psm: u32,
    pub name_upscale: u32,
    pub count_upscale: u32,
    /// A second name band ABOVE [`Self::name`] (eighth pass §1). The
    /// refinement screen vertically centres a one- or two-line block: an
    /// Intact cell (no tier label) puts its name in the lower band, while
    /// a labelled cell (e.g. `[Radiant]`) shifts the name UP into this
    /// band and drops the label into the lower one. Reading the upper
    /// band only when the lower read is a bare label recovers the name
    /// (and the tier) without a second OCR call on the common Intact
    /// case. `None` on screens with a single-line name.
    pub name_upper: Option<[f32; 4]>,
    /// Arcane rank-pip row as `[x0,y0,x1,y1]` fractions of one cell
    /// (eighth pass §3). Five diamonds sit under the name; the filled
    /// count is the rank. `None` on screens without pips.
    pub pips: Option<[f32; 4]>,
    /// Minimum fuzzy-match score to hold a read on this screen. The
    /// noisier the screen (icons in the crop), the lower the floor; the
    /// ambiguity margin guard stays constant.
    pub min_score: f32,
}

#[derive(serde::Deserialize)]
struct LayoutFile {
    #[serde(flatten)]
    tables: HashMap<String, LayoutDef>,
}

#[derive(serde::Deserialize)]
struct LayoutDef {
    cols: u32,
    rows: u32,
    ref_width: u32,
    ref_height: u32,
    x0: f32,
    cell_w: f32,
    y0: f32,
    cell_h: f32,
    name: [f32; 4],
    count: [f32; 4],
    count_style: String,
    name_psm: u32,
    count_psm: u32,
    name_upscale: u32,
    count_upscale: u32,
    #[serde(default)]
    name_upper: Option<[f32; 4]>,
    #[serde(default)]
    pips: Option<[f32; 4]>,
    #[serde(default = "default_min_score")]
    min_score: f32,
}

fn default_min_score() -> f32 {
    0.8
}

impl Layout {
    /// Parse every `[table]` in a layouts TOML file.
    pub fn load_file(path: &Path) -> Result<HashMap<String, Layout>, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("layouts {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<HashMap<String, Layout>, String> {
        let file: LayoutFile =
            toml::from_str(text).map_err(|e| format!("layouts: {e}"))?;
        let mut out = HashMap::new();
        for (key, d) in file.tables {
            let style = match d.count_style.as_str() {
                "x" => CountStyle::XPrefix,
                "badge" => CountStyle::Badge,
                other => return Err(format!("layout {key}: count_style {other}")),
            };
            out.insert(
                key.clone(),
                Layout {
                    key,
                    cols: d.cols,
                    rows: d.rows,
                    ref_width: d.ref_width,
                    ref_height: d.ref_height,
                    x0: d.x0,
                    cell_w: d.cell_w,
                    y0: d.y0,
                    cell_h: d.cell_h,
                    name: d.name,
                    count: d.count,
                    count_style: style,
                    name_psm: d.name_psm,
                    count_psm: d.count_psm,
                    name_upscale: d.name_upscale,
                    count_upscale: d.count_upscale,
                    name_upper: d.name_upper,
                    pips: d.pips,
                    min_score: d.min_score,
                },
            );
        }
        Ok(out)
    }

    pub fn cells_total(&self) -> usize {
        (self.cols as usize) * (self.rows as usize)
    }

    /// Crop this layout's cells out of a page, scaling the reference
    /// pixel geometry to the page's actual size. `None` marks a cell
    /// that falls (partly) outside the page — a partial cell at the
    /// viewport edge, which is never read (never guessed).
    pub fn split(&self, page: &GrayImage) -> Vec<Option<GrayImage>> {
        let sx = page.width as f32 / self.ref_width.max(1) as f32;
        let sy = page.height as f32 / self.ref_height.max(1) as f32;
        let cw = self.cell_w * sx;
        let ch = self.cell_h * sy;
        let mut out = Vec::with_capacity(self.cells_total());
        for r in 0..self.rows {
            for c in 0..self.cols {
                let x = (self.x0 * sx + c as f32 * cw).round() as i64;
                let y = (self.y0 * sy + r as f32 * ch).round() as i64;
                let w = cw.round() as i64;
                let h = ch.round() as i64;
                if x < 0 || y < 0 || x + w > page.width as i64 || y + h > page.height as i64 {
                    out.push(None);
                    continue;
                }
                out.push(Some(page.crop(x as u32, y as u32, w as u32, h as u32)));
            }
        }
        out
    }
}

/// Crop a `[x0,y0,x1,y1]` fraction-of-cell region, always returning at
/// least a 1x1 image so downstream code need not special-case.
/// Public for the Pass B form channels (`crate::lineread`), which
/// measure the same regions the readers read.
pub fn crop_region(cell: &GrayImage, region: [f32; 4]) -> GrayImage {
    let w = cell.width as f32;
    let h = cell.height as f32;
    let x0 = (region[0] * w).clamp(0.0, w) as u32;
    let y0 = (region[1] * h).clamp(0.0, h) as u32;
    let x1 = (region[2] * w)
        .clamp(0.0, w)
        .max(x0 as f32 + 1.0)
        .min(w) as u32;
    let y1 = (region[3] * h)
        .clamp(0.0, h)
        .max(y0 as f32 + 1.0)
        .min(h) as u32;
    cell.crop(x0, y0, (x1 - x0).max(1), (y1 - y0).max(1))
}

/// The badge window (pass 33): cell-relative region around the
/// parts count badge, slightly larger than the count region
/// itself so the whole ring is inside it. The Pass B built-copy
/// glyph lives in the same window and is matched in the same
/// canonical resample (`crate::lineread::glyph_score`).
pub const BADGE_WINDOW: [f32; 4] = [0.02, 0.0, 0.36, 0.20];
/// Canonical size the badge window is resampled to before template
/// matching — the size at which [`BADGE_TEMPLATE`] was measured
/// (2560×1440 captures), so detection is resolution-independent.
pub const BADGE_WINDOW_W: u32 = 62;
pub const BADGE_WINDOW_H: u32 = 36;
/// NCC floor for "the badge ring is present". Calibrated on both
/// censuses' aligned frames (see docs/thirty-third-pass.md):
/// non-badge cells score at most 0.34 (the built-copy glyph's
/// small circles are the closest confuser), genuine badges at
/// least 0.96 (standard) — 0.50 sits in empty space on both sides.
const BADGE_RING_NCC_MIN: f32 = 0.50;

/// The count badge's circled check, cropped from a verified ×6
/// cell (standard census frame-first-00, Glaive Prime Blade) at
/// the canonical window scale: 27×23, nominal window offset
/// (10, 2). The badge art is identical on every cell (deterministic
/// UI rendering), which is what makes a literal template viable.
const BADGE_TEMPLATE: [[u8; 27]; 23] = [
    [5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7, 12, 15, 13, 7, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 5, 18, 67, 89, 110, 118, 113, 90, 72, 23, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 72, 140, 155, 159, 138, 123, 138, 158, 156, 141, 87, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 8, 96, 159, 134, 62, 8, 5, 5, 5, 7, 53, 128, 158, 108, 15, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 94, 155, 94, 11, 5, 5, 5, 5, 5, 5, 5, 5, 87, 153, 109, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 70, 158, 93, 9, 5, 5, 5, 5, 5, 5, 5, 5, 6, 43, 157, 159, 95, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 19, 140, 135, 11, 5, 5, 5, 5, 5, 5, 5, 5, 7, 41, 158, 159, 159, 144, 28, 5, 5, 5, 5, 5],
    [5, 5, 5, 66, 155, 63, 5, 5, 5, 5, 5, 5, 5, 5, 7, 54, 149, 159, 159, 144, 158, 77, 5, 5, 5, 5, 5],
    [5, 5, 6, 87, 159, 8, 5, 5, 12, 5, 5, 5, 5, 5, 41, 149, 159, 159, 144, 39, 150, 102, 10, 5, 5, 5, 5],
    [5, 5, 11, 109, 138, 5, 6, 33, 150, 59, 7, 5, 7, 41, 158, 159, 159, 145, 49, 5, 110, 129, 18, 5, 5, 5, 5],
    [5, 5, 14, 117, 125, 5, 28, 142, 159, 143, 59, 7, 54, 149, 159, 159, 143, 50, 5, 5, 92, 137, 21, 5, 5, 5, 5],
    [5, 5, 12, 111, 138, 5, 11, 89, 159, 159, 150, 84, 149, 159, 159, 143, 38, 5, 5, 6, 102, 130, 18, 5, 5, 5, 5],
    [5, 5, 7, 89, 159, 8, 5, 5, 89, 153, 159, 159, 159, 159, 145, 49, 5, 5, 5, 6, 144, 105, 11, 5, 5, 5, 5],
    [5, 5, 5, 72, 157, 52, 5, 5, 11, 87, 153, 159, 159, 144, 50, 5, 5, 5, 5, 31, 158, 81, 6, 5, 5, 5, 5],
    [5, 5, 5, 19, 140, 131, 11, 5, 5, 11, 90, 159, 144, 39, 5, 5, 5, 5, 5, 122, 145, 31, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 85, 158, 88, 8, 5, 5, 5, 83, 50, 5, 5, 5, 5, 7, 73, 154, 105, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 107, 151, 82, 6, 6, 5, 5, 5, 5, 5, 5, 5, 72, 144, 118, 11, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 14, 107, 156, 128, 41, 5, 5, 5, 5, 5, 30, 122, 154, 118, 21, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 6, 93, 141, 157, 151, 113, 94, 110, 147, 158, 144, 104, 11, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 6, 6, 23, 77, 101, 127, 135, 129, 104, 80, 31, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 10, 17, 19, 18, 10, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
];

/// Bilinear resample to an exact size (centre-aligned mapping, as
/// in PIL's resize — the mapping the badge calibration used).
/// Public for the Pass B form channels (`crate::lineread`), which
/// resample the badge window for the glyph template match.
pub fn resize_bilinear(img: &GrayImage, w: u32, h: u32) -> GrayImage {
    let mut out = GrayImage::blank(w, h, 0);
    if img.width == 0 || img.height == 0 {
        return out;
    }
    let sx = img.width as f32 / w as f32;
    let sy = img.height as f32 / h as f32;
    for dy in 0..h {
        let fy = ((dy as f32 + 0.5) * sy - 0.5).clamp(0.0, (img.height - 1) as f32);
        let y0 = fy.floor() as u32;
        let y1 = (y0 + 1).min(img.height - 1);
        let wy = fy - y0 as f32;
        for dx in 0..w {
            let fx = ((dx as f32 + 0.5) * sx - 0.5).clamp(0.0, (img.width - 1) as f32);
            let x0 = fx.floor() as u32;
            let x1 = (x0 + 1).min(img.width - 1);
            let wx = fx - x0 as f32;
            let v = img.get(x0, y0) as f32 * (1.0 - wx) * (1.0 - wy)
                + img.get(x1, y0) as f32 * wx * (1.0 - wy)
                + img.get(x0, y1) as f32 * (1.0 - wx) * wy
                + img.get(x1, y1) as f32 * wx * wy;
            out.pixels[(dy * w + dx) as usize] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Is the parts count badge's ring present in this cell? (Pass 33,
/// addendum #97: ring-gated badge reads.) Template NCC of
/// [`BADGE_TEMPLATE`] over the canonical badge window, searched
/// over the offsets the calibration exercised (x 5..=15, y 0..=11 —
/// the vertical range absorbs the row-pitch phase a per-frame cut
/// can carry; stitched canonical crops sit at the nominal offset).
/// The one confuser by design is the built-copy glyph in the same
/// corner — its circles are a fraction of the badge's diameter and
/// it calibrates at ≤ 0.34, far under the floor. A cell whose
/// window is too small to judge answers "absent": an unreadable
/// badge must never become a digit read.
pub fn badge_ring_present(cell: &GrayImage) -> bool {
    let win = crop_region(cell, BADGE_WINDOW);
    if win.width < 16 || win.height < 10 {
        return false;
    }
    let win = resize_bilinear(&win, BADGE_WINDOW_W, BADGE_WINDOW_H);
    let (tw, th) = (27usize, 23usize);
    let tmean: f32 =
        BADGE_TEMPLATE.iter().flatten().map(|&v| v as f32).sum::<f32>() / (tw * th) as f32;
    let tcent: Vec<f32> = BADGE_TEMPLATE
        .iter()
        .flatten()
        .map(|&v| v as f32 - tmean)
        .collect();
    let tnorm: f32 = tcent.iter().map(|v| v * v).sum::<f32>().sqrt();
    if tnorm <= f32::EPSILON {
        return false;
    }
    let (ww, wh) = (win.width as usize, win.height as usize);
    let mut best = f32::MIN;
    for oy in 0..=11usize {
        for ox in 5..=15usize {
            if ox + tw > ww || oy + th > wh {
                continue;
            }
            let mut sum = 0.0f32;
            for y in 0..th {
                for x in 0..tw {
                    sum += win.pixels[(oy + y) * ww + ox + x] as f32;
                }
            }
            let pmean = sum / (tw * th) as f32;
            let (mut num, mut pss) = (0.0f32, 0.0f32);
            for y in 0..th {
                for x in 0..tw {
                    let pc = win.pixels[(oy + y) * ww + ox + x] as f32 - pmean;
                    num += pc * tcent[y * tw + x];
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
    best >= BADGE_RING_NCC_MIN
}

/// The eye badge window: the relic cell's top-left corner, where
/// the browse screen puts the unowned eye badge (and an owned
/// stack puts its count label). Cell-relative fractions.
pub const EYE_WINDOW: [f32; 4] = [0.0, 0.0, 0.24, 0.22];
/// Canonical size the eye window is resampled to before template
/// matching — the size at which [`EYE_TEMPLATE`] was measured
/// (2560x1440 captures), so detection is resolution-independent.
pub const EYE_WINDOW_W: u32 = 40;
pub const EYE_WINDOW_H: u32 = 38;
/// NCC floor for "the eye badge is present". Calibrated on Seth's
/// browse screenshot (a palette-clipped chat upload): its 12
/// unowned tiles score 0.81-0.97 against [`EYE_TEMPLATE`], its 10
/// owned tiles -0.10..0.07 — 0.60 sits in empty space on both
/// sides of a wide gap.
pub const EYE_NCC_MIN: f32 = 0.60;

/// The browse screen's unowned eye badge, averaged over the 12
/// unowned tiles of Seth's 2026-10-26 browse screenshot at the
/// canonical window scale: 40x38. The badge art is identical on
/// every tile (deterministic UI rendering), which is what makes
/// a literal template viable — the same bet [`BADGE_TEMPLATE`]
/// makes for the parts count badge.
const EYE_TEMPLATE: [[u8; 40]; 38] = [
    [5, 5, 6, 6, 5, 6, 4, 5, 7, 5, 5, 6, 6, 5, 4, 7, 5, 5, 6, 5, 6, 5, 6, 5, 5, 4, 4, 6, 5, 5, 5, 6, 5, 6, 6, 6, 5, 5, 6, 5],
    [5, 6, 6, 5, 5, 6, 5, 6, 6, 6, 6, 5, 6, 6, 6, 5, 5, 5, 6, 5, 5, 6, 6, 5, 5, 5, 5, 4, 5, 6, 7, 6, 6, 5, 6, 6, 5, 5, 6, 6],
    [6, 6, 5, 5, 5, 6, 6, 5, 6, 5, 6, 6, 6, 4, 5, 5, 5, 6, 5, 6, 6, 5, 5, 5, 5, 5, 6, 6, 5, 6, 6, 6, 6, 5, 4, 4, 6, 5, 5, 5],
    [5, 5, 6, 6, 5, 4, 6, 6, 5, 6, 6, 5, 5, 6, 5, 6, 4, 6, 5, 5, 5, 5, 5, 4, 6, 6, 5, 5, 6, 5, 6, 6, 5, 6, 6, 6, 5, 6, 6, 5],
    [6, 5, 7, 6, 6, 5, 5, 6, 5, 5, 6, 5, 6, 5, 5, 4, 6, 5, 4, 6, 4, 5, 5, 5, 5, 5, 5, 6, 5, 5, 5, 4, 6, 6, 5, 5, 5, 5, 5, 6],
    [6, 6, 6, 6, 5, 5, 5, 5, 5, 6, 5, 4, 6, 4, 6, 7, 6, 5, 6, 6, 5, 6, 5, 5, 7, 4, 6, 6, 5, 5, 5, 6, 6, 5, 6, 5, 6, 5, 6, 6],
    [5, 5, 5, 6, 6, 5, 5, 5, 6, 6, 6, 6, 4, 6, 5, 5, 6, 5, 5, 5, 5, 6, 5, 6, 6, 5, 6, 5, 6, 5, 5, 5, 5, 4, 5, 5, 6, 6, 4, 6],
    [5, 5, 4, 6, 5, 5, 4, 5, 5, 5, 6, 5, 6, 5, 6, 5, 6, 5, 6, 6, 5, 6, 6, 4, 6, 6, 6, 5, 5, 5, 5, 5, 6, 5, 6, 6, 5, 5, 6, 6],
    [6, 5, 6, 6, 6, 5, 5, 6, 4, 5, 6, 6, 7, 8, 9, 9, 9, 8, 8, 7, 8, 9, 8, 8, 8, 7, 8, 8, 9, 8, 8, 10, 8, 10, 9, 8, 8, 8, 8, 8],
    [5, 5, 5, 6, 5, 5, 5, 5, 6, 6, 5, 6, 8, 14, 18, 18, 18, 18, 18, 18, 18, 18, 18, 19, 19, 18, 18, 18, 18, 17, 18, 18, 19, 19, 18, 18, 18, 18, 18, 18],
    [6, 4, 5, 5, 6, 5, 4, 5, 6, 6, 5, 6, 8, 14, 22, 23, 23, 23, 23, 23, 22, 23, 23, 23, 23, 23, 23, 23, 24, 23, 23, 23, 23, 23, 22, 23, 22, 22, 23, 23],
    [5, 6, 6, 5, 5, 5, 7, 5, 5, 6, 5, 6, 10, 16, 19, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [5, 6, 5, 5, 6, 6, 5, 6, 5, 5, 5, 7, 8, 15, 18, 14, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [5, 5, 6, 4, 6, 5, 7, 5, 6, 5, 5, 5, 9, 16, 18, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [5, 5, 5, 5, 5, 4, 4, 5, 5, 6, 5, 5, 9, 16, 19, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [4, 6, 6, 5, 6, 6, 5, 5, 5, 5, 5, 5, 9, 16, 19, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [6, 5, 6, 6, 5, 6, 6, 5, 6, 5, 5, 6, 9, 15, 18, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [5, 5, 5, 5, 6, 6, 5, 4, 5, 5, 6, 7, 8, 15, 19, 13, 5, 5, 5, 5, 5, 5, 5, 5, 7, 11, 17, 18, 18, 15, 9, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [5, 5, 6, 5, 4, 5, 5, 6, 4, 5, 6, 6, 9, 15, 18, 14, 5, 5, 5, 5, 5, 5, 5, 9, 34, 84, 131, 142, 138, 112, 61, 15, 5, 5, 5, 5, 5, 5, 5, 7],
    [5, 5, 5, 5, 5, 6, 5, 4, 5, 6, 6, 6, 9, 15, 19, 13, 5, 5, 5, 5, 5, 6, 16, 48, 99, 135, 147, 149, 148, 142, 120, 76, 29, 7, 5, 5, 5, 5, 5, 8],
    [5, 5, 5, 6, 5, 5, 5, 5, 4, 6, 6, 6, 8, 16, 19, 13, 5, 5, 5, 5, 6, 15, 44, 86, 103, 73, 46, 43, 45, 57, 86, 102, 70, 24, 8, 5, 5, 5, 5, 7],
    [5, 4, 5, 6, 6, 5, 6, 5, 6, 5, 4, 6, 8, 15, 19, 13, 5, 5, 5, 9, 27, 57, 74, 61, 36, 20, 23, 31, 29, 20, 25, 47, 70, 70, 44, 15, 5, 5, 5, 7],
    [5, 5, 4, 4, 5, 5, 6, 5, 5, 5, 6, 5, 8, 15, 19, 13, 5, 5, 8, 20, 51, 84, 85, 45, 10, 23, 57, 89, 79, 40, 12, 23, 62, 93, 72, 34, 12, 5, 5, 7],
    [6, 6, 5, 4, 6, 6, 5, 5, 6, 6, 5, 6, 8, 15, 19, 13, 5, 5, 20, 48, 67, 56, 61, 42, 14, 48, 103, 138, 126, 79, 28, 20, 50, 68, 56, 59, 42, 5, 5, 7],
    [6, 5, 5, 6, 6, 5, 5, 5, 6, 5, 6, 5, 8, 15, 18, 13, 5, 5, 18, 46, 67, 61, 63, 41, 15, 52, 110, 143, 132, 85, 30, 20, 52, 74, 63, 60, 40, 5, 5, 7],
    [6, 7, 5, 5, 4, 4, 5, 5, 5, 5, 6, 6, 8, 14, 19, 13, 5, 5, 6, 17, 52, 93, 91, 42, 8, 17, 54, 94, 84, 34, 7, 22, 64, 104, 84, 33, 8, 5, 5, 7],
    [5, 5, 4, 5, 6, 5, 6, 6, 5, 5, 5, 6, 8, 15, 18, 13, 5, 5, 5, 7, 19, 49, 80, 78, 44, 17, 17, 25, 23, 16, 26, 59, 86, 73, 35, 10, 5, 5, 5, 7],
    [5, 5, 6, 5, 5, 5, 5, 5, 6, 5, 4, 6, 9, 15, 19, 13, 5, 5, 5, 5, 6, 14, 41, 82, 102, 85, 77, 75, 76, 80, 92, 95, 68, 22, 8, 5, 5, 5, 5, 7],
    [4, 5, 6, 6, 5, 6, 5, 6, 5, 6, 5, 6, 8, 14, 18, 13, 5, 5, 5, 5, 5, 6, 16, 51, 102, 139, 152, 154, 153, 147, 124, 79, 31, 7, 5, 5, 5, 5, 5, 7],
    [6, 5, 4, 5, 6, 5, 5, 5, 5, 4, 6, 5, 9, 15, 18, 13, 5, 5, 5, 5, 5, 5, 6, 13, 38, 75, 98, 101, 99, 89, 59, 23, 7, 5, 5, 5, 5, 5, 5, 7],
    [5, 5, 6, 6, 5, 4, 5, 5, 5, 5, 6, 6, 8, 15, 18, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 7, 8, 8, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [6, 6, 5, 5, 5, 6, 5, 5, 5, 4, 5, 6, 8, 14, 18, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [5, 5, 5, 6, 5, 5, 6, 5, 6, 5, 6, 6, 9, 14, 18, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [4, 6, 4, 4, 5, 5, 5, 6, 5, 5, 5, 5, 9, 15, 19, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [4, 6, 6, 5, 5, 6, 4, 6, 5, 5, 5, 6, 8, 15, 19, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [5, 4, 4, 5, 5, 5, 6, 5, 5, 4, 4, 4, 8, 15, 18, 13, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 7],
    [5, 5, 5, 5, 4, 4, 5, 6, 4, 5, 5, 7, 9, 15, 21, 20, 19, 19, 19, 20, 20, 19, 20, 19, 20, 19, 19, 19, 19, 19, 19, 20, 19, 19, 19, 19, 19, 19, 20, 20],
    [5, 5, 6, 5, 5, 6, 6, 5, 5, 4, 5, 5, 10, 16, 24, 26, 24, 23, 25, 23, 25, 24, 24, 24, 24, 24, 25, 25, 24, 24, 24, 25, 25, 24, 24, 24, 24, 24, 24, 24],
];

/// Template NCC of the eye badge over the canonical badge window
/// (pass 38, addendum #103): the pixel-evidence leg of the relic
/// owned-gate. Mirrors [`badge_ring_present`]'s resample-then-NCC
/// shape, at the single nominal offset — the layout cut is
/// grid-fitted, so the badge's phase is fixed, and the calibration
/// strip (hand-cut, phase jitter and all) still separated by 0.74.
/// A cell whose window is too small to judge scores 0: an
/// unreadable corner is never an eye.
pub fn eye_badge_score(cell: &GrayImage) -> f32 {
    let win = crop_region(cell, EYE_WINDOW);
    if win.width < 16 || win.height < 10 {
        return 0.0;
    }
    let win = resize_bilinear(&win, EYE_WINDOW_W, EYE_WINDOW_H);
    let (tw, th) = (EYE_WINDOW_W as usize, EYE_WINDOW_H as usize);
    let tmean: f32 =
        EYE_TEMPLATE.iter().flatten().map(|&v| v as f32).sum::<f32>() / (tw * th) as f32;
    let tcent: Vec<f32> = EYE_TEMPLATE
        .iter()
        .flatten()
        .map(|&v| v as f32 - tmean)
        .collect();
    let tnorm: f32 = tcent.iter().map(|v| v * v).sum::<f32>().sqrt();
    if tnorm <= f32::EPSILON {
        return 0.0;
    }
    let pmean: f32 = win.pixels.iter().map(|&p| p as f32).sum::<f32>() / (tw * th) as f32;
    let (mut num, mut pss) = (0.0f32, 0.0f32);
    for (i, &p) in win.pixels.iter().enumerate() {
        let pc = p as f32 - pmean;
        num += pc * tcent[i];
        pss += pc * pc;
    }
    if pss <= f32::EPSILON {
        return 0.0;
    }
    num / (pss.sqrt() * tnorm)
}

/// Is the browse screen's unowned eye badge present in this cell?
pub fn eye_badge_present(cell: &GrayImage) -> bool {
    eye_badge_score(cell) >= EYE_NCC_MIN
}

/// Grey level above which a pixel counts toward the eye glyph's
/// bright mass (pass 45, addendum #110). The native eye renders
/// as a flat, deterministic bright outline peaking at 154; the
/// research measurement (REFINEMENT_X1_DISCRIMINATOR,
/// 2026-10-27) found nothing else on a label-less tile puts
/// pixels anywhere near this level inside the badge window.
pub const EYE_BRIGHT_PX: u8 = 70;

/// Bright-mass floor for "the eye glyph is present on this
/// tile" (pass 45, addendum #110). Measured on Seth's 25
/// native refinement frames: unowned tiles read 69-71 bright
/// pixels in the canonical window, owned x1 tiles at most 20
/// across all 527 settled label-less appearances — any
/// threshold in [21, 68] classifies the corpus with zero
/// errors; 40 sits mid-gap.
pub const EYE_BRIGHT_MASS_MIN: u32 = 40;

/// Canonical size the relic count region is resampled to
/// before the label-presence check — its size at the
/// canonical capture cell (168x174 at 2560x1440), so the
/// check is resolution-independent like the eye window's.
pub const LABEL_REGION_W: u32 = 57;
pub const LABEL_REGION_H: u32 = 35;

/// Bright-mass floor for "a count label is physically
/// present in this tile's count region" (pass 45, addendum
/// #110). Measured on the corpus with production crops: a
/// bare corner reads 0 in the region, the eye glyph reads
/// exactly 71 (the whole blob sits inside the region), and
/// real count labels read 122-170 (frame 0's page minimum;
/// the unparsed "x15" of Lith M4 reads 138). 100 sits in the
/// (71, 122) gap. The check is a guard, not a gate: it only
/// ever VETOES an eye measurement on a tile whose corner
/// carries label-scale bright mass — a physically labelled
/// tile the count parser failed to read, whose strokes share
/// the eye window and must not be mistaken for the glyph.
pub const LABEL_PRESENT_MASS_MIN: u32 = 100;

/// The number of bright pixels ([`EYE_BRIGHT_PX`]) in the
/// cell's count region, resampled to the canonical region
/// size. A present-but-unparsed count label is the one
/// corner state whose bright strokes overlap the eye
/// window; this is how the refinement measure tells it
/// apart from the glyph (see [`LABEL_PRESENT_MASS_MIN`]).
pub fn count_region_bright_mass(cell: &GrayImage, layout: &Layout) -> u32 {
    let region = crop_region(cell, layout.count);
    if region.width < 8 || region.height < 8 {
        return 0;
    }
    let region = resize_bilinear(&region, LABEL_REGION_W, LABEL_REGION_H);
    region
        .pixels
        .iter()
        .filter(|&&p| p > EYE_BRIGHT_PX)
        .count() as u32
}

/// The number of bright pixels ([`EYE_BRIGHT_PX`]) in the
/// cell's canonical eye window — the pass-45 eye statistic.
/// Where the pass-38 template NCC fails on native frames (its
/// blob sits ~6x8 px off the native position and the scorer
/// tolerates no translation), a pixel count is registration-
/// free: the eye's outline mass lands in the window wherever
/// exactly it sits. A window too small to judge reads 0 — an
/// unreadable corner is never an eye. Valid on settled frames
/// only: a transient smears the glyph across the window edge
/// (one corpus transient reads 16 on an unowned tile), which
/// is why the disposition aggregates this per stack over the
/// session's appearances and takes the maximum.
pub fn eye_window_bright_mass(cell: &GrayImage) -> u32 {
    let win = crop_region(cell, EYE_WINDOW);
    if win.width < 16 || win.height < 10 {
        return 0;
    }
    let win = resize_bilinear(&win, EYE_WINDOW_W, EYE_WINDOW_H);
    win.pixels.iter().filter(|&&p| p > EYE_BRIGHT_PX).count() as u32
}

/// The measured eye template, exposed for capture-path tests
/// (the calibration itself is owned by this module's tests).
#[cfg(test)]
pub fn eye_template_for_test() -> &'static [[u8; 40]; 38] {
    &EYE_TEMPLATE
}

/// The relic icon region (the orb), cell-relative fractions: the
/// surface the browse fade dims.
pub const RELIC_ICON_REGION: [f32; 4] = [0.15, 0.10, 0.85, 0.70];

/// The icon's highlight level: the 90th-percentile luminance of
/// the icon region. The browse fade scales the whole tile, so a
/// faded icon's specular highlights collapse — on Seth's strip,
/// owned tiles measure 122-124 and unowned 49-55 (at the upload's
/// clipped scale) — while different eras' art keeps a similar
/// highlight structure when owned. The top decile measures the
/// fade where a mean would entangle the art.
pub fn icon_highlight(cell: &GrayImage) -> f64 {
    let band = crop_region(cell, RELIC_ICON_REGION);
    if band.pixels.is_empty() {
        return 0.0;
    }
    let mut v = band.pixels.clone();
    v.sort_unstable();
    let idx = ((v.len() as f64 * 0.90) as usize).min(v.len() - 1);
    v[idx] as f64
}

/// The brightest icon highlight among a frame's cells under this
/// layout (`None` when no cell is cut). The capture session
/// carries the running maximum across a session's relic frames
/// as the owned-brightness reference: the catalogue's unowned
/// tail can fill a whole late viewport, where a per-frame split
/// has no bright cluster to find.
pub fn relic_frame_bright(frame: &GrayImage, layout: &Layout) -> Option<f64> {
    layout
        .split(frame)
        .iter()
        .filter_map(|c| c.as_ref().map(icon_highlight))
        .fold(None, |acc: Option<f64>, v| {
            Some(acc.map_or(v, |a| a.max(v)))
        })
}

/// Cache key for one cropped region: dimensions + a caller salt,
/// hashed over pixels quantised to 4 bits (`p >> 4` tolerates the
/// sub-quantisation noise a re-render can introduce; a genuinely
/// different glyph changes high nibbles somewhere). Used by the
/// batched tesseract engine (addendum #81) and the capture header
/// cache — a scroll step re-shows most crops pixel-identical.
pub fn crop_key(sub: &GrayImage, salt: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (sub.width, sub.height, salt).hash(&mut h);
    sub.pixels
        .iter()
        .map(|p| p >> 4)
        .collect::<Vec<u8>>()
        .hash(&mut h);
    h.finish()
}

/// [`crop_key`] of an arbitrary `[x0,y0,x1,y1]` fraction-of-image
/// region (e.g. the capture header strip).
pub fn region_key(image: &GrayImage, region: [f32; 4]) -> u64 {
    crop_key(&crop_region(image, region), 0x4845_4144)
}

/// Pluggable OCR engine. Implementations must be conservative:
/// anything uncertain comes back low-confidence (the pipeline records
/// those cells as NOT held, never guessed).
pub trait OcrEngine {
    fn name(&self) -> &str;
    fn ocr_cell(&self, cell: &GrayImage) -> OcrRead;

    /// OCR one cell using an **explicit** layout. The capture flow reads
    /// cells from several screens (relics / parts / arcanes) with one
    /// engine, so it cannot rely on the engine's own stored layout; this
    /// lets it pass the routed layout per cell (eighth pass §2). The
    /// default keeps geometry-free engines (template/stub) unchanged.
    fn ocr_cell_in(&self, cell: &GrayImage, _layout: &Layout) -> OcrRead {
        self.ocr_cell(cell)
    }

    /// OCR a whole page of cells with one layout. The default reads
    /// cell-by-cell through [`Self::ocr_cell_in`]; engines with a
    /// per-call process cost (the tesseract CLI) override this to
    /// cache by crop hash and batch the misses into a single process
    /// (addendum #81 — a scroll step re-shows most cells
    /// pixel-identical, so only the new row should cost anything).
    fn ocr_cells_in(
        &self,
        cells: &[Option<GrayImage>],
        layout: &Layout,
    ) -> Vec<Option<OcrRead>> {
        cells
            .iter()
            .map(|c| c.as_ref().map(|c| self.ocr_cell_in(c, layout)))
            .collect()
    }

    /// OCR an arbitrary `[x0,y0,x1,y1]` fraction-of-IMAGE region (a header
    /// banner) to plain text. The capture flow uses this to classify which
    /// inventory screen a frame shows (eighth pass §2). Engines that cannot
    /// read arbitrary regions return no text — classification then falls
    /// through to "unclassified", which pauses collection rather than
    /// guessing a screen.
    fn read_text(
        &self,
        _image: &GrayImage,
        _region: [f32; 4],
        _psm: u32,
        _upscale: u32,
    ) -> String {
        String::new()
    }

    /// Read only the counts of already-cut cells (pass 33): the
    /// stitch path votes counts from per-instance crops whose names
    /// it never OCRs. The default goes through [`Self::ocr_cells_in`];
    /// engines with a per-call process cost override it to run the
    /// count regions alone.
    fn read_counts_in(
        &self,
        cells: &[Option<GrayImage>],
        layout: &Layout,
    ) -> Vec<Option<u32>> {
        self.ocr_cells_in(cells, layout)
            .into_iter()
            .map(|r| r.and_then(|read| read.count))
            .collect()
    }

    /// Batch variant of [`Self::read_text`]: many region reads in
    /// one call, results in request order. The default loops;
    /// engines with a per-call process cost override it to batch
    /// (addendum #81/#92) — the pass-29 line-1 strip reads go
    /// through here, so a frame's refused cells cost one batched
    /// read instead of one process spawn apiece.
    fn read_text_batch(
        &self,
        requests: &[(&GrayImage, [f32; 4], u32, u32)],
    ) -> Vec<String> {
        requests
            .iter()
            .map(|(image, region, psm, upscale)| {
                self.read_text(image, *region, *psm, *upscale)
            })
            .collect()
    }
}

/// No engine available: every read is empty at zero confidence, so the
/// pipeline outputs empty stock rather than guesses.
pub struct StubEngine;

impl OcrEngine for StubEngine {
    fn name(&self) -> &str {
        "stub"
    }

    fn ocr_cell(&self, _cell: &GrayImage) -> OcrRead {
        OcrRead::empty()
    }
}

/// `tesseract` CLI engine. It probes the bundled sidecar first, then
/// `$PATH` (addendum #30: the Flatpak carries a static tesseract as a
/// sidecar; the pass-3 trait is the seam). Each cell is cropped to the
/// layout's name region, upscaled, and handed to tesseract; the
/// count region is read separately.
pub struct TesseractCliEngine {
    /// Executable to run (the sidecar wrapper, or `tesseract` on PATH).
    pub binary: String,
    /// Where the binary came from (provenance for the summary).
    pub origin: &'static str,
    pub layout: Layout,
    /// Crop-hash read cache for the batched path (addendum #81):
    /// names/uppers keyed by name-region crop, counts by count-region
    /// crop (already parsed; `None` = no label, i.e. exactly one).
    cache: Mutex<OcrCache>,
}

/// Cached region reads for [`TesseractCliEngine::ocr_cells_in`].
/// Entries are keyed by [`crop_key`], which folds in the layout's
/// PSM/upscale salts, so the same pixels under a different layout
/// never alias. Bounded by a blunt reset in the batched reader.
#[derive(Default)]
struct OcrCache {
    names: HashMap<u64, String>,
    uppers: HashMap<u64, String>,
    counts: HashMap<u64, Option<u32>>,
    /// Line-1 strip reads served through
    /// [`OcrEngine::read_text_batch`] (pass 29 re-gate), keyed like
    /// the name crops with a strip salt.
    strips: HashMap<u64, String>,
}

/// Candidate sidecar locations, in probe order, before `$PATH`.
fn sidecar_candidates() -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("FRACKER_TESSERACT") {
        out.push((p.to_string_lossy().into_owned(), "env:FRACKER_TESSERACT"));
    }
    // Next to the running binary (Flatpak/deployed sidecar).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for rel in ["tesseract", "sidecar/tesseract", "../tools/tesseract/bin/tesseract"] {
                out.push((dir.join(rel).to_string_lossy().into_owned(), "exe-relative"));
            }
        }
    }
    // Repo-local tools dir (dev/CI), then the worker box's user-space
    // install at the path recorded in tools/tesseract/PROVENANCE.md.
    out.push((
        concat!(env!("CARGO_MANIFEST_DIR"), "/tools/tesseract/bin/tesseract").to_owned(),
        "repo-tools",
    ));
    out.push((
        "/opt/opencode/tools/tesseract/bin/tesseract".to_owned(),
        "user-space",
    ));
    out
}

impl TesseractCliEngine {
    fn runs(binary: &str) -> bool {
        std::process::Command::new(binary)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Probe order: bundled sidecar paths, then `tesseract` on `$PATH`.
    pub fn probe(layout: Layout) -> Option<Self> {
        for (bin, origin) in sidecar_candidates() {
            if Path::new(&bin).exists() && Self::runs(&bin) {
                return Some(Self {
                    binary: bin,
                    origin,
                    layout,
                    cache: Mutex::new(OcrCache::default()),
                });
            }
        }
        if Self::runs("tesseract") {
            return Some(Self {
                binary: "tesseract".to_owned(),
                origin: "PATH",
                layout,
                cache: Mutex::new(OcrCache::default()),
            });
        }
        None
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

    /// Run tesseract over one cropped+upscaled region.
    fn read_region(
        &self,
        cell: &GrayImage,
        region: [f32; 4],
        psm: u32,
        upscale: u32,
        whitelist: Option<&str>,
    ) -> String {
        let sub = crop_region(cell, region).upscale(upscale);
        self.read_prepared(&sub, psm, whitelist)
    }

    /// Run tesseract over one already cropped+upscaled image — the
    /// body `read_region` used to run inline, now also the per-image
    /// fallback for [`Self::read_batch`].
    fn read_prepared(&self, sub: &GrayImage, psm: u32, whitelist: Option<&str>) -> String {
        let dir = std::env::temp_dir().join(format!(
            "fracker-ocr-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cell.pgm");
        if std::fs::write(&path, sub.to_pgm_bytes()).is_err() {
            return String::new();
        }
        let mut cmd = std::process::Command::new(&self.binary);
        cmd.arg(&path)
            .arg("stdout")
            .arg("--psm")
            .arg(psm.to_string())
            .env("OMP_THREAD_LIMIT", "1");
        if let Some(wl) = whitelist {
            cmd.arg("-c").arg(format!("tessedit_char_whitelist={wl}"));
        }
        let out = cmd.output();
        let _ = std::fs::remove_file(&path);
        match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        }
    }

    /// One tesseract process for many already-upscaled images: the
    /// images are written as a batch dir plus a list file (tesseract's
    /// multi-image mode), the language model loads once, and pages
    /// come back `\f`-separated in list order. Any mismatch — spawn
    /// failure, non-zero exit, page count != image count — falls back
    /// to per-image reads, so a batch failure can never lose a read
    /// the old path would have made (addendum #81).
    fn read_batch(&self, subs: &[GrayImage], psm: u32, whitelist: Option<&str>) -> Vec<String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        if subs.is_empty() {
            return vec![];
        }
        let fallback = |s: &[GrayImage]| -> Vec<String> {
            s.iter().map(|i| self.read_prepared(i, psm, whitelist)).collect()
        };
        let dir = std::env::temp_dir().join(format!(
            "fracker-ocr-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        if std::fs::create_dir_all(&dir).is_err() {
            return fallback(subs);
        }
        let mut list = String::new();
        for (i, s) in subs.iter().enumerate() {
            let p = dir.join(format!("{i}.pgm"));
            if std::fs::write(&p, s.to_pgm_bytes()).is_err() {
                let _ = std::fs::remove_dir_all(&dir);
                return fallback(subs);
            }
            list.push_str(&format!("{}\n", p.display()));
        }
        let lp = dir.join("list.txt");
        if std::fs::write(&lp, list).is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return fallback(subs);
        }
        let mut cmd = std::process::Command::new(&self.binary);
        cmd.arg(&lp)
            .arg("stdout")
            .arg("--psm")
            .arg(psm.to_string())
            .env("OMP_THREAD_LIMIT", "1");
        if let Some(w) = whitelist {
            cmd.arg("-c").arg(format!("tessedit_char_whitelist={w}"));
        }
        let out = cmd.output();
        let _ = std::fs::remove_dir_all(&dir);
        let Ok(o) = out else { return fallback(subs) };
        if !o.status.success() {
            return fallback(subs);
        }
        let text = String::from_utf8_lossy(&o.stdout);
        let pages: Vec<String> = text
            .split('\x0c')
            .take(subs.len())
            .map(|p| {
                p.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        if pages.len() != subs.len() {
            return fallback(subs);
        }
        pages
    }

    fn read_count(&self, cell: &GrayImage, layout: &Layout) -> Option<u32> {
        // Ring gate (pass 33): a badge-style count exists only
        // where the badge's ring is detected. No ring means the
        // stack is ×1 (or the corner carries the built-copy glyph,
        // which is not a count) — never OCR the corner texture.
        if matches!(layout.count_style, CountStyle::Badge) && !badge_ring_present(cell) {
            return None;
        }
        let whitelist = match layout.count_style {
            CountStyle::XPrefix => "xX0123456789",
            CountStyle::Badge => "0123456789",
        };
        let raw = self.read_region(
            cell,
            layout.count,
            layout.count_psm,
            layout.count_upscale,
            Some(whitelist),
        );
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        // Both screens omit the label for a single item; anything <= 1 is
        // treated as "one", never as a spurious reading.
        digits.parse::<u32>().ok().filter(|&n| n > 1)
    }

    /// Arcane rank from the pip row: five evenly spaced diamonds; a
    /// filled pip has a bright CENTRE, an empty one is a faint outline
    /// (its centre is background). Sampling the slot-centre luma is
    /// deterministic (no OCR, no guessing) — eighth pass §1/§3.
    fn read_rank(&self, cell: &GrayImage, layout: &Layout) -> Option<u8> {
        let pips = layout.pips?;
        let band = crop_region(cell, pips);
        if band.width == 0 || band.height == 0 {
            return None;
        }
        const SLOTS: u32 = 5;
        // Filled slots measure a centre mean ~50-58 on Seth's shot; empty
        // outlines ~14. 30 sits safely between.
        const FILLED_MEAN_ABOVE: f64 = 30.0;
        let slot_w = band.width as f32 / SLOTS as f32;
        if slot_w < 1.0 {
            return None;
        }
        let mut filled = 0u8;
        for s in 0..SLOTS {
            let xc = ((s as f32 + 0.5) * slot_w) as u32;
            let mut sum = 0u64;
            let mut n = 0u64;
            for y in 0..band.height {
                for x in xc.saturating_sub(1)..=(xc + 1).min(band.width - 1) {
                    sum += band.get(x, y) as u64;
                    n += 1;
                }
            }
            if n > 0 && (sum as f64 / n as f64) > FILLED_MEAN_ABOVE {
                filled += 1;
            }
        }
        Some(filled)
    }
}

impl OcrEngine for TesseractCliEngine {
    fn name(&self) -> &str {
        "tesseract-cli"
    }

    /// Header/banner read for screen classification (§2). No whitelist:
    /// the header carries letters we must match ("REFINEMENT", "ARCANES").
    fn read_text(
        &self,
        image: &GrayImage,
        region: [f32; 4],
        psm: u32,
        upscale: u32,
    ) -> String {
        self.read_region(image, region, psm, upscale, None)
    }

    /// Batched region reads (pass 29 re-gate): group the requests
    /// by (psm, upscale), crop each region, and serve the group
    /// through the same crop-hash cache + single-process batch
    /// reader as the cell path ([`Self::fill_cache`] /
    /// [`Self::read_batch`]) — a frame's strip reads cost ~one
    /// tesseract launch, and a scroll step re-showing a cell
    /// serves its strip from the cache. Semantics mirror
    /// [`Self::read_region`] exactly: same crop, same upscale, no
    /// whitelist.
    fn read_text_batch(
        &self,
        requests: &[(&GrayImage, [f32; 4], u32, u32)],
    ) -> Vec<String> {
        let mut out: Vec<String> = vec![String::new(); requests.len()];
        let mut groups: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, (_, _, psm, upscale)) in requests.iter().enumerate() {
            groups.entry((*psm, *upscale)).or_default().push(i);
        }
        if groups.is_empty() {
            return out;
        }
        let mut cache = self.cache.lock().unwrap();
        if cache.strips.len() > 4096 {
            cache.strips.clear();
        }
        for ((psm, upscale), idxs) in groups {
            // The salt folds the OCR parameters plus a strip
            // marker into the crop keys, in the same style as the
            // cell path's salts, so strip crops key distinctly
            // from any other region class read at these settings.
            let salt = psm ^ (upscale << 8) ^ (0x57 << 16);
            let crops: Vec<(u64, GrayImage)> = idxs
                .iter()
                .map(|&i| {
                    let (image, region, _, _) = requests[i];
                    let s = crop_region(image, region);
                    (crop_key(&s, salt), s)
                })
                .collect();
            Self::fill_cache(self, &crops, psm, upscale, None, &mut cache.strips);
            for (j, &i) in idxs.iter().enumerate() {
                out[i] = cache.strips[&crops[j].0].clone();
            }
        }
        out
    }

    fn ocr_cell(&self, cell: &GrayImage) -> OcrRead {
        self.read_cell(cell, &self.layout)
    }

    fn ocr_cell_in(&self, cell: &GrayImage, layout: &Layout) -> OcrRead {
        self.read_cell(cell, layout)
    }

    /// Batched page read (addendum #81): crop every cell's name /
    /// upper-name / count regions, key them by [`crop_key`], serve
    /// repeats from the cache, and batch each region class's misses
    /// into ONE tesseract process per class (see [`Self::read_batch`]).
    /// A frame costs ~3 process launches; a scroll step re-shows most
    /// cells pixel-identical, so it costs ~2 (the new row's names +
    /// counts) instead of 60–90 per-cell launches. Semantics mirror
    /// [`Self::read_cell`] exactly: same regions, same upper-band rule,
    /// same count parsing (`> 1` or `None`), same confidence shape.
    fn ocr_cells_in(
        &self,
        cells: &[Option<GrayImage>],
        layout: &Layout,
    ) -> Vec<Option<OcrRead>> {
        let mut cache = self.cache.lock().unwrap();
        if cache.names.len() > 4096 {
            *cache = OcrCache::default();
        }
        // Salts fold the OCR parameters into the crop keys, so the
        // same pixels read under different settings never alias.
        let ns = layout.name_psm ^ (layout.name_upscale << 8);
        let cs = layout.count_psm
            ^ (layout.count_upscale << 8)
            ^ ((matches!(layout.count_style, CountStyle::XPrefix) as u32) << 16);
        let crop = |c: &GrayImage, r: [f32; 4], salt: u32| {
            let s = crop_region(c, r);
            (crop_key(&s, salt), s)
        };

        let live: Vec<&GrayImage> = cells.iter().flatten().collect();
        let names: Vec<(u64, GrayImage)> =
            live.iter().map(|c| crop(c, layout.name, ns)).collect();
        Self::fill_cache(
            self,
            &names,
            layout.name_psm,
            layout.name_upscale,
            None,
            &mut cache.names,
        );

        // Upper band only for cells whose lower read is a bare label
        // (or empty) — the same rule `read_cell` pays a second read for.
        if let Some(u) = layout.name_upper {
            let uppers: Vec<(u64, GrayImage)> = live
                .iter()
                .zip(&names)
                .filter(|(_, (k, _))| {
                    let t = &cache.names[k];
                    t.trim().is_empty() || t.contains('[')
                })
                .map(|(c, _)| crop(c, u, ns ^ 0x5555))
                .collect();
            Self::fill_cache(
                self,
                &uppers,
                layout.name_psm,
                layout.name_upscale,
                None,
                &mut cache.uppers,
            );
        }

        let counts: Vec<(u64, GrayImage)> =
            live.iter().map(|c| crop(c, layout.count, cs)).collect();
        // Ring gate (pass 33), the batched twin of the check in
        // `read_count`: badge-style counts are OCR'd only for cells
        // whose badge ring is detected; the rest never reach
        // tesseract and resolve to no count (×1). The gate is a
        // pure function of the cell's pixels, but its window is
        // larger than the count crop the cache keys on, so gate
        // outcomes are NOT cached — they are recomputed per cell
        // (the NCC is far cheaper than the OCR it gates).
        let ring_ok: Vec<bool> = if matches!(layout.count_style, CountStyle::Badge) {
            live.iter().map(|c| badge_ring_present(c)).collect()
        } else {
            vec![true; live.len()]
        };
        let wl = match layout.count_style {
            CountStyle::XPrefix => "xX0123456789",
            CountStyle::Badge => "0123456789",
        };
        let todo: Vec<(u64, GrayImage)> = counts
            .iter()
            .zip(ring_ok.iter())
            .filter(|((k, _), ok)| **ok && !cache.counts.contains_key(k))
            .map(|(c, _)| c.clone())
            .collect();
        let mut raw_counts: HashMap<u64, String> = HashMap::new();
        Self::fill_cache(
            self,
            &todo,
            layout.count_psm,
            layout.count_upscale,
            Some(wl),
            &mut raw_counts,
        );
        for (k, t) in raw_counts {
            let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
            cache
                .counts
                .insert(k, digits.parse::<u32>().ok().filter(|&n| n > 1));
        }

        let mut i = 0usize;
        cells
            .iter()
            .map(|c| {
                c.as_ref().map(|cell| {
                    let (nk, _) = &names[i];
                    let mut text = cache.names[nk].clone();
                    if let Some(u) = layout.name_upper {
                        if text.trim().is_empty() || text.contains('[') {
                            let uk = crop_key(&crop_region(cell, u), ns ^ 0x5555);
                            if let Some(up) = cache.uppers.get(&uk) {
                                if !up.trim().is_empty() {
                                    text = format!("{} {}", up.trim(), text.trim());
                                }
                            }
                        }
                    }
                    let read = OcrRead {
                        confidence: if text.is_empty() { 0.0 } else { 0.85 },
                        text,
                        // Ring-gated cells take their count from
                        // the cache; ungated cells never had their
                        // corner OCR'd and report no count.
                        count: if ring_ok[i] {
                            cache.counts[&counts[i].0]
                        } else {
                            None
                        },
                        rank: self.read_rank(cell, layout),
                    };
                    i += 1;
                    read
                })
            })
            .collect()
    }

    /// Counts-only batched reads (pass 33): the counts half of
    /// [`Self::ocr_cells_in`] run on its own — same count crop,
    /// same crop-hash cache, same ring gate, same parse rule
    /// (digits only, kept only when > 1), so a count read here and
    /// a count read there can never disagree. The stitch path
    /// votes per-instance counts through this without paying name
    /// OCR for crops it only needs the badge of.
    fn read_counts_in(
        &self,
        cells: &[Option<GrayImage>],
        layout: &Layout,
    ) -> Vec<Option<u32>> {
        let mut cache = self.cache.lock().unwrap();
        if cache.names.len() > 4096 {
            *cache = OcrCache::default();
        }
        let cs = layout.count_psm
            ^ (layout.count_upscale << 8)
            ^ ((matches!(layout.count_style, CountStyle::XPrefix) as u32) << 16);
        let live: Vec<(usize, &GrayImage)> = cells
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_ref().map(|c| (i, c)))
            .collect();
        let crops: Vec<(u64, GrayImage)> = live
            .iter()
            .map(|(_, c)| {
                let s = crop_region(c, layout.count);
                (crop_key(&s, cs), s)
            })
            .collect();
        let ring_ok: Vec<bool> = if matches!(layout.count_style, CountStyle::Badge) {
            live.iter().map(|(_, c)| badge_ring_present(c)).collect()
        } else {
            vec![true; live.len()]
        };
        let wl = match layout.count_style {
            CountStyle::XPrefix => "xX0123456789",
            CountStyle::Badge => "0123456789",
        };
        let todo: Vec<(u64, GrayImage)> = crops
            .iter()
            .zip(ring_ok.iter())
            .filter(|((k, _), ok)| **ok && !cache.counts.contains_key(k))
            .map(|(c, _)| c.clone())
            .collect();
        let mut raw_counts: HashMap<u64, String> = HashMap::new();
        Self::fill_cache(
            self,
            &todo,
            layout.count_psm,
            layout.count_upscale,
            Some(wl),
            &mut raw_counts,
        );
        for (k, t) in raw_counts {
            let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
            cache
                .counts
                .insert(k, digits.parse::<u32>().ok().filter(|&n| n > 1));
        }
        let mut out: Vec<Option<u32>> = vec![None; cells.len()];
        for (li, (ci, _)) in live.iter().enumerate() {
            if ring_ok[li] {
                out[*ci] = cache.counts[&crops[li].0];
            }
        }
        out
    }
}

impl TesseractCliEngine {
    /// Fill one cache map from the crops it does not have yet, in a
    /// single [`Self::read_batch`] (addendum #81). Duplicate keys in
    /// `crops` are read once. Crops are upscaled here, matching what
    /// `read_region` feeds tesseract on the per-cell path.
    fn fill_cache(
        &self,
        crops: &[(u64, GrayImage)],
        psm: u32,
        upscale: u32,
        whitelist: Option<&str>,
        map: &mut HashMap<u64, String>,
    ) {
        let mut seen = HashSet::new();
        let miss: Vec<&(u64, GrayImage)> = crops
            .iter()
            .filter(|(k, _)| !map.contains_key(k) && seen.insert(*k))
            .collect();
        if miss.is_empty() {
            return;
        }
        let imgs: Vec<GrayImage> = miss.iter().map(|(_, s)| s.upscale(upscale)).collect();
        for ((k, _), t) in miss.iter().zip(self.read_batch(&imgs, psm, whitelist)) {
            map.insert(*k, t);
        }
    }

    /// Read one cell's name/count/rank with an explicit layout (so the
    /// capture flow can route relics/parts/arcanes to the right regions
    /// with a single engine).
    fn read_cell(&self, cell: &GrayImage, layout: &Layout) -> OcrRead {
        let mut text = self.read_region(
            cell,
            layout.name,
            layout.name_psm,
            layout.name_upscale,
            None,
        );
        // Two-line cells (eighth pass §1): the lower band holds the
        // `[Radiant]`/`[Flawless]` label for a refined stack, with the
        // relic name displaced into the upper band. Only when the lower
        // read is a bare label (or empty) do we pay for the second read.
        if let Some(upper) = layout.name_upper {
            if text.trim().is_empty() || text.contains('[') {
                let up = self.read_region(
                    cell,
                    upper,
                    layout.name_psm,
                    layout.name_upscale,
                    None,
                );
                if !up.trim().is_empty() {
                    text = format!("{} {}", up.trim(), text.trim());
                }
            }
        }
        let confidence = if text.is_empty() { 0.0 } else { 0.85 };
        OcrRead {
            text,
            confidence,
            count: self.read_count(cell, layout),
            rank: self.read_rank(cell, layout),
        }
    }
}

/// Box status line for the pass summary.
pub fn engine_status() -> String {
    match TesseractCliEngine::probe(default_layout()) {
        Some(e) => format!("tesseract available ({} via {})", e.version(), e.origin),
        None => "tesseract NOT installed on this box (no root to install it); "
            .to_owned()
            + "shipping the OcrEngine trait + StubEngine; live reads come from the capture session",
    }
}

/// Fallback layout used only by [`engine_status`] when no layout file is
/// in hand (a bare probe). All real reads use a loaded layout.
fn default_layout() -> Layout {
    Layout {
        key: "relic".to_owned(),
        cols: 11,
        rows: 7,
        ref_width: 1941,
        ref_height: 1091,
        x0: 48.0,
        cell_w: 127.0,
        y0: 116.0,
        cell_h: 132.0,
        name: [0.063, 0.742, 0.937, 0.894],
        count: [0.10, 0.06, 0.44, 0.26],
        count_style: CountStyle::XPrefix,
        name_psm: 7,
        count_psm: 7,
        name_upscale: 6,
        count_upscale: 8,
        name_upper: Some([0.063, 0.62, 0.937, 0.75]),
        pips: None,
        min_score: 0.8,
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
                count: None,
                rank: None,
            };
        }
        OcrRead {
            text,
            confidence: conf,
            count: None,
            rank: None,
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
            count: None,
            rank: None,
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
    /// Discriminative tokens per name (pass 33, addendum #97): the
    /// name's HEAD tokens — everything before its suffix class's
    /// shared trailing run — or all its tokens when it shares no
    /// suffix with another name. These are the only tokens that
    /// distinguish the name from its siblings; [`Self::hold_permitted`]
    /// is defined over them. Computed once at construction (the
    /// derivation is a pairwise pass, far too costly per read).
    discriminative: Vec<Vec<String>>,
    /// Built suffix classes for the radius decoder (pass 29), keyed
    /// by suffix. Building a class costs a pairwise pass over its
    /// members' heads; a capture session refuses the same stacks
    /// frame after frame, so each class is computed once and shared.
    radius_cache: Arc<Mutex<HashMap<String, RadiusClass>>>,
}

impl Universe {
    pub fn from_names(names: &[String]) -> Self {
        let folded: Vec<String> = names.iter().map(|n| fold(n)).collect();
        let tokenised: Vec<Vec<&str>> = folded
            .iter()
            .map(|f| f.split(' ').filter(|t| !t.is_empty()).collect())
            .collect();
        let discriminative = (0..names.len())
            .map(|i| discriminative_tokens(&tokenised, i))
            .collect();
        Self {
            names: names.to_vec(),
            folded,
            discriminative,
            radius_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// All canonical names (diagnostics and tests).
    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Best fuzzy match for a raw read. Returns the canonical name and
    /// a 0..1 score. Matching is case/space-insensitive and forgives the
    /// glyph confusions OCR actually makes on this UI (0/O, 1/I/l,
    /// 5/S, 8/B, 2/Z, 6/G). The threshold lives with the caller so tests
    /// can pin it.
    pub fn best_match(&self, read: &str) -> Option<(&str, f32)> {
        self.best_match_margin(read).map(|(n, s, _)| (n, s))
    }

    /// As [`Self::best_match`], plus the runner-up score so the caller
    /// can refuse an ambiguous read (the never-guess rule): when the two
    /// best names are nearly tied we do NOT hold either.
    ///
    /// Icon art bleeds into a cell's name band (the arcanes screen in
    /// particular), so a read may carry leading garbage tokens. We try
    /// the read whole and with EVERY leading-token suffix trim (pass
    /// 27; the cap of 4 used to strand clean names sitting behind
    /// 5–10 icon tokens — the census's score-dilution class), keeping
    /// the best-scoring interpretation; the margin guard still
    /// applies, so trimming never turns noise into a confident guess.
    pub fn best_match_margin(&self, read: &str) -> Option<(&str, f32, f32)> {
        let cleaned = strip_tier_label(read);
        let cleaned = strip_relic_suffix(&cleaned);
        let tokens: Vec<&str> = cleaned.split_whitespace().collect();
        if tokens.is_empty() {
            return None;
        }
        let mut best: Option<(&str, f32, f32)> = None;
        for start in 0..tokens.len() {
            let cand = fold(&tokens[start..].join(" "));
            if cand.is_empty() {
                continue;
            }
            if let Some(m) = self.match_one(&cand) {
                if best.map_or(true, |b| m.1 > b.1) {
                    best = Some(m);
                }
            }
        }
        best
    }

    /// The top-scoring names for a raw read, best first, over
    /// the same interpretations [`Self::best_match_margin`]
    /// tries (whole read + every leading-token suffix trim) with
    /// the same scoring (`match_one`'s confusable-Levenshtein
    /// similarity) — each name keeps its best score across
    /// interpretations. Pass 35's synthesis candidate
    /// enumeration consumes this: a nomination is evidence, not
    /// a hold, so no margin or predicate logic lives here.
    pub fn top_matches(&self, read: &str, cap: usize) -> Vec<(&str, f32)> {
        let cleaned = strip_tier_label(read);
        let cleaned = strip_relic_suffix(&cleaned);
        let tokens: Vec<&str> = cleaned.split_whitespace().collect();
        if tokens.is_empty() || cap == 0 {
            return Vec::new();
        }
        let mut scores: Vec<(usize, f32)> = Vec::new();
        for start in 0..tokens.len() {
            let cand = fold(&tokens[start..].join(" "));
            if cand.is_empty() {
                continue;
            }
            for (i, folded) in self.folded.iter().enumerate() {
                let d = levenshtein_confusable(&cand, folded);
                let denom = cand.chars().count().max(folded.chars().count()).max(1) as f32;
                let score = 1.0 - d as f32 / denom;
                match scores.iter_mut().find(|(j, _)| *j == i) {
                    Some(e) => {
                        if score > e.1 {
                            e.1 = score;
                        }
                    }
                    None => scores.push((i, score)),
                }
            }
        }
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(cap);
        scores
            .into_iter()
            .map(|(i, s)| (self.names[i].as_str(), s))
            .collect()
    }

    /// The name's FORM SIBLING (pass 34): the built
    /// form of a blueprint name (`"X Blueprint"` -> `"X"`), or the
    /// blueprint form of a built name (`"X"` -> `"X Blueprint"`),
    /// when the sibling is itself in this universe. `None` when
    /// the item has only one form here — form evidence is then
    /// vacuous for the item, because no hold of it can mint or be
    /// confused with a sibling form. The form channels
    /// (`crate::lineread`) gate holds through this relation.
    pub fn form_sibling(&self, name: &str) -> Option<String> {
        if let Some(base) = name.strip_suffix(" Blueprint") {
            if self.names.iter().any(|n| n == base) {
                return Some(base.to_owned());
            }
        }
        let bp = format!("{name} Blueprint");
        if self.names.iter().any(|n| n == &bp) {
            return Some(bp);
        }
        None
    }

    /// The candidate's discriminative tokens (its head — the only
    /// tokens that distinguish it from its suffix-class siblings),
    /// or `None` for a name outside this universe. Diagnostics and
    /// tests; the hold paths use [`Self::hold_permitted`].
    pub fn discriminative_tokens_of(&self, candidate: &str) -> Option<&[String]> {
        self.names
            .iter()
            .position(|n| n == candidate)
            .map(|i| self.discriminative[i].as_slice())
    }

    /// THE hold predicate (pass 33, addendum #97) — the single
    /// condition every path that can hold an item must satisfy, on
    /// top of its own evidence bars. Two clauses:
    ///
    /// 1. **Head witness.** The evidence read must witness the
    ///    candidate's HEAD — its discriminative tokens, the only
    ///    ones distinguishing it from its suffix-class siblings
    ///    (see [`head_witnessed`]). A whole-name score counts
    ///    shared tokens as evidence: on a read carrying only a
    ///    shared suffix ("Prime Handle" — an icon-bleed fragment,
    ///    a composite cell, another item's tail), EVERY
    ///    "... Prime Handle" sibling scores high, the shortest
    ///    name's dilution is smallest, and the margin between
    ///    near-equal siblings can clear the guard on length alone
    ///    — the pass-32 HC gate's Bo Prime Handle shape (truths
    ///    Fragor / Venato / Karyst) and the reviewer's Q4
    ///    diagnosis. Zero discriminative witnesses must mean no
    ///    hold, whatever the diluted score says.
    /// 2. **No clean strict subset.** If every token of the read
    ///    is EXACTLY a token of the candidate and the read is
    ///    shorter than the candidate, the read is what a token
    ///    deletion of a longer name looks like ("Mesa Chassis",
    ///    of Mesa Prime Chassis Blueprint): holding any name but
    ///    the read's own exact name asserts tokens the read never
    ///    showed. The property battery (`hold_paths_never_hold_
    ///    a_different_item_on_token_subsets`) exercises exactly
    ///    this shape exhaustively. Noisy reads — garbled,
    ///    truncated or merged tokens — are never exact subsets,
    ///    so this clause leaves the matcher's dilution model its
    ///    legitimate territory (the gate measured a stricter
    ///    full-coverage variant refusing genuine holds on merged
    ///    "NidusPrime" and truncated "Protea P …" reads; that
    ///    variant was rejected on that evidence).
    ///
    /// Enforcement points (one function, every hold path):
    /// [`validate_read`] for the margin matcher, and the two
    /// radius decoder paths ([`Self::radius_decode_budget`],
    /// [`Self::radius_decode_strip`]) — each on its evidence
    /// read(s): the band read for the band path; for the strip
    /// path the union of the strip read (its head evidence) and
    /// the band read (its suffix evidence, tail-verified inside
    /// the call). The matcher's internal RANKING is deliberately
    /// not zeroed: the decoder's class nomination consumes it,
    /// and a nomination is not a hold — the pinned strip
    /// counter-case ("x Prime Receiver" band + "Lex Prime" strip)
    /// nominates the Receiver class from a band read whose head
    /// token is destroyed, which is legitimate because the class
    /// is verified by the witnessed tail and the hold itself
    /// still passes this predicate on the combined evidence.
    /// Zeroing the ranking would kill that mechanism; gating the
    /// hold does not. The predicate is a NECESSARY condition,
    /// never sufficient: the decoder's evidence floor (pass 32)
    /// still refuses 2-char fragments the predicate alone would
    /// pass ("le" sits within one edit of "lex"), and the
    /// matcher's score/margin bars still apply. Quantity of
    /// evidence and discriminativeness of evidence are separate
    /// questions; each path must answer both.
    pub fn hold_permitted(&self, read: &str, candidate: &str) -> bool {
        self.hold_permitted_tokens(&cleaned_read_tokens(read), candidate)
    }

    /// The predicate over pre-tokenised evidence (the strip
    /// decoder's two evidence reads union here).
    fn hold_permitted_tokens(&self, read_toks: &[String], candidate: &str) -> bool {
        let Some(idx) = self.names.iter().position(|n| n == candidate) else {
            return false;
        };
        if read_toks.is_empty() {
            return false;
        }
        let cand_toks: Vec<String> = self.folded[idx]
            .split(' ')
            .filter(|t| !t.is_empty())
            .map(squash_token)
            .filter(|t| !t.is_empty())
            .collect();
        // Clause 2: a clean strict subset holds only its own
        // exact name (which is not strict, so it passes here).
        if strict_exact_subset(read_toks, &cand_toks) {
            return false;
        }
        // Clause 1: the head must be witnessed.
        let head: Vec<String> = self.discriminative[idx]
            .iter()
            .map(|t| squash_token(t))
            .filter(|t| !t.is_empty())
            .collect();
        if head.is_empty() {
            return false;
        }
        // The candidate token following the head (for the merged
        // witness target), in squashed form.
        let head_folded_len = self.discriminative[idx].len();
        let next_tok = self.folded[idx]
            .split(' ')
            .filter(|t| !t.is_empty())
            .nth(head_folded_len)
            .map(squash_token)
            .filter(|t| !t.is_empty());
        head_witnessed(read_toks, &head, next_tok.as_deref())
    }

    /// Best name for one already-folded candidate plus its runner-up.
    fn match_one(&self, f: &str) -> Option<(&str, f32, f32)> {
        let mut best: Option<(&str, f32)> = None;
        let mut second = 0.0f32;
        for (name, folded) in self.names.iter().zip(self.folded.iter()) {
            let d = levenshtein_confusable(f, folded);
            let denom = f.chars().count().max(folded.chars().count()).max(1) as f32;
            let score = 1.0 - d as f32 / denom;
            match best {
                None => best = Some((name.as_str(), score)),
                Some((_, s)) if score > s => {
                    second = s;
                    best = Some((name.as_str(), score));
                }
                _ => {
                    if score > second {
                        second = score;
                    }
                }
            }
        }
        best.map(|(n, s)| (n, s, second))
    }
}

/// Head tokens of `tokenised[idx]` — its discriminative tokens
/// (pass 33): the tokens before the longest trailing run the name
/// shares with at least one other universe name, using the same
/// suffix derivation as the radius decoder's classes (see
/// [`Universe::derive_class_parts`]); all of the name's tokens when
/// no trailing run is shared (a classless name is discriminated by
/// every token it has).
fn discriminative_tokens(tokenised: &[Vec<&str>], idx: usize) -> Vec<String> {
    let toks = &tokenised[idx];
    if toks.len() >= 2 {
        for k in (1..toks.len()).rev() {
            let suffix = &toks[toks.len() - k..];
            let shared = tokenised
                .iter()
                .filter(|t| t.len() > k && &t[t.len() - k..] == suffix)
                .count();
            if shared >= 2 {
                return toks[..toks.len() - k]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            }
        }
    }
    toks.iter().map(|s| s.to_string()).collect()
}

/// How close a read token must sit to a head token to count as
/// witnessing it (metric-closure distance, pass 33). Longer heads
/// tolerate the decoder's witness cap of 2; a 3-char head ("lex",
/// "mag") tightens to 1 — an exact token or a single junk affix
/// ("xmag"). A head of ≤2 chars ("bo") requires metric EXACTNESS:
/// at cap 1, any 2-char token sharing one char ("to", from the
/// census's Venato read) would witness it, which is no witness at
/// all. Exactness under the metric closure still forgives the
/// glyph confusions themselves ("b0" is metrically "bo"), so the
/// tightening costs only non-confusable near-misses.
fn witness_cap(head_token: &str) -> usize {
    match head_token.chars().count() {
        0..=2 => 0,
        3 => 1,
        _ => 2,
    }
}

/// A token stripped to its alphanumerics — the same squash the
/// radius decoder applies to fragments, so junk glyphs (`"`, `*`,
/// `”`) around a real token cost no witness distance.
fn squash_token(t: &str) -> String {
    t.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// A read's evidence tokens: cleaned exactly as the matcher
/// cleans (tier labels and the relic suffix stripped), folded,
/// and squashed to alphanumerics (junk glyphs cost no witness
/// distance, as in the decoder's fragments).
fn cleaned_read_tokens(read: &str) -> Vec<String> {
    let cleaned = strip_tier_label(read);
    let cleaned = strip_relic_suffix(&cleaned);
    let folded = fold(&cleaned);
    folded
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(squash_token)
        .filter(|t| !t.is_empty())
        .collect()
}

/// Is the candidate's HEAD witnessed by the evidence read?
/// Witness shapes, per head token: a read token within
/// [`witness_cap`] — or the JOIN of two adjacent read tokens,
/// because OCR splits gear words mid-word ("akb olto" for
/// "akbolto") and neither half alone is within reach of the head
/// it jointly spells. OCR also MERGES words ("NidusPrime" for
/// "Nidus Prime"), so the head tokens concatenated — and the
/// head plus the first suffix token — are witness targets too,
/// but only against SINGLE read tokens: comparing joins against
/// merged targets would let "to prime" witness "bo prime" (the
/// pass-32 Bo shape entering through the side door), which is
/// why the two target classes are kept apart.
fn head_witnessed(read_toks: &[String], head: &[String], next_tok: Option<&str>) -> bool {
    for h in head {
        let cap = witness_cap(h);
        if read_toks
            .iter()
            .any(|w| levenshtein_confusable_metric_bounded(w, h, cap) <= cap)
        {
            return true;
        }
        if read_toks.windows(2).any(|w| {
            let j = format!("{}{}", w[0], w[1]);
            levenshtein_confusable_metric_bounded(&j, h, cap) <= cap
        }) {
            return true;
        }
    }
    // Merged targets, against single read tokens only — and the
    // witnessing token must SPAN the merge (be longer than the
    // largest component), so the read's own bare suffix token
    // cannot witness "boprime" merely by resembling its tail.
    let merged = head.concat();
    let longest_head = head.iter().map(|h| h.len()).max().unwrap_or(0);
    if merged.len() > longest_head || next_tok.is_some() {
        let cap = witness_cap(&merged);
        if read_toks.iter().any(|w| {
            w.len() > longest_head && levenshtein_confusable_metric_bounded(w, &merged, cap) <= cap
        }) {
            return true;
        }
        if let Some(n) = next_tok {
            let merged2 = format!("{merged}{n}");
            let cap = witness_cap(&merged2);
            if read_toks.iter().any(|w| {
                w.len() > n.len() && levenshtein_confusable_metric_bounded(w, &merged2, cap) <= cap
            }) {
                return true;
            }
        }
    }
    false
}

/// Is the read an EXACT strict token subset of the candidate —
/// every read token equal (post-squash) to a distinct candidate
/// token, with tokens left over? Such a read is
/// indistinguishable from a token deletion of the candidate
/// (or of a longer sibling), so it may hold only a name it
/// equals exactly — which is not strict, and passes.
fn strict_exact_subset(read_toks: &[String], cand_toks: &[String]) -> bool {
    if read_toks.len() >= cand_toks.len() {
        return false;
    }
    let mut used = vec![false; cand_toks.len()];
    for rt in read_toks {
        match cand_toks
            .iter()
            .enumerate()
            .find(|(i, ct)| !used[*i] && *ct == rt)
        {
            Some((i, _)) => used[i] = true,
            None => return false,
        }
    }
    true
}

/// The UI labels relic cells "Axi A1 Relic"; the universe stores "Axi A1".
/// Drop a trailing `relic` token before matching (never a real item name
/// suffix in this universe).
fn strip_relic_suffix(s: &str) -> &str {
    let t = s.trim_end();
    let start = t.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
    if start < t.len() && t[start..].eq_ignore_ascii_case("relic") {
        return t[..start].trim_end();
    }
    t
}

fn fold(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Glyph pairs OCR confuses on this UI, scored as an equal substitution.
/// Only letter<->digit (or symbol) pairs are listed, never two letters
/// (I vs L are both real relic prefixes and must stay distinct).
fn confusable(a: char, b: char) -> bool {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    if a == b {
        return true;
    }
    matches!(
        (a, b),
        ('0', 'o') | ('o', '0')
            | ('1', 'i') | ('i', '1')
            | ('1', 'l') | ('l', '1')
            | ('|', 'i') | ('i', '|')
            | ('|', '1') | ('1', '|')
            | ('|', 'l') | ('l', '|')
            | ('5', 's') | ('s', '5')
            | ('8', 'b') | ('b', '8')
            | ('2', 'z') | ('z', '2')
            | ('6', 'g') | ('g', '6')
            // tesseract renders the narrow UI "E" as a pound sign on the
            // 127-px relic cells; fold it so Axi E1 still validates.
            | ('£', 'e') | ('e', '£')
            // ... and the relic "S" as a dollar sign at 1440p. This is the
            // named cause of Lith S14's v0.2.0 drop (eighth pass §1): every
            // "S14"/"S12"/"S15" stack read as "$14"/"$12"/"$15" and failed
            // validation. Fold it so S-relics hold.
            | ('$', 's') | ('s', '$')
    )
}

/// Substitution predicate for the metric closure of [`confusable`]
/// (pass 29): two characters substitute at zero cost when they are
/// equal or in the same CONNECTED COMPONENT of the confusability
/// graph. The pairwise relation is not transitive — `1` is
/// confusable with both `i` and `l`, but `i` and `l` are not
/// pairwise confusable — so the pairwise distance is NOT a metric
/// (witness: d("inaros","1aros") = d("1aros","lavos") = 1 but
/// d("inaros","lavos") = 3), and bounded-distance balls under it
/// can overlap. The component closure is the smallest enlargement
/// that restores the triangle inequality; the radius decoder uses
/// it for ALL its distances (radii and fragment alike). The margin
/// matcher keeps the pairwise relation — `i` vs `l` must stay
/// distinct there (both are real relic prefixes).
fn confusable_metric(a: char, b: char) -> bool {
    fn component(c: char) -> u8 {
        match c.to_ascii_lowercase() {
            '0' | 'o' => 1,
            '1' | 'i' | 'l' | '|' => 2,
            '5' | 's' | '$' => 3,
            '8' | 'b' => 4,
            '2' | 'z' => 5,
            '6' | 'g' => 6,
            '£' | 'e' => 7,
            _ => 0,
        }
    }
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    a == b || (component(a) != 0 && component(a) == component(b))
}

/// Levenshtein DP over a zero-cost substitution predicate.
fn lev_pred(a: &str, b: &str, pred: fn(char, char) -> bool) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    lev_chars(&a, &b, pred)
}

/// [`lev_pred`] over pre-collected char slices (the radius
/// decoder decodes one fragment against every head in a class;
/// collecting each head's chars once at class build keeps the
/// per-decode loop allocation-free).
fn lev_chars(a: &[char], b: &[char], pred: fn(char, char) -> bool) -> usize {
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
            let sub = prev[j] + usize::from(!pred(ca, cb));
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Bounded variant of [`lev_chars`] (see [`lev_pred_bounded`]).
/// Banded: only cells within `limit` of the diagonal are computed
/// — a within-limit alignment path never leaves that band — and
/// the DP aborts as soon as a whole band row exceeds `limit`.
fn lev_chars_bounded(
    a: &[char],
    b: &[char],
    limit: usize,
    pred: fn(char, char) -> bool,
) -> usize {
    if a.len().abs_diff(b.len()) > limit {
        return limit + 1;
    }
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    const INF: usize = usize::MAX / 4;
    let mut prev: Vec<usize> = (0..=b.len()).map(|j| if j <= limit { j } else { INF }).collect();
    let mut cur = vec![INF; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        let i = i + 1;
        cur[0] = if i <= limit { i } else { INF };
        let j0 = i.saturating_sub(limit).max(1);
        let j1 = (i + limit).min(b.len());
        let mut row_min = cur[0];
        for j in j0..=j1 {
            let sub = prev[j - 1] + usize::from(!pred(ca, b[j - 1]));
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
            row_min = row_min.min(cur[j]);
        }
        if j1 < b.len() {
            cur[j1 + 1] = INF;
        }
        if row_min > limit {
            return limit + 1;
        }
        std::mem::swap(&mut prev, &mut cur);
        cur.fill(INF);
    }
    prev[b.len()]
}

/// Bounded variant of [`lev_pred`]: exact when the true distance
/// is `<= limit`, otherwise some value `> limit` (callers testing
/// `d <= limit` cannot tell the difference). A length difference
/// beyond `limit` settles it without any DP, and the DP aborts as
/// soon as a whole row exceeds `limit` (the final distance is
/// bounded below by every row's minimum).
fn lev_pred_bounded(a: &str, b: &str, limit: usize, pred: fn(char, char) -> bool) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    lev_chars_bounded(&a, &b, limit, pred)
}

/// Levenshtein with [`confusable`] substitutions costing zero.
pub fn levenshtein_confusable(a: &str, b: &str) -> usize {
    lev_pred(a, b, confusable)
}

/// [`levenshtein_confusable`] with an early exit (see
/// [`lev_pred_bounded`]).
pub fn levenshtein_confusable_bounded(a: &str, b: &str, limit: usize) -> usize {
    lev_pred_bounded(a, b, limit, confusable)
}

/// Levenshtein under the metric closure [`confusable_metric`] —
/// a true metric, and the only distance the radius decoder's
/// disjointness guarantee is stated in.
pub fn levenshtein_confusable_metric(a: &str, b: &str) -> usize {
    lev_pred(a, b, confusable_metric)
}

/// [`levenshtein_confusable_metric`] with an early exit.
pub fn levenshtein_confusable_metric_bounded(a: &str, b: &str, limit: usize) -> usize {
    lev_pred_bounded(a, b, limit, confusable_metric)
}

/// Plain edit distance (no confusable folding), kept for callers/tests
/// that want a literal distance.
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
    /// Stack count read from the UI label ("x8"/badge); `None` means one.
    pub count: Option<u32>,
    /// Relic refinement tier parsed from the cell (eighth pass §1).
    pub tier: RefinementTier,
    /// Arcane rank read from the pip row; `None` on screens without pips.
    pub rank: Option<u8>,
    /// Pass 38 (addendum #103): the relic owned-gate excluded this
    /// tile as an unowned catalogue entry. `held` is already false;
    /// this flag is the observable record the capture report lists.
    pub gate_excluded: bool,
    /// Pass 45 (addendum #110): the eye-window bright mass
    /// measured for this tile on a refinement frame, when it
    /// was held without a count label — the session-level
    /// unowned evidence for the three-state disposition (see
    /// [`RefinementEvidence`]). `None` when not measured: not
    /// a refinement frame, tile not held, or tile carried a
    /// count label (a label is positive owned evidence and
    /// needs no second signal).
    pub refine_eye_mass: Option<u32>,
    /// Pass 46 (addendum #111): the relic name-admission gate
    /// refused this read's resolved name (see
    /// [`relic_name_admission`]). `held` is already false;
    /// this record is what the finalize log names, so a
    /// refused read reads as a decision, not a loss.
    pub name_refused: Option<NameRefusal>,
}

/// One relic name resolution the admission gate refused
/// (pass 46, addendum #111): the raw read as the engine
/// produced it, the name the resolver would have imported,
/// and the nearest other universe name under the resolver's
/// own metric — the finalize log's record of what was not
/// imported and why.
#[derive(Debug, Clone, PartialEq)]
pub struct NameRefusal {
    /// The raw engine read, unmodified.
    pub raw: String,
    /// The resolved name that was refused.
    pub winner: String,
    /// The winner's resolver score (0..1).
    pub winner_score: f32,
    /// The nearest other universe name, when one scored.
    pub runner_up: Option<String>,
    /// The runner-up's resolver score (0..1).
    pub runner_up_score: f32,
}

impl NameRefusal {
    /// Winner minus runner-up under the resolver's metric.
    pub fn margin(&self) -> f32 {
        self.winner_score - self.runner_up_score
    }

    /// One-line log form: raw read, refused winner,
    /// runner-up, and the margin between them.
    pub fn describe(&self) -> String {
        match &self.runner_up {
            Some(r) => format!(
                "{:?} -> {} ({:.2}) over {} ({:.2}), margin {:.2}",
                self.raw,
                self.winner,
                self.winner_score,
                r,
                self.runner_up_score,
                self.margin()
            ),
            None => format!(
                "{:?} -> {} ({:.2}), no runner-up",
                self.raw, self.winner, self.winner_score
            ),
        }
    }
}

/// Ambiguity guard: the runner-up name must trail the winner by at least
/// this much, else the read is NOT held (never guess between two items).
pub const MATCH_MARGIN: f32 = 0.10;

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
        count: None,
        tier: RefinementTier::Unknown,
        rank: read.rank,
        gate_excluded: false,
        refine_eye_mass: None,
        name_refused: None,
    };
    if read.confidence < min_confidence {
        return out;
    }
    if let Some((name, score, second)) = universe.best_match_margin(&read.text) {
        out.matched = Some(name.to_owned());
        out.match_score = score;
        // The shared discriminative-coverage predicate (pass 33,
        // addendum #97): score and margin are whole-name measures
        // in which shared suffix tokens dilute the only evidence
        // that distinguishes siblings, so a read witnessing no
        // discriminative token of the winner is not held however
        // the diluted arithmetic lands.
        out.held = score >= min_score
            && (score - second) >= MATCH_MARGIN
            && universe.hold_permitted(&read.text, name);
        if out.held {
            out.count = read.count;
            // Tier only means something on a held relic read; elsewhere
            // it is unknown (never a silent assignment on noise).
            out.tier = RefinementTier::parse_label(&read.text);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Relic owned-gate (pass 38, addendum #103)
// ---------------------------------------------------------------------------
//
// The relic BROWSE screen shows the full catalogue: unowned tiles
// render faded with an eye badge top-left, and their name bands
// read as cleanly as owned ones — under the count contract (no
// label means exactly one) every unowned tile the scroll passes
// enters stock as x1 Intact, and a browse finalize REPLACES the
// relic bucket wholesale. The selection/refinement screens show
// owned tiles only, so the gate is built to be inert there and
// strict only where the evidence is clear:
//
// 1. Catalogue detection is per frame: one tile with a clear eye
//    badge ([`eye_badge_present`]) marks the frame browse-type.
//    Outside catalogue frames nothing changes — bit-identical.
// 2. Inside a catalogue frame, a held tile is excluded only when
//    EVERY leg agrees it is unowned: no parsed count label (a
//    label is positive owned evidence), a clear eye badge on the
//    tile itself, and an icon highlight in the frame's dim
//    cluster (a per-frame Otsu split that must separate cleanly),
//    or — when the frame has no clean split because the unowned
//    tail fills it — below 70% of the session's owned-brightness
//    reference. Any leg missing or ambiguous keeps the tile: a
//    phantom is the known bug; a false exclusion of a real relic
//    would be a worse, new one.

/// The minimum bright/dim separation a per-frame Otsu split must
/// show before it counts as a split at all (bright-cluster minimum
/// over dim-cluster maximum). Seth's strip separates at 2.2x; an
/// owned-only frame's art variance stays near 1.0x and never
/// splits.
const GATE_SPLIT_MIN_RATIO: f64 = 1.35;

/// With no clean per-frame split, a tile counts as dim only below
/// this fraction of the session's owned-brightness reference.
const GATE_REF_DIM_FRACTION: f64 = 0.70;

/// One frame's owned-gate outcome (pass 38).
#[derive(Debug, Clone, Default)]
pub struct RelicGateOutcome {
    /// The frame is a catalogue (browse) frame.
    pub catalogue: bool,
    /// Names of the held tiles the gate excluded, in cell order.
    pub excluded: Vec<String>,
}

/// The Otsu split of one frame's held-tile icon highlights:
/// `Some(threshold)` with dim tiles at or below it, but only when
/// the split separates cleanly ([`GATE_SPLIT_MIN_RATIO`] between
/// the clusters it makes). A unimodal frame gets `None` — Otsu
/// always returns a cut; the ratio is what makes it evidence.
fn otsu_dim_threshold(values: &[f64]) -> Option<f64> {
    if values.len() < 2 {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = sorted.iter().sum();
    let total_sq: f64 = sorted.iter().map(|v| v * v).sum();
    let n = sorted.len() as f64;
    let mean = total / n;
    let total_var = total_sq / n - mean * mean;
    if total_var <= f64::EPSILON {
        return None;
    }
    let mut best: Option<(f64, usize)> = None; // (between-class var, split idx)
    let mut sum0 = 0.0f64;
    for i in 0..sorted.len() - 1 {
        sum0 += sorted[i];
        if sorted[i + 1] <= sorted[i] {
            continue; // a cut separates distinct values only
        }
        let n0 = (i + 1) as f64;
        let n1 = n - n0;
        let mu0 = sum0 / n0;
        let mu1 = (total - sum0) / n1;
        let between = (n0 / n) * (n1 / n) * (mu0 - mu1) * (mu0 - mu1);
        if best.map(|(b, _)| between > b).unwrap_or(true) {
            best = Some((between, i));
        }
    }
    let (_, i) = best?;
    let (dim_max, bright_min) = (sorted[i], sorted[i + 1]);
    if dim_max > 0.0 && bright_min >= GATE_SPLIT_MIN_RATIO * dim_max {
        Some(dim_max)
    } else {
        None
    }
}

/// Apply the owned-gate to one relic frame's validated results,
/// positionally aligned with `cells`. Excluded results get
/// `held = false` and `gate_excluded = true`; everything else is
/// untouched, and non-relic layouts return immediately. The
/// caller owns the session's brightness reference: pass the
/// running owned-brightness maximum of the session's earlier
/// relic frames (see [`relic_frame_bright`]).
pub fn relic_owned_gate(
    cells: &[Option<GrayImage>],
    layout: &Layout,
    results: &mut [CellResult],
    bright_ref: Option<f64>,
) -> RelicGateOutcome {
    let mut outcome = RelicGateOutcome::default();
    if layout.key != "relic" {
        return outcome;
    }
    // Per-cell eye scores over every cut cell (held or not): one
    // clear eye anywhere marks the frame as browse-type.
    let eye: Vec<Option<f32>> = cells
        .iter()
        .map(|c| c.as_ref().map(eye_badge_score))
        .collect();
    outcome.catalogue = eye
        .iter()
        .flatten()
        .any(|&s| s >= EYE_NCC_MIN);
    if !outcome.catalogue {
        return outcome;
    }
    // Icon highlights of the held tiles (the split population: a
    // tile is present iff the reader held a name on it — on this
    // screen names read reliably, which is the bug's premise).
    let highlights: Vec<f64> = results
        .iter()
        .zip(cells.iter())
        .filter(|(r, _)| r.held)
        .map(|(_, c)| c.as_ref().map(icon_highlight).unwrap_or(0.0))
        .collect();
    let split = otsu_dim_threshold(&highlights);
    // The reference for the no-split case: the session's earlier
    // frames, extended by this frame's own brightest tile. At a
    // cold start on an all-unowned frame the reference is a dim
    // value and the 70% cut sits below every tile — fail-safe.
    let frame_max = highlights.iter().copied().fold(0.0f64, f64::max);
    let eff_ref = bright_ref.unwrap_or(0.0).max(frame_max);
    for (i, result) in results.iter_mut().enumerate() {
        if !result.held || result.count.is_some() {
            continue; // not held anyway, or a label proves ownership
        }
        let Some(cell) = cells.get(i).and_then(|c| c.as_ref()) else {
            continue;
        };
        if eye.get(i).copied().flatten().unwrap_or(0.0) < EYE_NCC_MIN {
            continue; // no clear eye on THIS tile: ambiguous, stays
        }
        let hl = icon_highlight(cell);
        let dim = match split {
            Some(thr) => hl <= thr,
            None => eff_ref > 0.0 && hl < GATE_REF_DIM_FRACTION * eff_ref,
        };
        if !dim {
            continue;
        }
        result.held = false;
        result.gate_excluded = true;
        if let Some(name) = &result.matched {
            outcome.excluded.push(name.clone());
        }
    }
    outcome
}

// ---------------------------------------------------------------------------
// Relic name admission (pass 46, addendum #111)
// ---------------------------------------------------------------------------
//
// Seth's ruling (2026-10-27): importing a relic under a name
// the read does not prove is actively false information in
// stock.json — strictly worse than missing the relic. A miss
// costs a re-sync; a false import fabricates ownership.
//
// The defect: the owned Meso C11 [Radiant] tile read as
// "Mesoll Relic [Radiant]" in both of its settled corpus
// appearances, and the resolver held "Meso L1" — a REAL,
// different relic, so universe membership cannot catch it.
// It is a sibling false-accept, and the resolver's own bars
// do not refuse it: the whole-string score is 0.857 (floor
// 0.8), the margin over the runner-up (Meso A11, 0.750) is
// 0.107 (guard 0.10), and the shared hold predicate passes
// because its merged-witness clause lets the one fused token
// "mesoll" testify for the head token and the code token at
// once. Score-space cannot fix this: corpus-wide, held
// relic reads run margins of 0.10-0.20 (median 0.143 —
// siblings sit one edit apart), and a clean "Meso C11" read
// itself only margins 0.125 over Meso A11. The garble's
// margin sits INSIDE the legitimate distribution.
//
// What separates it is evidence shape, the parts
// discipline's real core: every token of the name must be
// witnessed, and one blob of glyphs cannot testify twice.
// A relic name's tokens are each discriminative against a
// different sibling set (the era against the other eras'
// same-code names, the code against the same era's other
// codes — 205 of 265 codes exist in more than one era), so
// each needs its OWN read token. The fused read is exactly
// the shape where a destroyed discriminative glyph hides:
// "C11" burned down to "ll" and fused with the era, and no
// string metric can tell the ashes from a genuine fused
// "Meso L1" (the two are zero edits apart under the
// confusable metric closure). The fused shape is therefore
// refused as a shape, at any score.
//
// Calibration (the 25-frame corpus replay, 1,235 held
// reads): every legitimate held read assigns its winner's
// tokens to distinct read tokens, in order, with per-token
// confusable-Levenshtein distance at most 1 — worst total
// assignment cost 1, across clean reads, one-glyph garbles
// ("Ayi V2" for Axi V2), split glyphs ("| ith" for Lith) and
// junk-prefixed reads alike. The ONLY assignment-impossible
// held reads in the corpus are the two C11 garble
// appearances. The bar below is set at exactly that
// observation: per-token distance <=
// [`RELIC_TOKEN_WITNESS_MAX`], distinct tokens, in order.
//
// Known limit, stated plainly: a garble that preserves the
// token boundary ("Meso L1" read cleanly off a C11 tile)
// is indistinguishable from a true L1 read by ANY string
// evidence, and this gate does not pretend otherwise. The
// corpus contains no such case; the observed defect class
// is the fused read, and the fused read is what dies here.

/// Per-token confusable-Levenshtein budget for the relic
/// name witness (pass 46): a candidate token counts as
/// witnessed by a read token at most this many edits away.
/// Corpus calibration: every legitimate held read witnesses
/// every token at distance <= 1 (most at 0); the budget is
/// the observation, not a round number near it.
pub const RELIC_TOKEN_WITNESS_MAX: usize = 1;

/// Does the read witness EVERY token of `candidate`, each
/// by a DISTINCT read token, in order, within
/// [`RELIC_TOKEN_WITNESS_MAX`] confusable edits? The read
/// is cleaned exactly as the resolver cleans it (tier label
/// and the trailing "Relic" dropped) before tokenising, so
/// the witness judges the same evidence the resolver scored.
/// Extra read tokens (icon bleed, junk) testify for nothing
/// and cost nothing; a read with fewer tokens than the
/// candidate can never witness it.
pub fn relic_name_witnessed(read: &str, candidate: &str) -> bool {
    let cleaned = strip_tier_label(read);
    let cleaned = strip_relic_suffix(&cleaned);
    let read_toks: Vec<String> = fold(&cleaned)
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    let cand_folded = fold(candidate);
    let cand_toks: Vec<&str> = cand_folded.split(' ').filter(|t| !t.is_empty()).collect();
    if cand_toks.is_empty() || read_toks.len() < cand_toks.len() {
        return false;
    }
    // Ordered distinct assignment: candidate token k takes a
    // read token strictly after the one token k-1 took.
    fn assign(read: &[String], cand: &[&str], start: usize) -> bool {
        if cand.is_empty() {
            return true;
        }
        if read.len() - start < cand.len() {
            return false;
        }
        for i in start..=(read.len() - cand.len()) {
            if levenshtein_confusable(&read[i], cand[0]) <= RELIC_TOKEN_WITNESS_MAX
                && assign(read, &cand[1..], i + 1)
            {
                return true;
            }
        }
        false
    }
    assign(&read_toks, &cand_toks, 0)
}

/// The relic name-admission gate (pass 46): un-hold every
/// held result whose resolved name the read does not witness
/// token-by-token ([`relic_name_witnessed`]), and record the
/// refusal on the result ([`CellResult::name_refused`]) for
/// the finalize log. Runs FIRST among the relic post-read
/// passes — before the owned-gate and the refinement
/// measure — so a refused name feeds no other evidence: it
/// is not in stock, not in the disposition, not anywhere.
/// Applied on every path that writes relic stock (live
/// capture via `capture::read_cells`, folder ingest via
/// [`ingest_folder`]), with the same universe the resolver
/// itself scored against, so the discipline cannot drift
/// between paths. Parts and arcane reads never reach it
/// (both call sites key on the relic layout, exactly as the
/// owned-gate does).
pub fn relic_name_admission(results: &mut [CellResult], universe: &Universe) {
    for result in results.iter_mut() {
        if !result.held {
            continue;
        }
        let Some(winner) = result.matched.clone() else {
            continue;
        };
        if relic_name_witnessed(&result.raw_text, &winner) {
            continue;
        }
        let (runner_up, runner_up_score) = universe
            .top_matches(&result.raw_text, 3)
            .into_iter()
            .find(|(n, _)| *n != winner.as_str())
            .map(|(n, s)| (Some(n.to_owned()), s))
            .unwrap_or((None, 0.0));
        result.held = false;
        result.name_refused = Some(NameRefusal {
            raw: result.raw_text.clone(),
            winner,
            winner_score: result.match_score,
            runner_up,
            runner_up_score,
        });
    }
}

// ---------------------------------------------------------------------------
// Relic refinement-variant disposition (pass 43, addendum
// #108; pass 45, addendum #110)
// ---------------------------------------------------------------------------
//
// Two screens share ScreenClass::Relics. The SELECTION
// variant (fissure-flow picker) shows owned tiles only —
// unowned relics never render there. The REFINEMENT variant
// (orbiter segment; the header names it, "VOID RELICS /
// REFINEMENT") shows the full catalogue, and its tiles have
// exactly three states (Seth, 2026-10-27): a count label =
// owned xN (N > 1); no glyph = owned x1; the eye glyph =
// unowned. One screen, one model — the pass-38 strip was a
// crop of this same screen.
//
// Pass 43, working from the strip-calibrated template's
// failure on native frames (production scorer peaks at
// 0.328 over 1,925 cells), concluded the variant carried no
// badge and refused every label-less tile — safe, but it
// dropped every owned x1 stack on every refinement sync.
// The 2026-10-27 discriminator research (REFINEMENT_X1_
// DISCRIMINATOR) found the eye IS present on native frames
// and the template fails on REGISTRATION, not signal: its
// blob sits ~6x8 px off the native position and the scorer
// tolerates no translation. A one-number statistic of the
// same window — bright-pixel mass, [`eye_window_bright_mass`]
// — separates the corpus perfectly (unowned 69-71, owned at
// most 20 over 527 settled label-less appearances). The
// research also corrected the pass-43 phantom reckoning:
// only the 11 Axi were genuinely unowned; the 26 Meso/Neo
// "phantoms" were Seth's owned x1s the earlier sync's
// scroll never reached; and "Meso L1" was no tile at all —
// a name misread of the owned Meso C11 [Radiant] tile.
//
// Pass 45 therefore dispositions refinement stacks by the
// three-state model, at SESSION level ([`RefinementEvidence`]):
// a stack with a parsed count label on any settled
// appearance imports at that count; a label-less stack
// whose eye-window bright mass reaches
// [`EYE_BRIGHT_MASS_MIN`] on any settled appearance is
// unowned and excluded; any other label-less stack imports
// at x1. Per-frame code only MEASURES ([`relic_refinement_measure`]);
// the session aggregates, because a transient frame smears
// the glyph (one corpus transient reads mass 16 on an
// unowned tile) and the maximum over settled appearances is
// what makes the test robust — the same repeated-sighting
// discipline the parts pipeline already uses.
//
// One veto on the measurement: a tile whose count region
// carries label-scale bright mass is physically labelled
// even when its label fails to parse (the corpus's Lith M4
// x15, at the count region's edge — its strokes share the
// eye window, 52 bright px, and would convict a stack
// owned 15-deep). Such tiles are never measured
// ([`LABEL_PRESENT_MASS_MIN`]): a glyph IS present in
// their corner, so the three-state model owns them.
//
// The header's "ALL" is the era filter, not an owned
// filter, and the "OWNED ▾" control is a sort dropdown —
// the catalogue renders in full under both in all 25
// corpus frames. There is no filter state to condition on;
// the three-state model needs none.

/// Which relic screen variant a frame came from (pass 43).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelicScreenVariant {
    /// Not determined (folder ingest, replay seams). The rule
    /// infers from the reads: see [`relic_variant_from_reads`].
    Unknown,
    /// The fissure-flow picker. Pass 38's gate applies; this
    /// rule is inert.
    Selection,
    /// The orbiter refinement screen. Three-state disposition
    /// (see [`RefinementEvidence`]).
    Refinement,
}

/// The variant a frame's own reads prove, for callers with
/// no header signal: a held read carrying a refinement tier
/// (a `[Radiant]`-style second line) comes from the
/// refinement screen's tier-split stacks; the selection
/// strip's tiles carry no second lines (pass 38's fixture,
/// re-measured pass 43: every strip read parses Intact).
/// Anything else proves nothing and stays `Unknown` — the
/// failure mode is a refinement page whose visible stacks
/// are all Intact, which infers Unknown and is not measured
/// (its label-less tiles then import at x1 unmeasured; the
/// folder-ingest seam only — the live session path never
/// relies on this, it reads the variant off the header).
pub fn relic_variant_from_reads(results: &[CellResult]) -> RelicScreenVariant {
    if results.iter().any(|r| {
        r.held
            && matches!(
                r.tier,
                RefinementTier::Exceptional
                    | RefinementTier::Flawless
                    | RefinementTier::Radiant
            )
    }) {
        RelicScreenVariant::Refinement
    } else {
        RelicScreenVariant::Unknown
    }
}

/// Measure one refinement frame's label-less tiles (pass
/// 45): every held result without a parsed count label gets
/// its eye-window bright mass recorded in
/// `refine_eye_mass`. Nothing is held or refused here — the
/// disposition is a session-level decision
/// ([`RefinementEvidence`]) because the statistic is only
/// valid per settled appearance and the rule takes the
/// maximum over a stack's appearances. Labelled tiles are
/// not measured (a label is positive owned evidence), unheld
/// results are not measured (they never reach stock), and
/// non-refinement variants are untouched. Runs after the
/// owned-gate, so a tile the gate already refused is not
/// measured either.
///
/// One veto: a tile whose count region carries label-scale
/// bright mass ([`count_region_bright_mass`] >=
/// [`LABEL_PRESENT_MASS_MIN`]) is physically labelled even
/// though the parser read no count — the corpus's Lith M4
/// x15, whose label sits at the count region's edge and
/// parses in neither of its appearances. Its strokes fall
/// inside the eye window (52 bright px) and would convict a
/// stack Seth owns 15 of. Such a tile is not measured: it is
/// owned by the three-state model (a glyph IS present in
/// its corner), and it imports at the vote-resolved count
/// like any tile the disposition has no evidence against.
pub fn relic_refinement_measure(
    cells: &[Option<GrayImage>],
    layout: &Layout,
    results: &mut [CellResult],
    variant: RelicScreenVariant,
) {
    if layout.key != "relic" {
        return;
    }
    let variant = match variant {
        RelicScreenVariant::Unknown => relic_variant_from_reads(results),
        v => v,
    };
    if variant != RelicScreenVariant::Refinement {
        return;
    }
    for (i, result) in results.iter_mut().enumerate() {
        if !result.held || result.count.is_some() {
            continue;
        }
        let Some(cell) = cells.get(i).and_then(|c| c.as_ref()) else {
            continue;
        };
        if count_region_bright_mass(cell, layout) >= LABEL_PRESENT_MASS_MIN {
            continue; // physically labelled: owned, never the eye
        }
        result.refine_eye_mass = Some(eye_window_bright_mass(cell));
    }
}

/// Session-level evidence for the refinement three-state
/// disposition (pass 45, addendum #110): per relic stack,
/// whether any settled appearance carried a parsed count
/// label, and the maximum eye-window bright mass over its
/// label-less appearances. The disposition itself:
///
/// - labelled on any appearance -> owned, at the labelled
///   count (the count votes decide which);
/// - never labelled, max bright mass >=
///   [`EYE_BRIGHT_MASS_MIN`] -> unowned: the eye glyph was
///   seen; the stack is excluded from stock;
/// - never labelled, no eye seen -> owned x1.
///
/// Aggregation is by maximum, not per-frame vote: a single
/// settled sighting of the glyph is deterministic evidence,
/// while a single smeared transient (bright mass 16 on an
/// unowned tile in the corpus) must not outvote it — the
/// maximum is the aggregation that respects both facts.
#[derive(Debug, Clone, Default)]
pub struct RefinementEvidence {
    labelled: HashSet<(String, RefinementTier)>,
    eye_max: HashMap<(String, RefinementTier), u32>,
}

impl RefinementEvidence {
    /// Fold one frame's validated results into the evidence.
    /// Only held reads of known relic names count; unheld
    /// reads and non-relic names carry no disposition
    /// evidence.
    pub fn observe(&mut self, results: &[CellResult], relic_names: &HashSet<String>) {
        for r in results {
            if !r.held {
                continue;
            }
            let Some(name) = &r.matched else { continue };
            if !relic_names.contains(name) {
                continue;
            }
            let key = (name.clone(), r.tier);
            if r.count.is_some() {
                self.labelled.insert(key.clone());
            }
            if let Some(mass) = r.refine_eye_mass {
                let e = self.eye_max.entry(key).or_insert(0);
                *e = (*e).max(mass);
            }
        }
    }

    /// The session maximum eye-window bright mass recorded
    /// for one stack, when any label-less appearance was
    /// measured (diagnostics and tests).
    pub fn eye_max(&self, name: &str, tier: RefinementTier) -> Option<u32> {
        self.eye_max.get(&(name.to_owned(), tier)).copied()
    }

    /// Is this stack unowned under the disposition: never
    /// labelled, and the eye glyph seen on at least one
    /// settled appearance.
    pub fn is_unowned(&self, name: &str, tier: RefinementTier) -> bool {
        let key = (name.to_owned(), tier);
        !self.labelled.contains(&key)
            && self
                .eye_max
                .get(&key)
                .map(|&m| m >= EYE_BRIGHT_MASS_MIN)
                .unwrap_or(false)
    }

    /// Every stack the disposition excludes, sorted by name
    /// then tier — the finalize line's record of what a
    /// refinement sync refused to import as unowned.
    pub fn excluded_stacks(&self) -> Vec<(String, RefinementTier)> {
        let mut out: Vec<(String, RefinementTier)> = self
            .eye_max
            .keys()
            .filter(|k| self.is_unowned(&k.0, k.1))
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Drop every excluded stack from a stock built by
    /// union (the folder-ingest path, whose stock is a
    /// `from_results` union rather than session votes).
    pub fn apply_to(&self, stock: &mut StockState) {
        for (name, tier) in self.excluded_stacks() {
            let became_empty = match stock.relics.get_mut(&name) {
                Some(stacks) => {
                    stacks.tiers.remove(&tier);
                    stacks.tiers.is_empty()
                }
                None => false,
            };
            if became_empty {
                stock.relics.remove(&name);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Radius (bounded-distance) decoder (pass 29, addendum #92)
// ---------------------------------------------------------------------------
//
// The margin matcher measures a read against the WHOLE name, but on
// shared-suffix part names ("... Prime Neuroptics Blueprint") ~23 of
// ~31 characters are common to every sibling, so one or two glyph
// errors in the short leading HEAD (the gear/weapon word — the only
// discriminative segment) collapse the margin and the never-guess
// guard refuses reads a human would call certain (census: 20 of 37
// winner-config Neuroptics misses were margin refusals with
// best = truth). This decoder is the bounded-distance alternative,
// applied ONLY to reads the margin matcher refused:
//
// 1. The refused read's best candidate names a SUFFIX CLASS: every
//    universe name sharing the candidate's longest trailing token
//    run that at least one other name also ends with. The class is
//    a hypothesis, verified in step 2 — and it is only considered
//    at all when the candidate itself scored at or above
//    [`RADIUS_MIN_CANDIDATE_SCORE`], the matcher's own plausibility
//    floor: the decoder builds on the nomination only when the
//    matcher considers its best guess a plausible name.
// 2. The read's tail must fuzzy-align to that suffix: EVERY suffix
//    token must be witnessed by a distinct read token within
//    [`RADIUS_TOKEN_WITNESS`] (a truncated form counts — "pri"
//    witnesses "prime"), with the total alignment cost within
//    [`RADIUS_TAIL_BUDGET`]. A suffix token with no witness fails
//    the alignment outright, at any budget — the first gate's two
//    false accepts were exactly that shape (mid-scroll composite
//    cells whose tails witnessed only "prime" "verified" a class
//    whose distinguishing token was never seen; the re-gate's fix
//    is this witnessed rule, not a retuned budget). A read whose
//    tail does not actually carry the class suffix never reaches
//    the decoder, which is what stops a fragment from decoding in
//    a WRONG class whose heads happen to collide ("mag" heads
//    exist in several classes). The tokens before the split are
//    the FRAGMENT, squashed to alphanumerics so junk glyphs
//    (`"`, `*`, `”`) cost no edit budget.
// 3. The fragment is decoded against the class HEADS alone. Each
//    head A has radius t_A = floor((D_A - 1)/2), where D_A is its
//    distance to the nearest sibling head. By the triangle
//    inequality the balls of radius t_A around the heads are
//    disjoint, so at most one head can accept, and a false accept
//    requires more than t_truth edits of corruption in the truth's
//    own fragment. Radii derive from the universe at runtime —
//    there is no tuned threshold to drift.
//
//    The acceptance test also carries a MINIMUM-EVIDENCE FLOOR
//    (pass 32, addendum #95): a fragment decodes to head H only
//    when |F| >= max(3, |H| - t_H) — see
//    [`RADIUS_MIN_FRAGMENT_CHARS`]. The radius geometry bounds
//    how FAR a fragment may sit from its head; it says nothing
//    about how little of the head was observed at all. The HC
//    evaluation showed why that matters: the strip phase read
//    2-char artefacts off class-icon edges — identical fragments
//    across cells whose gear words were destroyed — and a 2-char
//    fragment is one deletion from the 3-char head "lex",
//    formally in-radius. The floor is inert on band fragments
//    (long by construction) and binds exactly where the evidence
//    is thinnest: short fragments against short heads. It costs
//    the genuine sub-3-char fragment (a "ma" that really was
//    Mag's) its decode — two observed characters cannot
//    identify a head among dozens of siblings, so the refusal
//    is the honest outcome, paid deliberately like the radius
//    ceiling itself.
//
//    All decoder distances use the METRIC CLOSURE of the confusable
//    relation ([`levenshtein_confusable_metric`]), not the pairwise
//    distance the margin matcher uses: the pairwise relation is
//    not transitive (`1` ~ `i` and `1` ~ `l` but `i` !~ `l`), its
//    distance violates the triangle inequality, and under it the
//    balls above DO overlap — the stress test's witness is
//    "1aros", one pairwise edit from both "inaros" and "lavos"
//    whose pairwise distance is 3 (their metric distance is 2,
//    so both radii are 0 and "1aros" refuses). The closure costs
//    a little coverage wherever a bridge exists; soundness is
//    not negotiable.
//
// The honest limit (pinned in tests): heads at distance 3 give
// t = 1, so a 2-edit corruption does NOT decode — Chroma's nearest
// sibling is Khora (D = 3), and the census's "Bhioma" read stays
// refused. The decoder recovers the ≤1-edit population only; that
// is the price of the proof, and it is paid deliberately.

/// Token-edit cost of skipping one read token (junk) during tail
/// alignment (see [`Universe::radius_fragment`]). Suffix tokens are
/// never skipped — each must be witnessed (see
/// [`RADIUS_TOKEN_WITNESS`]) — so this prices only the junk tokens
/// a bleed-damaged tail carries alongside the real suffix tokens.
const RADIUS_TOKEN_EDIT: usize = 3;

/// Witness cap for tail alignment: a suffix token counts as
/// present only when some distinct read token, in order, sits
/// within this metric-closure distance of it. A truncated token
/// ("pri" for "prime") sits at exactly 2; anything more distant
/// is not evidence the suffix token is there, and the suffix token
/// goes unwitnessed — which fails the alignment at any budget.
const RADIUS_TOKEN_WITNESS: usize = 2;

/// Judgement call (pass 29): the largest total token-alignment cost
/// at which a read's tail still counts as carrying the class suffix,
/// charged as the sum of witnessed-token distances plus
/// [`RADIUS_TOKEN_EDIT`] per skipped junk token. Pre-registered at
/// 4 and KEPT at 4 for the re-gate: the first gate's false accepts
/// came from unwitnessed suffix-token deletions priced at 3, and
/// the fix is the witnessed rule, not a retuned budget (budget 2
/// also scored clean on the first gate, but on the same data that
/// judged it — see docs/twenty-ninth-pass.md). This is the
/// decoder's one chosen constant; the radii themselves are
/// derived, not tuned.
pub const RADIUS_TAIL_BUDGET: usize = 4;

/// Score floor for class nomination: the decoder considers a
/// refused read's best candidate only when it scored at or above
/// the parts layout's validation `min_score`
/// (data/capture/layouts.toml) — the decoder trusts a nomination
/// only from a candidate the matcher itself considers plausible.
/// Second line of defence, not the primary fix: the first gate's
/// "Nova Prime" false accept scored 0.56, above this floor, and
/// is stopped by the witnessed-token rule; its sibling read
/// ("Nezh rime", 0.47) never nominates at all under the floor.
pub const RADIUS_MIN_CANDIDATE_SCORE: f32 = 0.50;

/// Minimum-evidence floor for a decode (pass 32, addendum #95):
/// a fragment decodes to head H only when
/// `|F| >= max(RADIUS_MIN_FRAGMENT_CHARS, |H| - t_H)`. The
/// absolute term is the binding constraint for short heads —
/// distance alone cannot express it, because a 2-char artefact
/// sits one deletion from the 3-char head "lex", formally
/// in-radius, yet two observed characters cannot identify a
/// head among dozens of siblings. The length-relative term is
/// already implied by `decode`'s length-difference check (a
/// fragment shorter than `|H| - t_H` is more than t_H deletions
/// away); it is stated in the rule so the floor reads as one
/// acceptance test, not two accidents.
const RADIUS_MIN_FRAGMENT_CHARS: usize = 3;

/// One suffix class of the universe, with the decoding radius of
/// every member's head precomputed. Built by
/// [`Universe::radius_class`].
#[derive(Debug, Clone)]
pub struct RadiusClass {
    /// Folded suffix tokens shared by every member.
    suffix: Vec<String>,
    /// (canonical name, folded head) per member.
    members: Vec<(String, String)>,
    /// Decoding radius t_A per member (same order as `members`).
    radii: Vec<usize>,
    /// Member heads as char vectors (same order), collected once
    /// so `decode` runs allocation-free per candidate.
    head_chars: Vec<Vec<char>>,
}

impl RadiusClass {
    /// The folded suffix tokens every member shares.
    pub fn suffix(&self) -> &[String] {
        &self.suffix
    }

    /// (canonical name, folded head, radius) per member.
    pub fn members(&self) -> impl Iterator<Item = (&str, &str, usize)> {
        self.members
            .iter()
            .zip(self.radii.iter())
            .map(|((n, h), t)| (n.as_str(), h.as_str(), *t))
    }

    /// Decode a squashed fragment against the class heads: the
    /// unique member whose head lies within its own radius of the
    /// fragment, or `None`. The geometry guarantees at most one
    /// in-radius head; if an implementation defect ever produced
    /// two, this refuses rather than picking one (never guess).
    ///
    /// A candidate is considered only when the fragment also
    /// clears the minimum-evidence floor (pass 32, addendum
    /// #95): `|F| >= max(RADIUS_MIN_FRAGMENT_CHARS, |H| - t_H)`.
    /// This is the one place both fragment sources (band and
    /// strip) are judged, so the floor applies uniformly.
    pub fn decode(&self, fragment: &str) -> Option<String> {
        if fragment.is_empty() {
            return None;
        }
        let frag: Vec<char> = fragment.chars().collect();
        let mut hit: Option<&str> = None;
        for (((name, _), chars), t) in self
            .members
            .iter()
            .zip(self.head_chars.iter())
            .zip(self.radii.iter())
        {
            // Minimum evidence (pass 32, addendum #95): the
            // fragment must carry at least
            // RADIUS_MIN_FRAGMENT_CHARS characters and account
            // for the head to within its radius in length. The
            // HC evaluation's strip phase returned 2-char
            // icon-edge artefacts ("le", "ie") that sit one
            // deletion from the 3-char head "lex" and minted
            // false accepts; a fragment that short is not
            // evidence, whatever its distance.
            if frag.len() < RADIUS_MIN_FRAGMENT_CHARS.max(chars.len().saturating_sub(*t)) {
                continue;
            }
            // Length alone settles most candidates: a fragment
            // whose length differs from the head's by more than
            // the radius cannot be within it.
            if frag.len().abs_diff(chars.len()) > *t {
                continue;
            }
            if lev_chars_bounded(&frag, chars, *t, confusable_metric) <= *t {
                if hit.is_some() {
                    return None;
                }
                hit = Some(name.as_str());
            }
        }
        hit.map(str::to_owned)
    }
}

/// Squash read tokens into a decoder fragment: lowercase
/// alphanumerics only, single spaces between surviving tokens.
/// Junk glyphs the icon bleed injects (`"`, `*`, `”`, `¥`) vanish
/// instead of consuming the fragment's edit budget; digits survive
/// (the confusable metric folds 5/S, 0/O, ... at zero cost).
fn squash_fragment(tokens: &[&str]) -> String {
    tokens
        .iter()
        .map(|t| {
            t.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Witnessed-alignment cost between a read's tail and a class
/// suffix. Every suffix token must be WITNESSED: matched, in
/// order, to a distinct read token whose metric-closure distance
/// ([`levenshtein_confusable_metric`]) is at most
/// [`RADIUS_TOKEN_WITNESS`], costing that distance. Read tokens
/// may be skipped as junk at [`RADIUS_TOKEN_EDIT`] each (before,
/// between, or after the witnesses). A suffix token with no
/// witness fails the alignment outright — the cost comes back
/// effectively infinite, so the caller's budget comparison refuses
/// at any budget. This is the structural fix from the pass-29
/// re-gate: the first gate's alignment charged a flat
/// [`RADIUS_TOKEN_EDIT`] for a wholly deleted suffix token, so a
/// composite cell whose tail witnessed only "prime" could
/// "verify" the chassis class (delete "chassis" for 3, substitute
/// "rime"→"prime" for 1) and decode. A substitution more distant
/// than the witness cap is likewise forbidden: a token that
/// distant is not evidence the suffix token is present.
fn token_align_cost(tail: &[&str], suffix: &[String]) -> usize {
    const INF: usize = usize::MAX / 4;
    let n = suffix.len();
    // prev[j]: cost of aligning the read tokens consumed so far
    // against suffix[..j] with every one of those suffix tokens
    // witnessed. Prefixes with an unwitnessed suffix token start
    // at INF and can never become finite.
    let mut prev: Vec<usize> = vec![INF; n + 1];
    prev[0] = 0;
    let mut cur = vec![INF; n + 1];
    for tok in tail {
        // Skipping this read token (junk) preserves every
        // witnessed prefix at insertion cost.
        cur[0] = prev[0].saturating_add(RADIUS_TOKEN_EDIT);
        for (j, want) in suffix.iter().enumerate() {
            let skip = prev[j + 1].saturating_add(RADIUS_TOKEN_EDIT);
            let d = levenshtein_confusable_metric_bounded(tok, want, RADIUS_TOKEN_WITNESS);
            let witness = if d <= RADIUS_TOKEN_WITNESS {
                prev[j].saturating_add(d)
            } else {
                INF
            };
            cur[j + 1] = skip.min(witness);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[n]
}

/// The strip path's pooled tail verification (see
/// [`Universe::radius_decode_strip`]): distinct-witness coverage
/// of the class suffix over the union of the strip and band
/// reads' tokens, with the suffix's final token witnessed by the
/// band read itself and the summed witness distance within
/// `budget`.
fn strip_tail_witnessed(
    band_read: &str,
    strip_read: &str,
    class: &RadiusClass,
    budget: usize,
) -> bool {
    let toks = |read: &str| -> Vec<String> {
        let cleaned = strip_tier_label(read);
        let cleaned = strip_relic_suffix(&cleaned);
        fold(&cleaned)
            .split(' ')
            .filter(|t| !t.is_empty())
            .map(|t| t.to_owned())
            .collect()
    };
    let band = toks(band_read);
    let strip = toks(strip_read);
    let suffix = class.suffix();
    if band.is_empty() || suffix.is_empty() {
        return false;
    }
    // The band anchors the tail: the suffix's final token must
    // be witnessed by a band token.
    let last = &suffix[suffix.len() - 1];
    if !band
        .iter()
        .any(|t| levenshtein_confusable_metric_bounded(t, last, RADIUS_TOKEN_WITNESS) <= RADIUS_TOKEN_WITNESS)
    {
        return false;
    }
    // Distinct witnesses over the pooled tokens (strip first —
    // it is the earlier line), summed distance within budget.
    let pooled: Vec<&String> = strip.iter().chain(band.iter()).collect();
    let mut used = vec![false; pooled.len()];
    let mut total = 0usize;
    for want in suffix {
        let mut best: Option<(usize, usize)> = None; // (distance, pool idx)
        for (i, tok) in pooled.iter().enumerate() {
            if used[i] {
                continue;
            }
            let d = levenshtein_confusable_metric_bounded(tok, want, RADIUS_TOKEN_WITNESS);
            if d <= RADIUS_TOKEN_WITNESS && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, i));
            }
        }
        let Some((d, i)) = best else { return false };
        used[i] = true;
        total += d;
        if total > budget {
            return false;
        }
    }
    true
}

/// The fragment of a line-1 strip read (band-relative top strip,
/// PSM 7): everything before the first token resembling "prime"
/// (the head is the only content above the shared suffix on line 1;
/// the strip may also carry bleed junk after it). Falls back to the
/// whole squashed read when no token resembles "prime" — the
/// decoder's radius then decides, and an inflated fragment simply
/// refuses.
pub fn radius_strip_fragment(read: &str) -> String {
    let cleaned = strip_tier_label(read);
    let folded = fold(&cleaned);
    let tokens: Vec<&str> = folded.split(' ').filter(|t| !t.is_empty()).collect();
    let cut = tokens
        .iter()
        .position(|t| levenshtein_confusable(t, "prime") <= 2 && t.len() >= 3)
        .filter(|&i| i > 0)
        .unwrap_or(tokens.len());
    squash_fragment(&tokens[..cut])
}

impl Universe {
    /// The suffix class of a canonical universe name: the longest
    /// trailing token run of the name that at least one OTHER
    /// universe name also ends with (leaving every member at least
    /// a one-token head). `None` when no such run exists — a name
    /// whose every suffix is unique has no siblings to confuse it
    /// with, and the margin matcher already owns its reads.
    pub fn radius_class(&self, candidate: &str) -> Option<RadiusClass> {
        let (suffix, members) = self.derive_class_parts(candidate)?;
        let key = suffix.join(" ");
        if let Ok(cache) = self.radius_cache.lock() {
            if let Some(hit) = cache.get(&key) {
                return Some(hit.clone());
            }
        }
        // D_A: distance from each head to the nearest OTHER member's
        // head (a duplicate head — impossible for distinct names in
        // one class — yields D = 0, radius 0, and `decode`'s two-hit
        // refusal covers the collision). The pairwise pass is the
        // expensive part of a class build, so it runs once per
        // suffix, not once per candidate (every member of a class
        // derives the same suffix).
        let head_chars: Vec<Vec<char>> =
            members.iter().map(|(_, h)| h.chars().collect()).collect();
        let radii = head_chars
            .iter()
            .enumerate()
            .map(|(i, head)| {
                let d = head_chars
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, other)| lev_chars(head, other, confusable_metric))
                    .min()
                    .unwrap_or(0);
                d.saturating_sub(1) / 2
            })
            .collect();
        let built = RadiusClass {
            suffix,
            members,
            radii,
            head_chars,
        };
        if let Ok(mut cache) = self.radius_cache.lock() {
            cache.insert(key, built.clone());
        }
        Some(built)
    }

    /// The cheap half of class derivation: the candidate's suffix
    /// and the class membership, with no radii. Returns `None` when
    /// no trailing token run is shared with another name.
    fn derive_class_parts(
        &self,
        candidate: &str,
    ) -> Option<(Vec<String>, Vec<(String, String)>)> {
        let folded = fold(candidate);
        let ctoks: Vec<&str> = folded.split(' ').filter(|t| !t.is_empty()).collect();
        if ctoks.len() < 2 {
            return None;
        }
        // Tokenised universe names, computed once per call (the
        // caller pays this only for refused reads).
        let tokenised: Vec<Vec<&str>> = self
            .folded
            .iter()
            .map(|f| f.split(' ').collect())
            .collect();
        for k in (1..ctoks.len()).rev() {
            let suffix: Vec<String> =
                ctoks[ctoks.len() - k..].iter().map(|s| s.to_string()).collect();
            let mut members: Vec<(String, String)> = Vec::new();
            for (name, toks) in self.names.iter().zip(tokenised.iter()) {
                if toks.len() > k && toks[toks.len() - k..] == suffix[..] {
                    let head = toks[..toks.len() - k].join(" ");
                    if !members.iter().any(|(n, _)| n == name) {
                        members.push((name.clone(), head));
                    }
                }
            }
            if members.len() >= 2 {
                return Some((suffix, members));
            }
        }
        None
    }

    /// Verify a read's tail against a class suffix and return its
    /// fragment: the split of the (cleaned, folded) read minimising
    /// [`token_align_cost`] against the suffix — with every suffix
    /// token witnessed — must land within `budget`, and the
    /// squashed fragment before the split must be non-empty. This
    /// is the class-evidence check — a read whose tail does not
    /// carry the suffix yields `None` even when its fragment would
    /// decode somewhere.
    pub fn radius_fragment(
        &self,
        read: &str,
        class: &RadiusClass,
        budget: usize,
    ) -> Option<String> {
        let cleaned = strip_tier_label(read);
        let cleaned = strip_relic_suffix(&cleaned);
        let folded = fold(&cleaned);
        let tokens: Vec<&str> = folded.split(' ').filter(|t| !t.is_empty()).collect();
        if tokens.is_empty() {
            return None;
        }
        let mut best: Option<(usize, usize)> = None; // (cost, split)
        for m in 0..=tokens.len() {
            let cost = token_align_cost(&tokens[m..], &class.suffix);
            if best.map_or(true, |(c, _)| cost < c) {
                best = Some((cost, m));
            }
        }
        let (cost, m) = best?;
        if cost > budget {
            return None;
        }
        let fragment = squash_fragment(&tokens[..m]);
        if fragment.is_empty() {
            None
        } else {
            Some(fragment)
        }
    }

    /// Full band-read decode at an explicit tail budget: class from
    /// the read's best margin candidate (trusted only at or above
    /// [`RADIUS_MIN_CANDIDATE_SCORE`]), tail verified against the
    /// class suffix, fragment decoded within the class. Returns the
    /// canonical name on a decode, `None` on any refusal.
    pub fn radius_decode_budget(&self, read: &str, budget: usize) -> Option<String> {
        let (name, score, _) = self.best_match_margin(read)?;
        if score < RADIUS_MIN_CANDIDATE_SCORE {
            return None;
        }
        let class = self.radius_class(name)?;
        let fragment = self.radius_fragment(read, &class, budget)?;
        // The shared hold predicate (pass 33): the decode is a
        // hold like any other, so the read must witness a
        // discriminative token of the name it lands on. (The
        // fragment usually witnesses it by construction; the check
        // is the uniformity guarantee — no path holds on suffix
        // evidence alone.)
        class
            .decode(&fragment)
            .filter(|decoded| self.hold_permitted(read, decoded))
    }

    /// [`Self::radius_decode_budget`] at the production budget.
    pub fn radius_decode(&self, read: &str) -> Option<String> {
        self.radius_decode_budget(read, RADIUS_TAIL_BUDGET)
    }

    /// Second-chance decode from a line-1 strip read: the class and
    /// its tail verification come from the BAND read (the strip
    /// carries no suffix evidence), the fragment from the strip.
    /// Returns `None` when the band read cannot establish the class
    /// or its tail does not verify — a strip fragment alone never
    /// picks a class.
    pub fn radius_decode_strip(&self, band_read: &str, strip_read: &str, budget: usize) -> Option<String> {
        let (name, score, _) = self.best_match_margin(band_read)?;
        if score < RADIUS_MIN_CANDIDATE_SCORE {
            return None;
        }
        let class = self.radius_class(name)?;
        // Tail verification is the class evidence; the band's own
        // fragment may be too damaged to decode (that is why the
        // strip exists), but the suffix must check out. The check
        // pools BOTH reads (pass 33 gate finding): the suffix's
        // first token usually sits on line 1 ("Gear Prime"), which
        // is the strip's whole content — a band read that truncated
        // it to "Pr"/"P" failed the band-only alignment on junk
        // skip costs even with a clean strip beside it. Pooling is
        // witness-set based, not sequence-based: every suffix
        // token needs a distinct witness token in the union of the
        // two reads, the band alone must witness the suffix's
        // FINAL token (the strip physically cannot contain the
        // name's last line, so the tail stays anchored in the
        // band), and the summed witness distance stays within
        // budget — the same total the band path allows.
        if !strip_tail_witnessed(band_read, strip_read, &class, budget) {
            return None;
        }
        let fragment = radius_strip_fragment(strip_read);
        // The shared hold predicate (pass 33), evaluated on this
        // path's combined evidence: the strip read is the head
        // evidence ("Lex Prime" witnesses "lex" — the band read
        // that nominated the class may carry no head evidence at
        // all, by design), the band read is the suffix evidence
        // (already tail-verified above).
        class.decode(&fragment).filter(|decoded| {
            let mut toks = cleaned_read_tokens(strip_read);
            toks.extend(cleaned_read_tokens(band_read));
            self.hold_permitted_tokens(&toks, decoded)
        })
    }
}

/// Per-relic refinement breakdown: one count per tier. Totals are sums.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RelicStacks {
    #[serde(default)]
    pub tiers: BTreeMap<RefinementTier, u32>,
}

impl RelicStacks {
    pub fn get(&self, tier: RefinementTier) -> u32 {
        self.tiers.get(&tier).copied().unwrap_or(0)
    }

    pub fn total(&self) -> u32 {
        self.tiers.values().sum()
    }
}

/// Per-arcane rank breakdown: one count per rank (0..=5).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArcaneStacks {
    #[serde(default)]
    pub ranks: BTreeMap<u8, u32>,
}

impl ArcaneStacks {
    pub fn get(&self, rank: u8) -> u32 {
        self.ranks.get(&rank).copied().unwrap_or(0)
    }

    pub fn total(&self) -> u32 {
        self.ranks.values().sum()
    }
}

/// Stock state file (eighth pass §1): relics keyed (relic, tier), parts by
/// name, arcanes keyed (arcane, rank). A stack identity is unique on
/// screen, so merging is a UNION: the same stack seen in N frames counts
/// ONCE (the max observed count), never a sum across frames. This is what
/// fixes both the name-only collisions and the overlapping-scroll double
/// count of the v0.2.0 ingest.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StockState {
    #[serde(default)]
    pub relics: BTreeMap<String, RelicStacks>,
    #[serde(default)]
    pub parts: BTreeMap<String, u32>,
    /// Arcane stacks from the Arcanes inventory screen (sixth pass §5,
    /// addendum #34). Neither the profile endpoint nor EE.log carries
    /// arcane state, so this screen is the steady-state source.
    #[serde(default)]
    pub arcanes: BTreeMap<String, ArcaneStacks>,
    /// Seth-asserted part holdings the capture has NOT verified
    /// (thirtieth pass, addendum #93): toggled from the Hunt tab,
    /// keyed by ROSTER part name, count stored as 1. The planner
    /// treats membership as a hold (progress-identical to a verified
    /// hold) but every surface renders it ⍻ (U+237B), never ✓.
    /// Capture finalize preserves this section verbatim except that
    /// entries the fresh capture verifies are pruned (see
    /// `CaptureSession::finalize_into`). Files written before this
    /// section existed load it as empty.
    #[serde(default)]
    pub unverified: BTreeMap<String, u32>,
}

impl StockState {
    /// Aggregate validated results into stock. Names in `relic_names` are
    /// relic stacks, names in `arcane_names` are arcane stacks, the rest
    /// loose parts. Merging is a union over stack identity (max count).
    pub fn from_results(
        results: &[CellResult],
        relic_names: &HashSet<String>,
        arcane_names: &HashSet<String>,
    ) -> Self {
        let mut stock = Self::default();
        for r in results {
            if !r.held {
                continue;
            }
            let Some(name) = &r.matched else { continue };
            let add = r.count.unwrap_or(1);
            if relic_names.contains(name) {
                stock
                    .relics
                    .entry(name.clone())
                    .or_default()
                    .merge(r.tier, add);
            } else if arcane_names.contains(name) {
                stock
                    .arcanes
                    .entry(name.clone())
                    .or_default()
                    .merge(r.rank.unwrap_or(0), add);
            } else {
                let e = stock.parts.entry(name.clone()).or_insert(0);
                *e = (*e).max(add);
            }
        }
        stock
    }

    /// Merge another capture into this one — a union over stack identity
    /// (eighth pass §2). Used by the Capture Inventory finalize step.
    pub fn merge_from(&mut self, other: &StockState) {
        for (name, stacks) in &other.relics {
            let dst = self.relics.entry(name.clone()).or_default();
            for (tier, n) in &stacks.tiers {
                let e = dst.tiers.entry(*tier).or_insert(0);
                *e = (*e).max(*n);
            }
        }
        for (name, stacks) in &other.arcanes {
            let dst = self.arcanes.entry(name.clone()).or_default();
            for (rank, n) in &stacks.ranks {
                let e = dst.ranks.entry(*rank).or_insert(0);
                *e = (*e).max(*n);
            }
        }
        for (name, n) in &other.parts {
            let e = self.parts.entry(name.clone()).or_insert(0);
            *e = (*e).max(*n);
        }
    }

    /// Per-relic totals (planner shape).
    pub fn relic_totals(&self) -> HashMap<String, u32> {
        self.relics
            .iter()
            .map(|(k, v)| (k.clone(), v.total()))
            .collect()
    }

    /// Per-arcane totals.
    pub fn arcane_totals(&self) -> HashMap<String, u32> {
        self.arcanes
            .iter()
            .map(|(k, v)| (k.clone(), v.total()))
            .collect()
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

    /// Planner shape: totals become known stock and the Radiant split
    /// drives crack-first advice (eighth pass §1).
    pub fn to_relic_stock(&self) -> crate::model::RelicStock {
        let totals = self.relic_totals();
        let radiant = self
            .relics
            .iter()
            .filter(|(_, s)| s.get(RefinementTier::Radiant) > 0)
            .map(|(k, s)| (k.clone(), s.get(RefinementTier::Radiant)))
            .collect();
        crate::model::RelicStock::Known(crate::model::KnownStock { totals, radiant })
    }

    /// Addendum #84 + #89: canonical per-roster-part counts of the
    /// captured part stock, for the planner. A roster part's count is
    /// its raw captured count PLUS the captured count of its
    /// built-component form (the part name minus the trailing
    /// " Blueprint"), because a built component and its blueprint
    /// both count toward the parent item. Stock names that are
    /// neither roster parts nor built forms of them drop out — the
    /// plan only reasons about roster parts.
    pub fn canonical_part_counts(
        &self,
        roster: &crate::model::Roster,
    ) -> HashMap<String, u32> {
        let mut canonical_of: HashMap<String, String> = HashMap::new();
        for item in &roster.items {
            for part in &item.parts {
                canonical_of.insert(part.name.clone(), part.name.clone());
                if let Some(built) =
                    crate::model::built_component_form(&item.name, &part.name)
                {
                    canonical_of.insert(built, part.name.clone());
                }
            }
        }
        let mut out: HashMap<String, u32> = HashMap::new();
        for (name, count) in &self.parts {
            if let Some(canon) = canonical_of.get(name) {
                *out.entry(canon.clone()).or_insert(0) += count;
            }
        }
        out
    }

    pub fn held_parts(&self) -> HashMap<String, u32> {
        self.parts.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    /// Unverified (Seth-asserted, ⍻) part holdings in planner shape:
    /// roster part name -> count (stored as 1). Keys are already
    /// canonical roster part names (the toggle endpoint validates
    /// against the roster), so no canonicalisation applies here —
    /// unlike `parts`, whose raw screen names need it (#84/#89).
    pub fn unverified_part_counts(&self) -> HashMap<String, u32> {
        self.unverified
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Toggle one part's unverified membership (thirtieth pass):
    /// absent -> asserted held (count 1), present -> removed.
    /// Returns the membership state AFTER the toggle. Name
    /// validation against the roster is the caller's job (the
    /// toggle endpoint); this is the pure state transition.
    pub fn toggle_unverified(&mut self, name: &str) -> bool {
        if self.unverified.remove(name).is_some() {
            false
        } else {
            self.unverified.insert(name.to_owned(), 1);
            true
        }
    }

    /// Held arcane counts (stopgap or captured) for the bootstrap.
    pub fn held_arcanes(&self) -> HashMap<String, u32> {
        self.arcane_totals()
    }

    /// Total relic copies, arcane copies and part copies (status lines).
    pub fn totals(&self) -> (u32, u32, u32) {
        (
            self.relics.values().map(RelicStacks::total).sum(),
            self.parts.values().copied().sum(),
            self.arcanes.values().map(ArcaneStacks::total).sum(),
        )
    }

    /// Number of distinct relic stacks (not copies) — the capture status
    /// counts stacks, and "512 relic stacks" is the §2 phrasing.
    pub fn relic_stack_count(&self) -> usize {
        self.relics.values().map(|s| s.tiers.len()).sum()
    }

    pub fn arcane_stack_count(&self) -> usize {
        self.arcanes.values().map(|s| s.ranks.len()).sum()
    }
}

impl RelicStacks {
    /// Fold one observed (tier, count) stack in with a union (max count).
    pub fn merge(&mut self, tier: RefinementTier, n: u32) {
        let e = self.tiers.entry(tier).or_insert(0);
        *e = (*e).max(n);
    }
}

impl ArcaneStacks {
    /// Fold one observed (rank, count) stack in with a union (max count).
    pub fn merge(&mut self, rank: u8, n: u32) {
        let e = self.ranks.entry(rank).or_insert(0);
        *e = (*e).max(n);
    }
}

/// Ingest one folder of screenshot pages: every `*.pgm`/`*.ppm`/`*.png`
/// file, sorted, each split by `layout`, every cell OCR'd + validated.
/// A partial cell at the viewport edge is never read (recorded as NOT
/// held), so an off-screen half-relic can't be guessed into stock. The
/// hold threshold is the layout's `min_score` (screen-dependent).
pub fn ingest_folder(
    dir: &Path,
    engine: &dyn OcrEngine,
    layout: &Layout,
    universe: &Universe,
    relic_names: &HashSet<String>,
    arcane_names: &HashSet<String>,
    min_confidence: f32,
) -> Result<(StockState, Vec<CellResult>), String> {
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("ocr input {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| {
                    x.eq_ignore_ascii_case("pgm")
                        || x.eq_ignore_ascii_case("ppm")
                        || x.eq_ignore_ascii_case("png")
                })
                .unwrap_or(false)
        })
        .collect();
    pages.sort();
    if pages.is_empty() {
        return Err(format!(
            "ocr input {}: no .png/.pgm/.ppm pages",
            dir.display()
        ));
    }
    let mut results = Vec::new();
    for page_path in &pages {
        let bytes =
            std::fs::read(page_path).map_err(|e| format!("ocr read {}: {e}", page_path.display()))?;
        let img = GrayImage::from_bytes(&bytes)?;
        // One batched call per page (addendum #81), as in capture.
        let cells = layout.split(&img);
        let reads = engine.ocr_cells_in(&cells, layout);
        let mut page_results: Vec<CellResult> = Vec::with_capacity(reads.len());
        for read in reads {
            let index = results.len() + page_results.len();
            // Partial cell at the page edge: never read, never held.
            let read = read.unwrap_or_else(OcrRead::empty);
            page_results.push(validate_read(
                &read,
                universe,
                min_confidence,
                layout.min_score,
                index,
            ));
        }
        // Pass 38: the owned-gate applies to folder ingest exactly
        // as to live capture — a browsed catalogue page must not
        // import its unowned tiles through the back door. No
        // session reference exists here; the gate works from the
        // page's own split.
        if layout.key == "relic" {
            // Pass 46: the name-admission gate applies here
            // first, on the same terms as live capture — a
            // garbled name must not import through the back
            // door either.
            relic_name_admission(&mut page_results, universe);
            relic_owned_gate(&cells, layout, &mut page_results, None);
            // Pass 45: the refinement measure applies here too,
            // and the folder as a whole is the session the
            // disposition aggregates over. No header exists on
            // this path, so the variant is inferred from the
            // page's reads (a refinement second line proves it)
            // — see relic_variant_from_reads for the failure
            // mode.
            relic_refinement_measure(
                &cells,
                layout,
                &mut page_results,
                RelicScreenVariant::Unknown,
            );
        }
        results.extend(page_results);
    }
    let mut stock = StockState::from_results(&results, relic_names, arcane_names);
    if layout.key == "relic" {
        let mut evidence = RefinementEvidence::default();
        evidence.observe(&results, relic_names);
        evidence.apply_to(&mut stock);
    }
    Ok((stock, results))
}

#[cfg(test)]
mod tests {

    #[test]
    fn write_png_round_trips_through_from_png() {
        let mut img = GrayImage::blank(7, 5, 0);
        for (i, px) in img.pixels.iter_mut().enumerate() {
            *px = (i * 37 % 256) as u8;
        }
        let dir = std::env::temp_dir().join(format!("fracker-png-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame.png");
        img.write_png(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let back = GrayImage::from_png(&bytes).unwrap();
        assert_eq!(back.width, img.width);
        assert_eq!(back.height, img.height);
        assert_eq!(back.pixels, img.pixels);
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;
    use crate::tuning::Tuning;
    use std::sync::OnceLock;

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

    /// Compact [`OcrRead`] builder for validation tests (no arcane rank).
    fn read(text: &str, confidence: f32, count: Option<u32>) -> OcrRead {
        OcrRead {
            text: text.to_owned(),
            confidence,
            count,
            rank: None,
        }
    }

    #[test]
    fn segments_even_grid() {
        let img = GrayImage::blank(100, 60, 0);
        let cells = split_flat_grid(&img, GridSpec { rows: 3, cols: 2 });
        assert_eq!(cells.len(), 6);
        assert!(cells.iter().all(|c| c.width == 50 && c.height == 20));
    }

    #[test]
    fn capture_layouts_include_arcanes_screen() {
        // Sixth pass §5 / eighth pass §3: the Arcanes inventory screen is
        // a first-class capture layout (addendum #34), now measured from
        // Seth's real shot (11x7 grid, `xN` counts, rank pips).
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let layouts =
            Layout::load_file(&base.join("data/capture/layouts.toml")).unwrap();
        for key in ["relic", "parts", "arcanes"] {
            assert!(layouts.contains_key(key), "missing layout {key}");
        }
        let arc = &layouts["arcanes"];
        assert_eq!(arc.cells_total(), 77, "11x7 arcane grid");
        assert!(matches!(arc.count_style, CountStyle::XPrefix));
        assert_eq!(arc.min_score, 0.5);
        assert!(arc.pips.is_some(), "measure rank from the pip row");
        // The relic screen reads a two-line name band (tier labels).
        assert!(layouts["relic"].name_upper.is_some());
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
        let exact = validate_read(&read("Neo C5", 0.95, None), &u, 0.5, 0.8, 0);
        assert!(exact.held);
        assert_eq!(exact.matched.as_deref(), Some("Neo C5"));
        // Typo still resolves (fuzzy), below-perfect score.
        let typo = validate_read(
            &read("Ivara Prime Chassis Blueprlnt", 0.9, None),
            &u,
            0.5,
            0.8,
            1,
        );
        assert!(typo.held, "typo resolves: {typo:?}");
        assert!(typo.match_score < 1.0);
        // Garbage never validates.
        let garbage = validate_read(&read("zzz qqq www", 0.9, None), &u, 0.5, 0.8, 2);
        assert!(!garbage.held);
        assert!(garbage.matched.is_some()); // best effort named, but NOT held
        // Low confidence never validates even when exact.
        let shy = validate_read(&read("Neo C5", 0.2, None), &u, 0.5, 0.8, 3);
        assert!(!shy.held);
    }

    /// Pass 46 test universe: a slice of the relic catalogue
    /// around the C11/L1 sibling pair.
    fn relic_test_universe() -> Universe {
        Universe::from_names(
            &[
                "Axi V2",
                "Lith M10",
                "Meso A11",
                "Meso C11",
                "Meso L1",
                "Meso L3",
                "Neo C5",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn relic_name_admission_refuses_fused_sibling_garble() {
        // The pass-46 defect, synthetically: the C11 tile's
        // fused garble. The resolver's own bars ADMIT it —
        // that is the bug — so assert the pre-gate hold first,
        // then the gate's refusal and its record.
        let u = relic_test_universe();
        let mut results = vec![validate_read(
            &read("Mesoll Relic [Radiant]", 0.9, None),
            &u,
            0.5,
            0.8,
            0,
        )];
        assert!(
            results[0].held,
            "pre-gate, the resolver admits the garble (the defect): {:?}",
            results[0]
        );
        assert_eq!(results[0].matched.as_deref(), Some("Meso L1"));
        relic_name_admission(&mut results, &u);
        assert!(!results[0].held, "the gate must refuse the fused garble");
        let nr = results[0]
            .name_refused
            .as_ref()
            .expect("refusal is recorded");
        assert_eq!(nr.winner, "Meso L1");
        assert!(nr.runner_up.is_some(), "record names the runner-up: {nr:?}");
        assert!(nr.margin() >= 0.0 && nr.margin() < 0.25, "record: {nr:?}");
        // A fused read refuses even when its string is an
        // unambiguous name: the shape is the defect class,
        // not the ambiguity.
        let mut fused = vec![validate_read(
            &read("MesoC11 Relic", 0.9, None),
            &u,
            0.5,
            0.8,
            0,
        )];
        if fused[0].held {
            relic_name_admission(&mut fused, &u);
            assert!(
                !fused[0].held,
                "fused clean read refuses as a shape: {fused:?}"
            );
        }
    }

    #[test]
    fn relic_name_admission_keeps_witnessed_reads() {
        // Clean reads of BOTH siblings import — the gate
        // refuses the garble, not the name it garbled into.
        // Noisy-but-witnessed reads import: one glyph wrong
        // per token, split glyphs, a junk token in front.
        let u = relic_test_universe();
        for (text, want) in [
            ("Meso L1 Relic", "Meso L1"),
            ("Meso C11 Relic", "Meso C11"),
            ("Meso C11 Relic [Radiant]", "Meso C11"),
            ("Ayi V2 Relic", "Axi V2"),
            ("Lith MI 0 Relic", "Lith M10"),
            ("x8 Meso C11 Relic", "Meso C11"),
        ] {
            let mut results = vec![validate_read(&read(text, 0.9, None), &u, 0.5, 0.8, 0)];
            assert!(results[0].held, "pre-gate hold for {text:?}: {:?}", results[0]);
            relic_name_admission(&mut results, &u);
            assert!(results[0].held, "gate keeps witnessed read {text:?}");
            assert_eq!(results[0].matched.as_deref(), Some(want));
            assert!(results[0].name_refused.is_none());
        }
    }

    #[test]
    fn strip_tier_label_strips_only_balanced_spans() {
        // Balanced labels still strip whole, as before.
        assert_eq!(strip_tier_label("Neo C5 [Radiant]"), "Neo C5 ");
        assert_eq!(strip_tier_label("a [b [c] d] e"), "a  e");
        assert_eq!(strip_tier_label("[Flawless] Neo C5"), " Neo C5");
        // Pass 27: an unmatched '[' costs only the bracket — the
        // census case (parts-07 #0) lost the entire name before.
        assert_eq!(
            strip_tier_label("[ Gauss Prime Blueprint"),
            " Gauss Prime Blueprint"
        );
        // A stray ']' drops alone; an unmatched span's text survives.
        assert_eq!(strip_tier_label("x ] y"), "x  y");
        assert_eq!(strip_tier_label("keep [this text"), "keep this text");
    }

    #[test]
    fn unmatched_bracket_read_still_validates() {
        // Census parts-07 #0 / parts-10 #0: the raw read carries a
        // leading '[' (a relic tier glyph) in front of the name.
        let u = Universe::from_names(
            &["Gauss Prime Blueprint", "Forma Blueprint"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );
        let r = validate_read(&read("[ Gauss Prime Blueprint", 0.9, None), &u, 0.5, 0.5, 0);
        assert!(r.held, "unmatched-bracket read validates: {r:?}");
        assert_eq!(r.matched.as_deref(), Some("Gauss Prime Blueprint"));
    }

    #[test]
    fn deep_suffix_trim_recovers_name_behind_junk_tokens() {
        // Census score-dilution class: the clean name sits behind
        // more leading junk tokens than the old 4-trim cap allowed.
        let u = universe();
        let r = validate_read(
            &read("O5 zz qq rr ss tt Forma Blueprint", 0.9, None),
            &u,
            0.5,
            0.5,
            0,
        );
        assert!(r.held, "deep trim validates: {r:?}");
        assert_eq!(r.matched.as_deref(), Some("Forma Blueprint"));
    }

    #[test]
    fn stock_splits_tiers_totals_sum_and_frames_union() {
        // Eighth pass §1: relics key by (relic, tier); totals are sums; a
        // repeated stack identity (the same cell seen twice) is a UNION
        // (max), never a sum across frames — the double-count fix.
        let u = universe();
        let rel = relics();
        let reads = vec![
            validate_read(&read("Neo C5", 0.9, None), &u, 0.5, 0.8, 0),
            // Same stack identity, seen again (overlapping scroll): union.
            validate_read(&read("Neo C5", 0.9, None), &u, 0.5, 0.8, 1),
            // A distinct tier of the same relic: adds to the total.
            validate_read(&read("Neo C5 [Radiant]", 0.9, Some(3)), &u, 0.5, 0.8, 2),
            validate_read(&read("Forma Blueprint", 0.9, None), &u, 0.5, 0.8, 3),
            validate_read(&read("zzz", 0.9, None), &u, 0.5, 0.8, 4),
        ];
        let stock = StockState::from_results(&reads, &rel, &HashSet::new());
        let neo = stock.relics.get("Neo C5").expect("Neo C5 stack");
        assert_eq!(neo.get(RefinementTier::Intact), 1, "same stack unions to one");
        assert_eq!(neo.get(RefinementTier::Radiant), 3);
        assert_eq!(neo.total(), 4, "totals sum across tiers");
        assert_eq!(stock.parts.get("Forma Blueprint"), Some(&1));
        // Planner shape: known relic stock with the radiant split.
        match stock.to_relic_stock() {
            crate::model::RelicStock::Known(k) => {
                assert_eq!(k.totals.get("Neo C5"), Some(&4));
                assert_eq!(k.radiant.get("Neo C5"), Some(&3));
            }
            crate::model::RelicStock::Unknown => panic!("stock must be known"),
        }
        let json = stock.to_json().unwrap();
        let back = StockState::from_json(&json).unwrap();
        assert_eq!(back.relics.get("Neo C5").map(RelicStacks::total), Some(4));
    }

    #[test]
    fn canonical_part_counts_sum_built_and_blueprint_forms() {
        // Addendum #84/#89: a roster part's canonical count sums its
        // blueprint form and its built form; the gear's own blueprint
        // has no built counterpart; non-roster names drop out.
        let roster = crate::model::Roster::from_json(
            r#"[{"name": "Khora Prime", "vaulted": true, "type": "warframe",
                 "parts": [{"name": "Khora Prime Blueprint"},
                           {"name": "Khora Prime Chassis Blueprint"}]}]"#,
        )
        .unwrap();
        let stock = StockState {
            parts: [
                ("Khora Prime Chassis Blueprint".to_owned(), 1),
                ("Khora Prime Chassis".to_owned(), 2),
                ("Khora Prime".to_owned(), 4),
                ("Unrelated Junk".to_owned(), 5),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let counts = stock.canonical_part_counts(&roster);
        assert_eq!(
            counts.get("Khora Prime Chassis Blueprint"),
            Some(&3),
            "blueprint 1 + built 2 sum onto the roster part"
        );
        assert_eq!(counts.len(), 1, "own-blueprint built form and junk drop out");
    }

    #[test]
    fn unverified_section_round_trips_and_old_files_load_empty() {
        // Thirtieth pass (#93): the unverified (⍻) section survives
        // a stock.json round-trip under its own top-level key...
        let mut stock = StockState::default();
        stock.parts.insert("Protea Prime Systems".to_owned(), 1);
        stock
            .unverified
            .insert("Protea Prime Systems Blueprint".to_owned(), 1);
        let json = stock.to_json().unwrap();
        assert!(
            json.contains("\"unverified\""),
            "the section serialises as its own top-level key: {json}"
        );
        let back = StockState::from_json(&json).unwrap();
        assert_eq!(back.unverified, stock.unverified, "round-trip preserves the section");
        assert_eq!(back.parts, stock.parts);
        assert_eq!(
            back.unverified_part_counts()
                .get("Protea Prime Systems Blueprint"),
            Some(&1)
        );
        // ...and a pre-pass-30 stock file (no section at all) loads
        // with the section empty — every existing stock.json stays
        // valid.
        let old = StockState::from_json(
            r#"{"relics": {}, "parts": {"Protea Prime Systems": 1}, "arcanes": {}}"#,
        )
        .unwrap();
        assert!(old.unverified.is_empty(), "missing section defaults empty");
        assert_eq!(old.parts.get("Protea Prime Systems"), Some(&1));
    }

    #[test]
    fn toggle_unverified_flips_membership() {
        // Thirtieth pass (#93): the pure state transition behind the
        // Hunt-tab click — on at count 1, off removes entirely.
        let mut stock = StockState::default();
        assert!(stock.toggle_unverified("Protea Prime Systems Blueprint"));
        assert_eq!(
            stock.unverified.get("Protea Prime Systems Blueprint"),
            Some(&1),
            "toggle-on asserts the hold at count 1"
        );
        assert!(!stock.toggle_unverified("Protea Prime Systems Blueprint"));
        assert!(
            !stock.unverified.contains_key("Protea Prime Systems Blueprint"),
            "toggle-off removes the entry, not a zero count"
        );
    }

    #[test]
    fn stock_merge_unions_overlapping_frames() {
        // §2: capture merge is a union over stack identity. Two frames
        // that both saw Axi G10 x14 and one that saw Axi A1 x8 produce
        // Axi G10 = 14 (NOT 28), Axi A1 = 8.
        let rel: HashSet<String> = ["Axi G10".to_owned(), "Axi A1".to_owned()].into_iter().collect();
        let u = Universe::from_names(
            &["Axi G10", "Axi A1"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );
        let mut a = StockState::from_results(
            &[validate_read(&read("Axi G10", 0.9, Some(14)), &u, 0.5, 0.8, 0)],
            &rel,
            &HashSet::new(),
        );
        let b = StockState::from_results(
            &[
                validate_read(&read("Axi G10", 0.9, Some(14)), &u, 0.5, 0.8, 0),
                validate_read(&read("Axi A1", 0.9, Some(8)), &u, 0.5, 0.8, 1),
            ],
            &rel,
            &HashSet::new(),
        );
        a.merge_from(&b);
        assert_eq!(a.relics.get("Axi G10").map(RelicStacks::total), Some(14));
        assert_eq!(a.relics.get("Axi A1").map(RelicStacks::total), Some(8));
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
        let stock = StockState::from_results(&[r], &rel, &HashSet::new());
        assert!(stock.relics.is_empty() && stock.parts.is_empty());
    }

    #[test]
    fn arcanes_route_to_the_arcane_bucket() {
        // Sixth pass §5: the Arcanes inventory screen feeds arcane stock.
        // Neither the profile nor EE.log carries arcane state (#34), so
        // this routing is the steady-state source until a capture lands.
        let names: Vec<String> = ["Melee Influence", "Axi A1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let u = Universe::from_names(&names);
        let relic_names: HashSet<String> = ["Axi A1".to_owned()].into_iter().collect();
        let arcane_names: HashSet<String> =
            ["Melee Influence".to_owned()].into_iter().collect();
        let reads = vec![
            validate_read(&read("Melee Influence", 0.9, Some(4)), &u, 0.5, 0.5, 0),
            validate_read(&read("Axi A1", 0.9, None), &u, 0.5, 0.5, 1),
        ];
        let stock = StockState::from_results(&reads, &relic_names, &arcane_names);
        assert_eq!(stock.held_arcanes().get("Melee Influence"), Some(&4));
        assert_eq!(stock.relics.get("Axi A1").map(RelicStacks::total), Some(1));
        assert!(stock.parts.is_empty(), "nothing falls through to parts");
        let back = StockState::from_json(&stock.to_json().unwrap()).unwrap();
        assert_eq!(back.held_arcanes().get("Melee Influence"), Some(&4));
    }

    #[test]
    fn tesseract_probe_matches_binary_presence() {
        // Probe order is bundled sidecar candidates, then $PATH; it
        // succeeds iff a candidate actually runs `--version`.
        let sidecar_configured = std::env::var_os("FRACKER_TESSERACT").is_some()
            || Path::new("/opt/opencode/tools/tesseract/bin/tesseract").exists()
            || Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tools/tesseract/bin/tesseract"
            ))
            .exists();
        let on_path = std::env::var_os("PATH")
            .map(|p| {
                std::env::split_paths(&p).any(|d| {
                    d.join("tesseract").exists() || d.join("tesseract.exe").exists()
                })
            })
            .unwrap_or(false);
        assert_eq!(
            TesseractCliEngine::probe(default_layout()).is_some(),
            sidecar_configured || on_path
        );
    }

    #[test]
    fn crop_key_is_stable_and_discriminates() {
        // Addendum #81 cache keys: identical crops key identically, a
        // sub-quantisation nudge (every pixel +1) still keys the same
        // (the `>> 4` tolerance), and a genuinely different crop does not.
        let a = GrayImage::blank(10, 4, 100);
        let mut nudged = a.clone();
        for p in nudged.pixels.iter_mut() {
            *p = p.saturating_add(1);
        }
        let different = GrayImage::blank(10, 4, 200);
        assert_eq!(crop_key(&a, 7), crop_key(&a.clone(), 7));
        assert_eq!(crop_key(&a, 7), crop_key(&nudged, 7));
        assert_ne!(crop_key(&a, 7), crop_key(&different, 7));
        assert_ne!(crop_key(&a, 7), crop_key(&a, 8), "salt separates settings");
    }

    #[test]
    fn batched_reads_match_per_cell_reads() {
        // Addendum #81 gate: the batched path must return EXACTLY what
        // the per-cell path returns for the same cells — same text,
        // same count, same rank — whatever tesseract makes of the
        // pixels. Skips when no tesseract is available (as the fixture
        // tests do). The layout exercises the upper-band rule too.
        let Some(engine) = TesseractCliEngine::probe(default_layout()) else {
            eprintln!("skip batched equivalence: no tesseract available");
            return;
        };
        let mut layout = flat_layout(1, 1, 200, 60);
        layout.name = [0.0, 0.3, 1.0, 1.0];
        layout.name_upper = Some([0.0, 0.0, 1.0, 0.3]);
        layout.count = [0.0, 0.0, 0.4, 0.3];
        layout.name_upscale = 2;
        layout.count_upscale = 2;

        let labelled = bitmap_font::render_text("NEO C5", 2);
        let mut noise = GrayImage::blank(200, 60, 18);
        for (i, p) in noise.pixels.iter_mut().enumerate() {
            *p = ((i * 37 + 11) % 251) as u8;
        }
        let cells: Vec<Option<GrayImage>> = vec![
            Some(labelled.clone()),
            Some(GrayImage::blank(200, 60, 18)),
            Some(noise),
            None,
            Some(labelled),
        ];
        let batched = engine.ocr_cells_in(&cells, &layout);
        assert_eq!(batched.len(), cells.len());
        for (cell, batch) in cells.iter().zip(&batched) {
            match (cell, batch) {
                (None, None) => {}
                (Some(c), Some(b)) => {
                    let single = engine.ocr_cell_in(c, &layout);
                    assert_eq!(b.text, single.text, "text must agree");
                    assert_eq!(b.count, single.count, "count must agree");
                    assert_eq!(b.rank, single.rank, "rank must agree");
                    assert_eq!(b.confidence, single.confidence);
                }
                other => panic!("None cells must stay None, got {other:?}"),
            }
        }
        // A second identical call is served from the crop cache and
        // must return the same reads again.
        let again = engine.ocr_cells_in(&cells, &layout);
        for (a, b) in batched.iter().zip(&again) {
            assert_eq!(a.as_ref().map(|r| &r.text), b.as_ref().map(|r| &r.text));
            assert_eq!(a.as_ref().and_then(|r| r.count), b.as_ref().and_then(|r| r.count));
        }
    }

    /// A layout with no sub-region cropping: the whole cell is the name
    /// region. Used by the synthetic-page test.
    fn flat_layout(cols: u32, rows: u32, w: u32, h: u32) -> Layout {
        Layout {
            key: "test".to_owned(),
            cols,
            rows,
            ref_width: w,
            ref_height: h,
            x0: 0.0,
            cell_w: w as f32 / cols.max(1) as f32,
            y0: 0.0,
            cell_h: h as f32 / rows.max(1) as f32,
            name: [0.0, 0.0, 1.0, 1.0],
            count: [0.0, 0.0, 1.0, 1.0],
            count_style: CountStyle::XPrefix,
            name_psm: 7,
            count_psm: 7,
            name_upscale: 1,
            count_upscale: 1,
            name_upper: None,
            pips: None,
            min_score: 0.8,
        }
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
        let layout = flat_layout(2, 1, page.width, page.height);
        let (stock, results) = ingest_folder(
            &dir,
            &StubEngine,
            &layout,
            &u,
            &rel,
            &HashSet::new(),
            0.5,
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
            let r = validate_read(&read(label, 0.9, None), &u, 0.5, 0.8, i);
            assert!(r.held, "known label validates: {label}");
        }
    }

    #[test]
    fn tuning_files_present_for_planner() {
        let t = Tuning::for_tests();
        assert!(!t.weeklies.is_empty());
    }

    // -----------------------------------------------------------------
    // Real-fixture integration (Seth's screenshots; gitignored). These
    // skip gracefully when the fixtures or a working tesseract is absent
    // (other machines won't have either), so the suite stays green.
    // -----------------------------------------------------------------

    /// Relic ground truth, row-major (see fixtures/ocr-samples/NOTES.md).
    const RELIC_GT: [&str; 77] = [
        "Axi A1", "Axi A4", "Axi A5", "Axi A9", "Axi A10", "Axi A11", "Axi A14", "Axi A16",
        "Axi A17", "Axi A18", "Axi A20", "Axi A22", "Axi B1", "Axi B2", "Axi B3", "Axi B4",
        "Axi B5", "Axi B6", "Axi B8", "Axi C1", "Axi C2", "Axi C3", "Axi C4", "Axi C6",
        "Axi C7", "Axi D1", "Axi D6", "Axi E1", "Axi E2", "Axi F2", "Axi G1", "Axi G4",
        "Axi G5", "Axi G8", "Axi G9", "Axi G10", "Axi G11", "Axi G12", "Axi G14", "Axi H1",
        "Axi H2", "Axi H4", "Axi H6", "Axi H7", "Axi I2", "Axi K1", "Axi K2", "Axi K4",
        "Axi K6", "Axi K8", "Axi K9", "Axi K10", "Axi K11", "Axi L1", "Axi L2", "Axi L3",
        "Axi L4", "Axi L6", "Axi M2", "Axi M6", "Axi N1", "Axi N2", "Axi N3", "Axi N6",
        "Axi N8", "Axi N9", "Axi N10", "Axi N13", "Axi O1", "Axi O3", "Axi O4", "Axi O5",
        "Axi O6", "Axi P1", "Axi P4", "Axi P6", "Axi P8",
    ];

    /// Part ground truth for the three full rows (row-major, 11x3).
    const PARTS_GT: [&str; 33] = [
        "Acceltra Prime Receiver", "Acceltra Prime Stock", "Afuris Prime Barrel",
        "Afuris Prime Link", "Afuris Prime Receiver", "Akarius Prime Barrel",
        "Akarius Prime Link", "Akarius Prime Receiver", "Akbolto Prime Barrel",
        "Akbolto Prime Link", "Akbronco Prime Link", "Akjagara Prime Link",
        "Akjagara Prime Receiver", "Aklex Prime Link", "Aksomati Prime Barrel",
        "Aksomati Prime Link", "Aksomati Prime Receiver", "Akstiletto Prime Barrel",
        "Akstiletto Prime Link", "Akstiletto Prime Receiver", "Akvasto Prime Link",
        "Ankyros Prime Blade", "Astilla Prime Barrel", "Astilla Prime Receiver",
        "Astilla Prime Stock", "Atlas Prime Chassis Blueprint",
        "Atlas Prime Systems Blueprint", "Ballistica Prime Receiver",
        "Ballistica Prime String", "Ballistica Prime Upper Limb",
        "Banshee Prime Chassis Blueprint", "Banshee Prime Neuroptics Blueprint",
        "Banshee Prime Systems Blueprint",
    ];

    /// Universe + relic/arcane name sets, built by the SAME helper the
    /// live capture and `--ocr-ingest` use (`capture::load_universe`:
    /// roster + relic names + the arcane set, scrubbed), so the tests and
    /// production can never drift. The combined set is the right test
    /// universe here: these fixtures exercise the ingest path end to
    /// end, not one screen's scoping.
    fn seed_universe() -> Option<(Universe, HashSet<String>, HashSet<String>)> {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        crate::capture::load_universe(&base.join("data"))
            .ok()
            .map(|set| (set.combined, set.relic_names, set.arcane_names))
    }

    /// Run one real fixture page through the full ingest path. `None`
    /// (=> the test returns early) when the fixture, layout, seed data or
    /// a working engine is missing.
    fn fixture_cells(file: &str, layout_key: &str) -> Option<Vec<CellResult>> {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let src = base.join("fixtures/ocr-samples").join(file);
        if !src.exists() {
            eprintln!("skip {file}: fixture absent");
            return None;
        }
        let layouts = Layout::load_file(&base.join("data/capture/layouts.toml")).ok()?;
        let layout = layouts.get(layout_key)?.clone();
        let (universe, relic_names, arcane_names) = seed_universe()?;
        let engine = match TesseractCliEngine::probe(layout.clone()) {
            Some(e) => e,
            None => {
                eprintln!("skip {file}: no tesseract available");
                return None;
            }
        };
        let dir = std::env::temp_dir().join(format!(
            "fracker-ocr-fx-{}-{}",
            file.replace('.', "_"),
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::copy(&src, dir.join(file)).ok()?;
        let res = ingest_folder(
            &dir,
            &engine,
            &layout,
            &universe,
            &relic_names,
            &arcane_names,
            0.5,
        );
        let _ = std::fs::remove_dir_all(&dir);
        res.ok().map(|(_, results)| results)
    }

    /// A synthetic relic cell at the canonical capture size
    /// (168x174): dark card, a checkerboard icon whose highlight
    /// percentile is the `bright` level (210) or its faded
    /// quarter (52), and — when `eye` — the measured eye template
    /// pasted at the badge window's nominal position.
    fn relic_cell(eye: bool, bright: bool) -> GrayImage {
        let mut cell = GrayImage::blank(168, 174, 6);
        let (hi, lo) = if bright { (210, 60) } else { (52, 15) };
        for y in 17..122 {
            for x in 25..143 {
                cell.set(x, y, if (x + y) % 2 == 0 { hi } else { lo });
            }
        }
        if eye {
            for (ty, row) in EYE_TEMPLATE.iter().enumerate() {
                for (tx, &v) in row.iter().enumerate() {
                    cell.set(tx as u32, ty as u32, v);
                }
            }
        }
        cell
    }

    fn held_cell_result(index: usize, name: &str, count: Option<u32>) -> CellResult {
        CellResult {
            index,
            raw_text: name.to_owned(),
            confidence: 1.0,
            matched: Some(name.to_owned()),
            match_score: 1.0,
            held: true,
            count,
            tier: RefinementTier::Intact,
            rank: None,
            gate_excluded: false,
            refine_eye_mass: None,
            name_refused: None,
        }
    }

    fn relic_layout() -> Layout {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let layouts = Layout::load_file(&base.join("data/capture/layouts.toml")).unwrap();
        layouts.get("relic").unwrap().clone()
    }

    #[test]
    fn eye_badge_score_separates_template_from_plain_corner() {
        assert!(
            eye_badge_score(&relic_cell(true, true)) >= 0.99,
            "the pasted template scores ~1"
        );
        assert!(
            eye_badge_score(&relic_cell(false, true)) < EYE_NCC_MIN,
            "an owned corner (icon texture, no badge) is not an eye"
        );
        assert!(
            icon_highlight(&relic_cell(false, true)) > 200.0,
            "bright icon highlight"
        );
        assert!(
            icon_highlight(&relic_cell(false, false)) < 60.0,
            "faded icon highlight collapses"
        );
    }

    #[test]
    fn owned_gate_excludes_only_dim_badged_uncounted_tiles() {
        // A mixed catalogue frame: 4 owned (2 labelled x3, 2 plain
        // x1) and 4 unowned (dim + eye, no label). Only the
        // unowned four may be excluded.
        let layout = relic_layout();
        let mut cells: Vec<Option<GrayImage>> = Vec::new();
        let mut results: Vec<CellResult> = Vec::new();
        let names = [
            "Lith A1", "Lith B2", "Meso C3", "Meso D4", "Neo E5", "Neo F6", "Axi G7",
            "Axi H8",
        ];
        for (i, name) in names.iter().enumerate() {
            let owned_tile = i < 4;
            cells.push(Some(relic_cell(!owned_tile, owned_tile)));
            let count = if i < 2 { Some(3) } else { None };
            results.push(held_cell_result(i, name, count));
        }
        let out = relic_owned_gate(&cells, &layout, &mut results, None);
        assert!(out.catalogue, "an eye anywhere marks the frame");
        assert_eq!(
            out.excluded,
            vec!["Neo E5", "Neo F6", "Axi G7", "Axi H8"],
            "exactly the unowned tiles"
        );
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r.held, i < 4, "tile {i} held state");
            assert_eq!(r.gate_excluded, i >= 4, "tile {i} exclusion flag");
        }
    }

    #[test]
    fn owned_gate_keeps_ambiguous_tiles() {
        // Every ambiguity resolves to KEEP: a dim tile with no eye
        // (no badge corroboration), a bright tile with an eye
        // (owned evidence wins), a dim badged tile with a parsed
        // count (the label proves ownership).
        let layout = relic_layout();
        let cells: Vec<Option<GrayImage>> = vec![
            Some(relic_cell(true, true)),  // eye trigger + bright owner
            Some(relic_cell(false, false)), // dim, no eye
            Some(relic_cell(true, true)),  // bright + eye
            Some(relic_cell(true, false)), // dim + eye + count (below)
        ];
        let mut results = vec![
            held_cell_result(0, "Lith A1", None),
            held_cell_result(1, "Meso B2", None),
            held_cell_result(2, "Neo C3", None),
            held_cell_result(3, "Axi D4", Some(3)),
        ];
        let out = relic_owned_gate(&cells, &layout, &mut results, None);
        assert!(out.catalogue);
        assert!(out.excluded.is_empty(), "nothing here is provably unowned");
        assert!(results.iter().all(|r| r.held));
    }

    #[test]
    fn owned_gate_inert_without_an_eye() {
        // Owned-only screens (and any frame with no badge) are
        // bit-identical: even uniformly dim tiles are untouched.
        let layout = relic_layout();
        let cells: Vec<Option<GrayImage>> = (0..4)
            .map(|_| Some(relic_cell(false, false)))
            .collect();
        let mut results: Vec<CellResult> = (0..4)
            .map(|i| held_cell_result(i, &format!("Axi A{i}"), None))
            .collect();
        let out = relic_owned_gate(&cells, &layout, &mut results, None);
        assert!(!out.catalogue);
        assert!(out.excluded.is_empty());
        assert!(results.iter().all(|r| r.held));
    }

    #[test]
    fn owned_gate_uniform_dim_frame_needs_the_session_reference() {
        // The catalogue tail: a whole frame of unowned tiles has
        // no bright cluster to split against. With the session's
        // owned-brightness reference the gate still excludes;
        // without it (cold start) it keeps everything — fail-safe.
        let layout = relic_layout();
        let mk = || {
            let cells: Vec<Option<GrayImage>> = (0..4)
                .map(|_| Some(relic_cell(true, false)))
                .collect();
            let results: Vec<CellResult> = (0..4)
                .map(|i| held_cell_result(i, &format!("Axi B{i}"), None))
                .collect();
            (cells, results)
        };
        let (cells, mut results) = mk();
        let out = relic_owned_gate(&cells, &layout, &mut results, Some(210.0));
        assert_eq!(out.excluded.len(), 4, "the reference exposes the tail");
        let (cells, mut results) = mk();
        let out = relic_owned_gate(&cells, &layout, &mut results, None);
        assert!(out.excluded.is_empty(), "no reference, no split, no exclusion");
        assert!(results.iter().all(|r| r.held));
    }

    /// A bare relic cell at the canonical capture size: dark,
    /// eye window empty (pass 45 measure tests).
    fn bare_cell() -> GrayImage {
        GrayImage::blank(168, 174, 6)
    }

    /// A relic cell carrying the eye glyph at its native
    /// placement (research bbox: x 23-41, y 22-33 cell-relative,
    /// flat peak 154): a thin outline plus pupil, ~65 bright
    /// px — the real glyph's mass, which is also what keeps
    /// it under the label-presence floor in the count region.
    fn eyed_cell() -> GrayImage {
        let mut cell = GrayImage::blank(168, 174, 6);
        for x in 23..41 {
            cell.set(x, 22, 154);
            cell.set(x, 33, 154);
        }
        for y in 22..34 {
            cell.set(23, y, 154);
            cell.set(40, y, 154);
        }
        for y in 26..29 {
            for x in 30..33 {
                cell.set(x, y, 154);
            }
        }
        cell
    }

    #[test]
    fn eye_window_bright_mass_counts_bright_pixels_only() {
        assert_eq!(eye_window_bright_mass(&bare_cell()), 0, "bare corner is empty");
        let mass = eye_window_bright_mass(&eyed_cell());
        assert!(mass >= EYE_BRIGHT_MASS_MIN, "eyed cell mass {mass}");
        // A sub-floor smudge (5x5 = 25 bright px) stays under
        // the floor — the transient-smear regime.
        let mut smudge = GrayImage::blank(168, 174, 6);
        for y in 22..27 {
            for x in 23..28 {
                smudge.set(x, y, 154);
            }
        }
        assert!(
            eye_window_bright_mass(&smudge) < EYE_BRIGHT_MASS_MIN,
            "smudge mass {}",
            eye_window_bright_mass(&smudge)
        );
        // Pixels AT the threshold grey do not count (the rule
        // is strictly above EYE_BRIGHT_PX).
        let mut at = GrayImage::blank(168, 174, EYE_BRIGHT_PX);
        for y in 22..34 {
            for x in 23..41 {
                at.set(x, y, EYE_BRIGHT_PX);
            }
        }
        assert_eq!(eye_window_bright_mass(&at), 0);
    }

    #[test]
    fn refinement_measure_skips_physically_labelled_tiles() {
        // The Lith M4 shape (corpus, frames 0/1): a solid
        // bright count label in the count region that the
        // parser did not read. Its strokes fill the eye
        // window too, but the tile is owned — a glyph IS
        // present in its corner — and must not be measured.
        let layout = relic_layout();
        let mut labelled = GrayImage::blank(168, 174, 6);
        for y in 12..42 {
            for x in 20..70 {
                labelled.set(x, y, 200);
            }
        }
        assert!(
            count_region_bright_mass(&labelled, &layout) >= LABEL_PRESENT_MASS_MIN,
            "label-scale region mass"
        );
        assert!(
            count_region_bright_mass(&eyed_cell(), &layout) < LABEL_PRESENT_MASS_MIN,
            "the eye glyph stays under the label floor"
        );
        assert_eq!(count_region_bright_mass(&bare_cell(), &layout), 0);
        let cells: Vec<Option<GrayImage>> = vec![Some(labelled)];
        let mut results = vec![held_cell_result(0, "Lith M4", None)];
        relic_refinement_measure(
            &cells,
            &layout,
            &mut results,
            RelicScreenVariant::Refinement,
        );
        assert_eq!(results[0].refine_eye_mass, None, "physically labelled: not measured");
        assert!(results[0].held);
    }

    #[test]
    fn refinement_measure_annotates_labelless_and_refuses_nothing() {
        // A refinement frame: two labelled stacks (not
        // measured — a label is owned evidence on its own) and
        // two label-less tiles, one bare (owned x1) and one
        // eyed (unowned). The measure only RECORDS the
        // statistic; disposition is the session's job.
        let layout = relic_layout();
        let cells: Vec<Option<GrayImage>> = vec![
            Some(bare_cell()),
            Some(bare_cell()),
            Some(bare_cell()),
            Some(eyed_cell()),
        ];
        let mut results = vec![
            held_cell_result(0, "Meso S7", Some(14)),
            held_cell_result(1, "Neo N1", Some(8)),
            held_cell_result(2, "Meso V15", None),
            held_cell_result(3, "Axi A2", None),
        ];
        relic_refinement_measure(
            &cells,
            &layout,
            &mut results,
            RelicScreenVariant::Refinement,
        );
        assert!(results.iter().all(|r| r.held), "the measure refuses nothing");
        assert_eq!(results[0].refine_eye_mass, None, "labelled: not measured");
        assert_eq!(results[1].refine_eye_mass, None, "labelled: not measured");
        assert_eq!(results[2].refine_eye_mass, Some(0), "bare corner measures empty");
        let eyed = results[3].refine_eye_mass.expect("eyed tile measured");
        assert!(eyed >= EYE_BRIGHT_MASS_MIN, "eyed tile mass {eyed}");
    }

    #[test]
    fn refinement_measure_inert_off_the_refinement_variant() {
        // Selection frames and undetermined frames with no
        // second line anywhere are not measured at all.
        let layout = relic_layout();
        for variant in [RelicScreenVariant::Selection, RelicScreenVariant::Unknown] {
            let cells: Vec<Option<GrayImage>> =
                vec![Some(eyed_cell()), Some(bare_cell())];
            let mut results = vec![
                held_cell_result(0, "Lith A1", None),
                held_cell_result(1, "Meso B2", Some(3)),
            ];
            relic_refinement_measure(&cells, &layout, &mut results, variant);
            assert!(
                results.iter().all(|r| r.refine_eye_mass.is_none()),
                "variant {variant:?} must be inert"
            );
        }
    }

    #[test]
    fn refinement_measure_infers_the_variant_from_a_second_line() {
        // Unknown variant (the ingest seam): one held read
        // carrying a refinement tier proves the frame is a
        // refinement frame, and the whole frame is measured —
        // the Intact label-less tile beside it included.
        let layout = relic_layout();
        let cells: Vec<Option<GrayImage>> =
            vec![Some(eyed_cell()), Some(bare_cell()), Some(bare_cell())];
        let mut results = vec![
            held_cell_result(0, "Lith G6", None),
            held_cell_result(1, "Meso C11", None),
            held_cell_result(2, "Axi A1", Some(8)),
        ];
        results[0].tier = RefinementTier::Radiant;
        results[1].tier = RefinementTier::Radiant;
        relic_refinement_measure(
            &cells,
            &layout,
            &mut results,
            RelicScreenVariant::Unknown,
        );
        assert!(results[0].refine_eye_mass.is_some(), "frame proven refinement: measured");
        assert!(results[1].refine_eye_mass.is_some(), "same frame: measured");
        assert_eq!(results[2].refine_eye_mass, None, "labelled: not measured");
    }

    #[test]
    fn refinement_evidence_dispositions_the_three_states() {
        let names: HashSet<String> = ["Axi A2", "Axi A9", "Lith G6", "Meso S7", "Meso V15"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut ev = RefinementEvidence::default();
        let mut frame = vec![
            held_cell_result(0, "Meso S7", Some(14)),
            held_cell_result(1, "Axi A2", None),
            held_cell_result(2, "Meso V15", None),
            held_cell_result(3, "Axi A9", None),
            held_cell_result(4, "Lith G6", None),
        ];
        frame[1].refine_eye_mass = Some(70); // settled eye
        frame[2].refine_eye_mass = Some(0); // bare: owned x1
        frame[3].refine_eye_mass = Some(16); // smeared transient only, so far
        frame[4].tier = RefinementTier::Radiant;
        frame[4].refine_eye_mass = Some(71);
        ev.observe(&frame, &names);
        assert!(ev.is_unowned("Axi A2", RefinementTier::Intact));
        assert!(ev.is_unowned("Lith G6", RefinementTier::Radiant));
        assert!(!ev.is_unowned("Lith G6", RefinementTier::Intact), "tiers are distinct stacks");
        assert!(!ev.is_unowned("Meso V15", RefinementTier::Intact), "bare window: owned x1");
        assert!(!ev.is_unowned("Axi A9", RefinementTier::Intact), "a smear alone does not convict");
        // A later settled appearance shows A9's eye clearly:
        // the session maximum, not any single frame, decides.
        let mut frame2 = vec![held_cell_result(0, "Axi A9", None)];
        frame2[0].refine_eye_mass = Some(69);
        ev.observe(&frame2, &names);
        assert!(ev.is_unowned("Axi A9", RefinementTier::Intact), "max over appearances");
        assert_eq!(ev.excluded_stacks().len(), 3);
        // A count label on any appearance is positive owned
        // evidence and outranks the eye record entirely.
        let frame3 = vec![held_cell_result(0, "Axi A2", Some(3))];
        ev.observe(&frame3, &names);
        assert!(!ev.is_unowned("Axi A2", RefinementTier::Intact), "label wins");
        assert_eq!(ev.excluded_stacks().len(), 2);
        // apply_to drops exactly the excluded stacks from a
        // union-built stock, and a name left with no tiers
        // disappears with them.
        let mut stock = StockState::default();
        for (name, tier, n) in [
            ("Axi A2", RefinementTier::Intact, 3),
            ("Axi A9", RefinementTier::Intact, 1),
            ("Lith G6", RefinementTier::Radiant, 1),
            ("Lith G6", RefinementTier::Intact, 36),
            ("Meso V15", RefinementTier::Intact, 1),
            ("Meso S7", RefinementTier::Intact, 14),
        ] {
            stock
                .relics
                .entry(name.to_owned())
                .or_default()
                .merge(tier, n);
        }
        ev.apply_to(&mut stock);
        assert!(!stock.relics.contains_key("Axi A9"), "unowned name dropped");
        assert_eq!(stock.relics["Axi A2"].get(RefinementTier::Intact), 3, "label win kept");
        assert_eq!(stock.relics["Lith G6"].get(RefinementTier::Radiant), 0, "tier dropped");
        assert_eq!(stock.relics["Lith G6"].get(RefinementTier::Intact), 36, "other tier kept");
        assert_eq!(stock.relics["Meso V15"].get(RefinementTier::Intact), 1, "x1 kept");
    }

    #[test]
    fn fixture_relic_clean_reads_all_cells_and_counts() {
        let Some(results) = fixture_cells("relics-axi-02-clean.png", "relic") else {
            return;
        };
        assert_eq!(results.len(), 77, "11x7 relic grid");
        let mut held = 0;
        for (i, r) in results.iter().enumerate() {
            if r.held {
                held += 1;
                assert_eq!(
                    r.matched.as_deref(),
                    Some(RELIC_GT[i]),
                    "held cell {i} must match ground truth"
                );
            }
        }
        // Every cell of the clean shot is legible; the never-guess rule
        // may still drop one pathological glyph read, so allow slack.
        assert!(held >= 70, "clean relic held only {held}/77");
        let count_of = |name: &str| {
            results
                .iter()
                .find(|r| r.matched.as_deref() == Some(name))
                .and_then(|r| r.count)
        };
        assert_eq!(count_of("Axi A1"), Some(8));
        assert_eq!(count_of("Axi K1"), Some(22));
        assert_eq!(count_of("Axi N2"), Some(16));
        assert_eq!(count_of("Axi G10"), Some(14));
    }

    #[test]
    fn fixture_relic_occluded_cells_are_not_guessed() {
        let Some(results) = fixture_cells("relics-axi-01.png", "relic") else {
            return;
        };
        assert_eq!(results.len(), 77);
        // The reward tooltip occludes the right-centre cells (rows 1-3,
        // cols 8-9). A correct pipeline flags them unreadable.
        for idx in [19, 20, 30, 31, 41, 42] {
            assert!(!results[idx].held, "occluded cell {idx} must not be held");
        }
        let held = results.iter().filter(|r| r.held).count();
        assert!(held >= 60, "occluded relic held only {held}/77");
        assert!(held < 77, "the occluded cells must not be guessed");
        for (i, r) in results.iter().enumerate() {
            if r.held {
                assert_eq!(
                    r.matched.as_deref(),
                    Some(RELIC_GT[i]),
                    "held cell {i} must agree with the clean shot"
                );
            }
        }
    }

    #[test]
    fn fixture_parts_three_row_grid_holds_only_real_parts() {
        let Some(results) = fixture_cells("parts-prime-01.png", "parts") else {
            return;
        };
        // 3 full rows; the clipped 4th row is not part of the layout.
        assert_eq!(results.len(), 33, "11x3 part grid");
        let mut held = 0;
        for (i, r) in results.iter().enumerate() {
            if r.held {
                held += 1;
                assert_eq!(
                    r.matched.as_deref(),
                    Some(PARTS_GT[i]),
                    "held part cell {i} must match ground truth"
                );
            }
        }
        assert!(held >= 5, "parts held only {held}/33");
    }

    // -----------------------------------------------------------------
    // Eighth pass §1/§3: Seth's real capture validation set (gitignored).
    // Each test skips gracefully when the fixtures or tesseract are absent.
    // -----------------------------------------------------------------

    fn seth_relic_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/ocr-samples/seth-relics-2026-10-16/relics")
    }

    /// Ingest a real capture directory through the full pipeline (layout
    /// key, seed universe). `None` when fixtures, seed data or a working
    /// engine are missing.
    fn ingest_real(dir: &Path, layout_key: &str) -> Option<(StockState, Vec<CellResult>)> {
        if !dir.is_dir() {
            eprintln!("skip {}: fixtures absent", dir.display());
            return None;
        }
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let layouts = Layout::load_file(&base.join("data/capture/layouts.toml")).ok()?;
        let layout = layouts.get(layout_key)?.clone();
        let (universe, relic_names, arcane_names) = seed_universe()?;
        let engine = match TesseractCliEngine::probe(layout.clone()) {
            Some(e) => e,
            None => {
                eprintln!("skip {}: no tesseract available", dir.display());
                return None;
            }
        };
        ingest_folder(dir, &engine, &layout, &universe, &relic_names, &arcane_names, 0.5).ok()
    }

    /// Seth's 9 relic shots, ingested once per test binary. The §1
    /// validation test and the §2 sequence test share this (the second
    /// caller blocks on the first, so the OCR pass runs only once).
    fn seth_relics_cached() -> Option<&'static (StockState, Vec<CellResult>)> {
        static CACHE: OnceLock<Option<(StockState, Vec<CellResult>)>> = OnceLock::new();
        CACHE
            .get_or_init(|| ingest_real(&seth_relic_dir(), "relic"))
            .as_ref()
    }

    /// Seth's real Arcanes shot, ingested once per test binary, shared by
    /// the §3 layout test and the §2 sequence test.
    fn seth_arcanes_cached() -> Option<&'static (StockState, Vec<CellResult>)> {
        static CACHE: OnceLock<Option<(StockState, Vec<CellResult>)>> = OnceLock::new();
        CACHE
            .get_or_init(|| {
                let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                let src = base.join("fixtures/ocr-samples/seth-arcanes-2026-10-16.png");
                if !src.exists() {
                    eprintln!("skip arcanes validation: fixture absent");
                    return None;
                }
                let dir = std::env::temp_dir()
                    .join(format!("fracker-ocr-arc-{}", std::process::id()));
                let _ = std::fs::create_dir_all(&dir);
                let copied = std::fs::copy(&src, dir.join("seth-arcanes.png")).is_ok();
                let ingested = copied
                    .then(|| ingest_real(&dir, "arcanes"))
                    .flatten();
                let _ = std::fs::remove_dir_all(&dir);
                ingested
            })
            .as_ref()
    }

    #[test]
    fn fixture_seth_relics_validation_set() {
        // Eighth pass §1 drop forensics. Ingest Seth's 9 live 1440p
        // relic shots and reproduce the Seth-confirmed cells, keep the
        // tier split, and prove the overlapping-scroll union.
        let Some((stock, results)) = seth_relics_cached() else {
            return;
        };
        assert_eq!(results.len(), 693, "9 shots x 11x7 cells");

        // Meso K4 x38: single stack, read correctly.
        let k4 = stock.relics.get("Meso K4").expect("Meso K4 present");
        assert_eq!(k4.total(), 38, "Meso K4 x38");

        // Lith G6: TWO stacks in one shot — x36 (Intact) + x1 ([Radiant]).
        // v0.2.0 kept 36 and discarded the split; we keep both.
        let g6 = stock.relics.get("Lith G6").expect("Lith G6 present");
        assert_eq!(g6.get(RefinementTier::Intact), 36, "G6 x36 intact stack");
        assert_eq!(g6.get(RefinementTier::Radiant), 1, "G6 x1 radiant stack");
        assert_eq!(g6.total(), 37, "G6 true total is 37");

        // Lith S14 x40: the v0.2.0 drop. Named cause was the `$`/`S`
        // glyph confusion (every S14 read as $14); the confusable fold
        // makes it hold.
        let s14 = stock.relics.get("Lith S14").expect("Lith S14 read");
        assert_eq!(s14.get(RefinementTier::Intact), 40, "Seth's x40 S14 stack");

        // Axi G10 appears x14 in two overlapping shots: the union must
        // count it ONCE (v0.2.0's stock.json says 28).
        assert_eq!(
            stock.relics.get("Axi G10").map(RelicStacks::total),
            Some(14),
            "overlapping-scroll stack counted once"
        );

        // Axi A1's captured x8 stack validates (the '1'/'l' confusion).
        assert_eq!(
            stock.relics.get("Axi A1").map(RelicStacks::total),
            Some(8),
            "Axi A1 x8"
        );

        // Coverage: nearly every cell holds, and the union removes the
        // v0.2.0 overlap double-counts (its name-only total was 3975).
        let held = results.iter().filter(|r| r.held).count();
        assert!(held >= 660, "held {held}/693");
        assert!(stock.relic_stack_count() >= 540, "stacks {}", stock.relic_stack_count());
        let copies: u32 = stock.relics.values().map(RelicStacks::total).sum();
        assert!(copies < 3975, "union beats double-counting: {copies} < 3975");

        // Tier classification accuracy, measured on the labeled subset:
        // every cell whose raw read carried a refinement label must
        // classify to a known tier (never a silent Unknown).
        let labeled: Vec<&CellResult> = results
            .iter()
            .filter(|r| r.held && r.raw_text.contains('['))
            .collect();
        assert!(labeled.len() >= 20, "labeled cells found: {}", labeled.len());
        let known = labeled
            .iter()
            .filter(|r| r.tier != RefinementTier::Unknown)
            .count();
        eprintln!(
            "Seth relic tier classification: {known}/{} = {:.1}%",
            labeled.len(),
            100.0 * known as f64 / labeled.len() as f64
        );
        assert_eq!(known, labeled.len(), "every tier label classifies");
        let radiant: u32 = stock
            .relics
            .values()
            .map(|s| s.get(RefinementTier::Radiant))
            .sum();
        assert!(radiant > 0, "radiant split recorded for the planner");
    }

    #[test]
    fn fixture_seth_arcanes_measured_layout() {
        // Eighth pass §3: geometry measured from Seth's real Arcanes
        // shot; reads validate against the arcane universe, counts are
        // `xN` (blank = 1) and the rank is the five-diamond pip row.
        let Some((stock, results)) = seth_arcanes_cached() else {
            return;
        };
        assert_eq!(results.len(), 77, "11x7 arcane grid");
        let held = results.iter().filter(|r| r.held).count();
        assert!(held >= 70, "arcanes held {held}/77");

        // The same arcane appears as MULTIPLE stacks at different ranks.
        let av = stock.arcanes.get("Arcane Avenger").expect("Arcane Avenger");
        assert!(av.ranks.len() >= 2, "Avenger at several ranks: {:?}", av.ranks);

        // Rank pips: the x14 Arcane Guardian is rank 0; a plain Guardian
        // (count 1) is rank 5.
        let g = stock.arcanes.get("Arcane Guardian").expect("Arcane Guardian");
        assert_eq!(g.get(0), 14, "Guardian x14 at rank 0");
        assert_eq!(g.get(5), 1, "a rank-5 Guardian stack");

        // Count convention: plain `xN` top-left, blank = 1 (x3 + plain).
        let accel = stock
            .arcanes
            .get("Arcane Acceleration")
            .expect("Arcane Acceleration");
        assert_eq!(accel.total(), 4, "x3 + plain = 4");

        // Two-line names ("Arcane Blade Charger") still validate.
        assert!(
            stock.arcanes.contains_key("Arcane Blade Charger"),
            "two-line name validates"
        );
    }

    // -----------------------------------------------------------------
    // Eighth pass §2: the box-side Capture Inventory gate. Feed Seth's real
    // shots as overlapping frame sequences through the capture pipeline
    // (no portal, no PipeWire) and prove the merged union equals the §1/§3
    // validation results with no double counts.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_seth_capture_sequence_unions_without_double_counts() {
        let Some(expected_relics) = seth_relics_cached() else {
            return;
        };
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let legend = "\n(skip §2 sequence: need the relic fixtures)";
        if !seth_relic_dir().is_dir() {
            eprintln!("{legend}");
            return;
        }
        let data = match crate::capture::CaptureData::load(&base.join("data")) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skip §2 sequence: {e}{legend}");
                return;
            }
        };
        let Some(layout) = data.layouts.get("relic").cloned() else {
            eprintln!("skip §2 sequence: no relic layout{legend}");
            return;
        };
        let Some(engine) = TesseractCliEngine::probe(layout) else {
            eprintln!("skip §2 sequence: no tesseract{legend}");
            return;
        };

        // Build an overlapping sequence: every shot appears twice (the
        // second is byte-identical, exercising the unchanged-frame skip)
        // and an untouched blank frame is sprinkled in (an unclassifiable
        // "menu" frame that must pause collection silently).
        let mut files: Vec<PathBuf> = std::fs::read_dir(seth_relic_dir())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map_or(false, |x| x == "png"))
            .collect();
        files.sort();
        assert!(files.len() >= 9, "expected Seth's 9 relic shots");

        let mut frames: Vec<GrayImage> = Vec::new();
        for (i, f) in files.iter().enumerate() {
            let bytes = std::fs::read(f).unwrap();
            let img = GrayImage::from_bytes(&bytes).unwrap();
            let blank = GrayImage::blank(img.width, img.height, 0);
            frames.push(img.clone());
            frames.push(blank); // unrelated frame between shots
            frames.push(img); // the same shot again (skip path)
            if i % 2 == 0 {
                frames.push(GrayImage::blank(64, 64, 0));
            }
        }
        // Include the real Arcanes screen too, so the sequence exercises
        // the arcanes layout + rank keys.
        let arcanes_png = base.join("fixtures/ocr-samples/seth-arcanes-2026-10-16.png");
        if let Ok(bytes) = std::fs::read(&arcanes_png) {
            if let Ok(img) = GrayImage::from_bytes(&bytes) {
                frames.push(img.clone());
                frames.push(img);
            }
        }

        let mut source = crate::capture::SequenceSource::new(frames);
        let stop = std::sync::atomic::AtomicBool::new(false);
        let ctx = data.context();
        let session = crate::capture::run_capture(
            &ctx,
            &engine,
            &mut source,
            &stop,
            0,
            |_| {},
        )
        .expect("sequence capture runs");

        assert!(session.has_any(), "classified at least one inventory screen");
        let got = session.stock();
        // The union over overlapping frames equals the one-shot §1 ingest
        // exactly: same tiers, same counts, no double counts.
        assert_eq!(
            got.relics, expected_relics.0.relics,
            "§2 relic union must equal the §1 validation result"
        );
        // And the same for the Arcanes screen.
        if let Some(expected_arc) = seth_arcanes_cached() {
            assert_eq!(
                got.arcanes, expected_arc.0.arcanes,
                "§2 arcane union must equal the §3 validation result"
            );
        }
        eprintln!(
            "§2 sequence union: {} relic stacks, {} arcanes (frames seen {}, read {}, skipped {})",
            got.relic_stack_count(),
            got.arcane_stack_count(),
            session.frames_seen(),
            session.frames_read(),
            session.frames_skipped(),
        );
    }

    // -----------------------------------------------------------------
    // Radius decoder (pass 29, addendum #92)
    // -----------------------------------------------------------------

    /// The REAL scoped parts universe from the committed data dir —
    /// the stress fixture is the shipped name set itself, so the
    /// radii and classes under test are the production ones.
    fn parts_universe() -> Universe {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        crate::capture::load_universe(&base.join("data"))
            .expect("load universe")
            .parts
    }

    #[test]
    fn radius_class_derives_the_neuroptics_blueprint_class() {
        let u = parts_universe();
        let class = u
            .radius_class("Mag Prime Neuroptics Blueprint")
            .expect("class exists");
        assert_eq!(class.suffix().join(" "), "prime neuroptics blueprint");
        assert_eq!(class.members().count(), 51);
        let get = |name: &str| {
            class
                .members()
                .find(|(n, _, _)| *n == name)
                .map(|(_, h, t)| (h.to_owned(), t))
        };
        // Distances measured on the real universe (NEUROPTICS_BRIEF
        // appendix): Mag's nearest head is Ash at 3, Chroma's is
        // Khora at 3, Ember's is Gyre at 4 — all radii 1.
        assert_eq!(get("Mag Prime Neuroptics Blueprint"), Some(("mag".into(), 1)));
        assert_eq!(
            get("Chroma Prime Neuroptics Blueprint"),
            Some(("chroma".into(), 1))
        );
        assert_eq!(
            get("Ember Prime Neuroptics Blueprint"),
            Some(("ember".into(), 1))
        );
        // Ball-disjointness inputs hold by construction: 2t + 1 <= D.
        for (name, head, t) in class.members() {
            let d = class
                .members()
                .filter(|(n2, _, _)| *n2 != name)
                .map(|(_, h2, _)| levenshtein_confusable_metric(head, h2))
                .min()
                .unwrap();
            assert!(2 * t + 1 <= d, "{name}: radius {t} but nearest head at {d}");
        }
        // A name with no shared suffix has no class.
        let solo = Universe::from_names(&["Unique Snowflake".to_string()]);
        assert!(solo.radius_class("Unique Snowflake").is_none());
    }

    #[test]
    fn radius_decoder_pinned_census_rows() {
        let u = parts_universe();
        // Margin refusals with best = truth whose fragments sit
        // inside the truth's radius (NEUROPTICS_BRIEF table).
        assert_eq!(
            u.radius_decode("Xmag Prime Neuroptics Blueprint").as_deref(),
            Some("Mag Prime Neuroptics Blueprint")
        );
        assert_eq!(
            u.radius_decode("Xnova Pritr Neuroptics Blueprint").as_deref(),
            Some("Nova Prime Neuroptics Blueprint")
        );
        assert_eq!(
            u.radius_decode("“Ember Pri Neuroptics Blueprint").as_deref(),
            Some("Ember Prime Neuroptics Blueprint")
        );
        // The Chroma/Khora limit, pinned honestly: "Bhioma" is a
        // 2-edit corruption of "chroma", whose radius is 1 (Khora
        // sits at distance 3). It stays refused — the decoder does
        // NOT recover this row, by design.
        assert_eq!(u.radius_decode("Bhioma Prime Neuroptics Blueprint"), None);
        // The counter-class: gear word destroyed, best margin
        // candidate a sibling (Nyx). Refused at tail alignment,
        // and the fragment decodes under NEITHER candidacy.
        assert_eq!(u.radius_decode("Inox Pring * europtics ” Blueprint"), None);
        let nyx = u.radius_class("Nyx Prime Neuroptics Blueprint").unwrap();
        assert_eq!(nyx.decode("inox"), None);
        let equinox = u
            .radius_class("Equinox Prime Neuroptics Blueprint")
            .unwrap();
        assert_eq!(equinox.decode("inox"), None);
        // The non-metric witness (the stress test's find): "1aros"
        // is ONE pairwise edit from both "inaros" and "lavos"
        // (`1` bridges `i` and `l`), so under the pairwise
        // distance their radius-1 balls would overlap. Under the
        // metric closure d(inaros, lavos) = 2, both radii are 0,
        // and the bridging fragment refuses — pinned so the
        // closure is never "simplified" back away.
        let class = u
            .radius_class("Inaros Prime Neuroptics Blueprint")
            .unwrap();
        let t_of = |name: &str| {
            class
                .members()
                .find(|(n, _, _)| *n == name)
                .map(|(_, _, t)| t)
                .unwrap()
        };
        assert_eq!(t_of("Inaros Prime Neuroptics Blueprint"), 0);
        assert_eq!(t_of("Lavos Prime Neuroptics Blueprint"), 0);
        assert_eq!(class.decode("1aros"), None);
        assert_eq!(
            class.decode("inaros").as_deref(),
            Some("Inaros Prime Neuroptics Blueprint")
        );
    }

    #[test]
    fn radius_fragment_requires_the_class_suffix() {
        let u = parts_universe();
        let neuro = u.radius_class("Mag Prime Neuroptics Blueprint").unwrap();
        // A chassis tail does not carry the neuroptics suffix: no
        // fragment, even though the head would decode — this is
        // the wrong-class guard (a "mag" head exists in both
        // classes; only the tail tells them apart).
        assert_eq!(
            u.radius_fragment("Mag Prime Chassis Blueprint", &neuro, RADIUS_TAIL_BUDGET),
            None
        );
        // The same read decodes in its OWN class.
        assert_eq!(
            u.radius_decode("Mag Prime Chassis Blueprint").as_deref(),
            Some("Mag Prime Chassis Blueprint")
        );
    }

    #[test]
    fn radius_strip_fragment_cuts_at_prime() {
        assert_eq!(radius_strip_fragment("“Mag Prime ¥"), "mag");
        assert_eq!(radius_strip_fragment("Dual Zoren Prine"), "dual zoren");
        assert_eq!(radius_strip_fragment("Xnova Pritr"), "xnova");
        assert_eq!(radius_strip_fragment("auinox"), "auinox");
        // Junk glyphs never survive the squash.
        assert_eq!(radius_strip_fragment("* Mag * Prime"), "mag");
    }

    #[test]
    fn radius_strip_decode_needs_the_band_class() {
        let u = parts_universe();
        // Band tail garbled beyond budget: the strip is never
        // consulted (a strip fragment alone cannot pick a class).
        assert_eq!(
            u.radius_decode_strip(
                "Inox Pring * europtics ” Blueprint",
                "Inox Prime",
                RADIUS_TAIL_BUDGET
            ),
            None
        );
        // Band read establishes + verifies the class; the strip
        // fragment decodes inside it.
        assert_eq!(
            u.radius_decode_strip(
                "Xmag Prime Neuroptics Blueprint",
                "“Mag Prime ¥",
                RADIUS_TAIL_BUDGET
            )
            .as_deref(),
            Some("Mag Prime Neuroptics Blueprint")
        );
    }

    #[test]
    fn radius_decoder_frame11_false_accepts_now_refuse() {
        let u = parts_universe();
        // Pass-29 gate, attempt 1: the only two false accepts in
        // 495 cells, both from the frame-parts-11 mid-scroll
        // composite, whose cells carry another item's line 1 only
        // (truths: Nezha / Nova Prime Systems Blueprint). The tail
        // witnesses "prime" and nothing else; the old alignment
        // charged a flat 3 for the wholly deleted distinguishing
        // token, "verified" the chassis class, and the fragment
        // decoded. The witnessed-token rule refuses both — asserted
        // at the production budget AND at a lax 8, pinning the
        // refusal as structural rather than a tuned threshold.
        for budget in [RADIUS_TAIL_BUDGET, 8] {
            assert_eq!(u.radius_decode_budget("Nezh rime", budget), None);
            assert_eq!(u.radius_decode_budget("Nova Prime", budget), None);
        }
        // Through the strip path too: the band tail never
        // verifies, so a strip fragment is never consulted.
        assert_eq!(
            u.radius_decode_strip("Nova Prime", "Nova Prime", RADIUS_TAIL_BUDGET),
            None
        );
        // The two reads exercise the two defences separately, and
        // both facts are pinned: "Nezh rime"'s best candidate
        // scores 0.47 — below the plausibility floor, so it never
        // nominates a class — while "Nova Prime"'s best candidate
        // is "Nova Prime Chassis" at 0.56, ABOVE the floor, and is
        // stopped by witnessing alone.
        let (_, nezh_score, _) = u.best_match_margin("Nezh rime").unwrap();
        assert!(
            nezh_score < RADIUS_MIN_CANDIDATE_SCORE,
            "Nezh rime best scores {nezh_score}, expected below the floor"
        );
        let (nova_name, nova_score, _) = u.best_match_margin("Nova Prime").unwrap();
        assert_eq!(nova_name, "Nova Prime Chassis");
        assert!(
            nova_score >= RADIUS_MIN_CANDIDATE_SCORE,
            "Nova Prime best scores {nova_score}, expected at/above the floor"
        );
    }

    #[test]
    fn radius_decoder_evidence_floor_boundary() {
        let u = parts_universe();
        // Lex is the shortest head of the Prime Receiver class
        // (t = 1 per the HC probes): its floor is max(3, 3 - 1)
        // = 3, so the clean head decodes while its 2-char
        // deletions — distance 1, formally in-radius — refuse.
        let receiver = u.radius_class("Lex Prime Receiver").expect("class exists");
        assert_eq!(
            receiver.decode("lex").as_deref(),
            Some("Lex Prime Receiver")
        );
        assert_eq!(receiver.decode("le"), None);
        assert_eq!(receiver.decode("lx"), None);
        assert_eq!(receiver.decode("ie"), None);
        // The same boundary on the tuned class: "mag" decodes,
        // "xmag" (the census's genuine junk-affixed fragment)
        // still decodes, "ma" refuses.
        let neuro = u
            .radius_class("Mag Prime Neuroptics Blueprint")
            .unwrap();
        assert_eq!(
            neuro.decode("mag").as_deref(),
            Some("Mag Prime Neuroptics Blueprint")
        );
        assert_eq!(
            neuro.decode("xmag").as_deref(),
            Some("Mag Prime Neuroptics Blueprint")
        );
        assert_eq!(neuro.decode("ma"), None);
    }

    #[test]
    fn radius_decoder_hc_artefact_fragments_now_refuse() {
        let u = parts_universe();
        // Addendum #95 (HC_REPORT.md §3): the shipped decoder's
        // first false accepts — five strip-phase cells in the
        // Prime Receiver class. The band reads' gear words are
        // destroyed (margin matcher refuses; the band decode
        // refuses), and the strip phase returned a 2-char
        // icon-edge artefact — identical across Knell, Hystrix
        // and Velox cells — that sat inside Lex's radius. Every
        // one of these shapes must now end REFUSED through the
        // public strip path.
        for (band, strip) in [
            ("Uo Prime Receiver", "Le"), // Knell, parts-05 #27
            ("a Prime Receiver", "Le"),  // Knell, parts-06 #27
            ("a Prime Receiver", "le"),  // Knell, parts-09 #27
            ("x Prime Receiver", "le"),  // Velox, parts-27 #24
            // Hystrix, parts-08 #16 — the worst case: the margin
            // best was the truth at 0.955, refused at margin
            // 0.098, and the strip converted that correct
            // refusal into a wrong hold.
            ("— H st ystrix Prime Receiver", "ie"),
        ] {
            assert_eq!(
                u.radius_decode_strip(band, strip, RADIUS_TAIL_BUDGET),
                None,
                "band {band:?} + strip {strip:?} must refuse (addendum #95)"
            );
        }
        // The genuine article through the same path and class: a
        // clean strip fragment of the short head still decodes —
        // the floor prices evidence, it does not ban short heads.
        assert_eq!(
            u.radius_decode_strip("x Prime Receiver", "Lex Prime", RADIUS_TAIL_BUDGET)
                .as_deref(),
            Some("Lex Prime Receiver")
        );
    }

    #[test]
    fn radius_tail_alignment_requires_witnessed_suffix_tokens() {
        let u = parts_universe();
        let class = u
            .radius_class("Mag Prime Neuroptics Blueprint")
            .expect("class exists");
        // Missing final suffix token: the fragment is perfect and
        // would decode, but "blueprint" is witnessed by nothing,
        // so there is no fragment at all.
        assert_eq!(
            u.radius_fragment("Mag Prime Neuroptics", &class, RADIUS_TAIL_BUDGET),
            None
        );
        // A missing MIDDLE token fails the same way ("neuroptics"
        // unwitnessed; the trailing "blueprint" cannot substitute
        // for it — witnesses are ordered).
        assert_eq!(
            u.radius_fragment("Mag Prime Blueprint", &class, RADIUS_TAIL_BUDGET),
            None
        );
        // A truncated final token within the witness cap still
        // verifies ("bluepri" sits at distance 2 from "blueprint")
        // and the read decodes through the public path.
        assert_eq!(
            u.radius_fragment("Mag Prime Neuroptics Bluepri", &class, RADIUS_TAIL_BUDGET)
                .as_deref(),
            Some("mag")
        );
        assert_eq!(
            u.radius_decode("Mag Prime Neuroptics Bluepri").as_deref(),
            Some("Mag Prime Neuroptics Blueprint")
        );
        // One junk token inside the tail is priced
        // (RADIUS_TOKEN_EDIT) but allowed: the suffix tokens
        // around it are all witnessed.
        assert_eq!(
            u.radius_fragment(
                "Mag Prime x9 Neuroptics Blueprint",
                &class,
                RADIUS_TAIL_BUDGET
            )
            .as_deref(),
            Some("mag")
        );
    }

    /// Tiny deterministic RNG (xorshift64*) for seeded corruptions —
    /// the stress test must be reproducible in CI.
    fn xorshift(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *state = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    #[test]
    fn bounded_levenshtein_agrees_with_unbounded() {
        let alpha: Vec<char> = "abcde01sbg£$|".chars().collect();
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut mk = |state: &mut u64| -> String {
            let n = (xorshift(state) % 15) as usize;
            (0..n)
                .map(|_| alpha[(xorshift(state) as usize) % alpha.len()])
                .collect()
        };
        for _ in 0..3000 {
            let a = mk(&mut state);
            let b = mk(&mut state);
            for (d, bd) in [
                (
                    levenshtein_confusable(&a, &b),
                    Box::new(|l: usize| levenshtein_confusable_bounded(&a, &b, l))
                        as Box<dyn Fn(usize) -> usize>,
                ),
                (
                    levenshtein_confusable_metric(&a, &b),
                    Box::new(|l: usize| levenshtein_confusable_metric_bounded(&a, &b, l))
                        as Box<dyn Fn(usize) -> usize>,
                ),
            ] {
                for limit in 0..=6usize {
                    let bd = bd(limit);
                    assert_eq!(
                        bd <= limit,
                        d <= limit,
                        "a={a:?} b={b:?} d={d} limit={limit} bounded={bd}"
                    );
                    if d <= limit {
                        assert_eq!(bd, d, "a={a:?} b={b:?} limit={limit}");
                    }
                }
            }
        }
    }

    /// Alphabet for synthetic corruptions: a–z plus the junk glyphs
    /// and digit confusions the census actually observed.
    const STRESS_ALPHABET: &[char] = &[
        'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r',
        's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '"', '“', '”', '‘', '*', '¥', '0', '1', '5', '8',
    ];

    fn one_edit_variants(head: &str) -> Vec<String> {
        let chars: Vec<char> = head.chars().collect();
        let mut out = Vec::new();
        for i in 0..chars.len() {
            let mut v = chars.clone();
            v.remove(i);
            out.push(v.iter().collect());
        }
        for i in 0..chars.len() {
            for &c in STRESS_ALPHABET {
                if c != chars[i] {
                    let mut v = chars.clone();
                    v[i] = c;
                    out.push(v.iter().collect());
                }
            }
        }
        for i in 0..=chars.len() {
            for &c in STRESS_ALPHABET {
                let mut v = chars.clone();
                v.insert(i, c);
                out.push(v.iter().collect());
            }
        }
        out
    }

    /// All substitutions of confusable twins (the zero-cost pairs of
    /// [`confusable`]) over subsets of the applicable positions.
    fn confusable_variants(head: &str) -> Vec<String> {
        fn twins(c: char) -> &'static [char] {
            match c {
                'o' => &['0'],
                'i' | 'l' => &['1', '|'],
                's' => &['5', '$'],
                'b' => &['8'],
                'z' => &['2'],
                'g' => &['6'],
                'e' => &['£'],
                '0' => &['o'],
                '1' => &['i', 'l'],
                '5' => &['s'],
                '8' => &['b'],
                '2' => &['z'],
                '6' => &['g'],
                _ => &[],
            }
        }
        let chars: Vec<char> = head.chars().collect();
        let positions: Vec<usize> = (0..chars.len())
            .filter(|&i| !twins(chars[i]).is_empty())
            .collect();
        let mut out = Vec::new();
        // Singles with every twin.
        for &i in &positions {
            for &t in twins(chars[i]) {
                let mut v = chars.clone();
                v[i] = t;
                out.push(v.iter().collect());
            }
        }
        // All subsets (first twin each) when the subset space is small.
        if positions.len() <= 6 {
            for mask in 1u32..(1 << positions.len()) {
                let mut v = chars.clone();
                for (bit, &i) in positions.iter().enumerate() {
                    if mask & (1 << bit) != 0 {
                        v[i] = twins(chars[i])[0];
                    }
                }
                out.push(v.iter().collect());
            }
        }
        out
    }

    fn junk_affix_variants(head: &str) -> Vec<String> {
        let mut out = Vec::new();
        for j in ['"', '“', '”', '*', '¥', 'x'] {
            out.push(format!("{j}{head}"));
            out.push(format!("{head}{j}"));
            out.push(format!("{j}{head}{j}"));
        }
        out
    }

    fn random_corruption(head: &str, edits: usize, state: &mut u64) -> String {
        let mut chars: Vec<char> = head.chars().collect();
        for _ in 0..edits {
            if chars.is_empty() {
                break;
            }
            let op = xorshift(state) % 3;
            let pos = (xorshift(state) as usize) % chars.len();
            match op {
                0 => chars[pos] = STRESS_ALPHABET[(xorshift(state) as usize) % STRESS_ALPHABET.len()],
                1 => {
                    chars.remove(pos);
                }
                _ => {
                    let c = STRESS_ALPHABET[(xorshift(state) as usize) % STRESS_ALPHABET.len()];
                    chars.insert(pos, c);
                }
            }
        }
        chars.iter().collect()
    }

    #[test]
    fn discriminative_tokens_are_the_head() {
        let u = parts_universe();
        // The head alone discriminates a name among its suffix
        // class; classless names keep every token.
        assert_eq!(
            u.discriminative_tokens_of("Mag Prime Neuroptics Blueprint")
                .unwrap(),
            ["mag"]
        );
        assert_eq!(
            u.discriminative_tokens_of("Bo Prime Handle").unwrap(),
            ["bo"]
        );
        assert_eq!(
            u.discriminative_tokens_of("Dual Zoren Prime Blade").unwrap(),
            ["dual", "zoren"]
        );
        assert_eq!(
            u.discriminative_tokens_of("Lex Prime Receiver").unwrap(),
            ["lex"]
        );
        assert!(u.discriminative_tokens_of("Not A Real Item").is_none());
    }

    #[test]
    fn hold_predicate_refuses_suffix_only_reads() {
        let u = parts_universe();
        // The Bo mechanism (pass 33, addendum #97): a read carrying
        // only a shared suffix witnesses no discriminative token of
        // any class member, so the shared hold predicate refuses
        // it for every member — including the junk shapes the
        // prototype's stage B held as Bo Prime Handle (truths
        // Fragor / Venato / Karyst: their gear words destroyed or
        // replaced by bleed, their suffixes intact).
        for read in [
            "Prime Handle",
            "= Prime Handle",
            "Ve to Prime Handle",
            "coh Prime Handle",
            "Prime Receiver",
            "Prime Neuroptics Blueprint",
        ] {
            for cand in [
                "Bo Prime Handle",
                "Fragor Prime Handle",
                "Lex Prime Receiver",
                "Mag Prime Neuroptics Blueprint",
            ] {
                assert!(
                    !u.hold_permitted(read, cand),
                    "{read:?} must not permit a hold of {cand}"
                );
            }
            let res = validate_read(
                &OcrRead {
                    text: read.to_owned(),
                    confidence: 1.0,
                    count: None,
                    rank: None,
                },
                &u,
                0.5,
                0.5,
                0,
            );
            assert!(!res.held, "{read:?} held {:?}", res.matched);
        }
        // Genuine reads witness their heads — exactly, junk-affixed,
        // and split across tokens.
        assert!(u.hold_permitted("Bo Prime Handle", "Bo Prime Handle"));
        assert!(u.hold_permitted(
            "Xmag Prime Neuroptics Blueprint",
            "Mag Prime Neuroptics Blueprint"
        ));
        assert!(u.hold_permitted(
            "Dual Zoren Prime Blade",
            "Dual Zoren Prime Blade"
        ));
        // Coverage (the property battery's Mesa shape): a read
        // missing a candidate token — "prime", "blueprint", any of
        // them — does not permit the hold, even with the head exact.
        assert!(!u.hold_permitted("Mesa Chassis", "Mesa Prime Chassis"));
        assert!(!u.hold_permitted(
            "Mesa Prime Chassis",
            "Mesa Prime Chassis Blueprint"
        ));
        assert!(!u.hold_permitted("Lex Prime", "Lex Prime Receiver"));
        // Noisy reads are not exact subsets, so the subset clause
        // stays silent and the matcher's dilution model rules: a
        // truncated suffix token ("bluepri", "P") does not block
        // the hold the production matcher makes.
        assert!(u.hold_permitted(
            "Mag Prime Neuroptics Bluepri",
            "Mag Prime Neuroptics Blueprint"
        ));
        assert!(u.hold_permitted(
            "Protea P Neuroptics Blueprint",
            "Protea Prime Neuroptics Blueprint"
        ));
        // A merged head token ("NidusPrime" for "Nidus Prime")
        // witnesses the head through the merged target.
        assert!(u.hold_permitted(
            "NidusPrime Systems Blueprint",
            "Nidus Prime Systems Blueprint"
        ));
        // ...but the read's own bare suffix token must not
        // witness a merged head+next target ("prime" ≁ "boprime"
        // as evidence for Bo): the pass-32 Bo shape stays refused.
        assert!(!u.hold_permitted("Ve to Prime Handle", "Bo Prime Handle"));
        // The "&" separator token is exempt from coverage (it
        // squashes to nothing; no read could witness it).
        assert!(u
            .names()
            .iter()
            .any(|n| n == "Mesa Prime Chassis" || n == "Silva & Aegis Prime Blade"));
        assert!(u.hold_permitted(
            "Silva & Aegis Prime Blade",
            "Silva & Aegis Prime Blade"
        ));
        // A sibling's head does not witness this candidate.
        assert!(!u.hold_permitted(
            "Mag Prime Neuroptics Blueprint",
            "Nova Prime Neuroptics Blueprint"
        ));
    }

    #[test]
    fn badge_ring_gate_detects_the_badge_and_refuses_the_glyph() {
        // Synthetic cell at census scale (182x181). The badge
        // window's origin inside the cell is (3, 0) and the
        // template's nominal offset within it is (10, 2).
        let mut cell = GrayImage::blank(182, 181, 5);
        for (y, row) in BADGE_TEMPLATE.iter().enumerate() {
            for (x, &v) in row.iter().enumerate() {
                cell.pixels[(2 + y) * 182 + 13 + x] = v;
            }
        }
        assert!(badge_ring_present(&cell), "the badge itself must pass");
        // Blank corner: nothing to detect.
        assert!(!badge_ring_present(&GrayImage::blank(182, 181, 5)));
        // A small bright ring — the built-copy glyph's circles are
        // a fraction of the badge's diameter — must not pass: the
        // glyph is not a count (pass 33 calibration put real glyph
        // cells at NCC <= 0.34 against the 0.50 floor).
        let mut glyph = GrayImage::blank(182, 181, 5);
        for y in 0..181 {
            for x in 0..182 {
                let (dx, dy) = (x as i32 - 20, y as i32 - 14);
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                if (d - 5.0).abs() < 1.5 {
                    glyph.pixels[y * 182 + x] = 159;
                }
            }
        }
        assert!(!badge_ring_present(&glyph), "a glyph circle is not the badge");
    }

    /// The pass-33 property test (brief §2.3, reviewer Q4): for
    /// every item in the parts universe, take its clean name as a
    /// read, delete each subset of its tokens (exhaustive — every
    /// name has ≤ 5 tokens, so ≤ 32 subsets), and run EVERY hold
    /// path: no path may ever return a different item on evidence
    /// that is merely partial. (Returning the item itself on
    /// partial evidence is the decoder working as designed; it is
    /// counted, not flagged.)
    ///
    /// One exception is forced by the universe's own structure,
    /// and the battery counts it separately rather than hiding it:
    /// a token subset of one item's name can BE another item's
    /// exact clean name — deleting "Blueprint" from "Loki Prime
    /// Systems Blueprint" yields "Loki Prime Systems" (the built
    /// form, first-class stock under #89 that must hold on its own
    /// name), and deleting the part word yields "Loki Prime
    /// Blueprint" (the gear's main blueprint, a different part).
    /// Such a read is indistinguishable from the sibling read
    /// cleanly, so the brief's property in its literal form is
    /// unsatisfiable over form siblings; the asserted property is
    /// the satisfiable one — a subset read that is NOT itself a
    /// complete item name never holds a different item. (This is
    /// also exactly why Pass B needs positive form evidence:
    /// token evidence alone cannot separate the forms.)
    ///
    /// Distinct reads are memoised — the paths are pure functions
    /// of the read — and the battery runs across the machine's
    /// cores. Pass 34 adds the fourth path: the line reader's
    /// text-level hold chain
    /// ([`crate::lineread::validate_line_read`]).
    #[test]
    fn hold_paths_never_hold_a_different_item_on_token_subsets() {
        use std::collections::HashMap;
        use std::sync::Mutex;
        let u = parts_universe();
        let names: Vec<String> = u.names().to_vec();
        assert!(names.len() > 500, "the real parts universe");
        let mut work: Vec<(usize, String)> = Vec::new(); // (item idx, subset read)
        let mut max_tokens = 0usize;
        for (idx, name) in names.iter().enumerate() {
            let toks: Vec<&str> = name.split_whitespace().collect();
            max_tokens = max_tokens.max(toks.len());
            assert!(toks.len() <= 6, "subset space stays tractable: {name}");
            for mask in 0..(1u32 << toks.len()) {
                let read = toks
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, t)| *t)
                    .collect::<Vec<_>>()
                    .join(" ");
                work.push((idx, read));
            }
        }
        let total_pairs = work.len();
        // Outcome per distinct read: (matcher hold, band decode,
        // strip decode, line-reader hold — the pass-34 path runs
        // the same text through the line reader's validation
        // chain: matcher, band-budget decode, assembled-as-strip
        // decode).
        type Outcome = (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let cache: Mutex<HashMap<String, Outcome>> = Mutex::new(HashMap::new());
        let violations: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let self_holds: Mutex<u64> = Mutex::new(0);
        let name_collisions: Mutex<u64> = Mutex::new(0);
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(8);
        let chunk = work.len().div_ceil(threads);
        std::thread::scope(|scope| {
            for part in work.chunks(chunk) {
                let (u, cache, violations, self_holds, names, name_collisions) = (
                    &u,
                    &cache,
                    &violations,
                    &self_holds,
                    &names,
                    &name_collisions,
                );
                scope.spawn(move || {
                    for (idx, read) in part {
                        let outcome = {
                            let hit = cache.lock().unwrap().get(read).cloned();
                            match hit {
                                Some(o) => o,
                                None => {
                                    let m = validate_read(
                                        &OcrRead {
                                            text: read.clone(),
                                            confidence: 1.0,
                                            count: None,
                                            rank: None,
                                        },
                                        u,
                                        0.5,
                                        0.5,
                                        0,
                                    );
                                    let lr = crate::lineread::validate_line_read(
                                        &OcrRead {
                                            text: read.clone(),
                                            confidence: 1.0,
                                            count: None,
                                            rank: None,
                                        },
                                        read,
                                        u,
                                        0.5,
                                        0.5,
                                        0,
                                        true,
                                        true,
                                        RADIUS_TAIL_BUDGET,
                                    );
                                    let o: Outcome = (
                                        if m.held { m.matched.clone() } else { None },
                                        u.radius_decode(read),
                                        u.radius_decode_strip(read, read, RADIUS_TAIL_BUDGET),
                                        if lr.held { lr.matched.clone() } else { None },
                                    );
                                    cache.lock().unwrap().insert(read.clone(), o.clone());
                                    o
                                }
                            }
                        };
                        let truth = &names[*idx];
                        for (path, got) in [
                            ("matcher", &outcome.0),
                            ("band decoder", &outcome.1),
                            ("strip decoder", &outcome.2),
                            ("line reader", &outcome.3),
                        ] {
                            match got {
                                Some(g) if g == truth => {
                                    *self_holds.lock().unwrap() += 1;
                                }
                                // The forced exception, documented
                                // on the test: the subset read IS
                                // the held item's exact clean name
                                // (a form sibling's — deleting
                                // "Blueprint" from a blueprint name
                                // yields the built form's name;
                                // deleting the part word yields
                                // the gear's own main blueprint).
                                // The read is indistinguishable
                                // from that item read cleanly, and
                                // built forms are first-class stock
                                // that MUST hold on their own name —
                                // no hold path can do otherwise.
                                Some(g) if fold(read) == fold(g) => {
                                    *name_collisions.lock().unwrap() += 1;
                                }
                                Some(g) => {
                                    violations.lock().unwrap().push(format!(
                                        "{path} held {g:?} on subset read {read:?} of {truth:?}"
                                    ));
                                }
                                None => {}
                            }
                        }
                    }
                });
            }
        });
        let distinct_reads = cache.lock().unwrap().len();
        let violations = violations.into_inner().unwrap();
        eprintln!(
            "property test: {} items, {} (item, subset) pairs (exhaustive subsets, max {} tokens/name), {} distinct reads x 4 hold paths, {} self-holds on partial evidence, {} exact-name collisions (forced: subset read is a form sibling's clean name), {} violations",
            names.len(),
            total_pairs,
            max_tokens,
            distinct_reads,
            self_holds.into_inner().unwrap(),
            name_collisions.into_inner().unwrap(),
            violations.len()
        );
        assert!(
            total_pairs > 5_000,
            "the battery exercised the subset space ({total_pairs} pairs)"
        );
        assert!(
            violations.is_empty(),
            "hold paths returned a different item:\n{}",
            violations
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// The gate's stress test (pass 29): for every multi-member
    /// suffix class of the real parts universe, corrupt fixture
    /// heads and assert the decoder's contract — a corruption
    /// within the truth's radius decodes to the truth (never a
    /// sibling: the balls are disjoint by construction, so an
    /// in-radius fragment has exactly one legal decode, and this
    /// test proves the implementation realises the geometry).
    /// Pass 32 adds the one designed exception: an in-radius
    /// variant shorter than the minimum-evidence floor
    /// (`max(3, |H| - t)`) must REFUSE — the floor is part of
    /// the shipped decoder, so the contract asserted here
    /// includes it. Such cases are counted and printed, never
    /// silently dropped from the battery. Out-of-radius
    /// corruptions carry no promise (a corruption that lands
    /// inside a sibling's ball is information-theoretically
    /// indistinguishable from that sibling read cleanly); their
    /// sibling-accept count is printed for the record.
    #[test]
    fn radius_decoder_stress_never_decodes_a_sibling_in_radius() {
        let u = parts_universe();
        // Distinct classes, keyed by suffix (membership is a
        // function of the suffix alone).
        let mut classes: Vec<RadiusClass> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for name in u.names() {
            if let Some(class) = u.radius_class(name) {
                if seen.insert(class.suffix().join(" ")) {
                    classes.push(class);
                }
            }
        }
        assert!(classes.len() >= 20, "fixture covers the real class structure");
        // The brief's named sibling classes get exhaustive 1-edit
        // coverage of EVERY member; small classes likewise; other
        // classes get exhaustive coverage of a seeded member sample
        // plus the full cheap battery on every member.
        const EXHAUSTIVE_SUFFIXES: &[&str] = &[
            "prime neuroptics blueprint",
            "prime systems blueprint",
            "prime chassis blueprint",
        ];
        let mut state = 0xC0FF_EE15_600D_F00Du64;
        let (mut decodes, mut in_radius, mut sibling_accepts, mut floor_refusals) =
            (0u64, 0u64, 0u64, 0u64);
        for class in &classes {
            let suffix = class.suffix().join(" ");
            let members: Vec<(&str, &str, usize)> = class.members().collect();
            // Construction invariant, asserted per class.
            for (name, head, t) in &members {
                let d = members
                    .iter()
                    .filter(|(n2, _, _)| n2 != name)
                    .map(|(_, h2, _)| levenshtein_confusable_metric(head, h2))
                    .min()
                    .unwrap();
                assert!(2 * t + 1 <= d, "{name}: radius {t}, nearest head {d}");
            }
            let exhaustive_all =
                EXHAUSTIVE_SUFFIXES.contains(&suffix.as_str()) || members.len() <= 12;
            // Decodes repeat heavily across members (junk and
            // confusable variants collide); memoise per class.
            let mut memo: HashMap<String, Option<String>> = HashMap::new();
            let mut decode = |class: &RadiusClass, memo: &mut HashMap<String, Option<String>>, v: &str| {
                memo.entry(v.to_owned())
                    .or_insert_with(|| class.decode(v))
                    .clone()
            };
            for (mi, (name, head, t)) in members.iter().enumerate() {
                let mut variants: Vec<String> = Vec::new();
                variants.extend(confusable_variants(head));
                variants.extend(junk_affix_variants(head));
                let sample = if members.len() > 100 { mi % 11 == 0 } else { mi % 5 == 0 };
                if exhaustive_all || sample {
                    variants.extend(one_edit_variants(head));
                }
                // Seeded random 2–3-edit corruptions.
                let random_n = if members.len() >= 30 { 60 } else { 40 };
                for _ in 0..random_n {
                    let edits = 2 + (xorshift(&mut state) % 2) as usize;
                    variants.push(random_corruption(head, edits, &mut state));
                }
                for v in &variants {
                    decodes += 1;
                    if v.is_empty() {
                        // The empty fragment is no evidence at
                        // all: decode refuses it by design, even
                        // when a short head's radius would
                        // formally contain it.
                        assert_eq!(decode(class, &mut memo, v), None);
                        continue;
                    }
                    let got = decode(class, &mut memo, v);
                    if levenshtein_confusable_metric(v, head) <= *t {
                        in_radius += 1;
                        // Minimum-evidence floor (pass 32): an
                        // in-radius variant below
                        // max(3, |H| - t) refuses BY DESIGN.
                        // (Within radius the length-relative term
                        // can never bind — distance <= t bounds
                        // the length difference — so this fires
                        // exactly on sub-3-char fragments of short
                        // heads.) Counted and printed below.
                        if v.chars().count()
                            < RADIUS_MIN_FRAGMENT_CHARS
                                .max(head.chars().count().saturating_sub(*t))
                        {
                            floor_refusals += 1;
                            assert_eq!(
                                got, None,
                                "below-floor in-radius corruption {v:?} of head {head:?} (t={t}) must refuse (pass 32 evidence floor)"
                            );
                        } else {
                            assert_eq!(
                                got.as_deref(),
                                Some(*name),
                                "in-radius corruption {v:?} of head {head:?} (t={t}) must decode to the truth"
                            );
                        }
                    } else if let Some(g) = &got {
                        if g != name {
                            sibling_accepts += 1;
                        }
                    }
                }
            }
            // Large classes additionally take a flat 10k seeded
            // random battery across random members.
            if members.len() >= 30 {
                for _ in 0..10_000 {
                    let (name, head, t) =
                        members[(xorshift(&mut state) as usize) % members.len()];
                    let edits = 2 + (xorshift(&mut state) % 2) as usize;
                    let v = random_corruption(head, edits, &mut state);
                    decodes += 1;
                    if v.is_empty() {
                        continue;
                    }
                    let got = decode(class, &mut memo, &v);
                    if levenshtein_confusable_metric(&v, head) <= t {
                        in_radius += 1;
                        // Pass 32 evidence floor, as in the
                        // per-member battery above.
                        if v.chars().count()
                            < RADIUS_MIN_FRAGMENT_CHARS.max(head.chars().count().saturating_sub(t))
                        {
                            floor_refusals += 1;
                            assert_eq!(
                                got, None,
                                "below-floor in-radius random corruption {v:?} of {head:?} (t={t}) must refuse (pass 32 evidence floor)"
                            );
                        } else {
                            assert_eq!(
                                got.as_deref(),
                                Some(name),
                                "in-radius random corruption {v:?} of {head:?} must decode to the truth"
                            );
                        }
                    } else if let Some(g) = &got {
                        if g != name {
                            sibling_accepts += 1;
                        }
                    }
                }
            }
        }
        eprintln!(
            "radius stress: {} classes, {} decodes, {} in-radius ({} decoded to truth, {} below-floor refusals by design), {} out-of-radius sibling accepts",
            classes.len(),
            decodes,
            in_radius,
            in_radius - floor_refusals,
            floor_refusals,
            sibling_accepts
        );
        assert!(in_radius > 10_000, "the battery exercised the radii");
    }
}




