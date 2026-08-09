//! StatusNotifierItem tray icon (KDE/Plasma native, works anywhere SNI does).

use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{Category, Icon, MenuItem, ToolTip, Tray, TrayMethods};

use crate::icon::{IconState, tray_pixmaps};
use crate::state::{Command, Shared, Status};

pub struct LvrTray {
    shared: Shared,
    status: Status,
}

impl LvrTray {
    pub fn new(shared: Shared) -> Self {
        let status = shared.status_snapshot();
        Self { shared, status }
    }

    fn icon_state(&self) -> IconState {
        if self.status.headset_connected {
            IconState::Connected
        } else if self.status.wivrn_running {
            IconState::Ready
        } else if self.status.watchdog_paused {
            IconState::Problem
        } else {
            IconState::Idle
        }
    }

    fn summary(&self) -> String {
        let mut lines = vec![format!(
            "WiVRn: {}{}",
            if self.status.wivrn_running {
                "running"
            } else {
                "stopped"
            },
            if self.status.watchdog_paused {
                " (watchdog paused)"
            } else {
                ""
            }
        )];
        lines.push(format!(
            "Headset: {}",
            if self.status.headset_connected {
                if self.status.headset_name.is_empty() {
                    "connected".to_string()
                } else {
                    format!("connected ({})", self.status.headset_name)
                }
            } else {
                "disconnected".to_string()
            }
        ));
        lines.push(format!(
            "VRChat: {}",
            if self.status.vrchat_running {
                "running"
            } else {
                "not running"
            }
        ));
        lines.push(format!(
            "Audio: {}",
            if self.status.audio_on_vr {
                "VR"
            } else {
                "desktop"
            }
        ));
        lines.push(format!(
            "Managed apps running: {}/{}",
            self.status.running_entry_count(),
            self.status.entries.len()
        ));
        lines.join("\n")
    }
}

