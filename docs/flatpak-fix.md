# Fix brief — Flatpak CI build failure (v0.1.0)

Branch: `opencode/flatpak-fix` (created from master; check it out).
The v0.1.0 tag build failed in CI at step "Build bundle". Seth
captured the log; Graf verified the artifacts locally. Fix ALL
of the following in one branch — the goal is a green CI build on
the next tag, not one-fix-per-tag iteration.

## Failure 1 (observed): leptonica cmake in-source refusal

CI log:
```
CMake Error at CMakeLists.txt:16 (message):
  CMake generation is not possible within the source directory!
```
Leptonica 1.85's CMakeLists hard-refuses when
`CMAKE_CURRENT_SOURCE_DIR == CMAKE_CURRENT_BINARY_DIR`, and the
flatpak-builder cmake-ninja configuration landed in-source.
Verified locally: the release tarball contains NO stray
CMakeCache.txt and all three manifest sha256s are CORRECT
(leptonica / tesseract / eng.traineddata) — do not touch the
checksums.

Fix: stop relying on the builder's cmake-ninja dir behavior for
the C modules. Build leptonica AND tesseract (same guard family)
with `buildsystem: simple` and explicit out-of-tree commands:
`cmake -S . -B _build <existing config-opts>`, `cmake --build
_build`, `cmake --install _build`. Keep every existing
config-opt.

## Failure 2 (certain, next in line): wrong SDK extension id

The manifest asks for `org.freedesktop.Sdk.Extension.rust-stable`
and appends `/usr/lib/sdk/rust-stable/bin`. The canonical
extension id is **`org.freedesktop.Sdk.Extension.ruststable`**
(mount: `/usr/lib/sdk/ruststable/bin`). `tests/packaging.rs`
line 58 currently ASSERTS the wrong string — fix the test to
assert the canonical id (and the mount path).

## Failure 3 (certain, next in line): cargo has no network

flatpak-builder builds offline; the fracker module runs
`cargo build --release --locked` with 197 lockfile packages and
no vendored sources — it cannot fetch crates.io inside the
build sandbox. Vendor them properly:

- Generate cargo sources from `Cargo.lock` (flatpak-cargo-generator
  or `cargo vendor` — your choice) and commit the generated
  sources/config into the repo.
- Wire the manifest's fracker module to build fully offline
  against the vendored sources.
- The vendored config must not break the normal local
  `cargo build` / `cargo test` on the box (scope it to the
  flatpak build, e.g. via the manifest's env/commands, not by
  hijacking the repo's default cargo config).

## Gates and deliverables

- `cargo build` clean, `cargo test` green (packaging tests
  updated per failure 2, plus an assertion that the manifest's
  cargo build is offline-capable: vendored sources referenced).
- ONE logical commit on `opencode/flatpak-fix`, then PUSH.
- Final summary to stdout: what changed per failure, and the
  exact vendoring mechanism you used so the next dependency
  bump can regenerate it.
- Out of scope: everything else in the manifest (permissions,
  tessdata, launcher) unless it blocks the build.
