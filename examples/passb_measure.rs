//! Pass B channel measurement (pass 34): run the production
//! stitch plan over a census, read every slot with BOTH readers
//! (band = `capture::read_cells`, line = `lineread::read_cells_lines`),
//! measure the three form channels on the canonical crops, and
//! dump one CSV row per slot joined to census truth. Analysis
//! (separation percentiles, certification error rates) is done
//! offline over the dump; nothing here changes production
//! behaviour.
//!
//! Usage:
//!   cargo run --release --example passb_measure -- \
//!     --frames ~/workspace/fracker-frames \
//!     --csv ~/workspace/fracker-frames/census/census_cells.csv \
//!     --data data [--legibility-join]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use fracker::capture::{read_cells, CaptureData};
use fracker::lineread;
use fracker::ocr::{GrayImage, TesseractCliEngine};
use fracker::stitch;

fn arg(args: &[String], key: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == key).map(|w| w[1].clone())
}

#[derive(Clone)]
struct Cell {
    frame: String,
    row: i32,
    col: u32,
    truth: String,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames_dir = PathBuf::from(arg(&args, "--frames").expect("--frames <dir>"));
    let csv_path = PathBuf::from(arg(&args, "--csv").expect("--csv <path>"));
    let data_dir = PathBuf::from(arg(&args, "--data").expect("--data <dir>"));
    let legibility_join = args.iter().any(|a| a == "--legibility-join");

