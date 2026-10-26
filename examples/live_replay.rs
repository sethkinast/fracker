//! Live-replay gate (pass 37): feed Seth's real v0.5.30
//! capture-debug dump (60 parts frames, `live-v0530/`) through
//! the production `CaptureSession` intake and check the
//! settle+time gate against the measured ground truth in
//! INTAKE_REPLAY.md:
//!
//!   * total accepts ≤ 21 (the shipped intake's count on the
//!     same frames — the gate may only remove offers);
//!   * every settled page is represented by at least one
//!     accept (page ranges from the replay's motion table);
//!   * the phase classification matches INTAKE_REPLAY §3:
//!     the five transient frames {11, 12, 34, 51, 52} measure
//!     off the settled band, everything else measures on it.
//!
//! Arrival times are modelled at the measured worker delivery
//! rate (~500 ms per frame; LIVE_DIAG2 §4) through the
//! session's feed clock — an input to the gate, not a policy
//! knob. The gate is intake-level: `finish_parts` is never
//! called; recognition is out of scope for this pass.
//!
//! Usage:
//!   cargo run --release --example live_replay -- \
//!     --frames ~/workspace/fracker-frames/live-v0530/capture-debug \
//!     --data data
//!
//! Exit code 0 and "GATE PASS" on success; non-zero with
//! "GATE FAIL" lines otherwise.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fracker::capture::{CaptureData, CaptureSession};
use fracker::ocr::{GrayImage, TesseractCliEngine};
use fracker::stitch;

fn arg(args: &[String], key: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == key)
        .map(|w| w[1].clone())
}

/// The transient frames per INTAKE_REPLAY §3 (fold phases
/// −19/+14/+51/+78/+30 px — off the lattice by ≫ the band).
const TRANSIENTS: [usize; 5] = [11, 12, 34, 51, 52];
/// Settled-page frame ranges (INTAKE_REPLAY §3): page 1's run,
/// page 2's REST run (13..33 — 11/12 are its landing
/// transients), page 3's run, page 4's run.
const PAGES: [(&str, usize, usize); 4] = [
    ("page 1", 0, 10),
    ("page 2", 13, 33),
    ("page 3", 35, 50),
    ("page 4", 53, 59),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames_dir = PathBuf::from(arg(&args, "--frames").expect("--frames <dir>"));
    let data_dir = PathBuf::from(arg(&args, "--data").expect("--data <dir>"));

    let data = CaptureData::load(&data_dir).expect("load capture data");
    let ctx = data.context();
    let layout = ctx.layouts.get("parts").expect("parts layout").clone();
    let engine = TesseractCliEngine::probe(layout.clone()).expect("tesseract engine");

    let mut session = CaptureSession::new();
    session.set_stitch_engine(Box::new(
        TesseractCliEngine::probe(layout.clone()).expect("tesseract stitch engine"),
    ));
    // Model the worker's delivery rate: one frame per ~500 ms.
    let tick = Arc::new(AtomicU64::new(0));
    let base = Instant::now();
    session.set_feed_clock(Box::new(move || {
        base + Duration::from_millis(500 * tick.fetch_add(1, Ordering::Relaxed))
    }));

    // The dump's parts frames in capture order.
    let mut names: Vec<String> = std::fs::read_dir(&frames_dir)
        .expect("read frames dir")
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            (name.starts_with("frame-parts-") && name.ends_with(".png")).then_some(name)
        })
        .collect();
    names.sort();
    assert_eq!(names.len(), 60, "expected the 60 dumped parts frames");

    let mut failures: Vec<String> = Vec::new();
    let mut accepts_in_page = [0usize; 4];
    println!(" idx  phase(px)  verdict");
    for (idx, name) in names.iter().enumerate() {
        let bytes = std::fs::read(frames_dir.join(name)).expect("read frame");
        let img = GrayImage::from_bytes(&bytes).expect("decode frame png");
        let phase = stitch::lattice_phase(&img, &layout);
        // Phase classification vs INTAKE_REPLAY §3.
        match phase {
            Some(p) if TRANSIENTS.contains(&idx) => {
                if p.abs() <= fracker::capture::SETTLED_PHASE_MAX_PX {
                    failures.push(format!(
                        "frame {idx}: transient measured ON-lattice (phase {p:+.2})"
                    ));
                }
            }
            Some(p) => {
                if p.abs() > fracker::capture::SETTLED_PHASE_MAX_PX {
                    failures.push(format!(
                        "frame {idx}: settled/landing frame measured OFF-lattice (phase {p:+.2})"
                    ));
                }
            }
            None => failures.push(format!(
                "frame {idx}: phase unmeasurable (no lattice fit on a real frame)"
            )),
        }
        // Feed and classify the intake verdict by counter delta.
        let before = (
            session.parts_frames_accepted(),
            session.parts_dupes_dropped(),
            session.parts_off_lattice_dropped(),
            session.parts_interval_dropped(),
            session.parts_latched_dropped(),
            session.parts_frames_capped(),
        );
        session.process_frame(&ctx, &engine, &img);
        let after = (
            session.parts_frames_accepted(),
            session.parts_dupes_dropped(),
            session.parts_off_lattice_dropped(),
            session.parts_interval_dropped(),
            session.parts_latched_dropped(),
            session.parts_frames_capped(),
        );
        let verdict = if after.0 > before.0 {
            "ACCEPT"
        } else if after.2 > before.2 {
            "off-lattice"
        } else if after.3 > before.3 {
            "interval"
        } else if after.4 > before.4 {
            "latched"
        } else if after.1 > before.1 {
            "duplicate"
        } else if after.5 > before.5 {
            "cap-refused"
        } else {
            "not-offered"
        };
        if verdict == "ACCEPT" {
            for (pi, (_, lo, hi)) in PAGES.iter().enumerate() {
                if (*lo..=*hi).contains(&idx) {
                    accepts_in_page[pi] += 1;
                }
            }
        }
        println!(
            " {idx:>3}  {}  {verdict}",
            phase.map_or_else(|| "   none".to_owned(), |p| format!("{p:+7.2}"))
        );
    }

    let accepts = session.parts_frames_accepted();
    println!(
        "intake totals: {} accepted, {} duplicates, {} off-lattice, {} interval, {} latched, {} cap-refused",
        accepts,
        session.parts_dupes_dropped(),
        session.parts_off_lattice_dropped(),
        session.parts_interval_dropped(),
        session.parts_latched_dropped(),
        session.parts_frames_capped()
    );
    if accepts > 21 {
        failures.push(format!(
            "accepts {accepts} exceed the shipped intake's 21 on the same frames"
        ));
    }
    for (pi, (label, _, _)) in PAGES.iter().enumerate() {
        if accepts_in_page[pi] == 0 {
            failures.push(format!("{label}: no settled page accept — page lost"));
        }
    }

    if failures.is_empty() {
        println!(
            "GATE PASS: {accepts} accepts (≤ 21), all four settled pages represented, phase classification matches INTAKE_REPLAY §3"
        );
    } else {
        for f in &failures {
            println!("GATE FAIL: {f}");
        }
        std::process::exit(1);
    }
}
