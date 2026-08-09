//! Persistent configuration for `lvr`.
//!
//! The config lives at `$XDG_CONFIG_HOME/lvr/config.toml` (usually
//! `~/.config/lvr/config.toml`). It is written atomically so a crash mid-save
//! can never leave a truncated file behind.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// What makes an autostart entry want to be running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// VRChat is running (matched against [`General::vrchat_match`]).
    Vrchat,
    /// The WiVRn server is up (D-Bus service present).
    WivrnRunning,
    /// A headset is actually connected to the WiVRn server.
    HeadsetConnected,
    /// Any process whose command line contains this string (case-insensitive).
    Process(String),
    /// Never triggers automatically; start/stop from the UI or tray only.
    Manual,
}

impl Trigger {
    pub const KINDS: [&'static str; 5] = [
        "VRChat running",
        "WiVRn running",
        "Headset connected",
        "Custom process",
        "Manual only",
    ];

    pub fn kind_index(&self) -> usize {
        match self {
            Trigger::Vrchat => 0,
            Trigger::WivrnRunning => 1,
            Trigger::HeadsetConnected => 2,
            Trigger::Process(_) => 3,
            Trigger::Manual => 4,
        }
    }

    pub fn from_kind_index(index: usize, process: &str) -> Trigger {
        match index {
            0 => Trigger::Vrchat,
            1 => Trigger::WivrnRunning,
            2 => Trigger::HeadsetConnected,
            3 => Trigger::Process(process.to_string()),
            _ => Trigger::Manual,
        }
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trigger::Vrchat => f.write_str("VRChat running"),
            Trigger::WivrnRunning => f.write_str("WiVRn running"),
            Trigger::HeadsetConnected => f.write_str("Headset connected"),
            Trigger::Process(p) => write!(f, "Process: {p}"),
            Trigger::Manual => f.write_str("Manual only"),
        }
    }
}

/// A single managed application.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutostartEntry {
    /// Stable identifier, used by tray/UI commands. Generated from the name.
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger: Trigger,
    /// Command line. Parsed with shell word splitting unless `use_shell`.
    pub command: String,
    /// Run the command through `sh -c` (needed for pipes, `&&`, globs, ...).
    pub use_shell: bool,
    /// Working directory; empty means "inherit".
    pub working_dir: String,
    /// Launch inside a terminal emulator so the app's console stays visible.
    pub console: bool,
    /// Substrings matched (case-insensitively) against running command lines,
    /// used both to detect "is it already running" and to stop it again.
    pub match_patterns: Vec<String>,
    /// Seconds to keep the app alive after its trigger disappears.
    /// `-1` means "never stop it automatically".
    pub grace_secs: i64,
    /// Seconds to wait after the trigger fires before launching.
    pub start_delay_secs: u64,
    /// Relaunch the app if it exits while its trigger is still active.
    /// Off by default so closing an app by hand keeps it closed.
    pub restart_on_exit: bool,
    /// Optional custom stop command (e.g. `flatpak kill dev.slimevr.SlimeVR`).
    pub stop_command: String,
    /// Include this entry in the "Stop everything VR" action.
    pub include_in_stop_all: bool,
}

impl Default for AutostartEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            trigger: Trigger::Manual,
            command: String::new(),
            use_shell: false,
            working_dir: String::new(),
            console: false,
            match_patterns: Vec::new(),
            grace_secs: 0,
            start_delay_secs: 0,
            restart_on_exit: false,
            stop_command: String::new(),
            include_in_stop_all: true,
        }
    }
}

