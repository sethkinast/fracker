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
///
/// Pass 36: the session stitches through the background bundle
/// machinery (progress-flushed bundles, one report per bundle),
/// so the join is per report. A census frame's position inside
/// its bundle is its ACCEPTED ordinal (tracked through the
/// session's intake counter — intake-dropped frames never reach
/// a bundle) minus the cumulative frame counts of the bundles
/// before it.
///
/// `--simulate-live` inflates every census frame x8 with small
/// deterministic brightness/gamma jitter (an animated-background
/// proxy): the intake dedupe must collapse the copies and the
/// session must produce the same distinct-held and 0 FA as the
/// base run, with bundle processing off the intake path.
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
    let argv: Vec<String> = std::env::args().collect();
    let simulate_bg = argv.iter().any(|a| a == "--simulate-live-bg");
    let simulate_live =
        simulate_bg || argv.iter().any(|a| a == "--simulate-live");
    println!(
        "session census: {} frames in order, {} cells, {} distinct truth{}",
        frame_order.len(),
        cells.len(),
        truth_distinct.len(),
        if simulate_bg {
            " [simulate-live-bg x8 — diagnostic]"
        } else if simulate_live {
            " [simulate-live x8]"
        } else {
            ""
        }
    );

    let data = CaptureData::load(data_dir).expect("load capture data");
    let mut ctx = data.context();
    // Pass 34 A/B overrides (the production defaults come from
    // CaptureData::context): --line-reader off reproduces the
    // pass-33 stack; --snap on runs the boundary-snap ablation;
    // --strip-offsets off isolates the strip phase policy.
    if argv.iter().any(|a| a == "--line-reader") {
        ctx.line_reader = arg(&argv, "--line-reader").as_deref() != Some("off");
    }
    if argv.iter().any(|a| a == "--snap") {
        ctx.snap_cells = arg(&argv, "--snap").as_deref() == Some("on");
    }
    if argv.iter().any(|a| a == "--strip-offsets") {
        ctx.strip_offsets = arg(&argv, "--strip-offsets").as_deref() != Some("off");
    }
    if argv.iter().any(|a| a == "--synthesis") {
        ctx.synthesis = arg(&argv, "--synthesis").as_deref() != Some("off");
    }
    println!(
        "pass34/35 toggles: line_reader={} snap={} strip_offsets={} synthesis={}",
        ctx.line_reader, ctx.snap_cells, ctx.strip_offsets, ctx.synthesis
    );
    let layout = ctx.layouts.get("parts").expect("parts layout").clone();
    let engine = TesseractCliEngine::probe(layout.clone()).expect("tesseract engine");

    let mut session = CaptureSession::new();
    // Pass 36: the background stitch path is the production path
    // — run the gate through it (second engine instance, bundle
    // dispatch in process_frame, blocking drain at finish).
    session.set_stitch_engine(Box::new(
        TesseractCliEngine::probe(layout.clone()).expect("tesseract stitch engine"),
    ));
    // Pass 37: the intake's time gate consumes the frames'
    // arrival times, which a replay has to MODEL — the base
    // gate walks viewports at Seth's page-at-a-time pace (one
    // per few seconds), the dense sim feeds at the measured
    // worker delivery rate (~500 ms; LIVE_DIAG2 §4). Arrival
    // spacing is an input to the gate, never a policy knob.
    let step_ms: u64 = if simulate_live { 500 } else { 3000 };
    let feed_tick = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let feed_base = std::time::Instant::now();
    session.set_feed_clock(Box::new(move || {
        feed_base
            + std::time::Duration::from_millis(
                step_ms * feed_tick.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            )
    }));

    // Feed the frames, tracking each census frame's ACCEPTED
    // ordinal (its index among the frames the intake let into
    // the stitch pool). Base mode replicates production's
    // byte-identical pre-dedupe: a byte-identical repeat never
    // reaches process_frame at all, and its cells join the twin
    // frame's slots. Simulate-live feeds every copy; a census
    // frame whose first copy the intake dropped as a duplicate
    // (only possible for the byte-identical twin) falls back to
    // its twin's ordinal the same way.
    let frame_bytes: Vec<Vec<u8>> = frame_order
        .iter()
        .map(|name| {
            std::fs::read(frames_dir.join(name))
                .unwrap_or_else(|e| panic!("read frame {name}: {e}"))
        })
        .collect();
    let mut ordinal_of: HashMap<usize, usize> = HashMap::new();
    // Pass 36 sim repair: EVERY accepted ordinal per census
    // frame, not just the representative. A bundle boundary can
    // split one position's accepted copies across two bundles;
    // the representative sits in the first bundle while the
    // second bundle's slots for the same position then join no
    // cells (and the next position's representative lands in a
    // later segment, orphaning the second bundle's head slots).
    // The join below tries every accepted ordinal and takes the
    // first placement whose slot key hits — a slot is the union
    // of its instances, so any instance's placement is a valid
    // attribution. Base mode has exactly one ordinal per frame
    // and is unchanged.
    let mut ordinals_of: HashMap<usize, Vec<usize>> = HashMap::new();
    let t0 = Instant::now();
    if simulate_live {
        for (ci, bytes) in frame_bytes.iter().enumerate() {
            let img = GrayImage::from_bytes(bytes).expect("decode frame png");
            for copy in 0..8usize {
                let f = if copy == 0 {
                    img.clone()
                } else if simulate_bg {
                    jitter_frame_bg(&img, &layout, ci * 16 + copy)
                } else {
                    jitter_frame(&img, ci * 16 + copy)
                };
                let before = session.parts_frames_accepted();
                session.process_frame(&ctx, &engine, &f);
                if session.parts_frames_accepted() > before {
                    ordinals_of.entry(ci).or_default().push(before as usize);
                    if copy == 0 {
                        ordinal_of.insert(ci, before as usize);
                    }
                } else if copy == 0 && ci > 0 && frame_bytes[ci] == frame_bytes[ci - 1] {
                    if let Some(&o) = ordinal_of.get(&(ci - 1)) {
                        ordinal_of.insert(ci, o);
                    }
                    if let Some(v) = ordinals_of.get(&(ci - 1)).cloned() {
                        ordinals_of.insert(ci, v);
                    }
                }
            }
        }
    } else {
        let mut prev_bytes: Option<&[u8]> = None;
        for (ci, bytes) in frame_bytes.iter().enumerate() {
            if prev_bytes == Some(bytes.as_slice()) {
                if let Some(&o) = ordinal_of.get(&(ci - 1)) {
                    ordinal_of.insert(ci, o);
                }
                if let Some(v) = ordinals_of.get(&(ci - 1)).cloned() {
                    ordinals_of.insert(ci, v);
                }
                continue;
            }
            let img = GrayImage::from_bytes(bytes).expect("decode frame png");
            let before = session.parts_frames_accepted();
            session.process_frame(&ctx, &engine, &img);
            if session.parts_frames_accepted() > before {
                ordinal_of.insert(ci, before as usize);
                ordinals_of.entry(ci).or_default().push(before as usize);
            }
            prev_bytes = Some(bytes);
        }
    }
    let feed_secs = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    session.finish_parts(&ctx, &engine);
    let finish_secs = t1.elapsed().as_secs_f64();
    println!(
        "session wall time: feed {:.1} s, finish {:.1} s, total {:.1} s ({} census frames)",
        feed_secs,
        finish_secs,
        feed_secs + finish_secs,
        frame_order.len()
    );
    println!(
        "intake: {} parts frames accepted, {} duplicates dropped, {} off-lattice dropped, {} interval dropped, {} latched dropped, {} cap-refused",
        session.parts_frames_accepted(),
        session.parts_dupes_dropped(),
        session.parts_off_lattice_dropped(),
        session.parts_interval_dropped(),
        session.parts_latched_dropped(),
        session.parts_frames_capped()
    );
    if let Some(fs) = session.failsafe_summary() {
        println!("{fs}");
    }
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
    // Pass 36 diagnostic: the resolved stock, join-free. The
    // cell join below attributes slots to census cells; the
    // stock is what production actually writes, and diffing it
    // across modes isolates system behaviour from join noise.
    for (name, count) in &stock.parts {
        println!("STOCK {name} = {count}");
    }

    let reports = session.stitch_reports();
    assert!(
        !reports.is_empty(),
        "census session produced no stitch reports"
    );
    // Ordinal -> (report, segment, scroll_rows). With
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
    let mut placement: HashMap<usize, (usize, usize, i32)> = HashMap::new();
    let mut bundle_base = 0usize;
    for (ri, report) in reports.iter().enumerate() {
        println!(
            "bundle {ri}: {} frames; session row pitch {:.2}, col pitch {:.2}",
            report.plan.frame_pitches.len(),
            report.plan.session_row_pitch,
            report.plan.session_col_pitch
        );
        for (si, seg) in report.plan.segments.iter().enumerate() {
            println!(
                "  segment {si}: {} frames, row pitch {:.2}, col pitch {:.2}, relinked {}",
                seg.frames.len(),
                seg.pitch.row_pitch,
                seg.pitch.col_pitch,
                seg.relinked
            );
            let phi_anchor = seg.frames.first().map(|p| p.phase).unwrap_or(0.0);
            for p in &seg.frames {
                let sr = ((p.g - phi_anchor) / seg.pitch.row_pitch).round() as i32;
                let delta = if legibility_join && p.phase > 0.235 * seg.pitch.row_pitch {
                    -1
                } else {
                    0
                };
                let ord = bundle_base + p.frame;
                placement.insert(ord, (ri, si, sr + delta));
                println!(
                    "  frame ordinal {} seg {si} phase {:.1} g {:.1} scroll_rows {sr} delta {delta}",
                    ord, p.phase, p.g
                );
            }
        }
        bundle_base += report.plan.frame_pitches.len();
    }
    // (report, segment, row, col) -> (report, slot index)
    let mut slot_idx: HashMap<(usize, usize, i32, u32), (usize, usize)> = HashMap::new();
    for (ri, report) in reports.iter().enumerate() {
        for (i, s) in report.slots.iter().enumerate() {
            slot_idx.insert((ri, s.segment, s.row, s.col), (ri, i));
        }
    }

    // Join cells to slots.
    let mut slot_truths: HashMap<(usize, usize), HashSet<String>> = HashMap::new();
    let mut slot_cells: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    let mut unjoined = 0usize;
    for (ci, cell) in cells.iter().enumerate() {
        let fi_csv = frame_order.iter().position(|f| f == &cell.frame).unwrap();
        let candidates: &[usize] = match ordinals_of.get(&fi_csv) {
            Some(v) => v,
            None => match ordinal_of.get(&fi_csv) {
                Some(o) => std::slice::from_ref(o),
                None => &[],
            },
        };
        let mut joined = false;
        for &ord in candidates {
            let Some(&(ri, si, sr)) = placement.get(&ord) else {
                continue;
            };
            let key = (ri, si, sr + cell.row, cell.col);
            if let Some(&s) = slot_idx.get(&key) {
                slot_truths.entry(s).or_default().insert(cell.truth.clone());
                slot_cells.entry(s).or_default().push(ci);
                joined = true;
                break;
            }
        }
        if !joined {
            unjoined += 1;
        }
    }
    if unjoined > 0 {
        println!("WARNING: {unjoined} census cells joined no slot");
    }
    let mut distinct_correct: HashSet<String> = HashSet::new();
    // Pass 35: which arbitration voices produced each correct
    // distinct item (for synthesis-gain attribution).
    let mut correct_vias: HashMap<String, HashSet<String>> = HashMap::new();
    let mut false_accepts: Vec<String> = Vec::new();
    let mut unmapped_held: Vec<String> = Vec::new();
    let mut slot_count_ok = 0usize;
    let mut slot_count_total = 0usize;
    let mut cell_count_ok = 0usize;
    let mut cell_count_total = 0usize;
    // frame -> correctly-held slots it contributed a usable instance to
    let mut frame_alive: HashMap<usize, HashSet<(usize, usize)>> = HashMap::new();

    for (ri, report) in reports.iter().enumerate() {
        for (s, slot) in report.slots.iter().enumerate() {
            let key = (ri, s);
            let held = if slot.coverage == fracker::capture::SlotCoverage::Held {
                slot.result.as_ref().and_then(|r| r.matched.clone())
            } else {
                None
            };
            let truths = slot_truths.get(&key);
            match (&held, truths) {
                (Some(h), Some(ts)) if ts.len() == 1 => {
                    let t = ts.iter().next().unwrap();
                    if h == t {
                        distinct_correct.insert(t.clone());
                        correct_vias
                            .entry(t.clone())
                            .or_default()
                            .insert(format!("{:?}", slot.held_via));
                        // counts: resolved session count vs truth
                        let resolved = stock.parts.get(h).copied().unwrap_or(1);
                        let cell_list = &slot_cells[&key];
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
                            frame_alive.entry(fi).or_default().insert(key);
                        }
                    } else {
                        false_accepts.push(format!(
                            "slot b{ri} seg{} row{} col{}: held {h:?}, truth {t:?} (cells: {}) [band={:?} line={:?} verdict={:?} via={:?} refuse={:?} glyph={:.2} mesh={:.2}]",
                            slot.segment,
                            slot.row,
                            slot.col,
                            cell_list_str(&slot_cells[&key], &cells, &frame_order),
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
                        "slot b{ri} seg{} row{} col{}: held {h:?} but its cells DISAGREE on truth {ts:?} — stitch defect",
                        slot.segment, slot.row, slot.col
                    ));
                }
                (Some(h), None) => {
                    unmapped_held.push(format!(
                        "slot b{ri} seg{} row{} col{}: held {h:?}, no census cell maps here",
                        slot.segment, slot.row, slot.col
                    ));
                }
                _ => {}
            }
        }
    }

    // Pass 34: arbitration attribution + glyph-implied sweep
    // (see the note after the coverage line).
    {
        let mut via_counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for report in reports {
            for slot in &report.slots {
                if slot.coverage == fracker::capture::SlotCoverage::Held {
                    *via_counts.entry(format!("{:?}", slot.held_via)).or_default() += 1;
                }
            }
        }
        println!("held-slot arbitration attribution: {via_counts:?}");
        for (ri, report) in reports.iter().enumerate() {
            for (s, slot) in report.slots.iter().enumerate() {
                let Some(implied) = &slot.glyph_implied else {
                    continue;
                };
                let own_correct = slot_truths
                    .get(&(ri, s))
                    .map(|truths| {
                        let h = slot.result.as_ref().and_then(|r| r.matched.as_deref());
                        truths.len() == 1 && h == truths.iter().next().map(|x| x.as_str())
                    })
                    .unwrap_or(false);
                if !own_correct {
                    println!(
                        "glyph-implied {implied:?} from b{ri} seg{} row{} col{}: implying slot not itself correct — not counted",
                        slot.segment, slot.row, slot.col
                    );
                } else if truth_distinct.contains(implied.as_str()) {
                    if distinct_correct.insert(implied.clone()) {
                        println!(
                            "glyph-implied distinct gain: {implied:?} (from b{ri} seg{} row{} col{})",
                            slot.segment, slot.row, slot.col
                        );
                    } else {
                        println!(
                            "glyph-implied {implied:?} from b{ri} seg{} row{} col{}: twin already held directly — corroborated, no gain",
                            slot.segment, slot.row, slot.col
                        );
                    }
                } else {
                    println!(
                        "glyph-implied {implied:?} from b{ri} seg{} row{} col{}: twin absent from census truth — UNVERIFIABLE, not counted",
                        slot.segment, slot.row, slot.col
                    );
                }
            }
        }
    }

    // Pass 35: synthesis accounting — per-slot holds with
    // scores, distinct gains attributable to synthesis alone,
    // outcome tally, candidate-set sizes, and the count of
    // hold-vs-hold name disagreements (ineligible by contract).
    {
        let mut gains: Vec<&str> = Vec::new();
        for (name, vias) in &correct_vias {
            if vias.len() == 1 && vias.iter().next().map(|v| v.as_str()) == Some("Some(Synthesis)") {
                gains.push(name.as_str());
            }
        }
        gains.sort();
        println!("--- synthesis ---");
        // Pass 36 integration: adjudication is session-level, so
        // every bundle report carries the same calibration record;
        // print it once, from the first report that has it.
        let session_calib = reports.iter().find_map(|r| r.synth_calib.as_ref());
        if let Some(c) = session_calib {
            println!(
                "calibration: {} slots; fit px={:.2} wrap={:.3} blur={:.2} level={:.2} gamma={:.2}",
                c.slots, c.fit.px_size, c.fit.wrap_frac, c.fit.blur, c.fit.level, c.fit.gamma
            );
            println!(
                "true scores q02/25/50/75/98: {:.3}/{:.3}/{:.3}/{:.3}/{:.3}",
                c.true_q[0], c.true_q[1], c.true_q[2], c.true_q[3], c.true_q[4]
            );
            println!(
                "wrong scores q02/25/50/75/98: {:.3}/{:.3}/{:.3}/{:.3}/{:.3}",
                c.wrong_q[0], c.wrong_q[1], c.wrong_q[2], c.wrong_q[3], c.wrong_q[4]
            );
            println!(
                "margins q05/10/35/50/90: {:.3}/{:.3}/{:.3}/{:.3}/{:.3}",
                c.margin_q[0], c.margin_q[1], c.margin_q[2], c.margin_q[3], c.margin_q[4]
            );
            println!(
                "plateau [{:.3}, {:.3}]; floor {:.3}; margin_thr {:.3}; grey {:.3}; exemplar n={} q05/50/95 {:.3}/{:.3}/{:.3} ex_floor {:.3}",
                c.plateau.0, c.plateau.1, c.floor, c.margin_thr, c.grey_thr,
                c.exemplar_n, c.exemplar_q[0], c.exemplar_q[1], c.exemplar_q[2], c.ex_floor
            );
        } else {
            println!("calibration: NONE (post-pass did not calibrate)");
        }
        for (ri, report) in reports.iter().enumerate() {
          for (s, slot) in report.slots.iter().enumerate() {
            if slot.held_via != Some(fracker::lineread::Via::Synthesis) {
                continue;
            }
            let truth = slot_truths.get(&(ri, s)).and_then(|ts| {
                if ts.len() == 1 {
                    ts.iter().next().map(|x| x.as_str())
                } else {
                    None
                }
            });
            let d = slot.synth.as_ref();
            println!(
                "synth hold seg{} row{} col{}: held {:?} truth {:?} score {:.3} margin {:.3} exemplar {:?}",
                slot.segment,
                slot.row,
                slot.col,
                slot.result.as_ref().and_then(|r| r.matched.as_deref()),
                truth,
                d.map(|d| d.best_score).unwrap_or(0.0),
                d.map(|d| d.margin).unwrap_or(0.0),
                d.and_then(|d| d.exemplar),
            );
          }
        }
        println!(
            "synthesis distinct gains ({}): {gains:?}",
            gains.len()
        );
        let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
        let mut cand_sizes: Vec<usize> = Vec::new();
        for report in reports {
            for slot in &report.slots {
                if let Some(d) = &slot.synth {
                    *tally.entry(d.outcome).or_default() += 1;
                    cand_sizes.push(d.candidates.len());
                }
            }
        }
        cand_sizes.sort_unstable();
        println!("synthesis outcomes: {tally:?}");
        if !cand_sizes.is_empty() {
            println!(
                "candidate sets: n={} min={} median={} max={}",
                cand_sizes.len(),
                cand_sizes[0],
                cand_sizes[cand_sizes.len() / 2],
                cand_sizes[cand_sizes.len() - 1]
            );
        }
        let disagreements = reports
            .iter()
            .flat_map(|r| r.slots.iter())
            .filter(|s| s.refuse_reason == Some("name-disagreement"))
            .count();
        println!("name-disagreement refusals (synthesis-ineligible): {disagreements}");
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
    // Pass 35: the post-stage-3 residue census — every missed
    // item with its slots' synthesis diagnostics (best
    // candidate, score, margin, outcome), or the structural
    // reason no adjudication happened.
    println!("residue census:");
    for m in &missed {
        let mut lines: Vec<String> = Vec::new();
        for (ri, report) in reports.iter().enumerate() {
          for (s, slot) in report.slots.iter().enumerate() {
            let carries = slot_truths
                .get(&(ri, s))
                .map(|ts| ts.len() == 1 && ts.iter().next().map(|x| x.as_str()) == Some(m))
                .unwrap_or(false);
            if !carries {
                continue;
            }
            match &slot.synth {
                Some(d) => lines.push(format!(
                    "seg{} row{} col{}: outcome={} best={:?} score={:.3} margin={:.3} exemplar={:?} cands={}",
                    slot.segment,
                    slot.row,
                    slot.col,
                    d.outcome,
                    d.best,
                    d.best_score,
                    d.margin,
                    d.exemplar,
                    d.candidates.len()
                )),
                None => lines.push(format!(
                    "seg{} row{} col{}: not adjudicated (coverage={:?} band={:?} line={:?} refuse={:?})",
                    slot.segment,
                    slot.row,
                    slot.col,
                    slot.coverage,
                    slot.band_held,
                    slot.line_held,
                    slot.refuse_reason
                )),
            }
          }
        }
        if lines.is_empty() {
            println!("  residue {m}: no adjudicable slot (never fully visible / outside the panorama)");
        }
        for l in lines {
            println!("  residue {m}: {l}");
        }
    }
    println!("per-frame alive contributions (correctly-held slots):");
    for (fi, name) in frame_order.iter().enumerate() {
        let n = frame_alive.get(&fi).map(|s| s.len()).unwrap_or(0);
        println!("  {name}: {n}");
    }
}

