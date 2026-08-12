//! State shared between the supervisor thread, the tray and the GUI.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Local};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::config::Config;

/// Commands the GUI and tray send to the supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run one supervisor pass right now instead of waiting for the next tick.
    Poke,
    StartWivrn,
    StopWivrn,
    RestartWivrn,
    /// Ask the headset to disconnect but leave the server running.
    DisconnectHeadset,
    StartEntry(String),
    StopEntry(String),
    StopAllVr,
    /// Route audio to the headset (`true`) or back to the desktop (`false`).
    SetAudioVr(bool),
    RefreshAudioDevices,
    /// Persist the current config to disk.
    SaveConfig,
    /// Point the managed Steam app at the named Proton profile, restarting
    /// Steam around the edit.
    SwitchSteamProfile(String),
    Quit,
}

/// What the supervisor last observed. Read by the GUI and tray.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub wivrn_running: bool,
    pub headset_connected: bool,
    pub headset_name: String,
    pub session_running: bool,
    pub vrchat_running: bool,
    pub watchdog_paused: bool,
    pub wivrn_failures: u32,
    pub default_sink: String,
    pub default_source: String,
    pub audio_on_vr: bool,
    pub entries: Vec<EntryStatus>,
    /// Name of the configured Steam profile matching what is on disk, if any.
    pub steam_profile: Option<String>,
    /// Compat tool the managed Steam app is pinned to right now.
    pub steam_compat_tool: String,
    /// A profile switch is in progress (Steam is being restarted).
    pub steam_switching: bool,
    pub sinks: Vec<AudioDevice>,
    pub sources: Vec<AudioDevice>,
    pub last_tick: Option<DateTime<Local>>,
}

impl Status {
    pub fn entry(&self, id: &str) -> Option<&EntryStatus> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn running_entry_count(&self) -> usize {
        self.entries.iter().filter(|e| e.running).count()
    }

    pub fn friendly_sink_label(&self) -> String {
        friendly_label(&self.default_sink, &self.sinks)
    }

