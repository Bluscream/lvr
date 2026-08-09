use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TriggerType {
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
pub struct AutostartRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub exec_cmd: String,
    pub trigger: TriggerType,
    /// Grace period in seconds. -1 means keep running indefinitely after trigger app stops.
    pub grace_period_secs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub auto_restart_wivrn: bool,
    pub auto_switch_audio: bool,
    pub wivrn_command: String,
    pub vrchat_process_pattern: String,
    pub saved_audio_sink: Option<String>,
    pub saved_audio_source: Option<String>,
    pub autostart_rules: Vec<AutostartRule>,
    pub poll_interval_secs: u64,
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
            autostart_rules: vec![
                AutostartRule {
                    id: "vrc-video-cacher".to_string(),
                    name: "VRCVideoCacher".to_string(),
                    enabled: true,
                    exec_cmd: "steam steam://rungameid/4296960".to_string(),
                    trigger: TriggerType::VRChat,
                    grace_period_secs: 120,
                },
                AutostartRule {
                    id: "vrcosc".to_string(),
                    name: "VRCOSC".to_string(),
                    enabled: true,
                    exec_cmd: "/home/blu/.local/bin/vrcosc".to_string(),
                    trigger: TriggerType::VRChat,
                    grace_period_secs: 120,
                },
                AutostartRule {
                    id: "vrcx-0".to_string(),
                    name: "VRCX-0".to_string(),
                    enabled: true,
                    exec_cmd: "/home/blu/AppImages/vrcx0.appimage --appimage-extract-and-run".to_string(),
                    trigger: TriggerType::VRChat,
                    grace_period_secs: -1, // Keep running
                },
                AutostartRule {
                    id: "vrcx-extras".to_string(),
                    name: "VRCX-Extras Companion".to_string(),
                    enabled: true,
                    exec_cmd: "/run/media/system/Data/Projects/vrcx-extras/start.sh".to_string(),
                    trigger: TriggerType::VRChat,
                    grace_period_secs: -1, // Keep running
                },
                AutostartRule {
                    id: "slimevr".to_string(),
                    name: "SlimeVR".to_string(),
                    enabled: true,
                    exec_cmd: "flatpak run dev.slimevr.SlimeVR".to_string(),
                    trigger: TriggerType::WiVRn,
                    grace_period_secs: 300,
                },
                AutostartRule {
                    id: "wayvr".to_string(),
                    name: "WayVR".to_string(),
                    enabled: false, // Disabled by default
                    exec_cmd: "env DESKTOPINTEGRATION=1 /home/blu/AppImages/wayvr.appimage".to_string(),
                    trigger: TriggerType::WiVRn,
                    grace_period_secs: 300,
                },
            ],
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("/home/blu/.config"));
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
                        warn!("Failed to parse config file {:?}: {}. Using default config.", path, e);
                    }
                },
                Err(e) => {
                    warn!("Failed to read config file {:?}: {}. Using default config.", path, e);
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
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {}", e))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        fs::write(&path, json).map_err(|e| format!("Failed to write config file: {}", e))?;
        info!("Saved configuration to {:?}", path);
        Ok(())
    }
}

mod dirs {
    use std::path::PathBuf;
    pub fn config_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
            })
    }
}
