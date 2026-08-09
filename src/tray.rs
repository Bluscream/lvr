use crate::service::{AppState, Command};
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Category, MenuItem, ToolTip, Tray};

/// The pieces of state the tray actually displays. Kept as a value so the tray
/// can be refreshed only when something visible changed, instead of redrawing
/// on a timer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraySnapshot {
    pub wivrn_running: bool,
    pub vrchat_running: bool,
    pub audio_connected: bool,
    pub watchdog: bool,
    pub audio_auto: bool,
}

impl TraySnapshot {
    pub fn of(state: &AppState) -> Self {
        let config = state.config_snapshot();
        Self {
            wivrn_running: *state
                .wivrn_running
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            vrchat_running: *state
                .vrchat_running
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            audio_connected: *state
                .wivrn_audio_connected
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            watchdog: config.auto_restart_wivrn,
            audio_auto: config.auto_switch_audio,
        }
    }
}

pub struct LinuxVrTray {
    pub state: AppState,
    pub snapshot: TraySnapshot,
}

impl LinuxVrTray {
    pub fn new(state: AppState) -> Self {
        let snapshot = TraySnapshot::of(&state);
        Self { state, snapshot }
    }
}

impl Tray for LinuxVrTray {
    fn id(&self) -> String {
        "lvr".to_string()
    }

    fn title(&self) -> String {
        "LinuxVR".to_string()
    }

    fn icon_name(&self) -> String {
        if self.snapshot.audio_connected {
            "audio-headset".to_string()
        } else if self.snapshot.wivrn_running {
            "media-playback-start".to_string()
        } else {
            "media-playback-stop".to_string()
        }
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    /// Left-clicking the tray icon opens the window, which is what users expect
    /// of a tray app.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.state.request_show_window();
    }

    fn tool_tip(&self) -> ToolTip {
        let status = format!(
            "WiVRn: {}\nVRChat: {}\nAudio: {}",
            if self.snapshot.wivrn_running {
                "Running"
            } else {
                "Stopped"
            },
            if self.snapshot.vrchat_running {
                "Running"
            } else {
                "Not running"
            },
            if self.snapshot.audio_connected {
                "VR headset"
            } else {
                "Desktop"
            }
        );
        ToolTip {
            title: "LinuxVR (lvr)".to_string(),
            description: status,
            icon_name: self.icon_name(),
            icon_pixmap: Vec::new(),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open LinuxVR GUI".to_string(),
                icon_name: "window-new".to_string(),
                activate: Box::new(|this: &mut Self| this.state.request_show_window()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "⚡ NUKE VR & Restart WiVRn".to_string(),
                icon_name: "process-stop".to_string(),
                activate: Box::new(|this: &mut Self| this.state.send(Command::Nuke)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "WiVRn Auto-Restart".to_string(),
                checked: self.snapshot.watchdog,
                activate: Box::new(|this: &mut Self| {
                    let enabled = this.state.update_config(|cfg| {
                        cfg.auto_restart_wivrn = !cfg.auto_restart_wivrn;
                        cfg.auto_restart_wivrn
                    });
                    this.state.add_log(if enabled {
                        "WiVRn auto-restart enabled."
                    } else {
                        "WiVRn auto-restart disabled."
                    });
                    this.state.send(Command::Poke);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Audio Auto-Switch".to_string(),
                checked: self.snapshot.audio_auto,
                activate: Box::new(|this: &mut Self| {
                    let enabled = this.state.update_config(|cfg| {
                        cfg.auto_switch_audio = !cfg.auto_switch_audio;
                        cfg.auto_switch_audio
                    });
                    this.state.add_log(if enabled {
                        "Audio auto-switching enabled."
                    } else {
                        "Audio auto-switching disabled."
                    });
                    this.state.send(Command::Poke);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit LinuxVR".to_string(),
                icon_name: "application-exit".to_string(),
                // Was `std::process::exit(0)`, which skipped the config save
                // and left the runtime socket behind.
                activate: Box::new(|this: &mut Self| {
                    this.state.set_quitting();
                    this.state.send(Command::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn the_snapshot_only_changes_when_something_visible_changes() {
        let (state, _rx) = AppState::new(Config::default());
        let first = TraySnapshot::of(&state);
        state.add_log("this is not shown in the tray");
        assert_eq!(first, TraySnapshot::of(&state));

        *state.wivrn_running.lock().unwrap() = true;
        assert_ne!(first, TraySnapshot::of(&state));
    }

    #[test]
    fn the_icon_reflects_the_status() {
        let (state, _rx) = AppState::new(Config::default());
        let mut tray = LinuxVrTray::new(state);
        assert_eq!(tray.icon_name(), "media-playback-stop");
        tray.snapshot.wivrn_running = true;
        assert_eq!(tray.icon_name(), "media-playback-start");
        tray.snapshot.audio_connected = true;
        assert_eq!(tray.icon_name(), "audio-headset");
    }

    #[test]
    fn activating_the_tray_asks_for_the_window() {
        let (state, _rx) = AppState::new(Config::default());
        let mut tray = LinuxVrTray::new(state.clone());
        tray.activate(0, 0);
        assert!(state.take_show_window());
    }
}
