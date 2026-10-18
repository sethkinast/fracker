//! Server-side tick persistence (ninth pass §2).
//!
//! The plan page's checkboxes used to persist only in `localStorage`,
//! which is dead/partitioned on a `file://` URL. With the resident serving
//! the page over loopback HTTP, the store of record moves here: the HTTP
//! server writes each change to `ticks.json` in the app data dir, keyed by
//! item id + in-game day, and re-applies them to every served page. Ticks
//! therefore survive regenerations, restarts, and browser switches;
//! `localStorage` remains only a progressive cache.
//!
//! Ticks belong to the daily reset, so only the current day is kept.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Completed plan-item ids per in-game day (`YYYY-MM-DD` -> ids).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickStore {
    #[serde(default)]
    pub days: HashMap<String, BTreeSet<String>>,
}

impl TickStore {
    /// Persisted location inside the app data dir.
    pub fn file(data_dir: &Path) -> PathBuf {
        data_dir.join("ticks.json")
    }

    /// Load the store, or an empty one when the file is missing/unreadable
    /// (a corrupt tick cache must never break report generation).
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Atomically persist (temp file + rename), creating the dir as needed.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("ticks dir {}: {e}", parent.display()))?;
            }
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| format!("ticks encode: {e}"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("ticks write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("ticks publish {}: {e}", path.display()))?;
        Ok(())
    }

    /// Record one checkbox state. An empty day or id is ignored (never a
    /// garbage key); clearing the last tick of a day drops the day.
    pub fn set(&mut self, day: &str, id: &str, done: bool) {
        let day = day.trim();
        let id = id.trim();
        if day.is_empty() || id.is_empty() {
            return;
        }
        let set = self.days.entry(day.to_owned()).or_default();
        if done {
            set.insert(id.to_owned());
        } else {
            set.remove(id);
        }
        if set.is_empty() {
            self.days.remove(day);
        }
    }

    /// True when `id` is ticked on `day`.
    pub fn is_done(&self, day: &str, id: &str) -> bool {
        self.days.get(day).map_or(false, |s| s.contains(id))
    }

    /// The completed ids for `day` (empty when none).
    pub fn ids_on(&self, day: &str) -> BTreeSet<String> {
        self.days.get(day).cloned().unwrap_or_default()
    }

    /// Drop every day but `day` (ticks belong to the daily reset).
    pub fn retain_day(&mut self, day: &str) {
        self.days.retain(|d, _| d == day);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fracker-ticks-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn set_is_done_and_clear_roundtrip() {
        let mut s = TickStore::default();
        assert!(!s.is_done("2026-10-18", "chunk-1"));
        s.set("2026-10-18", "chunk-1", true);
        s.set("2026-10-18", "wk-nightwave", true);
        assert!(s.is_done("2026-10-18", "chunk-1"));
        assert_eq!(s.ids_on("2026-10-18").len(), 2);
        s.set("2026-10-18", "chunk-1", false);
        assert!(!s.is_done("2026-10-18", "chunk-1"));
        // Clearing the last tick drops the day entirely.
        s.set("2026-10-18", "wk-nightwave", false);
        assert!(s.ids_on("2026-10-18").is_empty());
    }

    #[test]
    fn empty_day_or_id_is_ignored() {
        let mut s = TickStore::default();
        s.set("", "x", true);
        s.set("2026-10-18", "  ", true);
        assert!(s.days.is_empty());
    }

    #[test]
    fn save_and_load_roundtrip_is_atomic() {
        let dir = tmp("roundtrip");
        let path = TickStore::file(&dir);
        let mut s = TickStore::default();
        s.set("2026-10-18", "chunk-1", true);
        s.save(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists(), "no temp left behind");
        let loaded = TickStore::load(&path);
        assert_eq!(loaded, s);
        assert!(loaded.is_done("2026-10-18", "chunk-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_corrupt_file_is_empty() {
        let dir = tmp("bad");
        let path = TickStore::file(&dir);
        assert_eq!(TickStore::load(&path), TickStore::default());
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(TickStore::load(&path), TickStore::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ticks_are_day_scoped_and_retain_day_prunes() {
        let mut s = TickStore::default();
        s.set("2026-10-17", "chunk-1", true);
        s.set("2026-10-18", "chunk-2", true);
        assert!(!s.is_done("2026-10-18", "chunk-1"));
        s.retain_day("2026-10-18");
        assert!(s.is_done("2026-10-18", "chunk-2"));
        assert!(s.ids_on("2026-10-17").is_empty(), "yesterday retired");
    }
}
