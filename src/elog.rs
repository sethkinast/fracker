//! EE.log parsing, event classification, session state machine, and
//! account-ID candidate ranking.
//!
//! Attribution / thin-layer note (BRIEF §5, §16): this catalog is meant
//! to come from the upstream **`wf-log`** crate of the warframe-lite
//! project (MIT) as a Cargo git dependency. That integration was
//! attempted for this pass and genuinely would not resolve from here:
//! the crate is absent from crates.io (`wf-log` / `wf_log` both 404 on
//! the registry API, as the brief already notes) and no public
//! `warframe-lite` git remote exists to pin (github.com/warframe-lite
//! 404s; no `wf-log` repository is discoverable), so there is no rev to
//! pin and nothing for `cargo vendor` to cover. Per the brief's stated
//! fallback, the §7.2 marker catalog is therefore implemented locally
//! below, with this attribution comment. If upstream becomes reachable,
//! `classify()` is the seam to replace with the crate's classifier.
//!
//! All patterns below are ASSUMPTIONS until validated against Seth's
//! real log (BRIEF §7.1, §12). Tests use synthetic lines built from the
//! documented markers and are labeled synthetic.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Line model
// ---------------------------------------------------------------------------

/// One parsed EE.log line: (timestamp, subsystem, message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub timestamp: Option<String>,
    pub subsystem: String,
    pub message: String,
}

/// Parse an EE.log line. Real format (fixtures/VALIDATION.md):
/// `<elapsed.sec> <Subsystem> [<Level>]: <message>`
/// (e.g. `14.148 Sys [Info]: Logged in Jenterrobahne`). Unparseable
/// lines yield an empty-subsystem line carrying the raw text so the
/// tail never dies.
pub fn parse_line(raw: &str) -> LogLine {
    let raw = raw.trim_end_matches(['\r', '\n']);
    let (ts, rest) = split_timestamp(raw);
    if let Some((sub, msg)) = split_subsystem(rest) {
        return LogLine {
            timestamp: ts,
            subsystem: sub,
            message: msg,
        };
    }
    LogLine {
        timestamp: ts,
        subsystem: String::new(),
        message: rest.to_owned(),
    }
}

/// Split `Sub [Level]: message` (or bare `Sub: message`). The bracketed
/// level is consumed, not kept: classification keys on the subsystem.
fn split_subsystem(rest: &str) -> Option<(String, String)> {
    let (head, msg) = rest.split_once(':')?;
    let sub = head.split_whitespace().next()?;
    if sub.is_empty()
        || sub.len() > 24
        || !sub
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let tail = head[sub.len()..].trim();
    if !tail.is_empty() && !(tail.starts_with('[') && tail.ends_with(']')) {
        return None;
    }
    Some((sub.to_owned(), msg.trim_start().to_owned()))
}

fn split_timestamp(raw: &str) -> (Option<String>, &str) {
    let t = raw.trim_start();
    let inner = t.strip_prefix('[').and_then(|s| s.split_once(']'));
    if let Some((ts, rest)) = inner {
        if ts.chars().any(|c| c.is_ascii_digit()) && ts.contains(':') {
            return (Some(ts.to_owned()), rest.trim_start());
        }
    }
    // Bare "HH:MM:SS.mmm ..." prefix, or the real EE.log shape:
    // elapsed seconds ("14.148 Sys [Info]: ...").
    let mut parts = t.splitn(2, ' ');
    let head = parts.next().unwrap_or("");
    let tail = parts.next().unwrap_or("");
    if head.contains(':') && head.chars().all(|c| c.is_ascii_digit() || c == ':' || c == '.') {
        return (Some(head.to_owned()), tail);
    }
    if head.contains('.') && head.parse::<f64>().is_ok() {
        return (Some(head.to_owned()), tail);
    }
    (None, raw)
}

// ---------------------------------------------------------------------------
// Event catalog (BRIEF §7.2)
// ---------------------------------------------------------------------------

/// Classified mission/log events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `Sys` subsystem, message starts `Logged in `. Carries display name.
    LoggedIn { name: String },
    /// Message contains `with MissionInfo:`.
    MissionInfoSeen,
    /// Message contains `Game successfully connected to:`.
    LevelConnected,
    /// `VoidProjections: <account-id> gets reward <uniqueName>` for
    /// SETH's account id only (pass 49 rekey). Carries the reward's
    /// uniqueName path (his own relic's ROLL — never his pick, which
    /// the log cannot see). Classified only by
    /// [`classify_with_account`]: the context-free [`classify`] never
    /// produces this variant, because the id is the whole signal.
    /// The `ProjectionRewardChoice.lua` lines are NOT classified:
    /// `Relic rewards initialized` also fires when an endless
    /// mission's relic-selection flow closes with no reward screen
    /// at all (Seth's log, 2026-10-27, game t=3075.684) — a phantom
    /// arm — while the gets-reward line fires exactly once per true
    /// screen, for his id alone, host or client.
    RelicReward { roll: String },
    /// `ThemedDetailedPurchaseDialog.lua: PopulateInfo-><path>`
    /// whose path contains `/BoosterPacks/` (fifty-third
    /// pass): a relic pack purchase dialog opened — the arm
    /// signal for pack-open detection. Any pack source
    /// classifies (syndicate, Teshin, market — Seth's scope
    /// ruling); a PopulateInfo for anything else does not.
    /// Carries the item path (diagnostic only).
    PackPurchase { path: String },
    /// Message contains `DVRCAftermath`.
    RelicCrack,
    /// `ThemedProjectionManager.lua: PopulateInventoryGrid`.
    RelicInventoryOpen,
    /// `InventoryTest.lua: PopulateGrid`.
    InventorySellOpen,
    /// `Game` subsystem, contains `HostMigration`.
    HostMigration,
}

impl Event {
    /// True when this event marks a completed mission run (sixth pass
    /// §4): his reward grant (pass 49: the gets-reward moment, ~0.3 s
    /// before the old `Got rewards` marker — the same regeneration,
    /// a hair earlier) or the post-crack aftermath transmission on
    /// hub return. The live watch loop turns these into an automatic
    /// page regeneration.
    pub fn is_mission_end(&self) -> bool {
        matches!(self, Event::RelicReward { .. } | Event::RelicCrack)
    }
}

/// Substring classification per the §7.2 catalog.
pub fn classify(line: &LogLine) -> Option<Event> {
    classify_inner(line, "")
}

/// Classification with Seth's account id wired in (pass 49): the
/// only route that can produce [`Event::RelicReward`], which keys
/// on `VoidProjections: <account-id> gets reward …`. The id comes
/// from the caller (the resident passes
/// `report::PROFILE_ACCOUNT_ID` — the same source the profile
/// fetch uses, never a second copy).
pub fn classify_with_account(line: &LogLine, account_id: &str) -> Option<Event> {
    classify_inner(line, account_id)
}

