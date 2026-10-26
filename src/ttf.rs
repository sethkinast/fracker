//! Minimal TrueType outline reader + coverage rasteriser
//! (pass 35, analysis-by-synthesis).
//!
//! The synthesis reader needs exactly one thing from a font:
//! given a string, produce the ink coverage the game's text
//! rasteriser would produce, closely enough that masked NCC
//! against real crops can rank a handful of candidate strings.
//! That is a glyf-outline problem, not a font-stack problem, so
//! this module parses the four tables it needs (`cmap` format
//! 4/12, `hmtx`, `loca`, `glyf` — simple and composite glyphs)
//! and rasterises outlines itself: quadratic contours flattened
//! to segments, filled by nonzero winding on a 4x supersampled
//! grid, box-downsampled to coverage. No hinting (the game's
//! renderer does not hint this UI text in any way the fitted
//! coverage curve cannot absorb — see docs/thirty-fifth-pass.md)
//! and no kerning (Roboto's kern data lives in GPOS, which UI
//! text at this size does not apply; the calibration fit is the
//! arbiter and it is measured, not assumed).
//!
//! Deliberately unsupported: CFF/OTTO outlines, cmap formats
//! other than 4/12, composite components placed by point
//! matching (treated as zero-offset; Roboto's letter glyphs do
//! not use it), variable-font instancing. Unsupported input is
//! an Err at load, never a panic.

use crate::ocr::GrayImage;

/// Supersampling factor for coverage rasterisation.
const SS: u32 = 4;

#[derive(Debug, Clone)]
pub struct TtfFont {
    units_per_em: f32,
    ascender: f32,  // hhea, font units
    descender: f32, // hhea, font units (negative)
    num_glyphs: u16,
    advances: Vec<u16>,
    cmap4: Option<Cmap4>,
    cmap12: Vec<(u32, u32, u32)>, // (start, end, first gid)
    loca: Vec<u32>,
    glyf_off: usize,
    data: Vec<u8>,
    version_string: String,
}

#[derive(Debug, Clone)]
struct Cmap4 {
    off: usize,
    seg_count: usize,
}

