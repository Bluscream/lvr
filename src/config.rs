use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TriggerType {
    #[default]
    VRChat,
    WiVRn,
    Always,
}

impl std::fmt::Display for TriggerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriggerType::VRChat => write!(f, "VRChat"),
            TriggerType::WiVRn => write!(f, "WiVRn"),
            TriggerType::Always => write!(f, "Always"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutostartRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub exec_cmd: String,
    pub trigger: TriggerType,
    /// Grace period in seconds. -1 means keep running indefinitely after trigger app stops.
    pub grace_period_secs: i64,
    /// Case-insensitive substrings matched against a process' name, executable
    /// path and full command line. Used both to tell whether the app is already
    /// running and to decide what to terminate. Empty means "derive them from
    /// the command and the rule name".
    pub match_patterns: Vec<String>,
}

impl Default for AutostartRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            exec_cmd: String::new(),
            trigger: TriggerType::default(),
            grace_period_secs: 120,
            match_patterns: Vec::new(),
        }
    }
}

impl AutostartRule {
    /// Patterns actually used for matching, lowercased.
    ///
    /// The fallback deliberately uses the *first* word of the command (the
    /// program) rather than the last: for `foo.appimage --extract-and-run` the
    /// last word is a flag, which matches nothing and used to make the rule
    /// look permanently stopped.
    pub fn effective_patterns(&self) -> Vec<String> {
        let explicit: Vec<String> = self
            .match_patterns
            .iter()
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
        if !explicit.is_empty() {
            return explicit;
        }

        let mut patterns = Vec::new();
        if let Some(program) = self.exec_cmd.split_whitespace().next() {
            // Whole path first, then just the file name.
            let program = program.to_lowercase();
            if let Some(file_name) = Path::new(&program).file_name() {
                patterns.push(file_name.to_string_lossy().to_lowercase());
            }
            if !program.is_empty() && !patterns.contains(&program) {
                patterns.push(program);
            }
        }
        let name = self.name.trim().to_lowercase();
        if !name.is_empty() && !patterns.contains(&name) {
            patterns.push(name);
        }
        patterns
    }

    pub fn keeps_running(&self) -> bool {
        self.grace_period_secs < 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub auto_restart_wivrn: bool,
    pub auto_switch_audio: bool,
    pub wivrn_command: String,
    pub vrchat_process_pattern: String,
    pub saved_audio_sink: Option<String>,
    pub saved_audio_source: Option<String>,
    pub autostart_rules: Vec<AutostartRule>,
    pub poll_interval_secs: u64,
    /// Minimum gap between two launch attempts of the same rule. Without this,
    /// an app that is slow to appear in the process table (Steam, flatpak,
    /// AppImage extraction) is launched again on every single poll.
    pub spawn_debounce_secs: u64,
    /// How long a process gets to exit after SIGTERM before it is SIGKILLed.
    pub stop_grace_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_restart_wivrn: true,
            auto_switch_audio: true,
            wivrn_command: "flatpak run io.github.wivrn.wivrn".to_string(),
            vrchat_process_pattern: "VRChat".to_string(),
            saved_audio_sink: None,
            saved_audio_source: None,
            poll_interval_secs: 2,
            spawn_debounce_secs: 20,
            stop_grace_secs: 5,
            autostart_rules: vec![
                AutostartRule {
                    id: "vrc-video-cacher".to_string(),
                    name: "VRCVideoCacher".to_string(),
                    enabled: true,
                    exec_cmd: "steam steam://rungameid/4296960".to_string(),
                    trigger: TriggerType::VRChat,
                    grace_period_secs: 120,
                    // `steam <url>` exits immediately, so the rule must be
                    // matched against the app it launches, not the launcher.
                    match_patterns: vec!["vrcvideocacher".to_string()],
                },
                AutostartRule {
                    id: "vrcosc".to_string(),
                    name: "VRCOSC".to_string(),
                    enabled: true,
                    exec_cmd: "/home/blu/.local/bin/vrcosc".to_string(),
                    trigger: TriggerType::VRChat,
                    grace_period_secs: 120,
                    match_patterns: vec![
                        "vrcosc.exe".to_string(),
                        "/.local/bin/vrcosc".to_string(),
                    ],
                },
                AutostartRule {
                    id: "vrcx-0".to_string(),
                    name: "VRCX-0".to_string(),
                    enabled: true,
                    exec_cmd: "/home/blu/AppImages/vrcx0.appimage --appimage-extract-and-run"
                        .to_string(),
                    trigger: TriggerType::VRChat,
                    grace_period_secs: -1, // Keep running
                    match_patterns: vec!["vrcx0.appimage".to_string(), "vrcx-0".to_string()],
                },
                AutostartRule {
                    id: "vrcx-extras".to_string(),
                    name: "VRCX-Extras Companion".to_string(),
                    enabled: true,
                    exec_cmd: "/run/media/system/Data/Projects/vrcx-extras/start.sh".to_string(),
                    trigger: TriggerType::VRChat,
                    grace_period_secs: -1, // Keep running
                    match_patterns: vec!["vrcx-extras".to_string()],
                },
                AutostartRule {
                    id: "slimevr".to_string(),
                    name: "SlimeVR".to_string(),
                    enabled: true,
                    exec_cmd: "flatpak run dev.slimevr.SlimeVR".to_string(),
                    trigger: TriggerType::WiVRn,
                    grace_period_secs: 300,
                    match_patterns: vec![
                        "/app/main/slimevr".to_string(),
                        "slimevr.jar".to_string(),
                    ],
                },
                AutostartRule {
                    id: "wayvr".to_string(),
                    name: "WayVR".to_string(),
                    enabled: false, // Disabled by default
                    exec_cmd: "env DESKTOPINTEGRATION=1 /home/blu/AppImages/wayvr.appimage"
                        .to_string(),
                    trigger: TriggerType::WiVRn,
                    grace_period_secs: 300,
                    match_patterns: vec!["wayvr.appimage".to_string()],
                },
            ],
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        path.push("lvr");
        path.push("config.json");
        path
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<Config>(&content) {
                    Ok(cfg) => {
                        info!("Loaded configuration from {:?}", path);
                        return cfg;
                    }
                    Err(e) => {
                        // Never destroy a config we failed to understand: the
                        // user's rules are in there.
                        let backup = path.with_extension(format!(
                            "json.bad-{}",
                            chrono::Local::now().format("%Y%m%d%H%M%S")
                        ));
                        match fs::rename(&path, &backup) {
                            Ok(()) => warn!(
                                "Failed to parse config file {:?}: {}. \
                                 Kept it as {:?} and starting from defaults.",
                                path, e, backup
                            ),
                            Err(rename_err) => {
                                warn!(
                                    "Failed to parse config file {:?}: {}. \
                                     Could not back it up ({}), so it is left untouched \
                                     and defaults are used for this run.",
                                    path, e, rename_err
                                );
                                return Self::default();
                            }
                        }
                    }
                },
                Err(e) => {
                    warn!(
                        "Failed to read config file {:?}: {}. Using default config.",
                        path, e
                    );
                    return Self::default();
                }
            }
        }
        let default_cfg = Self::default();
        let _ = default_cfg.save();
        default_cfg
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {}", e))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        // Write-then-rename so a crash mid-save cannot truncate the config.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| format!("Failed to write config file: {}", e))?;
        fs::rename(&tmp, &path).map_err(|e| format!("Failed to replace config file: {}", e))?;
        info!("Saved configuration to {:?}", path);
        Ok(())
    }
}