/// Parse `VoidProjections: <account-id> gets reward <path>` —
/// Some(path) only when the id is exactly Seth's. Another player's
/// id (or the `Host/Client got reward info from` traffic) never
/// matches: the full id plus the literal ` gets reward ` must
/// follow the prefix, so even an id that prefixes his cannot.
fn gets_reward_roll(msg: &str, account_id: &str) -> Option<String> {
    if account_id.is_empty() {
        return None;
    }
    let rest = msg.strip_prefix("VoidProjections: ")?;
    let rest = rest.strip_prefix(account_id)?;
    let path = rest.strip_prefix(" gets reward ")?.trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_owned())
    }
}

fn classify_inner(line: &LogLine, account_id: &str) -> Option<Event> {
    let msg = line.message.as_str();
    if line.subsystem == "Sys" && msg.starts_with("Logged in ") {
        let name = msg["Logged in ".len()..].trim().to_owned();
        if !name.is_empty() {
            return Some(Event::LoggedIn { name });
        }
        return None;
    }
    if msg.contains("with MissionInfo:") {
        return Some(Event::MissionInfoSeen);
    }
    if msg.contains("Game successfully connected to:") {
        return Some(Event::LevelConnected);
    }
    if let Some(roll) = gets_reward_roll(msg, account_id) {
        return Some(Event::RelicReward { roll });
    }
    if let Some(path) = msg
        .strip_prefix("ThemedDetailedPurchaseDialog.lua: PopulateInfo->")
        .map(str::trim)
        .filter(|p| !p.is_empty() && p.contains("/BoosterPacks/"))
    {
        return Some(Event::PackPurchase {
            path: path.to_owned(),
        });
    }
    if msg.contains("DVRCAftermath") {
        return Some(Event::RelicCrack);
    }
    if msg.contains("ThemedProjectionManager.lua: PopulateInventoryGrid") {
        return Some(Event::RelicInventoryOpen);
    }
    if msg.contains("InventoryTest.lua: PopulateGrid") {
        return Some(Event::InventorySellOpen);
    }
    if line.subsystem == "Game" && msg.contains("HostMigration") {
        return Some(Event::HostMigration);
    }
    None
}

// ---------------------------------------------------------------------------
// Mission completions (ninth pass §4)
// ---------------------------------------------------------------------------

/// The identity a `MissionInfo` block carries for one armed mission. The
/// log prints the whole block (multi-line JSON) after
/// `Client loaded {"name":"…"} with MissionInfo:`; `alertId` / `alertTag`
/// / `missionType` / `faction` / `location` / the reward `ItemType`s are
/// what let a completion be matched to a live worldstate alert.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionInfo {
    /// The `{"name":"SolNode175_Alert"}` from the loading line.
    #[serde(default)]
    pub name: String,
    /// `alertId` — the strongest identity (matches a worldstate alert id).
    #[serde(default)]
    pub alert_id: String,
    /// `alertTag` — e.g. `RoadToTauEvent`.
    #[serde(default)]
    pub alert_tag: String,
    /// Raw mission type (`MT_HIVE`, `MT_EXTERMINATION`, …).
    #[serde(default)]
    pub mission_type: String,
    /// Raw faction (`FC_INFESTATION`, …).
    #[serde(default)]
    pub faction: String,
    /// Internal node (`SolNode175`). EE.log and DE both speak these keys,
    /// so alert matching compares them exactly where both sides carry one
    /// (addendum #51 bonus); display nodes are never compared.
    #[serde(default)]
    pub location: String,
    /// `missionReward.countedItems[].ItemType` uniqueNames.
    #[serde(default)]
    pub reward_items: Vec<String>,
}

impl MissionInfo {
    fn from_json(v: &serde_json::Value) -> Self {
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned()
        };
        let reward_items = v
            .get("missionReward")
            .and_then(|r| r.get("countedItems"))
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        e.get("ItemType").and_then(|t| t.as_str()).map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            name: String::new(),
            alert_id: s("alertId"),
            alert_tag: s("alertTag"),
            mission_type: s("missionType"),
            faction: s("faction"),
            location: s("location"),
            reward_items,
        }
    }

    /// True when the block names a specific alert (not a plain mission).
    pub fn is_alert(&self) -> bool {
        !self.alert_id.is_empty() || !self.alert_tag.is_empty()
    }
}

/// A mission the log proved completed (an end-of-match success marker
/// after its `MissionInfo` was armed), scoped to the UTC day it fired.
/// The resident persists these and feeds them to every report so a
/// finished alert strikes regardless of any manual tick.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionCompletion {
    #[serde(default)]
    pub info: MissionInfo,
    /// UTC date (`YYYY-MM-DD`) the completion was observed.
    #[serde(default)]
    pub day: String,
    /// UTC wall-clock instant the completion was observed (`RFC 3339`).
    /// Twelfth-pass §4: the activation guard needs a trustworthy time, so
    /// a completion can only strike an alert instance that was already
    /// live when the mission ended. Empty on legacy/unknown records —
    /// those can still strike on an exact `alertId` match, never by
    /// corroboration.
    #[serde(default)]
    pub observed_at: String,
}

/// Clock-skew tolerance (seconds) for the §4 activation guard: a
/// completion observed a hair before the feed's activation still counts.
const ACTIVATION_EPSILON_SECS: i64 = 120;

impl MissionCompletion {
    /// A positive identity match against a worldstate alert — never
    /// inferred. Matches when the alert ids are equal, or when the event
    /// tag agrees AND every piece of identity evidence corroborates it.
    ///
    /// **Addendum #64 (full instance predicate).** A non-id match is the
    /// conjunction of:
    ///
    /// - the same event tag;
    /// - the same SolNode, when both sides carry one;
    /// - the same mission type, when both sides carry one (the node-only
    ///   fallback applies only when a type is absent on a side);
    /// - the pass-12 activation guard: the completion must be observed
    ///   at/after the alert's activation (an unparseable activation stays
    ///   fail-closed).
    ///
    /// Node+type alone is not instance identity across *repeated*
    /// instances — event alerts rerun on the same handful of nodes all
    /// day — so the activation window supplies the time dimension. A
    /// shared payout resource is deliberately not identity evidence: it is
    /// common to every same-tag sibling (Seth's four Road To Tau alerts
    /// all paid 50 Moonflesh). The exact `alertId` path is instance
    /// identity already and stays as-is.
    pub fn matches_alert(&self, alert: &crate::model::Alert) -> bool {
        let info = &self.info;
        if !info.alert_id.is_empty() && !alert.id.is_empty() && info.alert_id == alert.id {
            return true;
        }
        if !self.corroborates_activation(alert) {
            return false;
        }
        let tag_eq =
            !info.alert_tag.is_empty() && info.alert_tag.eq_ignore_ascii_case(&alert.tag);
        if !tag_eq {
            return false;
        }
        // Both sides carrying a node makes the node a required conjunct.
        let node_both = !info.location.is_empty() && !alert.mission.sol_node.is_empty();
        if node_both && !info.location.eq_ignore_ascii_case(&alert.mission.sol_node) {
            return false;
        }
        // Both sides carrying a type makes the type a required conjunct.
        let mine = mission_type_key(&info.mission_type);
        let theirs = mission_type_key(&alert.mission.mission_type);
        let type_both = !mine.is_empty() && !theirs.is_empty();
        if type_both && mine != theirs {
            return false;
        }
        // At least one positive identity clue beyond the tag must exist:
        // the node on both sides, or the type on both sides. When a type
        // is absent on a side, a same-node pair is the documented
        // fallback; a bare tag alone is not identity.
        node_both || type_both
    }

