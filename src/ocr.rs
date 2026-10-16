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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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
}

impl OcrRead {
    pub fn empty() -> Self {
        Self {
            text: String::new(),
            confidence: 0.0,
            count: None,
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
                });
            }
        }
        if Self::runs("tesseract") {
            return Some(Self {
                binary: "tesseract".to_owned(),
                origin: "PATH",
                layout,
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
        cmd.arg(&path).arg("stdout").arg("--psm").arg(psm.to_string());
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

    fn read_count(&self, cell: &GrayImage) -> Option<u32> {
        let whitelist = match self.layout.count_style {
            CountStyle::XPrefix => "xX0123456789",
            CountStyle::Badge => "0123456789",
        };
        let raw = self.read_region(
            cell,
            self.layout.count,
            self.layout.count_psm,
            self.layout.count_upscale,
            Some(whitelist),
        );
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        // Both screens omit the label for a single item; anything <= 1 is
        // treated as "one", never as a spurious reading.
        digits.parse::<u32>().ok().filter(|&n| n > 1)
    }
}

impl OcrEngine for TesseractCliEngine {
    fn name(&self) -> &str {
        "tesseract-cli"
    }

    fn ocr_cell(&self, cell: &GrayImage) -> OcrRead {
        let text = self.read_region(
            cell,
            self.layout.name,
            self.layout.name_psm,
            self.layout.name_upscale,
            None,
        );
        let confidence = if text.is_empty() { 0.0 } else { 0.85 };
        OcrRead {
            text,
            confidence,
            count: self.read_count(cell),
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
            };
        }
        OcrRead {
            text,
            confidence: conf,
            count: None,
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
    pub fn best_match_margin(&self, read: &str) -> Option<(&str, f32, f32)> {
        let f = fold(strip_relic_suffix(read));
        if f.is_empty() {
            return None;
        }
        let mut best: Option<(&str, f32)> = None;
        let mut second = 0.0f32;
        for (name, folded) in self.names.iter().zip(self.folded.iter()) {
            let d = levenshtein_confusable(&f, folded);
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
    )
}

/// Levenshtein with [`confusable`] substitutions costing zero.
pub fn levenshtein_confusable(a: &str, b: &str) -> usize {
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
            let sub = prev[j] + usize::from(!confusable(ca, cb));
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
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
        }
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
                let add = r.count.unwrap_or(1);
                if relic_names.contains(name) {
                    *stock.relics.entry(name.clone()).or_insert(0) += add;
                } else {
                    *stock.parts.entry(name.clone()).or_insert(0) += add;
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
        for cell in layout.split(&img) {
            let index = results.len();
            match cell {
                Some(cell) => {
                    let read = engine.ocr_cell(&cell);
                    results.push(validate_read(
                        &read,
                        universe,
                        min_confidence,
                        layout.min_score,
                        index,
                    ));
                }
                // Partial cell at the page edge: never read, never held.
                None => results.push(validate_read(
                    &OcrRead::empty(),
                    universe,
                    min_confidence,
                    layout.min_score,
                    index,
                )),
            }
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
                count: None,
            }, &u, 0.5, 0.8, 0);
        assert!(exact.held);
        assert_eq!(exact.matched.as_deref(), Some("Neo C5"));
        // Typo still resolves (fuzzy), below-perfect score.
        let typo = validate_read(&OcrRead {
                text: "Ivara Prime Chassis Blueprlnt".into(),
                confidence: 0.9,
                count: None,
            }, &u, 0.5, 0.8, 1);
        assert!(typo.held, "typo resolves: {typo:?}");
        assert!(typo.match_score < 1.0);
        // Garbage never validates.
        let garbage = validate_read(&OcrRead {
                text: "zzz qqq www".into(),
                confidence: 0.9,
                count: None,
            }, &u, 0.5, 0.8, 2);
        assert!(!garbage.held);
        assert!(garbage.matched.is_some()); // best effort named, but NOT held
        // Low confidence never validates even when exact.
        let shy = validate_read(&OcrRead {
                text: "Neo C5".into(),
                confidence: 0.2,
                count: None,
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
                    count: None,
                }, &u, 0.5, 0.8, 0),
            validate_read(&OcrRead {
                    text: "Neo C5".into(),
                    confidence: 0.9,
                    count: Some(3),
                }, &u, 0.5, 0.8, 1),
            validate_read(&OcrRead {
                    text: "Forma Blueprint".into(),
                    confidence: 0.9,
                    count: None,
                }, &u, 0.5, 0.8, 2),
            validate_read(&OcrRead {
                    text: "zzz".into(),
                    confidence: 0.9,
                    count: None,
                }, &u, 0.5, 0.8, 3),
        ];
        let stock = StockState::from_results(&reads, &rel);
        assert_eq!(stock.relics.get("Neo C5"), Some(&4));
        assert_eq!(stock.parts.get("Forma Blueprint"), Some(&1));
        // Planner shape: known relic stock.
        match stock.to_relic_stock() {
            crate::model::RelicStock::Known(m) => assert_eq!(m.get("Neo C5"), Some(&4)),
            crate::model::RelicStock::Unknown => panic!("stock must be known"),
        }
        let json = stock.to_json().unwrap();
        let back = StockState::from_json(&json).unwrap();
        assert_eq!(back.relics.get("Neo C5"), Some(&4));
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
            let r = validate_read(
                &OcrRead {
                    text: label.to_string(),
                    confidence: 0.9,
                    count: None,
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

    /// Universe + relic-name set, exactly as `run_ocr_ingest` builds them.
    fn seed_universe() -> Option<(Universe, HashSet<String>)> {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let rel_text = std::fs::read_to_string(base.join("data/cache/relictables.json")).ok()?;
        let wish_text = std::fs::read_to_string(base.join("data/cache/wishlist.json")).ok()?;
        let relics = crate::model::RelicTables::from_json(&rel_text).ok()?;
        let roster = crate::model::Roster::from_json(&wish_text).ok()?;
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
        names.sort();
        names.dedup();
        let relic_names: HashSet<String> = relics.relics.keys().cloned().collect();
        Some((Universe::from_names(&names), relic_names))
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
        let (universe, relic_names) = seed_universe()?;
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
        let res = ingest_folder(&dir, &engine, &layout, &universe, &relic_names, 0.5);
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
}