impl AutostartEntry {
    /// Patterns actually used for detection: falls back to the executable's
    /// file stem so a freshly added entry still behaves sensibly.
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
        match shell_words::split(&self.command)
            .ok()
            .and_then(|parts| parts.into_iter().next())
        {
            Some(first) => Path::new(&first)
                .file_name()
                .map(|s| vec![s.to_string_lossy().to_lowercase()])
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    pub fn keeps_running(&self) -> bool {
        self.grace_secs < 0
    }

    /// Display name, falling back to the id for unnamed entries.
    pub fn name_or_id(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// egui zoom factor. Bumped above 1.0 so buttons stay hittable with VR
    /// controllers / from across the room.
    pub ui_scale: f32,
    /// Supervisor tick interval.
    pub poll_interval_ms: u64,
    /// Start with the window hidden (tray only).
    pub start_hidden: bool,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    /// Command-line substrings that identify VRChat (case-insensitive).
    pub vrchat_match: Vec<String>,
    /// Terminal used for `console = true` entries. `{cmd}` is replaced with the
    /// shell-quoted command. Empty means auto-detect.
    pub terminal: String,
    /// Ask before running "Stop everything VR".
    pub confirm_stop_all: bool,
    /// Lines of log history kept in memory / on screen.
    pub log_capacity: usize,
    /// Minimum gap between two launch attempts of the same entry. Gives slow
    /// starters (Steam, flatpak, AppImage extraction) time to show up in the
    /// process table before we would consider launching them again.
    pub relaunch_debounce_secs: u64,
    /// How long a stopped app gets to exit on SIGTERM before it is SIGKILLed.
    pub stop_grace_secs: u64,
    /// Show PIDs and extra debug information in status labels.
    pub show_debug_info: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            ui_scale: 1.25,
            poll_interval_ms: 1000,
            start_hidden: true,
            close_to_tray: true,
            vrchat_match: vec!["vrchat.exe".into()],
            terminal: String::new(),
            confirm_stop_all: true,
            log_capacity: 1000,
            relaunch_debounce_secs: 30,
            stop_grace_secs: 5,
            show_debug_info: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WivrnConfig {
    /// Keep the WiVRn server alive, restarting it when it crashes or is closed.
    pub watchdog: bool,
    /// Command used to (re)start WiVRn.
    pub start_command: String,
    /// How long the server must be gone before the watchdog restarts it.
    pub restart_delay_secs: u64,
    /// Give up after this many restarts that did not stick (0 = never give up).
    pub max_consecutive_failures: u32,
    /// Flatpak application id, used for the forceful `flatpak kill` fallback.
    pub flatpak_id: String,
}

impl Default for WivrnConfig {
    fn default() -> Self {
        Self {
            watchdog: true,
            start_command: "flatpak run --branch=stable --arch=x86_64 \
                            --command=/app/bin/wivrn-dashboard io.github.wivrn.wivrn"
                .into(),
            restart_delay_secs: 5,
            max_consecutive_failures: 5,
            flatpak_id: "io.github.wivrn.wivrn".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Switch default sink/source when the headset connects and disconnects.
    pub enabled: bool,
    pub vr_sink: String,
    pub vr_source: String,
    /// Device to return to on disconnect. Empty = whatever was active before.
    pub desktop_sink: String,
    pub desktop_source: String,
    /// Also move already-running streams over to the new device.
    pub move_streams: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            vr_sink: "wivrn.sink".into(),
            vr_source: "wivrn.source".into(),
            desktop_sink: String::new(),
            desktop_source: String::new(),
            move_streams: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub wivrn: WivrnConfig,
    pub audio: AudioConfig,
    #[serde(rename = "autostart")]
    pub autostart: Vec<AutostartEntry>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            wivrn: WivrnConfig::default(),
            audio: AudioConfig::default(),
            autostart: default_entries(),
        }
    }
}

impl Config {
    /// Config path, honouring `$LVR_CONFIG` and then XDG.
    pub fn default_path() -> PathBuf {
        if let Some(path) = std::env::var_os("LVR_CONFIG") {
            return PathBuf::from(path);
        }
        directories::ProjectDirs::from("", "", "lvr")
            .map(|dirs| dirs.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("lvr.toml"))
    }

    /// Load the config, creating it with sensible defaults if missing.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if !path.exists() {
            let config = Config::default();
            config.save(path)?;
            return Ok(config);
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut config: Config =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        config.normalize();
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    /// Fix up anything that would make the app misbehave: blank ids, duplicate
    /// ids, out-of-range scales.
    pub fn normalize(&mut self) {
        let mut seen: Vec<String> = Vec::with_capacity(self.autostart.len());
        for index in 0..self.autostart.len() {
            let base = if self.autostart[index].id.trim().is_empty() {
                slugify(&self.autostart[index].name)
            } else {
                slugify(&self.autostart[index].id)
            };
            let base = if base.is_empty() {
                format!("entry-{index}")
            } else {
                base
            };
            let mut candidate = base.clone();
            let mut suffix = 2;
            while seen.contains(&candidate) {
                candidate = format!("{base}-{suffix}");
                suffix += 1;
            }
            seen.push(candidate.clone());
            self.autostart[index].id = candidate;
        }

        self.general.ui_scale = self.general.ui_scale.clamp(0.6, 4.0);
        self.general.poll_interval_ms = self.general.poll_interval_ms.clamp(200, 60_000);
        self.general.log_capacity = self.general.log_capacity.clamp(50, 100_000);
        self.general.relaunch_debounce_secs = self.general.relaunch_debounce_secs.clamp(1, 3600);
        self.general.stop_grace_secs = self.general.stop_grace_secs.clamp(1, 300);
        self.wivrn.restart_delay_secs = self.wivrn.restart_delay_secs.clamp(1, 3600);
        self.general.vrchat_match = self
            .general
            .vrchat_match
            .iter()
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
        if self.general.vrchat_match.is_empty() {
            self.general.vrchat_match = General::default().vrchat_match;
        }
    }

    /// A unique id for a new entry named `name`.
    pub fn unique_id(&self, name: &str) -> String {
        let base = {
            let slug = slugify(name);
            if slug.is_empty() {
                "entry".to_string()
            } else {
                slug
            }
        };
        let mut candidate = base.clone();
        let mut suffix = 2;
        while self.autostart.iter().any(|e| e.id == candidate) {
            candidate = format!("{base}-{suffix}");
            suffix += 1;
        }
        candidate
    }

    pub fn entry(&self, id: &str) -> Option<&AutostartEntry> {
        self.autostart.iter().find(|e| e.id == id)
    }
}

pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = true;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/root"))
}

fn home_str(relative: &str) -> String {
    home().join(relative).to_string_lossy().into_owned()
}

/// The default set of managed apps, matching a typical Bazzite + WiVRn setup.
/// Everything here is editable in the Autostart tab.
fn default_entries() -> Vec<AutostartEntry> {
    vec![
        AutostartEntry {
            id: "vrcvideocacher".into(),
            name: "VRCVideoCacher".into(),
            enabled: true,
            trigger: Trigger::Vrchat,
            command: home_str("Desktop/VRCVideoCacher"),
            working_dir: home_str("Desktop"),
            console: true,
            match_patterns: vec!["vrcvideocacher".into()],
            grace_secs: 120,
            ..Default::default()
        },
        AutostartEntry {
            id: "vrcosc".into(),
            name: "VRCOSC".into(),
            enabled: true,
            trigger: Trigger::Vrchat,
            command: home_str(".local/bin/vrcosc"),
            // Matches both the launcher script and the Wine-side executable,
            // without matching every path that merely mentions VRCOSC.
            match_patterns: vec!["vrcosc.exe".into(), "/.local/bin/vrcosc".into()],
            grace_secs: 120,
            ..Default::default()
        },
        AutostartEntry {
            id: "vrcx".into(),
            name: "VRCX".into(),
            enabled: true,
            trigger: Trigger::Vrchat,
            command: format!(
                "{} --appimage-extract-and-run",
                home_str("AppImages/vrcx0.appimage")
            ),
            match_patterns: vec!["vrcx0.appimage".into(), "vrcx-0".into()],
            grace_secs: -1,
            ..Default::default()
        },
        AutostartEntry {
            id: "vrcx-extras".into(),
            name: "VRCX-Extras".into(),
            enabled: true,
            trigger: Trigger::Vrchat,
            command: "/run/media/system/Data/Projects/vrcx-extras/start.sh".into(),
            match_patterns: vec!["vrcx-extras".into(), "server.ts".into()],
            grace_secs: -1,
            ..Default::default()
        },
        AutostartEntry {
            id: "slimevr".into(),
            name: "SlimeVR".into(),
            enabled: true,
            trigger: Trigger::WivrnRunning,
            command: "flatpak run dev.slimevr.SlimeVR".into(),
            match_patterns: vec!["/app/main/slimevr".into(), "slimevr.jar".into()],
            grace_secs: 300,
            stop_command: "flatpak kill dev.slimevr.SlimeVR".into(),
            ..Default::default()
        },
        AutostartEntry {
            id: "wayvr".into(),
            name: "WayVR".into(),
            // Off by default: WiVRn's own XR-plugin autostart handles this.
            enabled: false,
            trigger: Trigger::WivrnRunning,
            command: format!(
                "env DESKTOPINTEGRATION=1 {}",
                home_str("AppImages/wayvr.appimage")
            ),
            match_patterns: vec!["wayvr.appimage".into()],
            grace_secs: 60,
            ..Default::default()
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_makes_stable_ids() {
        assert_eq!(slugify("VRCX-Extras"), "vrcx-extras");
        assert_eq!(slugify("  Hello, World!  "), "hello-world");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn normalize_dedupes_ids_and_fills_blanks() {
        let mut config = Config {
            autostart: vec![
                AutostartEntry {
                    id: String::new(),
                    name: "App".into(),
                    ..Default::default()
                },
                AutostartEntry {
                    id: String::new(),
                    name: "App".into(),
                    ..Default::default()
                },
                AutostartEntry {
                    id: String::new(),
                    name: "!!!".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        config.normalize();
        assert_eq!(config.autostart[0].id, "app");
        assert_eq!(config.autostart[1].id, "app-2");
        assert_eq!(config.autostart[2].id, "entry-2");
    }

    #[test]
    fn normalize_clamps_out_of_range_values() {
        let mut config = Config::default();
        config.general.ui_scale = 99.0;
        config.general.poll_interval_ms = 1;
        config.general.vrchat_match = vec!["  ".into()];
        config.normalize();
        assert_eq!(config.general.ui_scale, 4.0);
        assert_eq!(config.general.poll_interval_ms, 200);
        assert_eq!(config.general.vrchat_match, vec!["vrchat.exe".to_string()]);
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let config = Config::default();
        let text = toml::to_string_pretty(&config).expect("serialize");
        let parsed: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(config, parsed);
    }

    #[test]
    fn partial_config_uses_defaults_for_missing_fields() {
        let text = r#"
            [general]
            ui_scale = 2.0

            [[autostart]]
            name = "Thing"
            command = "/bin/true"
            trigger = "vrchat"
        "#;
        let mut config: Config = toml::from_str(text).expect("deserialize");
        config.normalize();
        assert_eq!(config.general.ui_scale, 2.0);
        assert!(config.general.close_to_tray);
        assert_eq!(config.autostart.len(), 1);
        assert_eq!(config.autostart[0].id, "thing");
        assert_eq!(config.autostart[0].trigger, Trigger::Vrchat);
        assert_eq!(config.wivrn.flatpak_id, "io.github.wivrn.wivrn");
    }

    #[test]
    fn custom_process_trigger_round_trips() {
        let entry = AutostartEntry {
            trigger: Trigger::Process("Foo.exe".into()),
            ..Default::default()
        };
        let text = toml::to_string(&entry).expect("serialize");
        let parsed: AutostartEntry = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed.trigger, Trigger::Process("Foo.exe".into()));
    }

    #[test]
    fn effective_patterns_fall_back_to_executable_name() {
        let entry = AutostartEntry {
            command: "/usr/bin/some-app --flag".into(),
            ..Default::default()
        };
        assert_eq!(entry.effective_patterns(), vec!["some-app".to_string()]);

        let entry = AutostartEntry {
            command: "/usr/bin/some-app".into(),
            match_patterns: vec!["  Explicit  ".into(), String::new()],
            ..Default::default()
        };
        assert_eq!(entry.effective_patterns(), vec!["explicit".to_string()]);
    }

    #[test]
    fn keeps_running_when_grace_is_negative() {
        let entry = AutostartEntry {
            grace_secs: -1,
            ..Default::default()
        };
        assert!(entry.keeps_running());
    }

    #[test]
    fn unique_id_avoids_collisions() {
        let config = Config::default();
        assert_eq!(config.unique_id("VRCX"), "vrcx-2");
        assert_eq!(config.unique_id("Brand New"), "brand-new");
    }
}