/// Pass-36 simulation jitter: a deterministic per-copy
/// gain/gamma perturbation (gain 0.960–1.040, gamma 0.960–1.040,
/// hashed from the copy's seed) standing in for the inventory
/// screens' animated background.
/// Diagnostic variant of [`jitter_frame`]: the gain/gamma
/// jitter applies only OUTSIDE the parts grid. Production
/// duplicate frames are pixel-identical inside the grid (the
/// game re-renders the same cell bitmaps — Seth's diagnosis of
/// the incident run was frames of page 1 identical); only the
/// background animates. `--simulate-live-bg` measures the
/// hotfix against that faithful duplicate model. The brief's
/// gate remains the whole-frame `--simulate-live`; this mode
/// exists to localise a gate failure, not to replace the gate.
fn jitter_frame_bg(img: &GrayImage, layout: &fracker::ocr::Layout, seed: usize) -> GrayImage {
    let mut out = jitter_frame(img, seed);
    let sx = img.width as f32 / layout.ref_width as f32;
    let sy = img.height as f32 / layout.ref_height as f32;
    let x0 = (layout.x0 * sx) as u32;
    let y0 = (layout.y0 * sy) as u32;
    let x1 = ((layout.x0 + layout.cols as f32 * layout.cell_w) * sx) as u32;
    let y1 = ((layout.y0 + layout.rows as f32 * layout.cell_h) * sy) as u32;
    for y in y0..y1.min(img.height) {
        for x in x0..x1.min(img.width) {
            out.set(x, y, img.get(x, y));
        }
    }
    out
}

fn jitter_frame(img: &GrayImage, seed: usize) -> GrayImage {
    let mut h = (seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xA076_1D64_78BD_642F;
    h ^= h << 13;
    h ^= h >> 7;
    h ^= h << 17;
    let gain = 0.96 + ((h % 81) as f32) / 1000.0;
    let gamma = 0.96 + (((h >> 8) % 81) as f32) / 1000.0;
    let mut out = img.clone();
    for p in &mut out.pixels {
        let v = (*p as f32) / 255.0;
        *p = (255.0 * v.powf(gamma) * gain).clamp(0.0, 255.0) as u8;
    }
    out
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
