//! The core-event log (fifty-first pass, Seth's spec): an
//! append-only JSON Lines store, `events.jsonl` in the app
//! data dir beside `stock.json`, recording the three state
//! changes Seth used to dig through `capture.log` by hand to
//! find — a reward-detection credit, a proven alert
//! completion, and a relic crack episode whose burst missed.
//! The resident is the single writer; the report reads the
//! file to render the Log tab and to carry the latest credit
//! to the page for the acquisition toast. A malformed line
//! is skipped at load, never fatal (the `ticks`/`tabs`
//! store-of-record precedent: a corrupt sidecar must not
//! break the plan).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The file is capped at the most recent this-many records;
/// the append that crosses the cap rewrites the tail.
pub const MAX_RECORDS: usize = 500;

/// Serialises appends: the watch thread (alert completions)
/// and the reward task (credits / crack misses) both write
/// from the one resident process.
static APPEND_GATE: Mutex<()> = Mutex::new(());

/// The three logged state changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Reward detection credited a part.
    Credit,
    /// An alert was seen completed (mission tracker proof).
    AlertDone,
    /// A relic crack episode whose burst detected nothing.
    CrackMiss,
}

/// One `events.jsonl` record. `ts` is UTC RFC 3339 (the
/// store's clock); rendering converts to local time. The
/// optional fields are kind-specific and absent (not null)
/// when they do not apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub ts: String,
    pub kind: EventKind,
    /// Credit: the roster part name that was credited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    /// Credit: the part's parent item (the toast's icon key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gear: Option<String>,
    /// AlertDone: the alert's identity as known at arm time
    /// (event tag / node from the armed `MissionInfo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// CrackMiss: the short plain reason class
    /// (`no bordered card` / `name not proven` / …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// CrackMiss: Seth's roll that episode, when the arm
    /// event's path resolved to a display name. Absent rather
    /// than a raw path when it did not resolve (pass 51 spec).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<String>,
}

impl EventRecord {
    pub fn credit(ts: String, part: String, gear: Option<String>) -> Self {
        Self {
            ts,
            kind: EventKind::Credit,
            part: Some(part),
            gear,
            label: None,
            reason: None,
            roll: None,
        }
    }

    pub fn alert_done(ts: String, label: String) -> Self {
        Self {
            ts,
            kind: EventKind::AlertDone,
            part: None,
            gear: None,
            label: Some(label),
            reason: None,
            roll: None,
        }
    }

    pub fn crack_miss(ts: String, reason: String, roll: Option<String>) -> Self {
        Self {
            ts,
            kind: EventKind::CrackMiss,
            part: None,
            gear: None,
            label: None,
            reason: Some(reason),
            roll,
        }
    }

    /// The Log tab's one-line plain-words rendering of the
    /// event (the timestamp is rendered separately).
    pub fn line(&self) -> String {
        match self.kind {
            EventKind::Credit => format!(
                "Relic cracked — {}",
                self.part.as_deref().unwrap_or("unknown part")
            ),
            EventKind::AlertDone => format!(
                "Alert completed — {}",
                self.label.as_deref().unwrap_or("alert")
            ),
            EventKind::CrackMiss => {
                let mut s = format!(
                    "Relic crack — no item detected ({})",
                    self.reason.as_deref().unwrap_or("missed")
                );
                if let Some(roll) = &self.roll {
                    s.push_str(&format!(" · roll: {roll}"));
                }
                s
            }
        }
    }
}

/// The store's location inside a data dir.
pub fn file(data_dir: &Path) -> PathBuf {
    data_dir.join("events.jsonl")
}