impl Tray for LvrTray {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").to_string()
    }

    fn title(&self) -> String {
        "LinuxVR".to_string()
    }

    fn category(&self) -> Category {
        Category::Hardware
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        tray_pixmaps(self.icon_state())
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "LinuxVR".to_string(),
            description: self.summary(),
            icon_name: String::new(),
            icon_pixmap: tray_pixmaps(self.icon_state()),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.shared.request_show_window();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let watchdog = self.shared.config().wivrn.watchdog;
        let audio_auto = self.shared.config().audio.enabled;
        let audio_on_vr = self.status.audio_on_vr;
        let entries = self.status.entries.clone();

        let mut items: Vec<MenuItem<Self>> = vec![
            StandardItem {
                label: "Open LinuxVR".into(),
                icon_name: "window-new".into(),
                activate: Box::new(|this: &mut Self| this.shared.request_show_window()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Restart WiVRn".into(),
                icon_name: "view-refresh".into(),
                activate: Box::new(|this: &mut Self| this.shared.send(Command::RestartWivrn)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if self.status.wivrn_running {
                    "Stop WiVRn".into()
                } else {
                    "Start WiVRn".into()
                },
                icon_name: if self.status.wivrn_running {
                    "media-playback-stop".into()
                } else {
                    "media-playback-start".into()
                },
                activate: {
                    let running = self.status.wivrn_running;
                    Box::new(move |this: &mut Self| {
                        this.shared.send(if running {
                            Command::StopWivrn
                        } else {
                            Command::StartWivrn
                        })
                    })
                },
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Stop everything VR".into(),
                icon_name: "process-stop".into(),
                activate: Box::new(|this: &mut Self| this.shared.send(Command::StopAllVr)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
        ];

        if !entries.is_empty() {
            let app_items: Vec<MenuItem<Self>> = entries
                .into_iter()
                .map(|entry| {
                    let id = entry.id.clone();
                    let running = entry.running;
                    StandardItem {
                        label: format!(
                            "{} {}",
                            if running { "■ Stop" } else { "▶ Start" },
                            entry.name
                        ),
                        activate: Box::new(move |this: &mut Self| {
                            this.shared.send(if running {
                                Command::StopEntry(id.clone())
                            } else {
                                Command::StartEntry(id.clone())
                            })
                        }),
                        ..Default::default()
                    }
                    .into()
                })
                .collect();
            items.push(
                SubMenu {
                    label: "Managed apps".into(),
                    submenu: app_items,
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(
            StandardItem {
                label: if audio_on_vr {
                    "Audio → desktop".into()
                } else {
                    "Audio → VR".into()
                },
                icon_name: "audio-headset".into(),
                activate: Box::new(move |this: &mut Self| {
                    this.shared.send(Command::SetAudioVr(!audio_on_vr))
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        items.push(
            CheckmarkItem {
                label: "WiVRn watchdog".into(),
                checked: watchdog,
                activate: Box::new(|this: &mut Self| {
                    let enabled = {
                        let mut config = this.shared.config();
                        config.wivrn.watchdog = !config.wivrn.watchdog;
                        config.wivrn.watchdog
                    };
                    this.shared.send(Command::SaveConfig);
                    this.shared.send(Command::Poke);
                    this.shared.info(if enabled {
                        "WiVRn watchdog enabled"
                    } else {
                        "WiVRn watchdog disabled"
                    });
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            CheckmarkItem {
                label: "Auto audio switching".into(),
                checked: audio_auto,
                activate: Box::new(|this: &mut Self| {
                    let enabled = {
                        let mut config = this.shared.config();
                        config.audio.enabled = !config.audio.enabled;
                        config.audio.enabled
                    };
                    this.shared.send(Command::SaveConfig);
                    this.shared.info(if enabled {
                        "Automatic audio switching enabled"
                    } else {
                        "Automatic audio switching disabled"
                    });
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Quit LinuxVR".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    this.shared.set_quitting();
                    this.shared.send(Command::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

/// Start the tray and keep it in sync with the supervisor's status.
pub async fn run(shared: Shared) {
    let handle = match LvrTray::new(shared.clone())
        .assume_sni_available(true)
        .spawn()
        .await
    {
        Ok(handle) => handle,
        Err(err) => {
            shared.warn(format!(
                "Tray unavailable ({err}); continuing without a tray icon"
            ));
            return;
        }
    };
    shared.info("Tray icon registered");

    let mut previous = shared.status_snapshot();
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if shared.is_quitting() {
            handle.shutdown().await;
            return;
        }
        let status = shared.status_snapshot();
        if tray_relevant_change(&previous, &status) {
            previous = status.clone();
            if handle
                .update(move |tray: &mut LvrTray| tray.status = status)
                .await
                .is_none()
            {
                return;
            }
        }
    }
}

/// Only redraw the tray when something it actually displays changed.
fn tray_relevant_change(before: &Status, after: &Status) -> bool {
    before.wivrn_running != after.wivrn_running
        || before.headset_connected != after.headset_connected
        || before.headset_name != after.headset_name
        || before.vrchat_running != after.vrchat_running
        || before.audio_on_vr != after.audio_on_vr
        || before.watchdog_paused != after.watchdog_paused
        || before.entries.len() != after.entries.len()
        || before
            .entries
            .iter()
            .zip(after.entries.iter())
            .any(|(a, b)| a.id != b.id || a.name != b.name || a.running != b.running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::EntryStatus;

    fn status() -> Status {
        Status {
            wivrn_running: true,
            entries: vec![EntryStatus {
                id: "a".into(),
                name: "A".into(),
                running: false,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn identical_status_does_not_redraw() {
        let a = status();
        let b = status();
        assert!(!tray_relevant_change(&a, &b));
    }

    #[test]
    fn headset_and_entry_changes_redraw() {
        let a = status();
        let mut b = status();
        b.headset_connected = true;
        assert!(tray_relevant_change(&a, &b));

        let mut c = status();
        c.entries[0].running = true;
        assert!(tray_relevant_change(&a, &c));

        let mut d = status();
        d.entries.clear();
        assert!(tray_relevant_change(&a, &d));
    }

    #[test]
    fn cosmetic_only_changes_do_not_redraw() {
        let a = status();
        let mut b = status();
        b.last_tick = Some(chrono::Local::now());
        b.default_sink = "something".into();
        assert!(!tray_relevant_change(&a, &b));
    }

    #[test]
    fn icon_state_follows_the_status() {
        let (shared, _rx) = crate::state::Shared::new(
            crate::config::Config::default(),
            std::path::PathBuf::from("/tmp/lvr-test.toml"),
        );
        let mut tray = LvrTray::new(shared);
        assert_eq!(tray.icon_state(), IconState::Idle);
        tray.status.wivrn_running = true;
        assert_eq!(tray.icon_state(), IconState::Ready);
        tray.status.headset_connected = true;
        assert_eq!(tray.icon_state(), IconState::Connected);
        tray.status = Status {
            watchdog_paused: true,
            ..Default::default()
        };
        assert_eq!(tray.icon_state(), IconState::Problem);
    }

    #[test]
    fn summary_mentions_every_headline_fact() {
        let (shared, _rx) = crate::state::Shared::new(
            crate::config::Config::default(),
            std::path::PathBuf::from("/tmp/lvr-test.toml"),
        );
        let mut tray = LvrTray::new(shared);
        tray.status = Status {
            wivrn_running: true,
            headset_connected: true,
            headset_name: "Meta Quest 3".into(),
            vrchat_running: true,
            audio_on_vr: true,
            ..Default::default()
        };
        let summary = tray.summary();
        assert!(summary.contains("WiVRn: running"));
        assert!(summary.contains("Meta Quest 3"));
        assert!(summary.contains("VRChat: running"));
        assert!(summary.contains("Audio: VR"));
    }
}