    let csv = std::fs::read_to_string(&csv_path).expect("read census csv");
    let mut cells: Vec<Cell> = Vec::new();
    let mut frame_order: Vec<String> = Vec::new();
    for line in csv.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 6 {
            continue;
        }
        let frame = f[0].to_string();
        if !frame_order.contains(&frame) {
            frame_order.push(frame.clone());
        }
        cells.push(Cell {
            frame,
            row: f[2].parse().unwrap_or(0),
            col: f[3].parse().unwrap_or(0),
            truth: f[4].to_string(),
        });
    }
    frame_order.sort();

    // Load frames, replicating production's consecutive
    // byte-identical dedupe (placement indices count fed frames).
    let mut frames: Vec<GrayImage> = Vec::new();
    let mut session_index: HashMap<usize, usize> = HashMap::new();
    {
        let mut prev: Option<Vec<u8>> = None;
        let mut fed = 0usize;
        for (ci, name) in frame_order.iter().enumerate() {
            let bytes = std::fs::read(frames_dir.join(name))
                .unwrap_or_else(|e| panic!("read frame {name}: {e}"));
            if prev.as_deref() == Some(bytes.as_slice()) {
                session_index.insert(ci, fed - 1);
                continue;
            }
            frames.push(GrayImage::from_bytes(&bytes).expect("decode frame png"));
            session_index.insert(ci, fed);
            fed += 1;
            prev = Some(bytes);
        }
    }
    let refs: Vec<&GrayImage> = frames.iter().collect();

    let data = CaptureData::load(&data_dir).expect("load capture data");
    let ctx = data.context();
    let layout = ctx.layouts.get("parts").expect("parts layout").clone();
    let engine = TesseractCliEngine::probe(layout.clone()).expect("tesseract engine");
    let universe = ctx.universes.for_class(fracker::capture::ScreenClass::Parts);

    let plan = stitch::plan_session(&refs, &layout);

    // Placement join, identical to census_replay's session mode.
    let mut placement: HashMap<usize, (usize, i32)> = HashMap::new();
    for (si, seg) in plan.segments.iter().enumerate() {
        let phi_anchor = seg.frames.first().map(|p| p.phase).unwrap_or(0.0);
        for p in &seg.frames {
            let sr = ((p.g - phi_anchor) / seg.pitch.row_pitch).round() as i32;
            let delta = if legibility_join && p.phase > 0.235 * seg.pitch.row_pitch {
                -1
            } else {
                0
            };
            placement.insert(p.frame, (si, sr + delta));
        }
    }
    let mut slot_truth: HashMap<(usize, i32, u32), HashSet<String>> = HashMap::new();
    for cell in &cells {
        let fi_csv = frame_order.iter().position(|f| f == &cell.frame).unwrap();
        let fi = session_index[&fi_csv];
        let Some(&(si, sr)) = placement.get(&fi) else {
            continue;
        };
        slot_truth
            .entry((si, sr + cell.row, cell.col))
            .or_default()
            .insert(cell.truth.clone());
    }

    println!("seg,row,col,truth,band_held,line_held,line_bp,cert_canon,cert_any,mesh_grad,mesh_tophat,mesh_inst,glyph,nlines,leftover,shadow,assembled");
    for (si, seg) in plan.segments.iter().enumerate() {
        let slots = stitch::slot_instances(&plan, si, &refs, &layout);
        let cw = seg.pitch.col_pitch.round().max(1.0) as u32;
        let ch = seg.pitch.row_pitch.round().max(1.0) as u32;
        let mut canons: Vec<Option<stitch::CanonicalCrop>> = Vec::with_capacity(slots.len());
        let mut cell_imgs: Vec<Option<GrayImage>> = Vec::with_capacity(slots.len());
        for slot in &slots {
            let cc = stitch::canonical_crop(slot, &refs, cw, ch);
            cell_imgs.push(cc.as_ref().map(|c| c.image.clone()));
            canons.push(cc);
        }
        let mut inst_ranges: Vec<std::ops::Range<usize>> = Vec::with_capacity(slots.len());
        for slot in &slots {
            let usable: Vec<&stitch::Instance> =
                slot.instances.iter().filter(|i| i.usable()).collect();
            let start = cell_imgs.len();
            if usable.len() > 1 {
                for i in &usable {
                    cell_imgs.push(Some(stitch::crop_integer(
                        &frames[i.frame],
                        i.fx, i.fy, cw, ch,
                    )));
                }
            }
            inst_ranges.push(start..cell_imgs.len());
        }
        let band_results = read_cells(&cell_imgs, &layout, &engine, &ctx, universe);
        let line_outcomes = lineread::read_cells_lines(&cell_imgs, &layout, &engine, &ctx, universe);

        for (idx, slot) in slots.iter().enumerate() {
            let truth = slot_truth
                .get(&(si, slot.row, slot.col))
                .map(|s| {
                    if s.len() == 1 {
                        s.iter().next().unwrap().clone()
                    } else {
                        format!("DISAGREE{s:?}")
                    }
                })
                .unwrap_or_default();
            // Band agreement (pass 33 rule).
            let mut band_names: Vec<&str> = Vec::new();
            if band_results[idx].held {
                if let Some(m) = band_results[idx].matched.as_deref() {
                    band_names.push(m);
                }
            }
            for ri in inst_ranges[idx].clone() {
                if band_results[ri].held {
                    if let Some(m) = band_results[ri].matched.as_deref() {
                        if !band_names.contains(&m) {
                            band_names.push(m);
                        }
                    }
                }
            }
            let band_held = if band_names.len() == 1 { band_names[0] } else { "" };
            // Line agreement (same rule over line outcomes).
            let mut line_names: Vec<String> = Vec::new();
            let mut consider = |o: &lineread::LineOutcome| {
                if o.result.held {
                    if let Some(m) = o.result.matched.as_deref() {
                        if !line_names.iter().any(|n| n == m) {
                            line_names.push(m.to_owned());
                        }
                    }
                }
            };
            consider(&line_outcomes[idx]);
            for ri in inst_ranges[idx].clone() {
                consider(&line_outcomes[ri]);
            }
            let line_held = if line_names.len() == 1 { line_names[0].as_str() } else { "" };
            let lo = &line_outcomes[idx];
            let mut line_bp = lo.bp_line;
            let mut cert_any = lo.certified;
            for ri in inst_ranges[idx].clone() {
                line_bp = line_bp || line_outcomes[ri].bp_line;
                cert_any = cert_any || line_outcomes[ri].certified;
            }
            let (mesh_grad, mesh_tophat, glyph) = match &canons[idx] {
                Some(cc) => {
                    let ms = lineread::mesh_scores(&cc.image, &layout);
                    (ms.margin_grad, ms.margin_tophat, lineread::glyph_score(&cc.image))
                }
                None => (0.0, 0.0, 0.0),
            };
            // Per-instance mesh (median of margin_grad over the
            // instance crops; the canonical crop when the slot
            // has a single usable instance and no separate
            // instance crops were cut).
            let mesh_inst = {
                let range = inst_ranges[idx].clone();
                let mut scores: Vec<f32> = Vec::new();
                if range.is_empty() {
                    if let Some(cc) = &canons[idx] {
                        scores.push(lineread::mesh_scores(&cc.image, &layout).margin_grad);
                    }
                } else {
                    for ri in range {
                        if let Some(img) = &cell_imgs[ri] {
                            scores.push(lineread::mesh_scores(img, &layout).margin_grad);
                        }
                    }
                }
                scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                if scores.is_empty() {
                    0.0
                } else {
                    scores[scores.len() / 2]
                }
            };
            let assembled: String = lo
                .assembled
                .chars()
                .map(|c| {
                    if c == ',' || c == '"' || c == '\n' || c == '\r' {
                        ';'
                    } else {
                        c
                    }
                })
                .collect();
            println!(
                "{si},{},{},\"{}\",\"{}\",\"{}\",{},{},{},{:.3},{:.3},{:.3},{:.3},{},{:.3},{:.3},\"{}\"",
                slot.row,
                slot.col,
                truth,
                band_held,
                line_held,
                line_bp as u8,
                lo.certified as u8,
                cert_any as u8,
                mesh_grad,
                mesh_tophat,
                mesh_inst,
                glyph,
                lo.detection.boxes.len(),
                lo.detection.leftover_run_frac,
                lo.detection.shadow_below_frac,
                assembled
            );
        }
    }
}
