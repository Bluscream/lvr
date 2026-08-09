use crate::service::AppState;
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Category, MenuItem, ToolTip, Tray};
use std::sync::{Arc, Mutex};

pub struct LinuxVrTray {
    pub state: AppState,
    pub nuke_trigger: Arc<Mutex<bool>>,
    pub show_gui_trigger: Arc<Mutex<bool>>,
}

impl LinuxVrTray {
    pub fn new(
        state: AppState,
        nuke_trigger: Arc<Mutex<bool>>,
        show_gui_trigger: Arc<Mutex<bool>>,
    ) -> Self {
        Self {
            state,
            nuke_trigger,
            show_gui_trigger,
        }
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
        "headset".to_string()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn tool_tip(&self) -> ToolTip {
        let wivrn = *self.state.wivrn_running.lock().unwrap();
        let audio = *self.state.wivrn_audio_connected.lock().unwrap();
        let status = format!(
            "LinuxVR Companion\nWiVRn: {}\nAudio Connected: {}",
            if wivrn { "Running" } else { "Stopped" },
            if audio { "Yes" } else { "No" }
        );
        ToolTip {
            title: "LinuxVR (lvr)".to_string(),
            description: status,
            icon_name: "headset".to_string(),
            icon_pixmap: Vec::new(),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let wivrn_auto = self
            .state
            .config
            .lock()
            .unwrap()
            .auto_restart_wivrn;
        let audio_auto = self
            .state
            .config
            .lock()
            .unwrap()
            .auto_switch_audio;

        vec![
            StandardItem {
                label: "Open LinuxVR GUI".to_string(),
                activate: Box::new(|this: &mut Self| {
                    *this.show_gui_trigger.lock().unwrap() = true;
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "⚡ NUKE VR & Restart WiVRn".to_string(),
                activate: Box::new(|this: &mut Self| {
                    *this.nuke_trigger.lock().unwrap() = true;
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "WiVRn Auto-Restart".to_string(),
                checked: wivrn_auto,
                activate: Box::new(|this: &mut Self| {
                    let mut cfg = this.state.config.lock().unwrap();
                    cfg.auto_restart_wivrn = !cfg.auto_restart_wivrn;
                    let _ = cfg.save();
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Audio Auto-Switch".to_string(),
                checked: audio_auto,
                activate: Box::new(|this: &mut Self| {
                    let mut cfg = this.state.config.lock().unwrap();
                    cfg.auto_switch_audio = !cfg.auto_switch_audio;
                    let _ = cfg.save();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit LinuxVR".to_string(),
                activate: Box::new(|_| {
                    std::process::exit(0);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
