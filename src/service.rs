use crate::audio::AudioSwitcher;
use crate::config::Config;
use crate::process::ProcessManager;
use chrono::Local;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Mutex<Config>>,
    pub log_messages: Arc<Mutex<Vec<String>>>,
    pub wivrn_running: Arc<Mutex<bool>>,
    pub vrchat_running: Arc<Mutex<bool>>,
    pub wivrn_audio_connected: Arc<Mutex<bool>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            log_messages: Arc::new(Mutex::new(Vec::new())),
            wivrn_running: Arc::new(Mutex::new(false)),
            vrchat_running: Arc::new(Mutex::new(false)),
            wivrn_audio_connected: Arc::new(Mutex::new(false)),
        }
    }

    pub fn add_log(&self, msg: impl Into<String>) {
        let timestamp = Local::now().format("%H:%M:%S").to_string();
        let log_line = format!("[{}] {}", timestamp, msg.into());
        info!("{}", log_line);
        let mut logs = self.log_messages.lock().unwrap();
        logs.push(log_line);
        if logs.len() > 200 {
            logs.remove(0);
        }
    }
}

pub struct ServiceWorker {
    pub state: AppState,
    pub process_manager: ProcessManager,
    pub audio_switcher: AudioSwitcher,
    pub last_wivrn_spawn: Option<std::time::Instant>,
}

impl ServiceWorker {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            process_manager: ProcessManager::new(),
            audio_switcher: AudioSwitcher::new(),
            last_wivrn_spawn: None,
        }
    }

    pub async fn start(mut self) {
        self.state.add_log("Starting LinuxVR Watchdog Service...");

        loop {
            let config_clone = {
                let cfg = self.state.config.lock().unwrap();
                cfg.clone()
            };

            // 1. Process checks and autostart / grace period updates
            self.process_manager.update_rules(&config_clone);

            let wivrn_running = self.process_manager.is_wivrn_running();
            let vrchat_running = self.process_manager.is_vrchat_running(&config_clone.vrchat_process_pattern);

            *self.state.wivrn_running.lock().unwrap() = wivrn_running;
            *self.state.vrchat_running.lock().unwrap() = vrchat_running;

            // WiVRn Watchdog (Auto-restart WiVRn if crashed/stopped, with 10s cooldown)
            let cooldown_passed = self
                .last_wivrn_spawn
                .map(|t| t.elapsed() > Duration::from_secs(10))
                .unwrap_or(true);

            if config_clone.auto_restart_wivrn && !wivrn_running && cooldown_passed {
                self.state.add_log("WiVRn is not running! Auto-restarting WiVRn...");
                self.last_wivrn_spawn = Some(std::time::Instant::now());
                let cmd = config_clone.wivrn_command.clone();
                let _ = std::process::Command::new("sh")
                    .args(["-c", &cmd])
                    .spawn();
            }

            // 2. Audio switcher check
            if config_clone.auto_switch_audio {
                let prev_state = self.audio_switcher.current_state.clone();
                self.audio_switcher.update();

                let connected = self.audio_switcher.current_state == crate::audio::AudioState::ConnectedToWiVRn;
                *self.state.wivrn_audio_connected.lock().unwrap() = connected;

                if prev_state != self.audio_switcher.current_state {
                    match self.audio_switcher.current_state {
                        crate::audio::AudioState::ConnectedToWiVRn => {
                            self.state.add_log("Audio switched to WiVRn headset!");
                        }
                        crate::audio::AudioState::Disconnected => {
                            self.state.add_log("Restored previous system default audio devices.");
                        }
                    }
                }
            }

            let poll = config_clone.poll_interval_secs.max(1);
            tokio::time::sleep(Duration::from_secs(poll)).await;
        }
    }

    pub fn nuke_vr(&mut self) {
        let config_clone = self.state.config.lock().unwrap().clone();
        self.state.add_log("Initiating Nuke VR & WiVRn Restart...");
        self.process_manager.nuke_vr_and_restart(&config_clone);
        self.audio_switcher.restore_previous_audio();
        *self.state.wivrn_audio_connected.lock().unwrap() = false;
        self.state.add_log("Nuke complete. WiVRn restart initiated.");
    }
}
