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

use fracker::capture::{read_frame, CaptureData};
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