    pub fn friendly_source_label(&self) -> String {
        friendly_label(&self.default_source, &self.sources)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryStatus {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub pids: Vec<u32>,
    pub trigger_active: bool,
    /// Seconds left before the entry is stopped, if a grace timer is ticking.
    pub stop_in_secs: Option<i64>,
    /// Seconds left before the entry is launched, if a start delay is ticking.
    pub start_in_secs: Option<i64>,
    /// User stopped it by hand; auto-start is suppressed until the trigger
    /// goes away and comes back.
    pub suppressed: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioDevice {
    pub name: String,
    pub description: String,
}

impl AudioDevice {
    pub fn label(&self) -> &str {
        if self.description.is_empty() {
            &self.name
        } else {
            &self.description
        }
    }
}

pub fn friendly_label(name: &str, devices: &[AudioDevice]) -> String {
    let name_trimmed = name.trim();
    if name_trimmed.is_empty() {
        return "unknown".to_string();
    }
    if let Some(dev) = devices.iter().find(|d| d.name == name_trimmed) {
        let label = dev.label().trim();
        if !label.is_empty() {
            return label.to_string();
        }
    }
    if name_trimmed == "wivrn.sink" {
        return "WiVRn Sink".to_string();
    }
    if name_trimmed == "wivrn.source" {
        return "WiVRn Source".to_string();
    }
    clean_node_name(name_trimmed)
}

fn clean_node_name(name: &str) -> String {
    let mut cleaned = name;
    for prefix in &["alsa_output.", "alsa_input.", "bluez_output.", "bluez_input."] {
        if let Some(rest) = cleaned.strip_prefix(prefix) {
            cleaned = rest;
            break;
        }
    }
    if let Some(pos) = cleaned.rfind('.') {
        if pos > 0 && pos < cleaned.len() - 1 {
            let suffix = &cleaned[pos + 1..];
            if suffix.contains("stereo") || suffix.contains("mono") || suffix.contains("multichannel") {
                cleaned = &cleaned[..pos];
            }
        }
    }
    if let Some(rest) = cleaned.strip_prefix("usb-") {
        cleaned = rest;
    } else if let Some(rest) = cleaned.strip_prefix("pci-") {
        cleaned = rest;
    }
    let mut result = String::new();
    for part in cleaned.split(['_', '-']) {
        if part.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(part);
    }
    if result.trim().is_empty() {
        name.to_string()
    } else {
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub at: DateTime<Local>,
    pub level: LogLevel,
    pub message: String,
}

impl LogLine {
    pub fn formatted(&self) -> String {
        format!(
            "{} [{}] {}",
            self.at.format("%H:%M:%S"),
            self.level.label(),
            self.message
        )
    }
}

/// Everything the three halves of the app need to see.
#[derive(Clone)]
pub struct Shared {
    inner: Arc<Inner>,
}

struct Inner {
    config: Mutex<Config>,
    config_path: PathBuf,
    status: Mutex<Status>,
    logs: Mutex<VecDeque<LogLine>>,
    commands: UnboundedSender<Command>,
    /// Mirror of `config.general.log_capacity`, so logging never has to take
    /// the config lock (and can therefore be called while holding it).
    log_capacity: AtomicUsize,
    /// Set when the tray (or a second instance) wants the window on screen.
    show_window: AtomicBool,
    /// Tab a second launch asked for, by name.
    requested_tab: Mutex<Option<String>>,
    /// Set when the app should exit for real rather than hide to tray.
    quitting: AtomicBool,
    /// egui repaint handle, installed once the GUI is up.
    repaint: Mutex<Option<egui::Context>>,
}

impl Shared {
    pub fn new(config: Config, config_path: PathBuf) -> (Self, UnboundedReceiver<Command>) {
        let (tx, rx) = unbounded_channel();
        let log_capacity = AtomicUsize::new(config.general.log_capacity);
        let shared = Shared {
            inner: Arc::new(Inner {
                config: Mutex::new(config),
                config_path,
                status: Mutex::new(Status::default()),
                logs: Mutex::new(VecDeque::new()),
                commands: tx,
                log_capacity,
                show_window: AtomicBool::new(false),
                requested_tab: Mutex::new(None),
                quitting: AtomicBool::new(false),
                repaint: Mutex::new(None),
            }),
        };
        (shared, rx)
    }

    pub fn config(&self) -> MutexGuard<'_, Config> {
        lock(&self.inner.config)
    }

    pub fn config_snapshot(&self) -> Config {
        self.config().clone()
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.inner.config_path
    }

    pub fn status(&self) -> MutexGuard<'_, Status> {
        lock(&self.inner.status)
    }

    pub fn status_snapshot(&self) -> Status {
        self.status().clone()
    }

    pub fn set_status(&self, status: Status) {
        *self.status() = status;
        self.request_repaint();
    }

    pub fn send(&self, command: Command) {
        // The receiver only goes away during shutdown, where dropped commands
        // are exactly what we want.
        let _ = self.inner.commands.send(command);
    }

    pub fn log(&self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        match level {
            LogLevel::Debug => tracing::debug!("{message}"),
            LogLevel::Info => tracing::info!("{message}"),
            LogLevel::Warn => tracing::warn!("{message}"),
            LogLevel::Error => tracing::error!("{message}"),
        }
        let capacity = self.inner.log_capacity.load(Ordering::Relaxed).max(1);
        let mut logs = lock(&self.inner.logs);
        logs.push_back(LogLine {
            at: Local::now(),
            level,
            message,
        });
        while logs.len() > capacity {
            logs.pop_front();
        }
        drop(logs);
        self.request_repaint();
    }

    pub fn info(&self, message: impl Into<String>) {
        self.log(LogLevel::Info, message);
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.log(LogLevel::Warn, message);
    }

    pub fn error(&self, message: impl Into<String>) {
        self.log(LogLevel::Error, message);
    }

    /// Re-read the log capacity from the config. Called once per supervisor
    /// tick so edits in the Settings tab take effect.
    pub fn sync_log_capacity(&self) {
        let capacity = self.config().general.log_capacity;
        self.inner.log_capacity.store(capacity, Ordering::Relaxed);
    }

    pub fn logs(&self) -> Vec<LogLine> {
        lock(&self.inner.logs).iter().cloned().collect()
    }

    pub fn clear_logs(&self) {
        lock(&self.inner.logs).clear();
    }

    pub fn save_config(&self) -> anyhow::Result<()> {
        let config = self.config_snapshot();
        config.save(&self.inner.config_path)
    }

    pub fn set_repaint_ctx(&self, ctx: egui::Context) {
        *lock(&self.inner.repaint) = Some(ctx);
    }

    pub fn request_repaint(&self) {
        if let Some(ctx) = lock(&self.inner.repaint).as_ref() {
            ctx.request_repaint();
        }
    }

    pub fn request_show_window(&self) {
        self.inner.show_window.store(true, Ordering::SeqCst);
        self.request_repaint();
    }

    pub fn take_show_window(&self) -> bool {
        self.inner.show_window.swap(false, Ordering::SeqCst)
    }

    /// Ask for the window *and* a specific tab (by name).
    pub fn request_show_tab(&self, tab: Option<String>) {
        *lock(&self.inner.requested_tab) = tab;
        self.request_show_window();
    }

    pub fn take_requested_tab(&self) -> Option<String> {
        lock(&self.inner.requested_tab).take()
    }

    pub fn set_quitting(&self) {
        self.inner.quitting.store(true, Ordering::SeqCst);
        self.request_repaint();
    }

    pub fn is_quitting(&self) -> bool {
        self.inner.quitting.load(Ordering::SeqCst)
    }
}

/// Mutex helper: a poisoned lock means some other thread panicked, which is not
/// a reason to take the whole app down with it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_label_uses_device_description_or_clean_name() {
        let devices = vec![AudioDevice {
            name: "alsa_output.usb-Smartlink_123.analog-stereo".into(),
            description: "Smartlink WG2 Headset".into(),
        }];
        assert_eq!(
            friendly_label("alsa_output.usb-Smartlink_123.analog-stereo", &devices),
            "Smartlink WG2 Headset"
        );
        assert_eq!(friendly_label("wivrn.sink", &[]), "WiVRn Sink");
        assert_eq!(friendly_label("wivrn.source", &[]), "WiVRn Source");
        assert_eq!(friendly_label("", &[]), "unknown");
        assert_eq!(
            friendly_label("alsa_output.usb-SmartlinkTechnology_WG2_20201111000001-00.analog-stereo", &[]),
            "SmartlinkTechnology WG2"
        );
    }

