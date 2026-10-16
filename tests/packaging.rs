//! Packaging contract (sixth pass §3): the Flatpak manifest and CI
//! workflow exist in-repo, parse, and agree with the data-owned paths.
//!
//! Everything here is verifiable from this box (aarch64): the manifest
//! text + structure, the workflow YAML, and the cross-file invariants.
//! The x86_64 bundle build itself runs in CI.

use std::path::PathBuf;

use fracker::config::EeLogPaths;

const APP_ID: &str = "io.github.sethkinast.Fracker";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let p = root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Deep-parse a YAML file with python3 + PyYAML when available. Returns
/// false (test skips) when the toolchain is missing; a parse error on an
/// available toolchain fails the test.
fn py_yaml_parse(rel: &str) -> bool {
    let script = "import sys, yaml\nwith open(sys.argv[1]) as f:\n    yaml.safe_load(f)\n";
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(root().join(rel))
        .output();
    match out {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            // Distinguish "no PyYAML" (skip) from a real parse failure.
            let err = String::from_utf8_lossy(&o.stderr);
            if err.contains("ModuleNotFoundError") || err.contains("ImportError") {
                eprintln!("skip {rel}: PyYAML unavailable");
                false
            } else {
                panic!("{rel} does not parse as YAML:\n{err}");
            }
        }
        Err(e) => {
            eprintln!("skip {rel}: python3 unavailable ({e})");
            false
        }
    }
}

#[test]
fn flatpak_manifest_builds_binary_and_ocr_sidecar() {
    let manifest = read("io.github.sethkinast.Fracker.yml");
    // App identity + runtime + the Rust SDK extension.
    assert!(manifest.contains(&format!("app-id: {APP_ID}")));
    assert!(manifest.contains("runtime: org.freedesktop.Platform"));
    assert!(manifest.contains("org.freedesktop.Sdk.Extension.rust-stable"));
    // No OCR = packaging failure: leptonica + tesseract + eng data are
    // all built/bundled, and the binary is compiled from source.
    for needle in [
        "name: leptonica",
        "name: tesseract",
        "eng.traineddata",
        "cargo build --release --locked",
    ] {
        assert!(manifest.contains(needle), "manifest missing: {needle}");
    }
    if py_yaml_parse("io.github.sethkinast.Fracker.yml") {
        eprintln!("manifest parsed as YAML");
    }
}

#[test]
fn manifest_grants_the_ee_log_containing_folder() {
    // Item 35: the folder grant is primary. The granted path must match
    // the data-owned Steam/Proton default in data/paths.toml.
    let paths =
        EeLogPaths::load(&root().join("data/paths.toml")).expect("data/paths.toml");
    let folder = paths.primary_containing_folder().expect("primary folder");
    let manifest = read("io.github.sethkinast.Fracker.yml");
    assert!(
        manifest.contains(&format!("--filesystem=~/{folder}")),
        "manifest must grant the EE.log containing folder ~/{folder}"
    );
    assert!(manifest.contains("--talk-name=org.kde.StatusNotifierWatcher"));
    assert!(manifest.contains("--share=network"));
    assert!(manifest.contains("--socket=session-bus"));
}

#[test]
fn launcher_points_engine_probes_at_bundled_seams() {
    let launcher = read("flatpak/fracker.sh");
    for needle in [
        "FRACKER_DATA_DIR",
        "TESSDATA_PREFIX",
        "FRACKER_TESSERACT",
        "/app/bin/fracker.bin",
    ] {
        assert!(launcher.contains(needle), "launcher missing: {needle}");
    }
    // The tesseract probe checks FRACKER_TESSERACT first.
    let probes = read("src/ocr.rs");
    let env_idx = probes.find("env:FRACKER_TESSERACT").expect("env probe");
    let path_idx = probes.find("\"tesseract\".to_owned()").expect("PATH probe");
    assert!(env_idx < path_idx, "sidecar/env probe must precede $PATH");
}

#[test]
fn desktop_and_metainfo_carry_the_app_id() {
    let desktop = read(&format!("flatpak/{APP_ID}.desktop"));
    assert!(desktop.contains(&format!("Icon={APP_ID}")));
    let metainfo = read(&format!("flatpak/{APP_ID}.metainfo.xml"));
    assert!(metainfo.contains(&format!("<id>{APP_ID}</id>")));
    assert!(root().join(format!("flatpak/{APP_ID}.svg")).exists());
}

#[test]
fn workflow_dispatches_and_attaches_a_single_bundle() {
    let wf = read(".github/workflows/flatpak.yml");
    assert!(wf.contains("workflow_dispatch:"));
    assert!(wf.contains("tags:"));
    assert!(wf.contains("flathub-infra/flatpak-github-actions/flatpak-builder@"));
    assert!(wf.contains("bundle: fracker.flatpak"));
    assert!(wf.contains("manifest-path: io.github.sethkinast.Fracker.yml"));
    // Attached to the run (artifact) and to the release (tags).
    assert!(wf.contains("actions/upload-artifact@"));
    assert!(wf.contains("action-gh-release@"));
    if py_yaml_parse(".github/workflows/flatpak.yml") {
        eprintln!("workflow parsed as YAML");
    }
}
