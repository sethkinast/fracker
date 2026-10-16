//! Event data packs (TOML): time-boxed event knowledge the planner
//! consumes generically. Pack v1 is Taubound (`data/packs/taubound.toml`):
//! alert payouts, shop totals, the community-gated Contegorus set, and
//! arcane shopping guidance. Packs expire; after expiry the planner
//! falls back to whatever world state carries.

use serde::Deserialize;

/// Parsed Taubound pack (subset of the TOML we rely on).
#[derive(Debug, Clone, Deserialize)]
pub struct TauboundPack {
    #[serde(default)]
    pub pack: PackMeta,
    #[serde(default)]
    pub event: PackEvent,
    #[serde(default)]
    pub community_goal: CommunityGoal,
    #[serde(default)]
    pub arcanes: ArcaneGuidance,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PackMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub expires: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PackEvent {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub resource: String,
    #[serde(default)]
    pub resource_type: String,
    #[serde(default)]
    pub alert_payout: u32,
    #[serde(default)]
    pub alert_payout_steel_path: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CommunityGoal {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub gated_set: String,
    #[serde(default)]
    pub gated_pieces: Vec<String>,
    #[serde(default)]
    pub gated_piece_price: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ArcaneGuidance {
    #[serde(default)]
    pub copies_per_rank5_set: u32,
    #[serde(default)]
    pub finish: Vec<String>,
    #[serde(default)]
    pub next: Vec<String>,
    #[serde(default)]
    pub playstyle: Vec<String>,
    #[serde(default)]
    pub skip: Vec<String>,
}

impl TauboundPack {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("taubound pack: {e}"))
    }

    /// True while the pack is live (now < expires). Unparseable expiry
    /// means expired — never serve stale event guidance as live.
    pub fn is_live(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.pack
            .expires
            .parse::<chrono::DateTime<chrono::Utc>>()
            .map(|t| now < t)
            .unwrap_or(false)
    }

    /// One-line arcane guidance for the page (terse): finish these,
    /// skip those.
    pub fn arcane_line(&self) -> String {
        format!(
            "Arcanes: finish {} — skip {}.",
            self.arcanes.finish.join(", "),
            self.arcanes.skip.join(", "),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_text() -> String {
        std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data/packs/taubound.toml"),
        )
        .unwrap()
    }

    #[test]
    fn pack_parses_with_brief_values() {
        let p = TauboundPack::from_toml(&pack_text()).unwrap();
        assert_eq!(p.pack.name, "taubound");
        assert_eq!(p.event.alert_payout, 50);
        assert_eq!(p.event.alert_payout_steel_path, 75);
        assert_eq!(p.community_goal.gated_pieces.len(), 3);
        assert!(p.community_goal.gated_pieces.contains(&"Contegorus Greaves".to_owned()));
        assert!(p.arcanes.finish.contains(&"Melee Retaliation".to_owned()));
        assert!(p.arcanes.finish.contains(&"Melee Influence".to_owned()));
        for s in ["Melee Vortex", "Secondary Surge", "Melee Fortification"] {
            assert!(p.arcanes.skip.contains(&s.to_owned()), "skip {s}");
        }
        assert_eq!(p.arcanes.copies_per_rank5_set, 21);
    }

    #[test]
    fn pack_expiry_gates_liveness() {
        let p = TauboundPack::from_toml(&pack_text()).unwrap();
        let during: chrono::DateTime<chrono::Utc> = "2026-10-16T00:00:00Z".parse().unwrap();
        let after: chrono::DateTime<chrono::Utc> = "2026-11-18T00:00:00Z".parse().unwrap();
        assert!(p.is_live(during));
        assert!(!p.is_live(after));
    }
}