/// The store clock: UTC now as RFC 3339.
pub fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Append one record, enforcing the cap. Best-effort by
/// design: the log is a sidecar to the state change itself
/// (the credit / the completion ledger already landed), so
/// an I/O failure is swallowed rather than failing the
/// resident path that produced the event — the same
/// discipline as the search-index sidecar (pass 47).
pub fn append(data_dir: &Path, rec: &EventRecord) {
    let _gate = APPEND_GATE.lock().unwrap_or_else(|p| p.into_inner());
    let path = file(data_dir);
    let Ok(line) = serde_json::to_string(rec) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let appended = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        Ok(())
    })();
    if appended.is_err() {
        return;
    }
    // Cap: only the crossing append pays the rewrite.
    if let Ok(text) = std::fs::read_to_string(&path) {
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.len() > MAX_RECORDS {
            let tail = &lines[lines.len() - MAX_RECORDS..];
            let mut body = tail.join("\n");
            body.push('\n');
            let tmp = path.with_extension("jsonl.tmp");
            if std::fs::write(&tmp, body).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

/// Load every record in file order (oldest first). A missing
/// file is an empty log; a malformed line is skipped — one
/// bad line must never break the Log tab's render.
pub fn load(data_dir: &Path) -> Vec<EventRecord> {
    let Ok(text) = std::fs::read_to_string(file(data_dir)) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The newest credit in the log, if any — the toast's
/// payload source.
pub fn latest_credit(events: &[EventRecord]) -> Option<&EventRecord> {
    events
        .iter()
        .rev()
        .find(|e| e.kind == EventKind::Credit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fracker-events-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roundtrip_and_schema_shape() {
        let dir = tmpdir("roundtrip");
        append(
            &dir,
            &EventRecord::credit(
                "2026-10-28T05:00:00Z".into(),
                "Voruna Prime Chassis Blueprint".into(),
                Some("Voruna Prime".into()),
            ),
        );
        append(
            &dir,
            &EventRecord::alert_done(
                "2026-10-28T05:01:00Z".into(),
                "RoadToTauEvent · SolNode175".into(),
            ),
        );
        append(
            &dir,
            &EventRecord::crack_miss(
                "2026-10-28T05:02:00Z".into(),
                "no bordered card".into(),
                None,
            ),
        );
        let text = std::fs::read_to_string(file(&dir)).unwrap();
        // The on-disk schema: kind snake_case, absent fields
        // omitted rather than null.
        assert!(text.contains("\"kind\":\"credit\""), "{text}");
        assert!(text.contains("\"kind\":\"alert_done\""), "{text}");
        assert!(text.contains("\"kind\":\"crack_miss\""), "{text}");
        assert!(!text.contains("\"roll\":null"), "{text}");
        let loaded = load(&dir);
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].kind, EventKind::Credit);
        assert_eq!(
            loaded[0].line(),
            "Relic cracked — Voruna Prime Chassis Blueprint"
        );
        assert_eq!(
            loaded[1].line(),
            "Alert completed — RoadToTauEvent · SolNode175"
        );
        assert_eq!(
            loaded[2].line(),
            "Relic crack — no item detected (no bordered card)"
        );
        assert_eq!(
            latest_credit(&loaded).unwrap().part.as_deref(),
            Some("Voruna Prime Chassis Blueprint")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crack_miss_line_carries_the_roll_when_resolved() {
        let rec = EventRecord::crack_miss(
            "2026-10-28T05:02:00Z".into(),
            "name not proven".into(),
            Some("Meso C11".into()),
        );
        assert_eq!(
            rec.line(),
            "Relic crack — no item detected (name not proven) · roll: Meso C11"
        );
    }

    #[test]
    fn malformed_lines_are_skipped_never_fatal() {
        let dir = tmpdir("malformed");
        std::fs::write(
            file(&dir),
            "{\"ts\":\"2026-10-28T05:00:00Z\",\"kind\":\"credit\",\"part\":\"A Prime Blade\"}\n\
             not json at all\n\
             {\"ts\":\"2026-10-28T05:01:00Z\",\"kind\":\"bogus_kind\"}\n\
             \n\
             {\"ts\":\"2026-10-28T05:02:00Z\",\"kind\":\"alert_done\",\"label\":\"X\"}\n",
        )
        .unwrap();
        let loaded = load(&dir);
        assert_eq!(loaded.len(), 2, "{loaded:?}");
        assert_eq!(loaded[0].kind, EventKind::Credit);
        assert_eq!(loaded[1].kind, EventKind::AlertDone);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cap_keeps_only_the_most_recent_records() {
        let dir = tmpdir("cap");
        let path = file(&dir);
        // Seed past the cap directly (fast), then one append
        // must trim the file back to MAX_RECORDS, keeping the
        // newest lines in order.
        let mut seed = String::new();
        for i in 0..(MAX_RECORDS + 25) {
            seed.push_str(&format!(
                "{{\"ts\":\"2026-10-28T05:00:00Z\",\"kind\":\"alert_done\",\"label\":\"A{i}\"}}\n"
            ));
        }
        std::fs::write(&path, seed).unwrap();
        append(
            &dir,
            &EventRecord::alert_done("2026-10-28T06:00:00Z".into(), "NEWEST".into()),
        );
        let loaded = load(&dir);
        assert_eq!(loaded.len(), MAX_RECORDS);
        assert_eq!(loaded.last().unwrap().label.as_deref(), Some("NEWEST"));
        // The oldest survivors are the seed's tail: A25 is the
        // first kept (25 dropped of 525 + 1 appended - 500).
        assert_eq!(loaded.first().unwrap().label.as_deref(), Some("A26"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_of_a_missing_file_is_empty() {
        let dir = tmpdir("missing");
        assert!(load(&dir).is_empty());
        assert!(latest_credit(&[]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
