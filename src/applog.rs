//! Resident app log (fifteenth pass §1, item 53).
//!
//! The tray Quit failed live twice while every unit/logic test passed. The
//! only way to see *where* a teardown stops is to write each step to a file
//! the user can hand back after one run. `resident.log` is that file:
//! append-only, timestamped, separate from the capture log.
//!
//! It is deliberately tiny and best-effort: a log that cannot be opened
//! degrades to a no-op and never changes runtime behavior.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Append-only `resident.log` in the app data dir.
#[derive(Clone)]
pub struct AppLog {
    path: PathBuf,
    file: Arc<Mutex<Option<File>>>,
}

impl AppLog {
    /// Open (append) `<data_dir>/resident.log`, creating the directory when
    /// needed. A failure to open is not fatal.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("resident.log");
        let _ = std::fs::create_dir_all(data_dir);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        Self {
            path,
            file: Arc::new(Mutex::new(file)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one timestamped line: `<local time> [area] message`.
    pub fn record(&self, area: &str, message: &str) {
        let Ok(mut guard) = self.file.lock() else {
            return;
        };
        let Some(f) = guard.as_mut() else {
            return;
        };
        use std::io::Write;
        let stamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z");
        let _ = writeln!(f, "{stamp} [{area}] {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fracker-applog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn appends_timestamped_lines_and_creates_the_dir() {
        let dir = tmp("append");
        let log = AppLog::open(&dir.join("state"));
        assert_eq!(log.path(), dir.join("state/resident.log"));
        log.record("teardown", "start");
        log.record("teardown", "done");
        let text = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("[teardown] start"), "{text}");
        assert!(lines[1].contains("[teardown] done"), "{text}");
        assert!(lines[0].starts_with("20"), "timestamped: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unopenable_path_is_a_silent_no_op() {
        // A path whose parent is a regular file cannot be created; logging
        // must never panic or change behavior.
        let dir = tmp("noop");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        let log = AppLog::open(&file);
        log.record("teardown", "still fine");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