    /// The §4 activation guard for corroboration paths. An alert with no
    /// recorded activation cannot be guarded, so the corroboration is
    /// allowed (legacy feeds with no `Activation` field at all). A
    /// present-but-unreadable activation is different: DE sent a value we
    /// could not parse, so the guard stays closed and the loss is already
    /// in the worldstate gaps (addendum #56b). A completion with no
    /// trustworthy observed-at cannot corroborate an *activated* alert —
    /// it may only strike on exact id.
    fn corroborates_activation(&self, alert: &crate::model::Alert) -> bool {
        if alert.activation_degraded {
            return false;
        }
        let activation = match alert.activation.parse::<chrono::DateTime<chrono::Utc>>() {
            Ok(t) => t,
            Err(_) if alert.activation.is_empty() => return true,
            Err(_) => return false,
        };
        let observed = match self.observed_at.parse::<chrono::DateTime<chrono::Utc>>() {
            Ok(t) => t,
            Err(_) => return false,
        };
        observed + chrono::Duration::seconds(ACTIVATION_EPSILON_SECS) >= activation
    }
}

/// Normalize a mission type so `MT_MOBILE_DEFENSE` and `Mobile Defense`
/// compare equal (`MOBILEDEFENSE`). Empty in, empty out.
fn mission_type_key(t: &str) -> String {
    let t = t.strip_prefix("MT_").unwrap_or(t);
    t.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// Buffers the multi-line `MissionInfo` JSON blocks the log prints and
/// pairs an armed mission with its end-of-match success marker. Feed
/// every raw line; it returns a [`MissionCompletion`] the moment one is
/// proven. A completion with no armed mission (the resident started
/// mid-session) is dropped — never inferred from proximity alone.
#[derive(Debug, Default)]
pub struct MissionLog {
    /// Accumulating JSON body (`Some` while a `MissionInfo` block is open).
    buf: Option<String>,
    /// The `{"name":"…"}` from the blocking line.
    pending_name: Option<String>,
    /// The most recently armed mission, awaiting its end marker.
    armed: Option<MissionInfo>,
}

impl MissionLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The mission currently armed (if any) — for tests/diagnostics.
    pub fn armed(&self) -> Option<&MissionInfo> {
        self.armed.as_ref()
    }

    /// Feed one raw EE.log line. `day` is the UTC date to stamp onto a
    /// completion and `observed_at` the UTC instant it was observed (both
    /// supplied by the watch loop from its own clock — EE.log timestamps
    /// are seconds-since-process-start, not wall clock). Returns the
    /// completion when an end-of-match success marker confirms the armed
    /// mission.
    pub fn feed(&mut self, raw: &str, day: &str, observed_at: &str) -> Option<MissionCompletion> {
        if let Some(buf) = self.buf.as_mut() {
            if buf.len() > 256 * 1024 {
                // A `with MissionInfo:` that never resolves into JSON must
                // not accumulate forever.
                self.buf = None;
                return None;
            }
            buf.push_str(raw.trim_end_matches(['\r', '\n']));
            buf.push('\n');
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(buf) {
                let mut info = MissionInfo::from_json(&v);
                info.name = self.pending_name.take().unwrap_or_default();
                self.armed = Some(info);
                self.buf = None;
            }
            return None;
        }
        let line = raw.trim_end_matches(['\r', '\n']);
        if line.contains("with MissionInfo:") {
            self.pending_name = extract_loaded_name(line);
            self.buf = Some(String::new());
            return None;
        }
        if line.contains("GiveMissionRewards. success=true")
            || line.contains("EndOfMatch.lua: Mission Succeeded")
        {
            if let Some(info) = self.armed.take() {
                return Some(MissionCompletion {
                    info,
                    day: day.to_owned(),
                    observed_at: observed_at.to_owned(),
                });
            }
        }
        None
    }
}

/// Pull `SolNode175_Alert` out of `Client loaded {"name":"…"} with
/// MissionInfo:`.
fn extract_loaded_name(line: &str) -> Option<String> {
    let at = line.find("\"name\":\"")? + "\"name\":\"".len();
    let rest = &line[at..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

// ---------------------------------------------------------------------------
// Session state machine (BRIEF §7.3)
// ---------------------------------------------------------------------------

/// Session detector. Session start = first write after >= 30 min of
/// inactivity (or file recreation), confirmed by a `LoggedIn` line.
/// Session end = 30 min without writes (staleness only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPhase {
    Idle,
    /// Activity seen, waiting for `LoggedIn` confirmation.
    Arming,
    Active,
}

#[derive(Debug, Clone)]
pub struct SessionTracker {
    pub phase: SessionPhase,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            phase: SessionPhase::Idle,
        }
    }

    /// Feed one classified event. `gap_secs` = seconds since the
    /// previous write (u64::MAX on file recreation/first write).
    /// Returns true on a session-start transition.
    ///
    /// Segmentation rule (fixtures/VALIDATION.md, confirmed): one file
    /// spans many sessions — split on `Logged in` + inactivity, NEVER on
    /// file boundaries. Any `Logged in` (re)starts a session, even on a
    /// short gap; a file edge alone never does.
    pub fn feed(&mut self, event: Option<&Event>, gap_secs: u64) -> bool {
        if gap_secs >= 30 * 60 {
            // Long silence (or recreate): drop back to Idle/Arming.
            self.phase = SessionPhase::Idle;
        }
        if matches!(event, Some(Event::LoggedIn { .. })) {
            self.phase = SessionPhase::Active;
            return true;
        }
        if matches!(self.phase, SessionPhase::Idle) && event.is_some() {
            self.phase = SessionPhase::Arming;
        }
        false
    }

    /// 30 min without writes ends (stales) the session.
    pub fn check_stale(&mut self, quiet_secs: u64) -> bool {
        if quiet_secs >= 30 * 60 && self.phase == SessionPhase::Active {
            self.phase = SessionPhase::Idle;
            return true;
        }
        false
    }
}

impl Default for SessionTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Account-ID candidates (BRIEF §7.4 steps 1-2)
// ---------------------------------------------------------------------------

