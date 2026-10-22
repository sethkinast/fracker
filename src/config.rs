//! Configuration: TOML file in the XDG config dir, watched reload not needed yet.
//!
//! BRIEF.md §9. The only stored secret is the Xuerian bookmark token
//! (a read/write key for a wishlist, nothing else).

use std::path::PathBuf;

/// Application configuration (BRIEF §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Explicit EE.log path. Empty = auto-detect from the standard
    /// Steam/Proton candidates (BRIEF §3).
    pub ee_log_path: String,
    /// Open the checklist page automatically on session start.
    pub pop_on_session_start: bool,
    /// Xuerian bookmark token. Empty = targets come from the bundled
    /// seed list and the tray status notes staleness.
    pub xuerian_token: String,
    /// Goal names pinned to the top of the plan ("working on these").
    /// A pin overrides every other ordering rule (BRIEF §10).
    pub pins: Vec<String>,
    /// Prefer Steel Path fissures as a tiebreak (BRIEF §17 cascade).
    /// Endless missions always default to the normal path (brain-off
    /// beats scaling enemies) regardless of this toggle.
    pub steel_path_tiebreak: bool,
    /// Loopback HTTP port the resident serves the plan on (ninth pass
    /// §2). The stored default; when it is taken the server falls back to
    /// an ephemeral port and records the live URL for the tray's Open
    /// action. 0 = always pick an ephemeral port.
    pub serve_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ee_log_path: String::new(),
            pop_on_session_start: true,
            xuerian_token: String::new(),
            pins: Vec::new(),
            steel_path_tiebreak: true,
            serve_port: 8791,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    Defaults,
    File,
}

impl Config {
    /// Parse TOML text. Unknown keys are ignored so future settings
    /// never break old binaries; missing keys fall back to defaults.
    pub fn from_toml(text: &str) -> Result<(Self, ConfigSource), String> {
        if text.trim().is_empty() {
            return Ok((Self::default(), ConfigSource::Defaults));
        }
        let table: toml::Table =
            toml::from_str(text).map_err(|e| format!("config parse: {e}"))?;
        let mut cfg = Self::default();
        if let Some(v) = table.get("ee_log_path").and_then(|v| v.as_str()) {
            cfg.ee_log_path = v.to_owned();
        }
        if let Some(v) = table
            .get("pop_on_session_start")
            .and_then(|v| v.as_bool())
        {
            cfg.pop_on_session_start = v;
        }
        if let Some(v) = table.get("xuerian_token").and_then(|v| v.as_str()) {
            cfg.xuerian_token = v.to_owned();
        }
        if let Some(v) = table.get("pins").and_then(|v| v.as_array()) {
            cfg.pins = v
                .iter()
                .filter_map(|e| e.as_str().map(str::to_owned))
                .collect();
        }
        if let Some(v) = table
            .get("steel_path_tiebreak")
            .and_then(|v| v.as_bool())
        {
            cfg.steel_path_tiebreak = v;
        }
        if let Some(v) = table.get("serve_port").and_then(|v| v.as_integer()) {
            cfg.serve_port = v.clamp(0, 65535) as u16;
        }
        Ok((cfg, ConfigSource::File))
    }

    /// Load from `path`, or defaults when the file does not exist.
    pub fn load(path: &std::path::Path) -> Result<(Self, ConfigSource), String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok((Self::default(), ConfigSource::Defaults))
            }
            Err(e) => Err(format!("config read {}: {e}", path.display())),
        }
    }

    /// True when targets must come from the bundled seed list.
    pub fn use_seed_targets(&self) -> bool {
        self.xuerian_token.trim().is_empty()
    }
}

/// XDG config dir for the app (`$XDG_CONFIG_HOME/fracker`).
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("fracker")
}

/// Default config file path.
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// XDG data dir for the app (derived state + rendered page).
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("fracker")
}

/// XDG cache dir for the app (HTTP caches with §6 TTLs).
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("~/.cache"))
        .join("fracker")
}

/// The resident report page's home (seventh pass §3): the app data dir
/// (`$XDG_DATA_HOME/fracker/plan.html`, or the Flatpak equivalent). The
/// tray's "Open plan page" opens exactly this file.
pub fn report_file() -> PathBuf {
    data_dir().join("plan.html")
}

/// Single-instance lock file (seventh pass §1), in the app data dir so
/// it survives across logins and is per-user.
pub fn instance_lock_file() -> PathBuf {
    data_dir().join("resident.lock")
}

/// EE.log discovery table (`data/paths.toml`, sixth pass §3). The
/// Steam/Proton default is data, not code: roots + tail live here so the
/// Flatpak manifest and the runtime share one source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EeLogPaths {
    /// Path appended to each Steam root to reach EE.log.
    pub tail: String,
    /// Steam install roots, in priority order (home-relative).
    pub steam_roots: Vec<String>,
}

