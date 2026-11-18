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
//! Scoping (pass 62 §5): most ticks belong to the daily reset, so only
//! the current day is kept — but Nightwave ticks (`nw-…`) belong to the
//! weekly reset. Weekly and Elite Weekly acts refresh every Sunday
//! 00:00 UTC, so an `nw-` tick is stored under the Sunday that starts
//! its week (`nw-week:YYYY-MM-DD`) and reads back on every later day of
//! that same week; the next Sunday's week starts unticked. Daily acts
//! turn over by id in the worldstate (a new day's daily act is a new
//! id), so week-scoping every `nw-` id is safe for them too.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

/// The store key prefix for Nightwave week buckets. A bucket holds the
/// `nw-…` ids ticked in the week starting on the Sunday in the key.
pub const NIGHTWAVE_WEEK_PREFIX: &str = "nw-week:";

/// True for a Nightwave act checkbox id (`nw-{act}` in the renderer).
pub fn is_nightwave_tick(id: &str) -> bool {
    id.starts_with("nw-")
}

/// The Sunday (UTC, `YYYY-MM-DD`) that starts the Nightwave week
/// containing `day` (`YYYY-MM-DD`). Nightwave Weekly and Elite Weekly
/// acts refresh every Sunday 00:00 UTC, so the week runs Sunday to
/// Saturday. `None` when `day` does not parse.
pub fn nightwave_week_sunday(day: &str) -> Option<String> {
    let date = NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d").ok()?;
    let back = i64::from(date.weekday().num_days_from_sunday());
    date.checked_sub_days(chrono::Days::new(back as u64))
        .map(|sunday| sunday.format("%Y-%m-%d").to_string())
}