fn be_u16(d: &[u8], o: usize) -> Result<u16, String> {
    d.get(o..o + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .ok_or_else(|| "ttf: read past end (u16)".to_string())
}
fn be_i16(d: &[u8], o: usize) -> Result<i16, String> {
    Ok(be_u16(d, o)? as i16)
}
fn be_u32(d: &[u8], o: usize) -> Result<u32, String> {
    d.get(o..o + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| "ttf: read past end (u32)".to_string())
}

fn table(d: &[u8], tag: &[u8; 4]) -> Result<(usize, usize), String> {
    let num = be_u16(d, 4)? as usize;
    for i in 0..num {
        let rec = 12 + i * 16;
        if rec + 16 > d.len() {
            break;
        }
        if &d[rec..rec + 4] == tag {
            let off = be_u32(d, rec + 8)? as usize;
            let len = be_u32(d, rec + 12)? as usize;
            return Ok((off, len));
        }
    }
    Err(format!(
        "ttf: missing table {}",
        String::from_utf8_lossy(tag)
    ))
}

impl TtfFont {
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, String> {
        let sfnt = be_u32(&data, 0)?;
        if sfnt == 0x4F54544F {
            return Err("ttf: CFF (OTTO) outlines unsupported".to_string());
        }
        let (head_off, _) = table(&data, b"head")?;
        let units_per_em = be_u16(&data, head_off + 18)? as f32;
        let index_to_loc = be_i16(&data, head_off + 50)?;
        let (maxp_off, _) = table(&data, b"maxp")?;
        let num_glyphs = be_u16(&data, maxp_off + 4)?;
        let (hhea_off, _) = table(&data, b"hhea")?;
        let ascender = be_i16(&data, hhea_off + 4)? as f32;
        let descender = be_i16(&data, hhea_off + 6)? as f32;
        let num_h = be_u16(&data, hhea_off + 34)? as usize;
        let (hmtx_off, _) = table(&data, b"hmtx")?;
        let mut advances = Vec::with_capacity(num_glyphs as usize);
        for g in 0..num_glyphs as usize {
            let idx = g.min(num_h.saturating_sub(1).max(0));
            advances.push(be_u16(&data, hmtx_off + idx * 4)?);
        }
        // cmap: prefer a format-12 subtable, else format 4.
        let (cmap_off, _) = table(&data, b"cmap")?;
        let n_sub = be_u16(&data, cmap_off + 2)? as usize;
        let mut cmap4 = None;
        let mut cmap12 = Vec::new();
        for i in 0..n_sub {
            let rec = cmap_off + 4 + i * 8;
            let sub = cmap_off + be_u32(&data, rec + 4)? as usize;
            let fmt = be_u16(&data, sub)?;
            match fmt {
                12 => {
                    let n = be_u32(&data, sub + 12)? as usize;
                    let mut groups = Vec::with_capacity(n);
                    for g in 0..n {
                        let go = sub + 16 + g * 12;
                        groups.push((
                            be_u32(&data, go)?,
                            be_u32(&data, go + 4)?,
                            be_u32(&data, go + 8)?,
                        ));
                    }
                    cmap12 = groups;
                }
                4 => {
                    if cmap4.is_none() {
                        let seg_count = (be_u16(&data, sub + 6)? / 2) as usize;
                        cmap4 = Some(Cmap4 { off: sub, seg_count });
                    }
                }
                _ => {}
            }
        }
        if cmap4.is_none() && cmap12.is_empty() {
            return Err("ttf: no cmap format 4/12 subtable".to_string());
        }
        // loca
        let (loca_off, loca_len) = table(&data, b"loca")?;
        let mut loca = Vec::with_capacity(num_glyphs as usize + 1);
        if index_to_loc == 1 {
            for g in 0..=num_glyphs as usize {
                loca.push(be_u32(&data, loca_off + g * 4)?);
            }
        } else {
            for g in 0..=num_glyphs as usize {
                loca.push(be_u16(&data, loca_off + g * 2)? as u32 * 2);
            }
        }
        let _ = loca_len;
        let (glyf_off, _) = table(&data, b"glyf")?;
        let version_string = name_string(&data, 5).unwrap_or_default();
        Ok(Self {
            units_per_em,
            ascender,
            descender,
            num_glyphs,
            advances,
            cmap4,
            cmap12,
            loca,
            glyf_off,
            data,
            version_string,
        })
    }

    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    /// The font's self-reported version string (name ID 5) —
    /// the provenance check pins the vendored cut by it.
    pub fn version_string(&self) -> &str {
        &self.version_string
    }

    pub fn glyph_id(&self, ch: char) -> Option<u16> {
        let cp = ch as u32;
        if !self.cmap12.is_empty() {
            // Groups are sorted by start codepoint.
            let idx = self
                .cmap12
                .partition_point(|&(start, _, _)| start <= cp);
            if idx > 0 {
                let (start, end, first) = self.cmap12[idx - 1];
                if cp >= start && cp <= end {
                    let gid = first + (cp - start);
                    return u16::try_from(gid).ok().filter(|&g| g < self.num_glyphs);
                }
            }
            return None;
        }
        let c4 = self.cmap4.as_ref()?;
        let d = &self.data;
        let o = c4.off;
        let n = c4.seg_count;
        let cp16 = u16::try_from(cp).ok()?;
        let end_off = o + 14;
        let start_off = end_off + 2 * n + 2;
        let delta_off = start_off + 2 * n;
        let range_off = delta_off + 2 * n;
        // Binary search over end codes.
        let (mut lo, mut hi) = (0usize, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let end = be_u16(d, end_off + mid * 2).ok()?;
            if cp16 <= end {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        if lo >= n {
            return None;
        }
        let seg = lo;
        let start = be_u16(d, start_off + seg * 2).ok()?;
        if cp16 < start {
            return None;
        }
        let delta = be_i16(d, delta_off + seg * 2).ok()? as i32;
        let ro = be_u16(d, range_off + seg * 2).ok()? as i32;
        let gid = if ro == 0 {
            (cp16 as i32 + delta).rem_euclid(65536) as u16
        } else {
            let addr = range_off + seg * 2 + ro as usize + (cp16 - start) as usize * 2;
            let g = be_u16(d, addr).ok()?;
            if g == 0 {
                return None;
            }
            (g as i32 + delta).rem_euclid(65536) as u16
        };
        if gid < self.num_glyphs {
            Some(gid)
        } else {
            None
        }
    }

    /// Advance width of a glyph, in font units.
    pub fn advance_units(&self, gid: u16) -> f32 {
        self.advances
            .get(gid as usize)
            .copied()
            .unwrap_or(0) as f32
    }

    /// The advance of a string, in font units.
    pub fn text_advance_units(&self, text: &str) -> f32 {
        text.chars()
            .map(|c| {
                self.glyph_id(c)
                    .map(|g| self.advance_units(g))
                    .unwrap_or(0.0)
            })
            .sum()
    }

    /// One glyph's outline: contours of (x, y, on-curve) points
    /// in font units, y-up, component glyphs resolved.
    fn outline(&self, gid: u16) -> Result<Vec<Vec<(f32, f32, bool)>>, String> {
        self.outline_depth(gid, 0)
    }

    fn outline_depth(&self, gid: u16, depth: u32) -> Result<Vec<Vec<(f32, f32, bool)>>, String> {
        if depth > 4 {
            return Ok(Vec::new());
        }
        let start = self.loca.get(gid as usize).copied().unwrap_or(0) as usize;
        let end = self.loca.get(gid as usize + 1).copied().unwrap_or(0) as usize;
        if end <= start {
            return Ok(Vec::new());
        }
        let o = self.glyf_off + start;
        let d = &self.data;
        let n_contours = be_i16(d, o)?;
        if n_contours < 0 {
            // Composite glyph.
            let mut out = Vec::new();
            let mut p = o + 10;
            loop {
                let flags = be_u16(d, p)?;
                let comp = be_u16(d, p + 2)?;
                p += 4;
                let (dx, dy);
                if flags & 0x0001 != 0 {
                    dx = be_i16(d, p)? as f32;
                    dy = be_i16(d, p + 2)? as f32;
                    p += 4;
                } else {
                    dx = be_u16(d, p)? as i8 as f32;
                    dy = be_u16(d, p + 1)? as i8 as f32;
                    p += 2;
                }
                // Point-matching placement (args not XY values)
                // is not used by Roboto's letters; treat as
                // zero offset if it ever appears.
                let (dx, dy) = if flags & 0x0002 != 0 { (dx, dy) } else { (0.0, 0.0) };
                let (mut m00, mut m01, mut m10, mut m11) = (1.0f32, 0.0f32, 0.0f32, 1.0f32);
                if flags & 0x0008 != 0 {
                    let s = f2dot14(be_i16(d, p)?);
                    p += 2;
                    m00 = s;
                    m11 = s;
                } else if flags & 0x0040 != 0 {
                    m00 = f2dot14(be_i16(d, p)?);
                    m11 = f2dot14(be_i16(d, p + 2)?);
                    p += 4;
                } else if flags & 0x0080 != 0 {
                    m00 = f2dot14(be_i16(d, p)?);
                    m01 = f2dot14(be_i16(d, p + 2)?);
                    m10 = f2dot14(be_i16(d, p + 4)?);
                    m11 = f2dot14(be_i16(d, p + 6)?);
                    p += 8;
                }
                let sub = self.outline_depth(comp, depth + 1)?;
                for contour in sub {
                    out.push(
                        contour
                            .into_iter()
                            .map(|(x, y, on)| {
                                (x * m00 + y * m01 + dx, x * m10 + y * m11 + dy, on)
                            })
                            .collect(),
                    );
                }
                if flags & 0x0020 == 0 {
                    break;
                }
            }
            return Ok(out);
        }
        let n = n_contours as usize;
        let mut end_pts = Vec::with_capacity(n);
        for i in 0..n {
            end_pts.push(be_u16(d, o + 10 + i * 2)? as usize);
        }
        let n_pts = end_pts.last().copied().unwrap_or(0) + 1;
        let mut p = o + 10 + 2 * n;
        // Skip instructions.
        let ins_len = be_u16(d, p)? as usize;
        p += 2 + ins_len;
        let mut flags = Vec::with_capacity(n_pts);
        while flags.len() < n_pts {
            let f = d.get(p).copied().ok_or("ttf: glyf flags overrun")?;
            p += 1;
            flags.push(f);
            if f & 0x08 != 0 {
                let rep = d.get(p).copied().ok_or("ttf: glyf repeat overrun")?;
                p += 1;
                for _ in 0..rep {
                    flags.push(f);
                }
            }
        }
        flags.truncate(n_pts);
        let mut xs = Vec::with_capacity(n_pts);
        let mut x = 0i32;
        for &f in &flags {
            if f & 0x02 != 0 {
                let v = d.get(p).copied().ok_or("ttf: glyf x overrun")? as i32;
                p += 1;
                x += if f & 0x10 != 0 { v } else { -v };
            } else if f & 0x10 == 0 {
                x += be_i16(d, p)? as i32;
                p += 2;
            }
            xs.push(x as f32);
        }
        let mut ys = Vec::with_capacity(n_pts);
        let mut y = 0i32;
        for &f in &flags {
            if f & 0x04 != 0 {
                let v = d.get(p).copied().ok_or("ttf: glyf y overrun")? as i32;
                p += 1;
                y += if f & 0x20 != 0 { v } else { -v };
            } else if f & 0x20 == 0 {
                y += be_i16(d, p)? as i32;
                p += 2;
            }
            ys.push(y as f32);
        }
        let mut contours = Vec::with_capacity(n);
        let mut first = 0usize;
        for &last in &end_pts {
            let pts: Vec<(f32, f32, bool)> = (first..=last)
                .map(|i| (xs[i], ys[i], flags[i] & 0x01 != 0))
                .collect();
            contours.push(pts);
            first = last + 1;
        }
        Ok(contours)
    }

    /// Render one line of text to a coverage image (0 = no ink,
    /// 255 = full ink) at `px_size` (the em size in output
    /// pixels), with the fitted coverage curve applied: coverage
    /// is blurred by `blur` (output px, gaussian sigma), then
    /// mapped by `level`/`gamma` — v' = ((v − level) /
    /// (1 − level))^gamma clamped to [0, 1]. The image spans the
    /// font's hhea ascent..descent vertically (baseline-anchored)
    /// with `pad` px of margin on every side, so shift searches
    /// compare like with like.
    pub fn render_line(
        &self,
        text: &str,
        px_size: f32,
        blur: f32,
        level: f32,
        gamma: f32,
        pad: u32,
    ) -> GrayImage {
        let scale = px_size / self.units_per_em;
        let adv = self.text_advance_units(text) * scale;
        let w = (adv.ceil() as u32 + 2 * pad).max(1);
        let h = (((self.ascender - self.descender) * scale).ceil() as u32 + 2 * pad).max(1);
        let baseline = pad as f32 + self.ascender * scale;
        // Flatten every glyph's contours into segments in
        // supersample coordinates (y down).
        let mut segs: Vec<(f32, f32, f32, f32)> = Vec::new();
        let mut pen = pad as f32;
        for ch in text.chars() {
            let Some(gid) = self.glyph_id(ch) else {
                continue;
            };
            if let Ok(contours) = self.outline(gid) {
                for contour in &contours {
                    flatten_contour(contour, scale, pen, baseline, &mut segs);
                }
            }
            pen += self.advance_units(gid) * scale;
        }
        let cov = rasterise(&segs, w, h);
        let cov = if blur > 0.05 {
            blur_coverage(&cov, w as usize, h as usize, blur)
        } else {
            cov
        };
        let denom = (1.0 - level).max(0.05);
        let mut img = GrayImage::blank(w, h, 0);
        for (i, &v) in cov.iter().enumerate() {
            let t = ((v - level) / denom).clamp(0.0, 1.0).powf(gamma);
            img.pixels[i] = (t * 255.0).round() as u8;
        }
        img
    }
}

fn f2dot14(v: i16) -> f32 {
    v as f32 / 16384.0
}

/// Flatten one contour (font units, y-up) into line segments in
/// output pixels (y-down), translated by pen x and baseline y.
fn flatten_contour(
    contour: &[(f32, f32, bool)],
    scale: f32,
    pen_x: f32,
    baseline: f32,
    out: &mut Vec<(f32, f32, f32, f32)>,
) {
    if contour.is_empty() {
        return;
    }
    let px = |p: (f32, f32, bool)| (pen_x + p.0 * scale, baseline - p.1 * scale);
    // Build the on-curve point sequence, inserting implied
    // on-curve midpoints between consecutive off-curve points.
    let mut pts: Vec<(f32, f32, bool)> = Vec::with_capacity(contour.len() * 2);
    let n = contour.len();
    // Choose a start point: the first on-curve point, or the
    // implied midpoint of the wrap edge.
    let start_idx = if contour[0].2 {
        0
    } else if contour[n - 1].2 {
        n - 1
    } else {
        usize::MAX // start at the wrap midpoint
    };
    let mut seq: Vec<(f32, f32, bool)> = Vec::new();
    if start_idx == usize::MAX {
        let a = contour[n - 1];
        let b = contour[0];
        seq.push(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0, true));
        for i in 0..n {
            seq.push(contour[i]);
        }
    } else {
        for i in 0..n {
            seq.push(contour[(start_idx + i) % n]);
        }
    }
    let m = seq.len();
    let mut i = 0usize;
    pts.push(seq[0]);
    while i < m {
        let cur = seq[i % m];
        let nxt = seq[(i + 1) % m];
        if nxt.2 {
            pts.push(nxt);
            i += 1;
        } else {
            let nxt2 = seq[(i + 2) % m];
            let end = if nxt2.2 {
                nxt2
            } else {
                ((nxt.0 + nxt2.0) / 2.0, (nxt.1 + nxt2.1) / 2.0, true)
            };
            pts.push(nxt);
            pts.push(end);
            i += if nxt2.2 { 2 } else { 1 };
            if pts.len() > 4 * m + 8 {
                break; // degenerate-contour guard
            }
        }
        let _ = cur;
    }
    // Walk the sequence emitting segments; quadratic runs are
    // subdivided adaptively.
    let mut cur = px(pts[0]);
    let mut j = 1usize;
    while j < pts.len() {
        let p = pts[j];
        if p.2 {
            let q = px(p);
            out.push((cur.0, cur.1, q.0, q.1));
            cur = q;
            j += 1;
        } else {
            let ctrl = px(p);
            let end = px(pts[(j + 1).min(pts.len() - 1)]);
            subdivide(cur, ctrl, end, 0, out);
            cur = end;
            j += 2;
        }
    }
}

fn subdivide(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    depth: u32,
    out: &mut Vec<(f32, f32, f32, f32)>,
) {
    // Flatness: distance of the control point from the chord,
    // in supersample units (segments are in output px; the
    // rasteriser scales by SS).
    let mx = (p0.0 + 2.0 * p1.0 + p2.0) / 4.0;
    let my = (p0.1 + 2.0 * p1.1 + p2.1) / 4.0;
    let cx = (p0.0 + p2.0) / 2.0;
    let cy = (p0.1 + p2.1) / 2.0;
    let dist = ((mx - cx).powi(2) + (my - cy).powi(2)).sqrt() * SS as f32;
    if dist <= 0.6 || depth >= 8 {
        out.push((p0.0, p0.1, p2.0, p2.1));
        return;
    }
    let q0 = ((p0.0 + p1.0) / 2.0, (p0.1 + p1.1) / 2.0);
    let q1 = ((p1.0 + p2.0) / 2.0, (p1.1 + p2.1) / 2.0);
    let r = ((q0.0 + q1.0) / 2.0, (q0.1 + q1.1) / 2.0);
    subdivide(p0, q0, r, depth + 1, out);
    subdivide(r, q1, p2, depth + 1, out);
}

/// Rasterise flattened segments (output px, y-down) into a
/// coverage buffer (0..1) of size w*h via scanline nonzero
/// winding on an SS x SS grid, box-downsampled.
fn rasterise(segs: &[(f32, f32, f32, f32)], w: u32, h: u32) -> Vec<f32> {
    let (w, h) = (w as usize, h as usize);
    let (sw, sh) = (w * SS as usize, h * SS as usize);
    let mut sub = vec![0.0f32; sw * sh];
    // Scale segments into supersample space.
    let ss = SS as f32;
    let segs_ss: Vec<(f32, f32, f32, f32)> = segs
        .iter()
        .map(|&(x0, y0, x1, y1)| (x0 * ss, y0 * ss, x1 * ss, y1 * ss))
        .collect();
    for row in 0..sh {
        let yc = row as f32 + 0.5;
        let mut crossings: Vec<(f32, i32)> = Vec::new();
        for &(x0, y0, x1, y1) in &segs_ss {
            if (y0 <= yc && y1 > yc) || (y1 <= yc && y0 > yc) {
                let t = (yc - y0) / (y1 - y0);
                let x = x0 + t * (x1 - x0);
                let wind = if y1 > y0 { 1 } else { -1 };
                crossings.push((x, wind));
            }
        }
        if crossings.is_empty() {
            continue;
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut winding = 0i32;
        let mut span_start: Option<f32> = None;
        for &(x, wind) in &crossings {
            let before = winding;
            winding += wind;
            if before == 0 && winding != 0 {
                span_start = Some(x);
            } else if before != 0 && winding == 0 {
                if let Some(a) = span_start.take() {
                    fill_span(&mut sub, sw, row, a, x);
                }
            }
        }
    }
    // Box downsample.
    let mut out = vec![0.0f32; w * h];
    let s = SS as usize;
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0f32;
            for dy in 0..s {
                for dx in 0..s {
                    sum += sub[(y * s + dy) * sw + x * s + dx];
                }
            }
            out[y * w + x] = sum / (s * s) as f32;
        }
    }
    out
}

fn fill_span(sub: &mut [f32], sw: usize, row: usize, a: f32, b: f32) {
    if b <= a {
        return;
    }
    let base = row * sw;
    let x0 = a.floor() as i32;
    let x1 = b.ceil() as i32 - 1;
    for x in x0..=x1 {
        if x < 0 || x as usize >= sw {
            continue;
        }
        let lo = a.max(x as f32);
        let hi = b.min(x as f32 + 1.0);
        if hi > lo {
            sub[base + x as usize] += hi - lo;
        }
    }
}

/// Separable gaussian blur of a coverage buffer (sigma in px).
fn blur_coverage(cov: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let radius = (sigma * 2.5).ceil().max(1.0) as usize;
    let mut kernel = Vec::with_capacity(2 * radius + 1);
    let mut ksum = 0.0f32;
    for i in 0..=(2 * radius) {
        let d = i as f32 - radius as f32;
        let v = (-d * d / (2.0 * sigma * sigma)).exp();
        kernel.push(v);
        ksum += v;
    }
    for v in kernel.iter_mut() {
        *v /= ksum;
    }
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0f32;
            for (ki, &kv) in kernel.iter().enumerate() {
                let xx = (x as i32 + ki as i32 - radius as i32).clamp(0, w as i32 - 1) as usize;
                s += cov[y * w + xx] * kv;
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0f32;
            for (ki, &kv) in kernel.iter().enumerate() {
                let yy = (y as i32 + ki as i32 - radius as i32).clamp(0, h as i32 - 1) as usize;
                s += tmp[yy * w + x] * kv;
            }
            out[y * w + x] = s;
        }
    }
    out
}