impl EeLogPaths {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let table: toml::Table =
            toml::from_str(text).map_err(|e| format!("paths parse: {e}"))?;
        let ee = table.get("ee_log").and_then(|v| v.as_table());
        let tail = ee
            .and_then(|t| t.get("tail"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let steam_roots: Vec<String> = ee
            .and_then(|t| t.get("steam_roots"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if tail.trim().is_empty() || steam_roots.is_empty() {
            return Err("paths: [ee_log] needs a tail and steam_roots".to_owned());
        }
        Ok(Self { tail, steam_roots })
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("paths {}: {e}", path.display()))?;
        Self::from_toml(&text)
    }

    /// Default table under the repo/install `data/` dir.
    pub fn load_default() -> Result<Self, String> {
        Self::load(&crate::tuning::data_dir().join("paths.toml"))
    }

    /// The EE.log CONTAINING FOLDER for the primary Steam root — the
    /// directory the sandbox needs granted (item 35: the folder grant is
    /// primary). Home-relative, so the manifest can use `~/…`.
    pub fn primary_containing_folder(&self) -> Option<String> {
        let root = self.steam_roots.first()?;
        let tail_dir = self.tail.rsplit_once('/').map(|(d, _)| d)?;
        Some(format!("{root}/{tail_dir}"))
    }
}

/// Standard EE.log candidates under Proton/Steam (BRIEF §3), built from
/// the data-owned discovery table. Returned in priority order; the
/// caller picks the first that exists.
pub fn ee_log_candidates(home: &std::path::Path, paths: &EeLogPaths) -> Vec<PathBuf> {
    paths
        .steam_roots
        .iter()
        .map(|root| home.join(root).join(&paths.tail))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_brief_section_9() {
        let cfg = Config::default();
        assert_eq!(cfg.ee_log_path, "");
        assert!(cfg.pop_on_session_start);
        assert_eq!(cfg.xuerian_token, "");
        assert!(cfg.pins.is_empty());
        assert!(cfg.use_seed_targets());
        assert_eq!(cfg.serve_port, 8791, "sane stored default loopback port");
    }

    #[test]
    fn empty_file_means_defaults() {
        let (cfg, src) = Config::from_toml("").unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(src, ConfigSource::Defaults);
    }

    #[test]
    fn parses_full_example() {
        let text = r#"
ee_log_path = "/mnt/games/steam/EE.log"
pop_on_session_start = false
xuerian_token = "sekrit"
pins = ["Khora Prime Blueprint", "Ivara Prime Chassis Blueprint"]
steel_path_tiebreak = false
serve_port = 9123
"#;
        let (cfg, src) = Config::from_toml(text).unwrap();
        assert_eq!(src, ConfigSource::File);
        assert_eq!(cfg.ee_log_path, "/mnt/games/steam/EE.log");
        assert!(!cfg.pop_on_session_start);
        assert_eq!(cfg.xuerian_token, "sekrit");
        assert_eq!(cfg.pins.len(), 2);
        assert!(!cfg.use_seed_targets());
        assert!(!cfg.steel_path_tiebreak);
        assert_eq!(cfg.serve_port, 9123);
    }

    #[test]
    fn serve_port_is_clamped_to_a_valid_range() {
        let (cfg, _) = Config::from_toml("serve_port = 99999\n").unwrap();
        assert_eq!(cfg.serve_port, 65535);
        let (zero, _) = Config::from_toml("serve_port = 0\n").unwrap();
        assert_eq!(zero.serve_port, 0, "0 = ephemeral");
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let (cfg, _) = Config::from_toml("future_setting = 42\n").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn missing_file_means_defaults() {
        let (cfg, src) =
            Config::load(std::path::Path::new("/nonexistent/fracker.toml")).unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(src, ConfigSource::Defaults);
    }

    #[test]
    fn candidates_cover_brief_section_3_roots() {
        let paths = paths_fixture();
        let home = std::path::Path::new("/home/seth");
        let c = ee_log_candidates(home, &paths);
        assert_eq!(c.len(), 4);
        assert!(c[0].to_string_lossy().contains("Games/SteamLibrary"));
        assert!(c[1].to_string_lossy().contains(".local/share/Steam"));
        assert!(c.iter().all(|p| p.to_string_lossy().ends_with("Warframe/EE.log")));
    }

    fn paths_fixture() -> EeLogPaths {
        EeLogPaths::load(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/paths.toml"),
        )
        .expect("data/paths.toml loads")
    }

    #[test]
    fn ee_log_paths_come_from_data() {
        let paths = paths_fixture();
        assert_eq!(
            paths.tail,
            "steamapps/compatdata/230410/pfx/drive_c/users/steamuser/AppData/Local/Warframe/EE.log"
        );
        assert_eq!(paths.steam_roots[0], "/run/media/system/Games/SteamLibrary");
        // The containing folder the sandbox must grant (item 35).
        assert_eq!(
            paths.primary_containing_folder().unwrap(),
            "/run/media/system/Games/SteamLibrary/steamapps/compatdata/230410/pfx/drive_c/users/steamuser/AppData/Local/Warframe"
        );
    }

    #[test]
    fn ee_log_paths_require_tail_and_roots() {
        assert!(EeLogPaths::from_toml("[ee_log]\ntail = \"EE.log\"\n").is_err());
        assert!(EeLogPaths::from_toml("[ee_log]\nsteam_roots = [\".steam\"]\n").is_err());
    }
}
