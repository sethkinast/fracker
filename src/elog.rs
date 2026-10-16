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
    /// `ProjectionRewardChoice.lua: Relic rewards initialized` or `...: Got rewards`.
    RelicRewardScreen,
    /// Message contains `DVRCAftermath`.
    RelicCrack,
    /// `ThemedProjectionManager.lua: PopulateInventoryGrid`.
    RelicInventoryOpen,
    /// `InventoryTest.lua: PopulateGrid`.
    InventorySellOpen,
    /// `Game` subsystem, contains `HostMigration`.
    HostMigration,
}

/// Substring classification per the §7.2 catalog.
pub fn classify(line: &LogLine) -> Option<Event> {
    classify_inner(line)
}

fn classify_inner(line: &LogLine) -> Option<Event> {
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
    if msg.contains("ProjectionRewardChoice.lua: Relic rewards initialized")
        || msg.contains("ProjectionRewardChoice.lua: Got rewards")
    {
        return Some(Event::RelicRewardScreen);
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
        // rewards initialized`.
        assert_eq!(
            classify(&real(
                "Script",
                "ProjectionRewardChoice.lua: Relic rewards initialized"
            )),
            Some(Event::RelicRewardScreen)
        );
        // transcribed-from-real-log: `ProjectionRewardChoice.lua: Got
        // rewards`.
        assert_eq!(
            classify(&real("Script", "ProjectionRewardChoice.lua: Got rewards")),
            Some(Event::RelicRewardScreen)
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
}