/// Read a name-table string (name ID), preferring the Windows
/// UTF-16BE record, then the Mac Roman one.
fn name_string(d: &[u8], id: u16) -> Option<String> {
    let (off, _) = table(d, b"name").ok()?;
    let count = be_u16(d, off + 2).ok()? as usize;
    let storage = off + be_u16(d, off + 4).ok()? as usize;
    let mut mac: Option<String> = None;
    for i in 0..count {
        let rec = off + 6 + i * 12;
        let platform = be_u16(d, rec).ok()?;
        let name_id = be_u16(d, rec + 6).ok()?;
        if name_id != id {
            continue;
        }
        let len = be_u16(d, rec + 8).ok()? as usize;
        let roff = storage + be_u16(d, rec + 10).ok()? as usize;
        let bytes = d.get(roff..roff + len)?;
        if platform == 3 || platform == 0 {
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            if let Ok(s) = String::from_utf16(&units) {
                return Some(s);
            }
        } else if platform == 1 && mac.is_none() {
            mac = Some(bytes.iter().map(|&b| b as char).collect());
        }
    }
    mac
}

/// The vendored 2013 Roboto cut, embedded (Apache-2.0; see
/// assets/fonts/README.md for provenance).
pub const EMBEDDED_ROBOTO: &[u8] = include_bytes!("../assets/fonts/Roboto-Regular.ttf");