mod dirs {
    use std::path::PathBuf;
    pub fn config_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appimage_rule_matches_the_program_not_the_trailing_flag() {
        let rule = AutostartRule {
            name: "VRCX-0".to_string(),
            exec_cmd: "/home/blu/AppImages/vrcx0.appimage --appimage-extract-and-run".to_string(),
            ..Default::default()
        };
        let patterns = rule.effective_patterns();
        assert!(patterns.contains(&"vrcx0.appimage".to_string()));
        assert!(!patterns.iter().any(|p| p.starts_with("--")));
    }

    #[test]
    fn explicit_patterns_win_over_derived_ones() {
        let rule = AutostartRule {
            name: "Whatever".to_string(),
            exec_cmd: "steam steam://rungameid/4296960".to_string(),
            match_patterns: vec!["  VRCVideoCacher ".to_string(), String::new()],
            ..Default::default()
        };
        assert_eq!(
            rule.effective_patterns(),
            vec!["vrcvideocacher".to_string()]
        );
    }

    #[test]
    fn a_multi_word_rule_name_still_yields_usable_patterns() {
        // "VRCX-Extras Companion" never matched a process name; the command's
        // file name has to carry the match instead.
        let rule = AutostartRule {
            name: "VRCX-Extras Companion".to_string(),
            exec_cmd: "/run/media/system/Data/Projects/vrcx-extras/start.sh".to_string(),
            ..Default::default()
        };
        assert!(rule.effective_patterns().contains(&"start.sh".to_string()));
    }

    #[test]
    fn keeps_running_when_grace_is_negative() {
        let rule = AutostartRule {
            grace_period_secs: -1,
            ..Default::default()
        };
        assert!(rule.keeps_running());
    }

    #[test]
    fn a_config_missing_new_fields_still_loads() {
        // Configs written by older builds have no match_patterns /
        // spawn_debounce_secs; they must not fail to parse.
        let json = r#"{
            "auto_restart_wivrn": false,
            "autostart_rules": [
                {"id": "a", "name": "A", "exec_cmd": "/bin/true", "trigger": "WiVRn"}
            ]
        }"#;
        let cfg: Config = serde_json::from_str(json).expect("old configs must still parse");
        assert!(!cfg.auto_restart_wivrn);
        assert!(
            cfg.auto_switch_audio,
            "missing fields fall back to defaults"
        );
        assert_eq!(cfg.spawn_debounce_secs, 20);
        assert_eq!(cfg.autostart_rules.len(), 1);
        assert_eq!(cfg.autostart_rules[0].grace_period_secs, 120);
        assert!(cfg.autostart_rules[0].match_patterns.is_empty());
    }

    #[test]
    fn defaults_round_trip_through_json() {
        let cfg = Config::default();
        let json = serde_json::to_string_pretty(&cfg).expect("serialize");
        let parsed: Config = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.autostart_rules.len(), cfg.autostart_rules.len());
        assert_eq!(parsed.wivrn_command, cfg.wivrn_command);
    }
}