/// A 24-hex candidate ID with its rank signals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCandidate {
    pub id: String,
    /// (a) seen on a line containing the local display name.
    pub with_display_name: bool,
    /// (b) seen on a `VoidProjections:` line with `to host!` or
    /// `Client got reward info from`.
    pub void_projection_host: bool,
    /// (c) seen on an `Updated playerInfo for player` line with `Host=true`.
    pub updated_player_info_host: bool,
    pub sightings: u32,
}

impl AccountCandidate {
    fn rank_score(&self) -> u32 {
        (self.with_display_name as u32) * 100
            + (self.void_projection_host as u32) * 10
            + (self.updated_player_info_host as u32) * 5
            + self.sightings.min(5)
    }
}

/// Scan lines for candidate IDs: lowercase hex runs of exactly 24 chars
/// (uppercase `mm=` matchmaking GUIDs excluded). Step 3 (profile-API
/// confirmation) needs a real ID and is out of scope — ranked candidates
/// are returned best-first so confirmation can slot in.
pub fn rank_account_candidates(
    lines: &[LogLine],
    display_name: &str,
) -> Vec<AccountCandidate> {
    let mut map: HashMap<String, AccountCandidate> = HashMap::new();
    let name_lower = display_name.to_lowercase();
    for line in lines {
        for token in extract_hex24(&line.message) {
            let entry = map.entry(token.clone()).or_insert(AccountCandidate {
                id: token.clone(),
                with_display_name: false,
                void_projection_host: false,
                updated_player_info_host: false,
                sightings: 0,
            });
            entry.sightings += 1;
            if !name_lower.is_empty()
                && line.message.to_lowercase().contains(&name_lower)
            {
                entry.with_display_name = true;
            }
            if line.message.contains("VoidProjections:")
                && (line.message.contains("to host!")
                    || line.message.contains("Client got reward info from"))
            {
                entry.void_projection_host = true;
            }
            if line.message.contains("Updated playerInfo for player")
                && line.message.contains("Host=true")
            {
                entry.updated_player_info_host = true;
            }
        }
    }
    let mut out: Vec<AccountCandidate> = map.into_values().collect();
    out.sort_by(|a, b| {
        b.rank_score()
            .cmp(&a.rank_score())
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Lowercase hex runs of exactly 24 chars. Runs embedded in longer hex
/// strings do not count; uppercase runs (matchmaking GUIDs) excluded.
pub fn extract_hex24(message: &str) -> Vec<String> {
    let bytes = message.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_hexdigit() {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j - i == 24 && message[i..j].chars().all(|c| !c.is_ascii_uppercase()) {
                out.push(message[i..j].to_owned());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Markers below are transcribed-from-real-log: verbatim message text
    // catalogued in fixtures/VALIDATION.md (third-fixture section),
    // wrapped in the real `<elapsed.sec> Sub [Level]:` line shape.
    // (`HostMigration` has no verbatim exemplar in the catalog — its
    // test line stays synthetic.)

    /// Real-shaped line with a verbatim transcribed message.
    fn real(sub: &str, msg: &str) -> LogLine {
        parse_line(&format!("14.148 {sub} [Info]: {msg}"))
    }

    #[test]
    fn real_line_shape_parses_subsystem_and_level() {
        let l = parse_line("14.148 Sys [Info]: Logged in Jenterrobahne");
        assert_eq!(l.timestamp.as_deref(), Some("14.148"));
        assert_eq!(l.subsystem, "Sys");
        assert_eq!(l.message, "Logged in Jenterrobahne");
        // Bracketed shape without elapsed prefix still parses.
        let b = parse_line("[12:00:00.000] Sys [Info]: Logged in Jenterrobahne");
        assert_eq!(b.subsystem, "Sys");
        assert_eq!(b.message, "Logged in Jenterrobahne");
    }

    #[test]
    fn transcribed_catalog_markers() {
        // transcribed-from-real-log: `Logged in Jenterrobahne` (Sys/Info).
        assert_eq!(
            classify(&real("Sys", "Logged in Jenterrobahne")),
            Some(Event::LoggedIn {
                name: "Jenterrobahne".into()
            })
        );
        // transcribed-from-real-log: `Client loaded
        // {"name":"SolNode173_Alert"} with MissionInfo:` (+ full JSON).
        assert_eq!(
            classify(&real(
                "Game",
                r#"Client loaded {"name":"SolNode173_Alert"} with MissionInfo: {"missionType":"MT_EXTERMINATION"}"#
            )),
            Some(Event::MissionInfoSeen)
        );
        // transcribed-from-real-log: `===[ Game successfully connected
        // to: /Lotus/Levels/Proc/TheNewWar/PartTwo/TNWDrifterCampMain/
        // BoC8ACA.lp ]===` (hub transition, no MissionInfo armed).
        assert_eq!(
            classify(&real(
                "Game",
                "===[ Game successfully connected to: /Lotus/Levels/Proc/TheNewWar/PartTwo/TNWDrifterCampMain/ BoC8ACA.lp ]==="
            )),
            Some(Event::LevelConnected)
        );
        // transcribed-from-real-log: `ProjectionRewardChoice.lua: Relic
        // rewards initialized`. Pass 49: NOT classified — the same line
        // fires when an endless mission's relic-selection flow closes
        // with no reward screen (the phantom arm), so it can no longer
        // arm or regenerate anything.
        assert_eq!(
            classify(&real(
                "Script",
                "ProjectionRewardChoice.lua: Relic rewards initialized"
            )),
            None
        );
        assert_eq!(
            classify_with_account(
                &real(
                    "Script",
                    "ProjectionRewardChoice.lua: Relic rewards initialized"
                ),
                "5420fba23846322173707618"
            ),
            None
        );
        // transcribed-from-real-log: `ProjectionRewardChoice.lua: Got
        // rewards`. Pass 49: NOT classified either — the gets-reward
        // line below supersedes it as the reward moment (~0.3 s earlier
        // in the same screen's log sequence).
        assert_eq!(
            classify(&real("Script", "ProjectionRewardChoice.lua: Got rewards")),
            None
        );
        assert_eq!(
            classify_with_account(
                &real("Script", "ProjectionRewardChoice.lua: Got rewards"),
                "5420fba23846322173707618"
            ),
            None
        );
        // transcribed-from-real-log: `ProjectionRewardChoice.lua:
        // Selection countdown done` — terminal countdown, no state change.
        assert_eq!(
            classify(&real(
                "Script",
                "ProjectionRewardChoice.lua: Selection countdown done"
            )),
            None
        );
        // transcribed-from-real-log: `ProjectionRewardChoice.lua: Relic
        // reward screen shut down` — teardown, no state change.
        assert_eq!(
            classify(&real(
                "Script",
                "ProjectionRewardChoice.lua: Relic reward screen shut down"
            )),
            None
        );
        // transcribed-from-real-log: `DVRCAftermath` transmission on hub
        // return after a crack.
        assert_eq!(
            classify(&real("Game", "DVRCAftermath transmission")),
            Some(Event::RelicCrack)
        );
        // transcribed-from-real-log:
        // `ThemedProjectionManager.lua: PopulateInventoryGrid`.
        assert_eq!(
            classify(&real(
                "Script",
                "ThemedProjectionManager.lua: PopulateInventoryGrid"
            )),
            Some(Event::RelicInventoryOpen)
        );
        // transcribed-from-real-log: `InventoryTest.lua: PopulateGrid`.
        assert_eq!(
            classify(&real("Script", "InventoryTest.lua: PopulateGrid")),
            Some(Event::InventorySellOpen)
        );
        // Synthetic (no verbatim exemplar in the catalog).
        assert_eq!(
            classify(&real("Game", "something HostMigration happened")),
            Some(Event::HostMigration)
        );
        assert_eq!(classify(&real("Script", "HostMigration x")), None);
        // Explicitly NOT relied on (unreliable per wf-log).
        assert_eq!(
            classify(&real("Game", "FrameworkCmd::OpenLevel x")),
            None
        );
        // Validated negative: a plain alert run logs NO reward-screen or
        // reward-claim lines — Nightwave/vascular noise must not classify.
        assert_eq!(
            classify(&real("Game", "Mission progress XP: 33867")),
            None
        );
    }

    #[test]
    fn gets_reward_arms_only_for_seths_id() {
        // Pass 49 fixture (transcribed from EE-2.log, Seth host): the
        // true reward screen's grant line — his id, his roll's path.
        let his = real(
            "Sys",
            "VoidProjections: 5420fba23846322173707618 gets reward /Lotus/StoreItems/Types/Recipes/WarframeRecipes/HarrowPrimeHelmetBlueprint",
        );
        assert_eq!(
            classify_with_account(&his, "5420fba23846322173707618"),
            Some(Event::RelicReward {
                roll: "/Lotus/StoreItems/Types/Recipes/WarframeRecipes/HarrowPrimeHelmetBlueprint"
                    .to_owned()
            })
        );
        // The context-free classifier never arms: the id is the signal.
        assert_eq!(classify(&his), None);
        // A squadmate's grant line (another id) must not arm — in
        // Seth's log other players normally surface only as
        // `Host/Client got reward info from <id>`, but the id check
        // is what guarantees it either way.
        let theirs = real(
            "Sys",
            "VoidProjections: 65a6fbd660283fc6b30c4ec3 gets reward /Lotus/StoreItems/Types/Recipes/Weapons/BurstonPrimeBlueprint",
        );
        assert_eq!(
            classify_with_account(&theirs, "5420fba23846322173707618"),
            None
        );
        // The reward-info traffic about his own id is not a grant.
        let info = real(
            "Sys",
            "VoidProjections: Host got reward info from 5420fba23846322173707618",
        );
        assert_eq!(
            classify_with_account(&info, "5420fba23846322173707618"),
            None
        );
        // Client-side shape (Seth, 2026-10-27 23:56, confirmed live):
        // the same line with his id when someone else hosts.
        let client = real(
            "Sys",
            "VoidProjections: 5420fba23846322173707618 gets reward /Lotus/StoreItems/Types/Recipes/WarframeRecipes/SevagothPrimeSystemsBlueprint",
        );
        assert!(matches!(
            classify_with_account(&client, "5420fba23846322173707618"),
            Some(Event::RelicReward { .. })
        ));
    }

    #[test]
    fn pack_purchase_dialog_arms_on_any_booster_packs_path() {
        // Pass 53: Seth's live shape (2026-10-28) — the
        // syndicate relic pack's purchase dialog.
        let syndicate = real(
            "Script",
            "ThemedDetailedPurchaseDialog.lua: PopulateInfo->/Lotus/StoreItems/Types/BoosterPacks/RandomSyndicateProjectionPack",
        );
        assert_eq!(
            classify(&syndicate),
            Some(Event::PackPurchase {
                path: "/Lotus/StoreItems/Types/BoosterPacks/RandomSyndicateProjectionPack"
                    .to_owned()
            })
        );
        // Any BoosterPacks source arms (Seth's scope ruling),
        // not just the syndicate path.
        let other = real(
            "Script",
            "ThemedDetailedPurchaseDialog.lua: PopulateInfo->/Lotus/StoreItems/Types/BoosterPacks/WhateverPack",
        );
        assert!(matches!(
            classify(&other),
            Some(Event::PackPurchase { .. })
        ));
        // A PopulateInfo for a non-pack item never arms.
        let booster = real(
            "Script",
            "ThemedDetailedPurchaseDialog.lua: PopulateInfo->/Lotus/StoreItems/Types/Boosters/AffinityBooster",
        );
        assert_eq!(classify(&booster), None);
        // A BoosterPacks path on a different dialog line
        // shape is not the arm.
        let other_dialog = real(
            "Script",
            "SomeOtherDialog.lua: PopulateInfo->/Lotus/StoreItems/Types/BoosterPacks/RandomSyndicateProjectionPack",
        );
        assert_eq!(classify(&other_dialog), None);
        // Never a mission-end (no regen rides the arm).
        assert!(!Event::PackPurchase { path: "/x".into() }.is_mission_end());
    }

    #[test]
    fn mission_end_events_drive_regeneration() {
        // Sixth pass §4: reward screen + post-crack aftermath are the
        // mission-end signals the watch loop turns into a regen.
        // Pass 49: the reward-screen signal is now the gets-reward
        // grant (RelicReward), not the ProjectionRewardChoice lines.
        assert!(Event::RelicReward {
            roll: "/Lotus/X".to_owned()
        }
        .is_mission_end());
        assert!(Event::RelicCrack.is_mission_end());
        assert!(!Event::MissionInfoSeen.is_mission_end());
        assert!(!Event::LevelConnected.is_mission_end());
        assert!(!Event::HostMigration.is_mission_end());
        assert!(!Event::LoggedIn { name: "S".into() }.is_mission_end());
    }

    #[test]
    fn session_start_needs_login_after_silence() {
        let mut t = SessionTracker::new();
        assert!(!t.feed(Some(&Event::MissionInfoSeen), 10));
        assert_eq!(t.phase, SessionPhase::Arming);
        assert!(t.feed(
            Some(&Event::LoggedIn { name: "S".into() }),
            3600
        ));
        assert_eq!(t.phase, SessionPhase::Active);
        assert!(t.check_stale(1800));
        assert_eq!(t.phase, SessionPhase::Idle);
        assert!(!t.check_stale(10));
    }

    #[test]
    fn one_file_many_sessions_split_on_login_not_edges() {
        // Confirmed rule: one file spans sessions/squads; a second
        // `Logged in` restarts the session even on a SHORT gap, and a
        // file edge (fresh lines, no login) never starts one.
        let login = || {
            classify(&real("Sys", "Logged in Jenterrobahne")).map(|e| e)
        };
        let mut t = SessionTracker::new();
        // File edge: activity without login arms, never starts.
        assert!(!t.feed(Some(&Event::MissionInfoSeen), 5));
        assert!(!t.feed(Some(&Event::LevelConnected), 5));
        assert_eq!(t.phase, SessionPhase::Arming);
        // First login: session 1.
        assert!(t.feed(login().as_ref(), 5));
        assert!(!t.feed(Some(&Event::MissionInfoSeen), 60));
        // Second login, short gap (same file): session 2.
        assert!(
            t.feed(login().as_ref(), 60),
            "re-login always restarts the session"
        );
    }

    #[test]
    fn hex24_ranking_prefers_display_name_lines() {
        // transcribed-from-real-log: `VoidProjections: Client got
        // reward info from <24-hex account ID>` (×4, one per squad
        // member); second name↔ID source: `SendSessionUpdate` params
        // carry `"memberAccountId"`.
        let id_a = "5420fba23846322173707618";
        let id_b = "1234567890abcdef12345678";
        let lines = vec![
            real(
                "Game",
                &format!("VoidProjections: Client got reward info from {id_b}"),
            ),
            real(
                "Game",
                &format!("VoidProjections: Client got reward info from {id_a}"),
            ),
            real(
                "Game",
                &format!(
                    "SendSessionUpdate params {{\"memberAccountId\":\"{id_a}\",\"name\":\"Jenterrobahne\"}}"
                ),
            ),
        ];
        let ranked = rank_account_candidates(&lines, "Jenterrobahne");
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].id, id_a);
        assert!(ranked[0].with_display_name);
        assert!(ranked[0].void_projection_host);
    }

    #[test]
    fn hex24_excludes_uppercase_and_wrong_lengths() {
        assert!(extract_hex24("mm=ABCDEF0123456789ABCDEF01").is_empty());
        assert!(extract_hex24("short abc123").is_empty());
        assert_eq!(extract_hex24("id abcdef0123456789abcdef01.").len(), 1);
        // Embedded in a longer hex run: no match.
        assert!(extract_hex24("abcdef0123456789abcdef0123").is_empty());
    }

    // ---- ninth pass §4: log-derived mission completions ----

    /// The exact two-line shape Seth's log prints: the loading line, then
    /// the multi-line MissionInfo JSON (trimmed here to the identity
    /// fields), then the end-of-match success markers.
    fn armed_block(name: &str, alert_id: &str, mtype: &str) -> Vec<String> {
        vec![
            format!(
                "48.805 Sys [Info]: Client loaded {{\"name\":\"{name}\"}} with MissionInfo:"
            ),
            "{".to_owned(),
            format!("    \"missionType\" : \"{mtype}\","),
            "    \"faction\" : \"FC_INFESTATION\",".to_owned(),
            "    \"missionReward\" : {".to_owned(),
            "        \"countedItems\" : [ {".to_owned(),
            "            \"ItemCount\" : 50,".to_owned(),
            "            \"ItemType\" : \"/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource\""
                .to_owned(),
            "        } ]".to_owned(),
            "    },".to_owned(),
            "    \"location\" : \"SolNode175\",".to_owned(),
            format!("    \"alertId\" : \"{alert_id}\","),
            "    \"alertTag\" : \"RoadToTauEvent\"".to_owned(),
            "}".to_owned(),
        ]
    }

    #[test]
    fn mission_log_arms_from_multiline_json_and_completes_on_success() {
        let mut log = MissionLog::new();
        let mut completion = None;
        for line in armed_block("SolNode175_Alert", "6ad0d7360dfe89c5d82290a2", "MT_HIVE") {
            completion = completion.or(log.feed(&line, "2026-10-17", "2026-10-17T12:00:00Z"));
        }
        // Still armed, no completion until an end-of-match success marker.
        assert!(completion.is_none());
        let armed = log.armed().expect("mission armed from the JSON block");
        assert_eq!(armed.name, "SolNode175_Alert");
        assert_eq!(armed.alert_id, "6ad0d7360dfe89c5d82290a2");
        assert_eq!(armed.alert_tag, "RoadToTauEvent");
        assert_eq!(armed.mission_type, "MT_HIVE");
        assert_eq!(armed.location, "SolNode175");
        assert_eq!(armed.reward_items.len(), 1);

        completion = log.feed(
            "251.004 Script [Info]: EndOfMatch.lua: Mission Succeeded",
            "2026-10-17",
            "2026-10-17T12:00:00Z",
        );
        let c = completion.expect("completion after the success marker");
        assert_eq!(c.info.alert_id, "6ad0d7360dfe89c5d82290a2");
        assert_eq!(c.day, "2026-10-17");
        // A second marker does not double-fire.
        assert!(log
            .feed(
                "251.004 Script [Info]: EndOfMatch.lua: EndOfMatch.lua: GiveMissionRewards. success=true",
                "2026-10-17",
                "2026-10-17T12:00:00Z"
            )
            .is_none());
    }

    #[test]
    fn mission_log_never_infers_without_a_positive_marker() {
        let mut log = MissionLog::new();
        for line in armed_block("SolNode175_Alert", "abc", "MT_HIVE") {
            let _ = log.feed(&line, "2026-10-17", "2026-10-17T12:00:00Z");
        }
        // A FAILED mission must not complete.
        assert!(log
            .feed(
                "251.004 Script [Info]: EndOfMatch.lua: Mission Failed",
                "2026-10-17",
                "2026-10-17T12:00:00Z"
            )
            .is_none());
        // A marker with no armed mission (tail started mid-mission) drops.
        let mut empty = MissionLog::new();
        assert!(empty
            .feed(
                "251.004 Script [Info]: EndOfMatch.lua: Mission Succeeded",
                "2026-10-17",
                "2026-10-17T12:00:00Z"
            )
            .is_none());
    }

    #[test]
    fn completions_match_alerts_on_positive_identity_only() {
        use crate::model::{Alert, AlertMission, AlertReward, CountedItem};
        let mut info = MissionInfo {
            alert_id: "6ad0d7360dfe89c5d82290a2".to_owned(),
            alert_tag: "RoadToTauEvent".to_owned(),
            mission_type: "MT_HIVE".to_owned(),
            faction: "FC_INFESTATION".to_owned(),
            location: "SolNode175".to_owned(),
            reward_items: vec![
                "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource".to_owned(),
            ],
            ..Default::default()
        };
        let completion = MissionCompletion {
            info: info.clone(),
            day: "2026-10-17".to_owned(),
            observed_at: "2026-10-17T12:00:00Z".to_owned(),
        };
        let alert = |id: &str, tag: &str, mtype: &str| Alert {
            id: id.to_owned(),
            tag: tag.to_owned(),
            activation: String::new(),
            activation_degraded: false,
            expiry: String::new(),
            mission: AlertMission {
                node: "Nimus (Eris)".to_owned(),
                mission_type: mtype.to_owned(),
                faction: "Infested".to_owned(),
                reward: AlertReward {
                    counted_items: vec![CountedItem {
                        unique_name: "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource".to_owned(),
                        kind: "Tau Resource".to_owned(),
                        count: 50,
                    }],
                },
                ..Default::default()
            },
        };
        // Exact alert id: strongest match even if the tag/type drift.
        assert!(completion.matches_alert(&alert("6ad0d7360dfe89c5d82290a2", "Other", "Survival")));
        // Tag + normalized mission type (`MT_HIVE` == `Hive`).
        assert!(completion.matches_alert(&alert("", "RoadToTauEvent", "Hive")));
        // A shared payout resource is NOT identity: every same-tag
        // sibling pays the same event currency (addendum #64).
        assert!(info.is_alert());
        info.mission_type = "MT_SOMETHING_NEW".to_owned();
        let c2 = MissionCompletion {
            info: info.clone(),
            day: "d".to_owned(),
            observed_at: "2026-10-17T12:00:00Z".to_owned(),
        };
        assert!(!c2.matches_alert(&alert("", "RoadToTauEvent", "Mobile Defense")));
        // A different tag with no id or type match never strikes.
        assert!(!completion.matches_alert(&alert("", "SomeOtherEvent", "Survival")));
    }

    #[test]
    fn sol_node_is_exact_positive_evidence_for_alert_completion() {
        // Addendum #51 bonus: EE.log and DE both speak SolNode keys, so an
        // exact internal-node match corroborates the alert instance (the
        // activation guard still applies); a different SolNode does not.
        // Addendum #64 adds the type conjunct: when a type is present on
        // both sides it must agree, so same-node/different-type siblings
        // are distinct instances. A type-absent side keeps the node-only
        // fallback.
        use crate::model::{Alert, AlertMission};
        let completion = MissionCompletion {
            info: MissionInfo {
                alert_tag: "RoadToTauEvent".to_owned(),
                location: "SolNode175".to_owned(),
                mission_type: "MT_HIVE".to_owned(),
                ..Default::default()
            },
            day: "2026-10-17".to_owned(),
            observed_at: "2026-10-17T12:00:00Z".to_owned(),
        };
        let alert = |sol_node: &str, mission_type: &str| Alert {
            tag: "RoadToTauEvent".to_owned(),
            activation: "2026-10-17T11:00:00Z".to_owned(),
            mission: AlertMission {
                node: "Naeglar (Eris)".to_owned(),
                sol_node: sol_node.to_owned(),
                mission_type: mission_type.to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(completion.matches_alert(&alert("SolNode175", "MT_HIVE")));
        assert!(!completion.matches_alert(&alert("SolNode999", "MT_HIVE")));
        // Same node, different present type: not the same instance.
        assert!(!completion.matches_alert(&alert("SolNode175", "MT_MOBILE_DEFENSE")));
        // Type absent on the alert side: node-only fallback still holds.
        assert!(completion.matches_alert(&alert("SolNode175", "")));
        // The activation guard still gates the node path.
        let mut early = alert("SolNode175", "MT_HIVE");
        early.activation = "2026-10-18T00:00:00Z".to_owned();
        assert!(!completion.matches_alert(&early));
    }

    /// Addendum #59 regression, exactly Seth's live case: four same-tag
    /// Road To Tau alerts with distinct nodes/types and the SAME shared
    /// event currency. One completion may strike only its node's alert;
    /// the old OR-of-tag+type/tag+reward struck all four.
    #[test]
    fn one_completion_strikes_exactly_one_same_tag_sibling() {
        use crate::model::{Alert, AlertMission, AlertReward, CountedItem};
        let shared_reward = "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource";
        let completion = MissionCompletion {
            info: MissionInfo {
                alert_id: String::new(),
                alert_tag: "RoadToTauEvent".to_owned(),
                mission_type: "MT_HIVE".to_owned(),
                location: "SolNode175".to_owned(),
                reward_items: vec![shared_reward.to_owned()],
                ..Default::default()
            },
            day: "2026-10-21".to_owned(),
            observed_at: "2026-10-21T12:00:00Z".to_owned(),
        };
        let sibling = |node: &str, mtype: &str| Alert {
            id: String::new(),
            tag: "RoadToTauEvent".to_owned(),
            activation: "2026-10-21T11:00:00Z".to_owned(),
            mission: AlertMission {
                node: format!("{} (Eris)", mtype),
                sol_node: node.to_owned(),
                mission_type: mtype.to_owned(),
                faction: "Infested".to_owned(),
                reward: AlertReward {
                    counted_items: vec![CountedItem {
                        unique_name: shared_reward.to_owned(),
                        kind: "Tau Resource".to_owned(),
                        count: 50,
                    }],
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let siblings = [
            sibling("SolNode175", "Hive"),
            sibling("SolNode176", "Mobile Defense"),
            sibling("SolNode177", "Extermination"),
            sibling("SolNode178", "Survival"),
        ];
        let struck: Vec<usize> = siblings
            .iter()
            .enumerate()
            .filter(|(_, a)| completion.matches_alert(a))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(struck, vec![0], "only the matching SolNode is struck");
        // Even a same-tag sibling with the same mission type (but a
        // different node) must not corroborate once both nodes are present.
        assert!(!completion.matches_alert(&sibling("SolNode176", "Hive")));
    }

    /// Brief #21 §1 regression: same tag and same node, different mission
    /// types. Node-only corroboration used to strike both siblings; the
    /// type conjunct leaves exactly one.
    #[test]
    fn same_node_different_type_siblings_strike_exactly_one() {
        use crate::model::{Alert, AlertMission, AlertReward, CountedItem};
        let shared_reward = "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource";
        let completion = MissionCompletion {
            info: MissionInfo {
                alert_tag: "RoadToTauEvent".to_owned(),
                mission_type: "MT_HIVE".to_owned(),
                location: "SolNode175".to_owned(),
                reward_items: vec![shared_reward.to_owned()],
                ..Default::default()
            },
            day: "2026-10-21".to_owned(),
            observed_at: "2026-10-21T12:00:00Z".to_owned(),
        };
        let sibling = |mtype: &str| Alert {
            tag: "RoadToTauEvent".to_owned(),
            activation: "2026-10-21T11:00:00Z".to_owned(),
            mission: AlertMission {
                node: "Nimus (Eris)".to_owned(),
                sol_node: "SolNode175".to_owned(),
                mission_type: mtype.to_owned(),
                reward: AlertReward {
                    counted_items: vec![CountedItem {
                        unique_name: shared_reward.to_owned(),
                        kind: "Tau Resource".to_owned(),
                        count: 50,
                    }],
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let siblings = [
            sibling("Hive"),
            sibling("Mobile Defense"),
            sibling("Extermination"),
        ];
        let struck: Vec<usize> = siblings
            .iter()
            .enumerate()
            .filter(|(_, a)| completion.matches_alert(a))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(struck, vec![0], "only the matching mission type is struck");
    }

    /// Brief #21 §1 regression: the activation window is part of identity.
    /// Same node and type as an earlier completion, but the alert belongs
    /// to a rerun instance that only activated afterwards, so it must not
    /// be struck.
    #[test]
    fn rerun_completion_before_activation_is_not_struck() {
        use crate::model::{Alert, AlertMission};
        let completion = MissionCompletion {
            info: MissionInfo {
                alert_tag: "RoadToTauEvent".to_owned(),
                mission_type: "MT_HIVE".to_owned(),
                location: "SolNode175".to_owned(),
                ..Default::default()
            },
            day: "2026-10-21".to_owned(),
            observed_at: "2026-10-21T12:00:00Z".to_owned(),
        };
        let rerun = Alert {
            tag: "RoadToTauEvent".to_owned(),
            activation: "2026-10-21T13:00:00Z".to_owned(),
            mission: AlertMission {
                node: "Nimus (Eris)".to_owned(),
                sol_node: "SolNode175".to_owned(),
                mission_type: "MT_HIVE".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!completion.matches_alert(&rerun));
        // The same instance, once live, is struck.
        let mut live = rerun.clone();
        live.activation = "2026-10-21T11:00:00Z".to_owned();
        assert!(completion.matches_alert(&live));
    }

    /// Twelfth-pass §4 (instance-strict): a corroboration-only match must
    /// fall inside the alert instance's live window. Yesterday's
    /// same-tag completion can never strike an alert activated today,
    /// while an exact `alertId` match still strikes regardless of time.
    #[test]
    fn corroboration_is_instance_strict_but_id_match_is_not() {
        use crate::model::{Alert, AlertMission, AlertReward, CountedItem};
        let info = MissionInfo {
            alert_id: String::new(),
            alert_tag: "RoadToTauEvent".to_owned(),
            mission_type: "MT_HIVE".to_owned(),
            faction: "FC_INFESTATION".to_owned(),
            location: "SolNode175".to_owned(),
            reward_items: vec![
                "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource".to_owned(),
            ],
            ..Default::default()
        };
        let alert = |activation: &str| Alert {
            id: String::new(),
            tag: "RoadToTauEvent".to_owned(),
            activation: activation.to_owned(),
            activation_degraded: false,
            expiry: String::new(),
            mission: AlertMission {
                node: "Nimus (Eris)".to_owned(),
                mission_type: "Hive".to_owned(),
                faction: "Infested".to_owned(),
                reward: AlertReward {
                    counted_items: vec![CountedItem {
                        unique_name: "/Lotus/Types/Gameplay/Events/RoadToTauEvent/RoadToTauEventResource".to_owned(),
                        kind: "Tau Resource".to_owned(),
                        count: 50,
                    }],
                },
                ..Default::default()
            },
        };
        // (a) yesterday's completion vs today's activated alert: NOT struck.
        let yesterday = MissionCompletion {
            info: info.clone(),
            day: "2026-10-16".to_owned(),
            observed_at: "2026-10-16T23:00:00Z".to_owned(),
        };
        assert!(
            !yesterday.matches_alert(&alert("2026-10-17T00:00:00Z")),
            "a pre-activation same-tag completion must not strike"
        );
        // (b) a corroborating completion inside the window IS struck.
        let today = MissionCompletion {
            info: info.clone(),
            day: "2026-10-17".to_owned(),
            observed_at: "2026-10-17T00:05:00Z".to_owned(),
        };
        assert!(
            today.matches_alert(&alert("2026-10-17T00:00:00Z")),
            "a post-activation corroborating completion must strike"
        );
        // A present-but-unreadable activation is NOT the legacy empty feed:
        // the guard stays closed instead of failing open (addendum #56b).
        let mut degraded = alert("");
        degraded.activation = "not-a-timestamp".to_owned();
        degraded.activation_degraded = true;
        assert!(
            !today.matches_alert(&degraded),
            "a degraded activation must not skip the guard"
        );
        // The genuinely legacy feed (field absent) keeps its documented
        // behavior: corroboration is allowed when no activation exists.
        assert!(
            today.matches_alert(&alert("")),
            "legacy empty activation still corroborates"
        );
        // A completion with no trustworthy observed-at cannot corroborate.
        let unknown = MissionCompletion {
            info: info.clone(),
            day: "2026-10-17".to_owned(),
            observed_at: String::new(),
        };
        assert!(!unknown.matches_alert(&alert("2026-10-17T00:00:00Z")));
        // (c) exact alertId stays instance identity: struck even from
        // before the recorded activation and with no observed-at.
        let by_id = MissionCompletion {
            info: MissionInfo {
                alert_id: "6ad0d7360dfe89c5d82290a2".to_owned(),
                ..info.clone()
            },
            day: "2026-10-16".to_owned(),
            observed_at: String::new(),
        };
        let id_alert = Alert {
            id: "6ad0d7360dfe89c5d82290a2".to_owned(),
            tag: "Other".to_owned(),
            activation: "2026-10-17T00:00:00Z".to_owned(),
            ..alert("2026-10-17T00:00:00Z")
        };
        assert!(by_id.matches_alert(&id_alert));
    }

    /// Ninth pass §4 audit: parse Seth's real EE.log (gitignored) and
    /// prove each of the three armed alert missions completes exactly
    /// once with the identity the log printed. Skipped when the fixture
    /// is absent (a box that lacks his log).
    #[test]
    fn real_fixture_log_yields_the_three_proven_completions() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/ocr-samples/ee-log-2026-10-17.log");
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("skip §4 fixture audit: need {}", path.display());
            return;
        };
        let mut log = MissionLog::new();
        let mut done = Vec::new();
        for raw in text.lines() {
            if let Some(c) = log.feed(raw, "2026-10-17", "2026-10-17T12:00:00Z") {
                done.push(c);
            }
        }
        let ids: Vec<&str> = done.iter().map(|c| c.info.alert_id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "6ad0d7360dfe89c5d82290a2",
                "6ad0d7360dfe89c5d82290a4",
                "6ad0d7360dfe89c5d82290a6",
            ],
            "each armed mission completes exactly once, in log order"
        );
        assert_eq!(done[0].info.mission_type, "MT_HIVE");
        assert!(done.iter().all(|c| c.info.alert_tag == "RoadToTauEvent"));
        assert!(done.iter().all(|c| c.info.reward_items.len() == 1));
        assert!(done.iter().all(|c| c.day == "2026-10-17"));
    }
}