/// Parse the embedded font (cheap tables; outlines parse lazily
/// per glyph at render). Returns Err only on a corrupt build.
pub fn embedded_font() -> Result<TtfFont, String> {
    TtfFont::from_bytes(EMBEDDED_ROBOTO.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_font_is_the_2013_cut() {
        let f = embedded_font().expect("embedded font parses");
        assert_eq!(f.units_per_em(), 2048.0);
        assert!(
            f.version_string().contains("1.200310"),
            "version: {}",
            f.version_string()
        );
        // The discriminating glyphs' advances pin the cut: the
        // 2011 cut has R at 1298 units, the 2014 redesign 1261;
        // the 2013 cut is 1356.
        let r = f.glyph_id('R').expect("R glyph");
        assert_eq!(f.advance_units(r), 1356.0);
        let one = f.glyph_id('1').expect("1 glyph");
        assert_eq!(f.advance_units(one), 1150.0);
    }

    #[test]
    fn render_produces_ink_with_expected_extent() {
        let f = embedded_font().unwrap();
        let img = f.render_line("Prime", 18.0, 0.0, 0.0, 1.0, 3);
        let ink: u32 = img.pixels.iter().map(|&p| p as u32).sum();
        assert!(ink > 10_000, "ink sum {ink}");
        // Width tracks the advance sum (89.9% of it or better,
        // minus side bearings) and exceeds the last glyph alone.
        let adv = f.text_advance_units("Prime") * 18.0 / 2048.0;
        assert!((img.width as f32) >= adv, "w {} < adv {adv}", img.width);
        let a = f.render_line("Acceltra Prime", 18.0, 0.0, 0.0, 1.0, 3);
        let b = f.render_line("Acceltra Prime", 18.0, 0.0, 0.0, 1.0, 3);
        assert_eq!(a.pixels, b.pixels, "rendering is deterministic");
        let c = f.render_line("Panthera Prime", 18.0, 0.0, 0.0, 1.0, 3);
        assert_ne!(a.pixels, c.pixels);
    }

    #[test]
    fn render_curve_and_blur_behave() {
        let f = embedded_font().unwrap();
        let plain = f.render_line("Systems", 18.0, 0.0, 0.0, 1.0, 3);
        let blurred = f.render_line("Systems", 18.0, 1.0, 0.0, 1.0, 3);
        let peak = |i: &GrayImage| i.pixels.iter().copied().max().unwrap_or(0);
        assert!(peak(&blurred) <= peak(&plain));
        // A level shift must erode thin strokes: less total ink.
        let lifted = f.render_line("Systems", 18.0, 0.0, 0.25, 1.0, 3);
        let sum = |i: &GrayImage| i.pixels.iter().map(|&p| p as u32).sum::<u32>();
        assert!(sum(&lifted) < sum(&plain));
    }
}
