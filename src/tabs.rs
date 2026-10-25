//! Server-side tab persistence (thirteenth pass §2, addendum 46).
//!
//! The served plan's CSS-radio tab state reset to First Crack on every
//! 60s meta-refresh, kicking Seth out of Hunt/Backlog/Routine. Because the
//! resident serves the page itself (item 41), the active tab is recorded
//! here — the same store-of-record pattern as [`crate::ticks`] — and the
//! server renders the matching radio `checked` on subsequent serves. The
//! selection therefore survives the meta-refresh, app restarts, and new
//! regenerations alike.
//!
//! Item 52 originally scoped the choice to one plan generation (a new
//! report reset to First Crack). Addendum #86 supersedes that rule: a
//! fresh generation is minted on every regeneration — which the
//! resident triggers on each worldstate tick, minutes apart — so the
//! reset fired constantly and the #46 persistence never held in
//! practice. Per Seth's instruction the remembered tab now persists
//! across generations; the generation is still recorded with the
//! choice, and a slug that names no current tab falls back to First
//! Crack at render time.
//!
//! The value is a stable tab *slug* (see `render::TABS`), not a radio id,
//! so a future reordering of the tabs does not silently change what a
//! remembered selection means.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The active plan tab, as a stable slug (`hunt`, `backlog`, …), scoped to
/// the report generation that was being served when it was chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabStore {
    #[serde(default)]
    pub active: String,
    /// The `<body data-generation>` token of the report most recently
    /// served or chosen under. Recorded for observability; since
    /// addendum #86 it no longer gates the choice. Empty for a
    /// pre-item-52 file.
    #[serde(default)]
    pub generation: String,
}

impl TabStore {
    /// Persisted location inside the app data dir.
    pub fn file(data_dir: &Path) -> PathBuf {
        data_dir.join("plan-tab.json")
    }

    /// Load the store, or an empty one when the file is missing/unreadable
    /// (a corrupt tab cache must never break plan serving).
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
                    .map_err(|e| format!("tab dir {}: {e}", parent.display()))?;
            }
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| format!("tab encode: {e}"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("tab write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("tab publish {}: {e}", path.display()))?;
        Ok(())
    }

    /// Record the active tab slug. Empty input is ignored so a blank
    /// request can never clobber the remembered tab.
    pub fn set(&mut self, slug: &str) {
        let slug = slug.trim();
        if !slug.is_empty() {
            self.active = slug.to_owned();
        }
    }

    /// The remembered slug (`""` when nothing has been recorded).
    pub fn active(&self) -> &str {
        &self.active
    }

    /// Record the generation of the report being served or chosen
    /// under. Addendum #86 (superseding item 52's reset rule): the
    /// remembered choice is NOT cleared on a generation change —
    /// generations churn every few minutes, so clearing made the tab
    /// reset on ordinary refreshes. Returns true when the recorded
    /// generation changed (the caller should persist the store).
    pub fn scope_to(&mut self, generation: &str) -> bool {
        if self.generation == generation {
            return false;
        }
        self.generation = generation.to_owned();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fracker-tabs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn set_and_load_roundtrip_is_atomic() {
        let dir = tmp("roundtrip");
        let path = TabStore::file(&dir);
        assert_eq!(TabStore::load(&path), TabStore::default());
        let mut s = TabStore::default();
        s.set("hunt");
        assert_eq!(s.active(), "hunt");
        s.save(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists(), "no temp left behind");
        assert_eq!(TabStore::load(&path).active(), "hunt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_set_is_ignored() {
        let mut s = TabStore::default();
        s.set("  ");
        assert_eq!(s.active(), "", "a blank slug never clobbers the remembered tab");
        s.set("routine");
        s.set("");
        assert_eq!(s.active(), "routine");
    }

    #[test]
    fn missing_or_corrupt_file_is_empty() {
        let dir = tmp("bad");
        let path = TabStore::file(&dir);
        assert_eq!(TabStore::load(&path), TabStore::default());
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(TabStore::load(&path), TabStore::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Addendum #86 (superseding item 52): a tab choice survives
    /// generation changes — refreshes of any kind land back on it.
    #[test]
    fn scope_keeps_the_choice_across_generations() {
        let mut s = TabStore::default();
        assert!(s.scope_to("gen-1"), "first scope is a change");
        s.set("hunt");
        assert!(!s.scope_to("gen-1"), "same generation is not a change");
        assert_eq!(s.active(), "hunt");

        // A NEW regeneration records the new generation but keeps the
        // choice (generations churn on every worldstate tick).
        assert!(s.scope_to("gen-2"), "generation change is recorded");
        assert_eq!(s.active(), "hunt", "the choice survives regeneration");
        assert_eq!(s.generation, "gen-2");
    }

    #[test]
    fn a_pre_item_52_store_loads_as_unscoped() {
        let dir = tmp("legacy");
        let path = TabStore::file(&dir);
        std::fs::write(&path, r#"{"active":"routine"}"#).unwrap();
        let mut s = TabStore::load(&path);
        assert_eq!(s.active(), "routine");
        assert_eq!(s.generation, "");
        assert!(s.scope_to("gen-1"), "scoping records the generation");
        assert_eq!(s.active(), "routine", "the legacy choice is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
