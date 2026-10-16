//! Derived state store (BRIEF §8): all derived, all rebuildable.
//! JSON file with atomic writes (temp file + rename).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Per-session record: missions seen, cracks, reward screens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionRecord {
    #[serde(default)]
    pub started: String,
    #[serde(default)]
    pub missions: Vec<String>,
    #[serde(default)]
    pub cracks: u32,
    #[serde(default)]
    pub reward_screens: u32,
}

/// Everything in §8. Rebuildable from public sources + log events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub display_name: String,
    /// Last XPInfo pull, uniqueNames of owned equipment.
    #[serde(default)]
    pub profile_snapshot_items: Vec<String>,
    #[serde(default)]
    pub profile_snapshot_at: String,
    /// Last Xuerian pull (wants).
    #[serde(default)]
    pub targets_snapshot: Vec<String>,
    #[serde(default)]
    pub targets_snapshot_at: String,
    /// Owned relic counts. `None` = unknown (pre-§8.1-bootstrap).
    #[serde(default)]
    pub relic_stock: Option<HashMap<String, u32>>,
    /// Held prime-part counts from the bootstrap dump + reward inference.
    #[serde(default)]
    pub parts_held: HashMap<String, u32>,
    #[serde(default)]
    pub session_log: Vec<SessionRecord>,
    /// Weekly item id -> completion ISO timestamp.
    #[serde(default)]
    pub weekly_ledger: HashMap<String, String>,
    /// Daily gauge id ("focus", "standing:<syndicate>", "sortie") -> value + date.
    #[serde(default)]
    pub daily_ledger: HashMap<String, DailyEntry>,
    /// Estimated event-resource earnings from event alert-mission
    /// completions (pack-defined resource).
    #[serde(default)]
    pub moonflesh_estimate: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DailyEntry {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub value: u32,
    #[serde(default)]
    pub estimated: bool,
}

impl AppState {
    pub fn state_file(data_dir: &Path) -> PathBuf {
        data_dir.join("state.json")
    }

    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let path = Self::state_file(data_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("state {}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("state read {}: {e}", path.display())),
        }
    }

    /// Atomic save: write temp + rename.
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| format!("state mkdir {}: {e}", data_dir.display()))?;
        let path = Self::state_file(data_dir);
        let tmp = path.with_extension("json.tmp");
        let text =
            serde_json::to_string_pretty(self).map_err(|e| format!("state encode: {e}"))?;
        std::fs::write(&tmp, text).map_err(|e| format!("state write: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("state rename: {e}"))?;
        Ok(())
    }

    /// Prune session log to the rolling 90-day window (BRIEF §8).
    pub fn prune_sessions(&mut self, now: DateTime<Utc>) {
        let cutoff = now - chrono::Duration::days(90);
        self.session_log.retain(|s| {
            s.started
                .parse::<DateTime<Utc>>()
                .map(|t| t >= cutoff)
                .unwrap_or(true)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_atomic_save() {
        let dir = std::env::temp_dir().join("fracker-state-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut st = AppState::default();
        st.display_name = "SethPrime".into();
        st.targets_snapshot = vec!["Khora Prime Blueprint".into()];
        st.save(&dir).unwrap();
        assert!(dir.join("state.json").exists());
        assert!(!dir.join("state.json.tmp").exists());
        let back = AppState::load(&dir).unwrap();
        assert_eq!(back.display_name, "SethPrime");
        assert_eq!(back.targets_snapshot.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_default() {
        let st =
            AppState::load(Path::new("/nonexistent-fracker-dir")).unwrap();
        assert_eq!(st.account_id, "");
        assert!(st.relic_stock.is_none());
    }
}
