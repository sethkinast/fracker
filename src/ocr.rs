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
fn crop_region(cell: &GrayImage, region: [f32; 4]) -> GrayImage {
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
        let wl = match layout.count_style {
            CountStyle::XPrefix => "xX0123456789",
            CountStyle::Badge => "0123456789",
        };
        let todo: Vec<(u64, GrayImage)> = counts
            .iter()
            .filter(|(k, _)| !cache.counts.contains_key(k))
            .cloned()
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
                        count: cache.counts[&counts[i].0],
                        rank: self.read_rank(cell, layout),
                    };
                    i += 1;
                    read
                })
            })
            .collect()
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
    /// Built suffix classes for the radius decoder (pass 29), keyed
    /// by suffix. Building a class costs a pairwise pass over its
    /// members' heads; a capture session refuses the same stacks
    /// frame after frame, so each class is computed once and shared.
    radius_cache: Arc<Mutex<HashMap<String, RadiusClass>>>,
}

impl Universe {
    pub fn from_names(names: &[String]) -> Self {
        Self {
            names: names.to_vec(),
            folded: names.iter().map(|n| fold(n)).collect(),
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
    };
    if read.confidence < min_confidence {
        return out;
    }
    if let Some((name, score, second)) = universe.best_match_margin(&read.text) {
        out.matched = Some(name.to_owned());
        out.match_score = score;
        out.held = score >= min_score && (score - second) >= MATCH_MARGIN;
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
        class.decode(&fragment)
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
        // Tail verification on the band read is the class evidence;
        // its own fragment may be too damaged to decode (that is
        // why the strip exists), but the suffix must check out.
        self.radius_fragment(band_read, &class, budget)?;
        let fragment = radius_strip_fragment(strip_read);
        class.decode(&fragment)
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
        for read in reads {
            let index = results.len();
            // Partial cell at the page edge: never read, never held.
            let read = read.unwrap_or_else(OcrRead::empty);
            results.push(validate_read(
                &read,
                universe,
                min_confidence,
                layout.min_score,
                index,
            ));
        }
    }
    let stock = StockState::from_results(&results, relic_names, arcane_names);
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




