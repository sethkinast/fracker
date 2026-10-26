//! Census replay (pass 29 gate): drive the production capture path
//! (`CaptureData` + `CaptureSession`-equivalent `read_frame`) over
//! the census frames and score per-cell holds against the census
//! ground truth.
//!
//! Usage:
//!   cargo run --example census_replay -- \
//!     --frames ~/workspace/fracker-frames \
//!     --csv ~/workspace/fracker-frames/census/census_cells.csv \
//!     --data data \
//!     [--decoder on|off] [--strip on|off] [--budget N]
//!
//! The decoder/strip/budget switches map onto `CaptureContext`
//! fields, so the A/B runs the exact production code with only the
//! gated feature toggled. Output: per-frame held/correct/false
//! counts, union totals, the Neuroptics subset, and mean read
//! wall time per frame.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use fracker::capture::{read_frame, CaptureData, CaptureSession};
use fracker::ocr::{GrayImage, TesseractCliEngine};

fn arg(args: &[String], key: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == key)
        .map(|w| w[1].clone())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames_dir = PathBuf::from(arg(&args, "--frames").expect("--frames <dir>"));
    let csv_path = PathBuf::from(arg(&args, "--csv").expect("--csv <path>"));
    let data_dir = PathBuf::from(arg(&args, "--data").expect("--data <dir>"));
    if args.iter().any(|a| a == "--session") {
        session_main(
            &frames_dir,
            &csv_path,
            &data_dir,
            args.iter().any(|a| a == "--legibility-join"),
        );
        return;
    }
    let decoder = arg(&args, "--decoder").as_deref() != Some("off");
    let strip = arg(&args, "--strip").as_deref() != Some("off");
    let budget: usize = arg(&args, "--budget")
        .and_then(|v| v.parse().ok())
        .unwrap_or(fracker::ocr::RADIUS_TAIL_BUDGET);

    // Ground truth: frame -> idx -> truth (fields: frame,idx,row,
    // col,truth,... — part names contain no commas).
    let csv = std::fs::read_to_string(&csv_path).expect("read census csv");
    let mut truth: BTreeMap<String, HashMap<usize, String>> = BTreeMap::new();
    for line in csv.lines().skip(1) {
        let mut it = line.split(',');
        let frame = it.next().unwrap_or_default().to_string();
        let idx: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let _row = it.next();
        let _col = it.next();
        let name = it.next().unwrap_or_default().to_string();
        truth.entry(frame).or_default().insert(idx, name);
    }
    let total_cells: usize = truth.values().map(|m| m.len()).sum();
    println!(
        "census: {} frames, {} cells; decoder={} strip={} budget={}",
        truth.len(),
        total_cells,
        decoder,
        strip,
        budget
    );

    let data = CaptureData::load(&data_dir).expect("load capture data");
    let mut ctx = data.context();
    ctx.radius_decoder = decoder;
    ctx.radius_strip = strip;
    ctx.radius_tail_budget = budget;
    let layout = ctx
        .layouts
        .get("parts")
        .expect("parts layout")
        .clone();
    let engine = TesseractCliEngine::probe(layout.clone()).expect("tesseract engine");
    let universe = ctx.universes.for_class(fracker::capture::ScreenClass::Parts);

    let mut union_correct: HashSet<String> = HashSet::new();
    let mut union_false: HashMap<String, String> = HashMap::new(); // matched -> truth
    let (mut cells_correct, mut cells_false) = (0usize, 0usize);
    let (mut neuro_total, mut neuro_correct, mut neuro_false) = (0usize, 0usize, 0usize);
    let mut neuro_distinct: HashSet<String> = HashSet::new();
    let mut total_read_ms = 0f64;

    for (frame_name, cells) in &truth {
        let bytes = std::fs::read(frames_dir.join(frame_name))
            .unwrap_or_else(|e| panic!("read frame {frame_name}: {e}"));
        let img = GrayImage::from_bytes(&bytes).expect("decode frame png");
        let class = ctx.classifier.classify(&engine, &img);
        if class != fracker::capture::ScreenClass::Parts {
            println!("{frame_name}: classified {class:?}, SKIPPED");
            continue;
        }
        let t0 = Instant::now();
        let results = read_frame(&layout, &img, &engine, &ctx, universe);
        total_read_ms += t0.elapsed().as_secs_f64() * 1000.0;
        let (mut held, mut correct) = (0usize, 0usize);
        for (idx, want) in cells {
            let Some(r) = results.get(*idx) else {
                continue;
            };
            let is_neuro = want.contains("Neuroptics");
            if is_neuro {
                neuro_total += 1;
            }
            if r.held {
                held += 1;
                if r.matched.as_deref() == Some(want.as_str()) {
                    correct += 1;
                    cells_correct += 1;
                    union_correct.insert(want.clone());
                    if is_neuro {
                        neuro_correct += 1;
                        neuro_distinct.insert(want.clone());
                    }
                } else {
                    cells_false += 1;
                    let got = r.matched.clone().unwrap_or_default();
                    union_false
                        .entry(got.clone())
                        .or_insert_with(|| want.clone());
                    if is_neuro {
                        neuro_false += 1;
                    }
                    println!(
                        "FALSE {frame_name} #{idx}: truth={want:?} matched={got:?} raw={:?}",
                        results[*idx].raw_text
                    );
                }
            }
        }
        println!("{frame_name}: held {held}, correct {correct}");
    }

    let frames_read = truth.len() as f64;
    println!("---");
    println!(
        "cells: {} correct, {} FALSE ACCEPTS",
        cells_correct, cells_false
    );
    println!(
        "union distinct: {} correct, {} false",
        union_correct.len(),
        union_false.len()
    );
    for (got, want) in &union_false {
        println!("  union false: held {got:?} (truth {want:?})");
    }
    println!(
        "neuroptics: {}/{} instances correct, {} false, {} distinct",
        neuro_correct, neuro_total, neuro_false, neuro_distinct.len()
    );
    println!(
        "mean read_frame wall time: {:.0} ms/frame",
        total_read_ms / frames_read
    );
}