    fn shared() -> Shared {
        Shared::new(Config::default(), PathBuf::from("/nonexistent/config.toml")).0
    }

    #[test]
    fn logs_are_capped_at_capacity() {
        let shared = shared();
        shared.config().general.log_capacity = 5;
        shared.sync_log_capacity();
        for i in 0..20 {
            shared.info(format!("line {i}"));
        }
        let logs = shared.logs();
        assert_eq!(logs.len(), 5);
        assert_eq!(logs[4].message, "line 19");
    }

    #[test]
    fn show_window_flag_is_consumed_once() {
        let shared = shared();
        assert!(!shared.take_show_window());
        shared.request_show_window();
        assert!(shared.take_show_window());
        assert!(!shared.take_show_window());
    }

    #[test]
    fn a_requested_tab_is_delivered_once() {
        let shared = shared();
        assert_eq!(shared.take_requested_tab(), None);
        shared.request_show_tab(Some("logs".into()));
        assert!(shared.take_show_window());
        assert_eq!(shared.take_requested_tab().as_deref(), Some("logs"));
        assert_eq!(shared.take_requested_tab(), None);
    }

    #[test]
    fn commands_reach_the_receiver() {
        let (shared, mut rx) = Shared::new(Config::default(), PathBuf::from("/tmp/x.toml"));
        shared.send(Command::RestartWivrn);
        assert_eq!(rx.try_recv().ok(), Some(Command::RestartWivrn));
    }

    #[test]
    fn audio_device_label_prefers_description() {
        let device = AudioDevice {
            name: "wivrn.sink".into(),
            description: String::new(),
        };
        assert_eq!(device.label(), "wivrn.sink");
        let device = AudioDevice {
            name: "wivrn.sink".into(),
            description: "WiVRn".into(),
        };
        assert_eq!(device.label(), "WiVRn");
    }
}