/// The store key for the Nightwave week containing `day` — the bucket
/// an `nw-` tick made on `day` is stored under. `None` when `day` does
/// not parse (the caller falls back to the plain day key).
pub fn nightwave_week_key(day: &str) -> Option<String> {
    nightwave_week_sunday(day).map(|sunday| format!("{NIGHTWAVE_WEEK_PREFIX}{sunday}"))
}

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
    /// Nightwave ticks (`nw-…`) are week-scoped: they are stored under
    /// the `nw-week:` bucket for the tick's week (see
    /// [`nightwave_week_key`]), not the day, so a tick made Monday is
    /// still there Friday and only the Sunday reset clears it. A day
    /// that does not parse falls back to the plain day key.
    pub fn set(&mut self, day: &str, id: &str, done: bool) {
        let day = day.trim();
        let id = id.trim();
        if day.is_empty() || id.is_empty() {
            return;
        }
        let key = if is_nightwave_tick(id) {
            nightwave_week_key(day).unwrap_or_else(|| day.to_owned())
        } else {
            day.to_owned()
        };
        let set = self.days.entry(key.clone()).or_default();
        if done {
            set.insert(id.to_owned());
        } else {
            set.remove(id);
        }
        if set.is_empty() {
            self.days.remove(&key);
        }
    }

    /// True when `id` is ticked on `day`. A Nightwave id reads back on
    /// any day of the week it was ticked in — from its `nw-week:`
    /// bucket, or from a plain day key in the same week (ticks stored
    /// before pass 62, which keyed every tick by day).
    pub fn is_done(&self, day: &str, id: &str) -> bool {
        if is_nightwave_tick(id) {
            return self
                .nightwave_keys_for(day)
                .iter()
                .any(|k| self.days.get(k).map_or(false, |s| s.contains(id)));
        }
        self.days.get(day).map_or(false, |s| s.contains(id))
    }

    /// The completed ids for `day` (empty when none). For a Nightwave
    /// id this is the week's done set, not just today's ticks: an act
    /// ticked Monday is still done Friday.
    pub fn ids_on(&self, day: &str) -> BTreeSet<String> {
        let mut out = self.days.get(day).cloned().unwrap_or_default();
        for key in self.nightwave_keys_for(day) {
            if let Some(set) = self.days.get(&key) {
                out.extend(set.iter().filter(|id| is_nightwave_tick(id)).cloned());
            }
        }
        out
    }

    /// Every store key that can hold a Nightwave tick for `day`'s week:
    /// the `nw-week:` bucket, plus any plain day key in that week
    /// (pre-pass-62 storage, read for migration). Empty when `day`
    /// does not parse.
    fn nightwave_keys_for(&self, day: &str) -> Vec<String> {
        let Some(sunday) = nightwave_week_sunday(day) else {
            return Vec::new();
        };
        let Some(week_start) = NaiveDate::parse_from_str(&sunday, "%Y-%m-%d").ok() else {
            return Vec::new();
        };
        let mut keys = vec![format!("{NIGHTWAVE_WEEK_PREFIX}{sunday}")];
        for (key, _) in &self.days {
            if key.starts_with(NIGHTWAVE_WEEK_PREFIX) {
                continue;
            }
            if let Ok(date) = NaiveDate::parse_from_str(key, "%Y-%m-%d") {
                if date >= week_start
                    && date < week_start + chrono::Duration::days(7)
                {
                    keys.push(key.clone());
                }
            }
        }
        keys
    }

    /// Drop every day but `day`, and every Nightwave week but `day`'s
    /// week (ticks belong to the daily reset, Nightwave ticks to the
    /// Sunday reset). The server calls this after recording a tick
    /// made today, so stale days and last week's acts leave the file.
    pub fn retain_current(&mut self, day: &str) {
        let week_key = nightwave_week_key(day);
        self.days.retain(|d, _| {
            d == day || week_key.as_ref().map_or(false, |w| d == w)
        });
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
    fn ticks_are_day_scoped_and_retain_current_prunes() {
        let mut s = TickStore::default();
        s.set("2026-10-17", "chunk-1", true);
        s.set("2026-10-18", "chunk-2", true);
        assert!(!s.is_done("2026-10-18", "chunk-1"));
        s.retain_current("2026-10-18");
        assert!(s.is_done("2026-10-18", "chunk-2"));
        assert!(s.ids_on("2026-10-17").is_empty(), "yesterday retired");
    }

    /// Pass 62 §5: the Nightwave week starts Sunday 00:00 UTC.
    /// 2026-11-15 is a Sunday; 2026-11-16 (Mon) through 2026-11-21
    /// (Sat) share its week, and 2026-11-22 (Sun) starts a new one.
    #[test]
    fn nightwave_week_sunday_anchors_on_sunday() {
        assert_eq!(nightwave_week_sunday("2026-11-15").as_deref(), Some("2026-11-15"));
        assert_eq!(nightwave_week_sunday("2026-11-16").as_deref(), Some("2026-11-15"));
        assert_eq!(nightwave_week_sunday("2026-11-20").as_deref(), Some("2026-11-15"));
        assert_eq!(nightwave_week_sunday("2026-11-21").as_deref(), Some("2026-11-15"));
        assert_eq!(nightwave_week_sunday("2026-11-22").as_deref(), Some("2026-11-22"));
        assert_eq!(nightwave_week_sunday("2026-11-23").as_deref(), Some("2026-11-22"));
        assert_eq!(nightwave_week_sunday("not-a-day"), None);
        assert_eq!(
            nightwave_week_key("2026-11-16").as_deref(),
            Some("nw-week:2026-11-15")
        );
    }

    /// Pass 62 §5, the acceptance case: tick Monday, still done
    /// Friday; cleared after the Sunday reset. Daily ticks keep
    /// their day scope alongside.
    #[test]
    fn nightwave_tick_survives_the_week_and_clears_after_sunday_reset() {
        let mut s = TickStore::default();
        // Stored under the week bucket, not the day.
        s.set("2026-11-16", "nw-weekly-sortie", true);
        assert!(s.days.contains_key("nw-week:2026-11-15"));
        assert!(!s.days.contains_key("2026-11-16"));
        assert!(s.is_done("2026-11-16", "nw-weekly-sortie"), "ticked Monday");
        assert!(s.is_done("2026-11-20", "nw-weekly-sortie"), "still done Friday");
        assert!(s.is_done("2026-11-21", "nw-weekly-sortie"), "still done Saturday");
        assert!(
            !s.is_done("2026-11-22", "nw-weekly-sortie"),
            "Sunday reset clears it"
        );
        assert!(
            !s.is_done("2026-11-23", "nw-weekly-sortie"),
            "new week starts unticked"
        );
        assert!(
            s.ids_on("2026-11-20").contains("nw-weekly-sortie"),
            "Friday's done set carries the week's tick"
        );
        // A daily tick made the same Monday does NOT survive.
        s.set("2026-11-16", "chunk-1", true);
        assert!(!s.is_done("2026-11-17", "chunk-1"));
    }

    #[test]
    fn nightwave_tick_unticks_within_the_week() {
        let mut s = TickStore::default();
        s.set("2026-11-16", "nw-elite-1", true);
        assert!(s.is_done("2026-11-20", "nw-elite-1"));
        s.set("2026-11-20", "nw-elite-1", false);
        assert!(!s.is_done("2026-11-20", "nw-elite-1"));
        assert!(!s.days.contains_key("nw-week:2026-11-15"), "empty week drops");
    }

    /// Migration: ticks stored before pass 62 under a plain day key
    /// still read back on later days of that same week.
    #[test]
    fn legacy_day_keyed_nightwave_tick_reads_within_its_week() {
        let mut s = TickStore::default();
        s.days
            .entry("2026-11-16".to_owned())
            .or_default()
            .insert("nw-legacy".to_owned());
        assert!(s.is_done("2026-11-20", "nw-legacy"));
        assert!(!s.is_done("2026-11-22", "nw-legacy"), "next week is clear");
    }

    #[test]
    fn retain_current_keeps_this_week_nightwave_and_drops_last_week() {
        let mut s = TickStore::default();
        s.set("2026-11-16", "nw-last-week", true);
        s.set("2026-11-17", "chunk-old", true);
        s.set("2026-11-23", "nw-this-week", true);
        s.retain_current("2026-11-23");
        assert!(!s.is_done("2026-11-23", "nw-last-week"), "last week pruned");
        assert!(s.is_done("2026-11-23", "nw-this-week"), "this week kept");
        assert!(!s.is_done("2026-11-23", "chunk-old"), "old daily pruned");
    }
}
