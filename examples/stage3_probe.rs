//! Pass 35 development probe (not shipped logic): why does
//! the synthesis fit score held slots' true names so low?
//! Rebuilds the standard census session (synthesis off),
//! re-derives canonical crops for both-readers-held slots,
//! and (a) grid-searches rasteriser fits per slot against
//! the OBSERVED lines, (b) dumps PNGs — observed name band
//! vs renders — for eyeball comparison.
//!
//!   stage3_probe --frames DIR --csv census_cells.csv --data DIR

use fracker::capture::{CaptureData, CaptureSession, SlotCoverage};
use fracker::lineread::Via;
use fracker::ocr::{GrayImage, TesseractCliEngine};
use fracker::synth::{observe_slot, FitParams, Scorer};
use std::collections::HashMap;
use std::path::Path;

fn arg(argv: &[String], name: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == name)
        .and_then(|i| argv.get(i + 1))
        .cloned()
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let frames_dir = arg(&argv, "--frames").expect("--frames");
    let data_dir = arg(&argv, "--data").expect("--data");
    let csv_path = arg(&argv, "--csv").expect("--csv");

    let csv = std::fs::read_to_string(&csv_path).expect("csv");
    let mut order: Vec<String> = Vec::new();
    for line in csv.lines().skip(1) {
        let f = line.split(',').next().unwrap_or("").trim().to_string();
        if !f.is_empty() && !order.contains(&f) {
            order.push(f);
        }
    }
    let frames: Vec<GrayImage> = order
        .iter()
        .map(|n| {
            let bytes = std::fs::read(Path::new(&frames_dir).join(n))
                .unwrap_or_else(|e| panic!("read frame {n}: {e}"));
            GrayImage::from_bytes(&bytes)
                .unwrap_or_else(|e| panic!("decode frame {n}: {e}"))
        })
        .collect();
    let data = CaptureData::load(Path::new(&data_dir)).expect("capture data");
    let ctx = data.context();
    let layout = ctx.layouts.get("parts").expect("parts layout").clone();
    let engine = TesseractCliEngine::probe(layout.clone()).expect("tesseract");

    let mut session = CaptureSession::new();
    for f in &frames {
        session.process_frame(&ctx, &engine, f);
    }
    session.finish_parts(&ctx, &engine);
    let reports = session.stitch_reports();
    let report = &reports[0];
    let held_n = report
        .slots
        .iter()
        .filter(|s| s.coverage == SlotCoverage::Held)
        .count();
    println!("held {held_n} slots (synthesis off)");

    let refs: Vec<&GrayImage> = frames.iter().collect();
    let mut slot_idx: HashMap<(usize, i32, u32), usize> = HashMap::new();
    for (i, s) in report.slots.iter().enumerate() {
        slot_idx.insert((s.segment, s.row, s.col), i);
    }
    struct Work {
        truth: String,
        cell: GrayImage,
        obs: fracker::synth::SlotObs,
    }
    let mut calib: Vec<Work> = Vec::new();
    for (si, seg) in report.plan.segments.iter().enumerate() {
        let slots = fracker::stitch::slot_instances(&report.plan, si, &refs, &layout);
        let cw = seg.pitch.col_pitch.round().max(1.0) as u32;
        let ch = seg.pitch.row_pitch.round().max(1.0) as u32;
        for slot in &slots {
            let Some(&ri) = slot_idx.get(&(si, slot.row, slot.col)) else {
                continue;
            };
            let rep = &report.slots[ri];
            if !(rep.coverage == SlotCoverage::Held && rep.held_via == Some(Via::Both)) {
                continue;
            }
            let Some(cc) = fracker::stitch::canonical_crop(slot, &refs, cw, ch) else {
                continue;
            };
            let truth = rep
                .result
                .as_ref()
                .and_then(|r| r.matched.clone())
                .unwrap_or_default();
            let (obs, _det) = observe_slot(&cc.image, &layout);
            calib.push(Work {
                truth,
                cell: cc.image,
                obs,
            });
        }
    }
    println!("calibration slots: {}", calib.len());

    let font = match arg(&argv, "--font") {
        Some(p) => {
            let bytes = std::fs::read(&p).expect("read --font file");
            let f = fracker::ttf::TtfFont::from_bytes(bytes).expect("parse --font file");
            println!("font override: {p} (version {:?})", f.version_string());
            f
        }
        None => fracker::ttf::embedded_font().expect("embedded font"),
    };
    let mut scorer = Scorer::new(font, FitParams::default());

    // (a) Per-slot best over a wide grid, plus the score at
    // the production-chosen fit and at the naive default.
    let prod = FitParams {
        px_size: 16.5,
        wrap_frac: 0.925,
        blur: 0.7,
        level: 0.0,
        gamma: 0.55,
    };
    let grid_px = [16.5f32, 17.0, 17.5];
    let grid_g = [0.55f32, 0.9, 1.15];
    let grid_b = [0.4f32, 0.7];
    let grid_l = [0.0f32];
    let grid_w = [0.88f32, 0.94, 1.0];
    let sample = &calib[..calib.len().min(16)];
    let mut sum_best = 0.0f32;
    let mut sum_prod = 0.0f32;
    let mut sum_def = 0.0f32;
    for w in sample {
        let mut best = (f32::MIN, FitParams::default());
        for &px in &grid_px {
            for &g in &grid_g {
                for &b in &grid_b {
                    for &l in &grid_l {
                        for &wr in &grid_w {
                            let fit = FitParams {
                                px_size: px,
                                gamma: g,
                                blur: b,
                                level: l,
                                wrap_frac: wr,
                            };
                            scorer.set_fit(fit);
                            let s = scorer.lines_mean(&w.obs, &w.truth);
                            if s > best.0 {
                                best = (s, fit);
                            }
                        }
                    }
                }
            }
        }
        scorer.set_fit(prod);
        let sp = scorer.lines_mean(&w.obs, &w.truth);
        scorer.set_fit(FitParams::default());
        let sd = scorer.lines_mean(&w.obs, &w.truth);
        sum_best += best.0;
        sum_prod += sp;
        sum_def += sd;
        println!(
            "{:?}: lines={} best {:.3} @ px{} g{} b{} l{} w{} | prod {:.3} | default {:.3}",
            w.truth,
            w.obs.lines.len(),
            best.0,
            best.1.px_size,
            best.1.gamma,
            best.1.blur,
            best.1.level,
            best.1.wrap_frac,
            sp,
            sd
        );
    }
    let n = sample.len() as f32;
    println!(
        "means over {} slots: best {:.3} prod {:.3} default {:.3}",
        sample.len(),
        sum_best / n,
        sum_prod / n,
        sum_def / n
    );

    // (b) PNG dumps for the first three slots: observed band,
    // render at default fit, render at production fit.
    for (i, w) in calib.iter().take(3).enumerate() {
        let band = fracker::lineread::name_band(&w.cell, &layout);
        save_gray(&format!("/tmp/probe_{i}_obs.png"), &band);
        for (tag, fit) in [("def", FitParams::default()), ("prod", prod)] {
            scorer.set_fit(fit);
            let lines = scorer.wrapped_lines(&w.truth, w.obs.band_w);
            let mut imgs = Vec::new();
            for t in &lines {
                imgs.push(scorer.render(t));
            }
            let h: u32 = imgs.iter().map(|g| g.height + 2).sum();
            let wd: u32 = imgs.iter().map(|g| g.width).max().unwrap_or(1);
            let mut canvas = GrayImage::blank(wd, h, 0);
            let mut y = 0;
            for g in &imgs {
                for yy in 0..g.height {
                    for xx in 0..g.width {
                        canvas.set(xx, y + yy, 255 - g.get(xx, yy));
                    }
                }
                y += g.height + 2;
            }
            save_gray(&format!("/tmp/probe_{i}_{tag}.png"), &canvas);
        }
        println!("slot {i} {:?}: dumps written", w.truth);
    }
}

fn save_gray(path: &str, img: &GrayImage) {
    // Upscale 3x nearest for visibility.
    img.upscale(3).write_png(Path::new(path)).expect("write png");
}