/// Pass-33 session gate: drive the production `CaptureSession`
/// (header classification, pending accumulation, stitch at
/// finalize) over the census frames IN ORDER, then score the
/// stitch's per-slot verdicts against census truth by joining the
/// plan's placements back to census cells: a census cell (frame f,
/// row r, col c) belongs to the slot at panorama row
/// round((g_f − φ_anchor)/P) + r in f's segment. Instances mapping
/// to one slot must agree on truth — a disagreement is a stitch
/// defect and is reported as such.
#[allow(clippy::too_many_lines)]
fn session_main(
    frames_dir: &std::path::Path,
    csv_path: &std::path::Path,
    data_dir: &std::path::Path,
    legibility_join: bool,
) {
    // Ground truth: ordered frame list + cells.
    let csv = std::fs::read_to_string(csv_path).expect("read census csv");
    let mut cells: Vec<SessionCell> = Vec::new();
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
        cells.push(SessionCell {
            frame,
            row: f[2].parse().unwrap_or(0),
            col: f[3].parse().unwrap_or(0),
            truth: f[4].to_string(),
            truth_count: f[5].parse().unwrap_or(1).max(1),
        });
    }
    frame_order.sort();
    let truth_distinct: HashSet<&str> = cells.iter().map(|c| c.truth.as_str()).collect();
    println!(
        "session census: {} frames in order, {} cells, {} distinct truth",
        frame_order.len(),
        cells.len(),
        truth_distinct.len()
    );

    let data = CaptureData::load(data_dir).expect("load capture data");
    let mut ctx = data.context();
    // Pass 34 A/B overrides (the production defaults come from
    // CaptureData::context): --line-reader off reproduces the
    // pass-33 stack; --snap on runs the boundary-snap ablation;
    // --strip-offsets off isolates the strip phase policy.
    let argv: Vec<String> = std::env::args().collect();
    if argv.iter().any(|a| a == "--line-reader") {
        ctx.line_reader = arg(&argv, "--line-reader").as_deref() != Some("off");
    }
    if argv.iter().any(|a| a == "--snap") {
        ctx.snap_cells = arg(&argv, "--snap").as_deref() == Some("on");
    }
    if argv.iter().any(|a| a == "--strip-offsets") {
        ctx.strip_offsets = arg(&argv, "--strip-offsets").as_deref() != Some("off");
    }
    println!(
        "pass34 toggles: line_reader={} snap={} strip_offsets={}",
        ctx.line_reader, ctx.snap_cells, ctx.strip_offsets
    );
    let layout = ctx.layouts.get("parts").expect("parts layout").clone();
    let engine = TesseractCliEngine::probe(layout.clone()).expect("tesseract engine");

    let t0 = Instant::now();
    let mut session = CaptureSession::new();
    // Production dedupes consecutive identical frames by content
    // signature, so a byte-identical repeat never reaches the
    // stitch — and placement indices count FED frames, not CSV
    // frames. Replicate the dedupe and remember the mapping (the
    // HC census lists a byte-identical first-00/parts-00 pair).
    let mut session_index: HashMap<usize, usize> = HashMap::new();
    {
        let mut prev_bytes: Option<Vec<u8>> = None;
        let mut fed = 0usize;
        for (ci, name) in frame_order.iter().enumerate() {
            let bytes = std::fs::read(frames_dir.join(name))
                .unwrap_or_else(|e| panic!("read frame {name}: {e}"));
            if prev_bytes.as_deref() == Some(bytes.as_slice()) {
                session_index.insert(ci, fed - 1);
                continue;
            }
            let img = GrayImage::from_bytes(&bytes).expect("decode frame png");
            session.process_frame(&ctx, &engine, &img);
            session_index.insert(ci, fed);
            fed += 1;
            prev_bytes = Some(bytes);
        }
    }
    session.finish_parts(&ctx, &engine);
    println!(
        "session wall time: {:.1} s for {} frames",
        t0.elapsed().as_secs_f64(),
        frame_order.len()
    );
    println!(
        "coverage: {}",
        session.coverage_summary().unwrap_or_default()
    );
    // Pass 34 glyph-implied accounting: a slot held CORRECTLY
    // as a blueprint whose built-copy glyph fired implies its
    // built sibling is held. The implied name counts toward
    // distinct-correct when it is itself in the census truth
    // (it is a real item the capture proves owned); when the
    // twin never appears in the census it is reported as
    // unverifiable — never counted, never a false accept. The
    // pass runs after the slot loop below via a second sweep;
    // see `implied_sweep` note in the gate section.
    let stock = session.stock().clone();

    let reports = session.stitch_reports();
    assert_eq!(
        reports.len(),
        1,
        "census session must stitch in one finish (frame count under the flush cap)"
    );
    let report = &reports[0];
    println!(
        "session row pitch {:.2}, col pitch {:.2}",
        report.plan.session_row_pitch, report.plan.session_col_pitch
    );
    for (si, seg) in report.plan.segments.iter().enumerate() {
        println!(
            "segment {si}: {} frames, row pitch {:.2}, col pitch {:.2}, relinked {}",
            seg.frames.len(),
            seg.pitch.row_pitch,
            seg.pitch.col_pitch,
            seg.relinked
        );
    }

    // frame index -> (segment, scroll_rows). With
    // --legibility-join (the HC census's convention: its rows are
    // content rows assigned by name legibility in the nominal
    // layout cell, per hc_truth.py / HC_REPORT), a frame whose
    // folded phase exceeds ~0.235 of a pitch has its census row 0
    // on the stitch's lattice row -1: the previous row's name band
    // overlaps the nominal first cell more than row 0's own band
    // does (band overlap of row -1 is phi, of row 0 is
    // 0.47*P - phi; they cross at phi = 0.235*P). The standard
    // census numbers rows on the fitted lattice itself, so it
    // joins with delta 0 throughout (verified: zero truth
    // disagreements across all 495 cells).
    let mut placement: HashMap<usize, (usize, i32)> = HashMap::new();
    for (si, seg) in report.plan.segments.iter().enumerate() {
        let phi_anchor = seg.frames.first().map(|p| p.phase).unwrap_or(0.0);
        for p in &seg.frames {
            let sr = ((p.g - phi_anchor) / seg.pitch.row_pitch).round() as i32;
            let delta = if legibility_join && p.phase > 0.235 * seg.pitch.row_pitch {
                -1
            } else {
                0
            };
            placement.insert(p.frame, (si, sr + delta));
            println!(
                "frame {} seg {si} phase {:.1} g {:.1} scroll_rows {sr} delta {delta}",
                p.frame, p.phase, p.g
            );
        }
    }
    // slot key -> report slot index
    let mut slot_idx: HashMap<(usize, i32, u32), usize> = HashMap::new();
    for (i, s) in report.slots.iter().enumerate() {
        slot_idx.insert((s.segment, s.row, s.col), i);
    }

    // Join cells to slots.
    let mut slot_truths: HashMap<usize, HashSet<String>> = HashMap::new();
    let mut slot_cells: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut unjoined = 0usize;
    for (ci, cell) in cells.iter().enumerate() {
        let fi_csv = frame_order.iter().position(|f| f == &cell.frame).unwrap();
        let fi = session_index[&fi_csv];
        let Some(&(si, sr)) = placement.get(&fi) else {
            unjoined += 1;
            continue;
        };
        let key = (si, sr + cell.row, cell.col);
        match slot_idx.get(&key) {
            Some(&s) => {
                slot_truths.entry(s).or_default().insert(cell.truth.clone());
                slot_cells.entry(s).or_default().push(ci);
            }
            None => unjoined += 1,
        }
    }
    if unjoined > 0 {
        println!("WARNING: {unjoined} census cells joined no slot");
    }
    let mut distinct_correct: HashSet<String> = HashSet::new();
    let mut false_accepts: Vec<String> = Vec::new();
    let mut unmapped_held: Vec<String> = Vec::new();
    let mut slot_count_ok = 0usize;
    let mut slot_count_total = 0usize;
    let mut cell_count_ok = 0usize;
    let mut cell_count_total = 0usize;
    // frame -> correctly-held slots it contributed a usable instance to
    let mut frame_alive: HashMap<usize, HashSet<usize>> = HashMap::new();

    for (s, slot) in report.slots.iter().enumerate() {
        let held = if slot.coverage == fracker::capture::SlotCoverage::Held {
            slot.result.as_ref().and_then(|r| r.matched.clone())
        } else {
            None
        };
        let truths = slot_truths.get(&s);
        match (&held, truths) {
            (Some(h), Some(ts)) if ts.len() == 1 => {
                let t = ts.iter().next().unwrap();
                if h == t {
                    distinct_correct.insert(t.clone());
                    // counts: resolved session count vs truth
                    let resolved = stock.parts.get(h).copied().unwrap_or(1);
                    let cell_list = &slot_cells[&s];
                    let mut counts: Vec<u32> =
                        cell_list.iter().map(|&ci| cells[ci].truth_count).collect();
                    counts.sort_unstable();
                    let truth_mode = counts[counts.len() / 2];
                    slot_count_total += 1;
                    if resolved == truth_mode {
                        slot_count_ok += 1;
                    }
                    for &ci in cell_list {
                        cell_count_total += 1;
                        if resolved == cells[ci].truth_count {
                            cell_count_ok += 1;
                        }
                        let fi = frame_order
                            .iter()
                            .position(|f| f == &cells[ci].frame)
                            .unwrap();
                        // usable contribution is approximated by
                        // the cell being fully in its frame's grid;
                        // census cells are all in-grid sightings.
                        frame_alive.entry(fi).or_default().insert(s);
                    }
                } else {
                    false_accepts.push(format!(
                        "slot seg{} row{} col{}: held {h:?}, truth {t:?} (cells: {}) [band={:?} line={:?} verdict={:?} via={:?} refuse={:?} glyph={:.2} mesh={:.2}]",
                        slot.segment,
                        slot.row,
                        slot.col,
                        cell_list_str(&slot_cells[&s], &cells, &frame_order),
                        slot.band_held,
                        slot.line_held,
                        slot.form_verdict,
                        slot.held_via,
                        slot.refuse_reason,
                        slot.glyph_score,
                        slot.mesh_score
                    ));
                }
            }
            (Some(h), Some(ts)) => {
                false_accepts.push(format!(
                    "slot seg{} row{} col{}: held {h:?} but its cells DISAGREE on truth {ts:?} — stitch defect",
                    slot.segment, slot.row, slot.col
                ));
            }
            (Some(h), None) => {
                unmapped_held.push(format!(
                    "slot seg{} row{} col{}: held {h:?}, no census cell maps here",
                    slot.segment, slot.row, slot.col
                ));
            }
            _ => {}
        }
    }

    // Pass 34: arbitration attribution + glyph-implied sweep
    // (see the note after the coverage line).
    {
        let mut via_counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for slot in &report.slots {
            if slot.coverage == fracker::capture::SlotCoverage::Held {
                *via_counts.entry(format!("{:?}", slot.held_via)).or_default() += 1;
            }
        }
        println!("held-slot arbitration attribution: {via_counts:?}");
        for (s, slot) in report.slots.iter().enumerate() {
            let Some(implied) = &slot.glyph_implied else {
                continue;
            };
            let own_correct = slot_truths
                .get(&s)
                .map(|truths| {
                    let h = slot.result.as_ref().and_then(|r| r.matched.as_deref());
                    truths.len() == 1 && h == truths.iter().next().map(|x| x.as_str())
                })
                .unwrap_or(false);
            if !own_correct {
                println!(
                    "glyph-implied {implied:?} from seg{} row{} col{}: implying slot not itself correct — not counted",
                    slot.segment, slot.row, slot.col
                );
            } else if truth_distinct.contains(implied.as_str()) {
                if distinct_correct.insert(implied.clone()) {
                    println!(
                        "glyph-implied distinct gain: {implied:?} (from seg{} row{} col{})",
                        slot.segment, slot.row, slot.col
                    );
                } else {
                    println!(
                        "glyph-implied {implied:?} from seg{} row{} col{}: twin already held directly — corroborated, no gain",
                        slot.segment, slot.row, slot.col
                    );
                }
            } else {
                println!(
                    "glyph-implied {implied:?} from seg{} row{} col{}: twin absent from census truth — UNVERIFIABLE, not counted",
                    slot.segment, slot.row, slot.col
                );
            }
        }
    }

    let neuro_distinct: HashSet<&String> = distinct_correct
        .iter()
        .filter(|n| n.contains("Neuroptics"))
        .collect();
    let missed: Vec<&str> = truth_distinct
        .iter()
        .filter(|t| !distinct_correct.contains(**t))
        .copied()
        .collect();

    println!("--- session gate ---");
    println!(
        "distinct correct: {} / {} truth",
        distinct_correct.len(),
        truth_distinct.len()
    );
    println!(
        "neuroptics distinct: {}",
        neuro_distinct.len()
    );
    println!("false accepts: {}", false_accepts.len());
    for fa in &false_accepts {
        println!("  FA {fa}");
    }
    println!("held slots with no census cell: {}", unmapped_held.len());
    for u in &unmapped_held {
        println!("  UNMAPPED {u}");
    }
    println!(
        "counts (slot level): {slot_count_ok}/{slot_count_total} correct"
    );
    println!(
        "counts (cell level): {cell_count_ok}/{cell_count_total} correct"
    );
    println!("missed distinct ({}):", missed.len());
    for m in &missed {
        println!("  missed {m}");
    }
    println!("per-frame alive contributions (correctly-held slots):");
    for (fi, name) in frame_order.iter().enumerate() {
        let n = frame_alive.get(&fi).map(|s| s.len()).unwrap_or(0);
        println!("  {name}: {n}");
    }
}

#[derive(Clone)]
struct SessionCell {
    frame: String,
    row: i32,
    col: u32,
    truth: String,
    truth_count: u32,
}

fn cell_list_str(idxs: &[usize], cells: &[SessionCell], _frames: &[String]) -> String {
    idxs.iter()
        .map(|&i| {
            let c = &cells[i];
            format!("{} r{}c{}", c.frame, c.row, c.col)
        })
        .collect::<Vec<_>>()
        .join(", ")
}
