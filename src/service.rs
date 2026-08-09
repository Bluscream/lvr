use crate::audio::AudioSwitcher;
use crate::config::Config;
use crate::process::ProcessManager;
use chrono::Local;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tracing::info;

/// Things the GUI and the tray ask the supervisor to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Kill everything VR and restart WiVRn.
    Nuke,
    /// Run a supervisor pass now instead of waiting for the next tick.
    Poke,
    /// Shut the supervisor down.
    Quit,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Mutex<Config>>,
    pub log_messages: Arc<Mutex<Vec<String>>>,
    pub wivrn_running: Arc<Mutex<bool>>,
    pub vrchat_running: Arc<Mutex<bool>>,
    pub wivrn_audio_connected: Arc<Mutex<bool>>,
    /// Shared so that the nuke action and the GUI's "restore" button operate on
    /// the switcher that actually holds the saved devices. They used to build a
    /// fresh AudioSwitcher, whose saved devices were always None, so restoring
    /// audio silently did nothing.
    pub audio_switcher: Arc<Mutex<AudioSwitcher>>,
    commands: Sender<Command>,
    show_window: Arc<AtomicBool>,
    quitting: Arc<AtomicBool>,
    /// egui repaint handle, installed once the GUI is up, so background events
    /// reach the screen even when the user is not moving the mouse.
    repaint: Arc<Mutex<Option<eframe::egui::Context>>>,
}

/// A poisoned lock means another thread panicked; that is no reason to bring
/// the whole app down with it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AppState {
    pub fn new(config: Config) -> (Self, Receiver<Command>) {
        let (tx, rx) = channel();
        let state = Self {
            config: Arc::new(Mutex::new(config)),
            log_messages: Arc::new(Mutex::new(Vec::new())),
            wivrn_running: Arc::new(Mutex::new(false)),
            vrchat_running: Arc::new(Mutex::new(false)),
            wivrn_audio_connected: Arc::new(Mutex::new(false)),
            audio_switcher: Arc::new(Mutex::new(AudioSwitcher::new())),
            commands: tx,
            show_window: Arc::new(AtomicBool::new(false)),
            quitting: Arc::new(AtomicBool::new(false)),
            repaint: Arc::new(Mutex::new(None)),
        };
        (state, rx)
    }

    pub fn add_log(&self, msg: impl Into<String>) {
        let timestamp = Local::now().format("%H:%M:%S").to_string();
        let log_line = format!("[{}] {}", timestamp, msg.into());
        info!("{}", log_line);
        let mut logs = lock(&self.log_messages);
        logs.push(log_line);
        if logs.len() > 200 {
            logs.remove(0);
        }
        drop(logs);
        self.request_repaint();
    }

    pub fn send(&self, command: Command) {
        // The receiver only goes away during shutdown, where dropping the
        // command is exactly what we want.
        let _ = self.commands.send(command);
    }

    /// Mutate the config under the lock, then save it *after* releasing the
    /// lock: `Config::save` writes to disk, and holding the mutex across that
    /// stalled the supervisor thread on every checkbox click.
    pub fn update_config<R>(&self, edit: impl FnOnce(&mut Config) -> R) -> R {
        let (result, snapshot) = {
            let mut config = lock(&self.config);
            let result = edit(&mut config);
            (result, config.clone())
        };
        if let Err(e) = snapshot.save() {
            self.add_log(format!("Failed to save config: {}", e));
        }
        result
    }

    pub fn config_snapshot(&self) -> Config {
        lock(&self.config).clone()
    }

    pub fn set_repaint_ctx(&self, ctx: eframe::egui::Context) {
        *lock(&self.repaint) = Some(ctx);
    }

    pub fn request_repaint(&self) {
        if let Some(ctx) = lock(&self.repaint).as_ref() {
            ctx.request_repaint();
        }
    }

    pub fn request_show_window(&self) {
        self.show_window.store(true, Ordering::SeqCst);
        self.request_repaint();
    }

    /// Consumes the request, so the window is raised once per ask.
    pub fn take_show_window(&self) -> bool {
        self.show_window.swap(false, Ordering::SeqCst)
    }

    pub fn set_quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        self.request_repaint();
    }

    pub fn is_quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }
}

pub struct ServiceWorker {
    pub state: AppState,
    pub process_manager: ProcessManager,
    pub last_wivrn_spawn: Option<std::time::Instant>,
}

impl ServiceWorker {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            process_manager: ProcessManager::new(),
            last_wivrn_spawn: None,
        }
    }

    /// The supervisor loop.
    ///
    /// Deliberately synchronous and on its own OS thread: every step here is
    /// blocking work (scanning `/proc`, running `pactl`, waiting for processes
    /// to die). It used to run inside a tokio task, where a multi-second
    /// SIGTERM wait would occupy a runtime worker.
    pub fn run(mut self, commands: Receiver<Command>) {
        self.state.add_log("Starting LinuxVR Watchdog Service...");

        loop {
            self.tick();

            let poll = Duration::from_secs(self.state.config_snapshot().poll_interval_secs.max(1));
            match commands.recv_timeout(poll) {
                Ok(Command::Nuke) => self.nuke_vr(),
                Ok(Command::Poke) => {}
                Ok(Command::Quit) => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }

            if self.state.is_quitting() {
                break;
            }
        }

        self.state.add_log("Watchdog service stopped.");
    }

    fn tick(&mut self) {
        let config = self.state.config_snapshot();

        // 1. Process checks and autostart / grace period updates
        self.process_manager.update_rules(&config);

        let wivrn_running = self.process_manager.is_wivrn_running();
        let vrchat_running = self
            .process_manager
            .is_vrchat_running(&config.vrchat_process_pattern);

        *lock(&self.state.wivrn_running) = wivrn_running;
        *lock(&self.state.vrchat_running) = vrchat_running;

        // 2. WiVRn watchdog (auto-restart if crashed/stopped, with a cooldown)
        let cooldown_passed = self
            .last_wivrn_spawn
            .map(|t| t.elapsed() > Duration::from_secs(10))
            .unwrap_or(true);

        if config.auto_restart_wivrn && !wivrn_running && cooldown_passed {
            self.state
                .add_log("WiVRn is not running! Auto-restarting WiVRn...");
            self.last_wivrn_spawn = Some(std::time::Instant::now());
            let _ = std::process::Command::new("sh")
                .args(["-c", &config.wivrn_command])
                .spawn();
        }

        // 3. Audio switcher check
        if config.auto_switch_audio {
            let (prev_state, new_state) = {
                let mut switcher = lock(&self.state.audio_switcher);
                let prev_state = switcher.current_state.clone();
                switcher.update();
                (prev_state, switcher.current_state.clone())
            };

            let connected = new_state == crate::audio::AudioState::ConnectedToWiVRn;
            *lock(&self.state.wivrn_audio_connected) = connected;

            if prev_state != new_state {
                match new_state {
                    crate::audio::AudioState::ConnectedToWiVRn => {
                        self.state.add_log("Audio switched to WiVRn headset!");
                    }
                    crate::audio::AudioState::Disconnected => {
                        self.state
                            .add_log("Restored previous system default audio devices.");
                    }
                }
            }
        }

        self.state.request_repaint();
    }

    pub fn nuke_vr(&mut self) {
        let config = self.state.config_snapshot();
        self.state.add_log("Initiating Nuke VR & WiVRn Restart...");
        self.process_manager.nuke_vr_and_restart(&config);
        lock(&self.state.audio_switcher).restore_previous_audio();
        *lock(&self.state.wivrn_audio_connected) = false;
        self.state
            .add_log("Nuke complete. WiVRn restart initiated.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_reach_the_supervisor() {
        let (state, rx) = AppState::new(Config::default());
        state.send(Command::Nuke);
        assert_eq!(rx.try_recv().ok(), Some(Command::Nuke));
    }

    #[test]
    fn the_show_window_request_is_consumed_once() {
        let (state, _rx) = AppState::new(Config::default());
        assert!(!state.take_show_window());
        state.request_show_window();
        assert!(state.take_show_window());
        assert!(!state.take_show_window(), "a raise must not repeat forever");
    }

    #[test]
    fn quitting_is_visible_to_every_clone() {
        let (state, _rx) = AppState::new(Config::default());
        let other = state.clone();
        assert!(!other.is_quitting());
        state.set_quitting();
        assert!(other.is_quitting());
    }

    #[test]
    fn the_log_is_capped() {
        let (state, _rx) = AppState::new(Config::default());
        for i in 0..250 {
            state.add_log(format!("line {}", i));
        }
        let logs = lock(&state.log_messages);
        assert_eq!(logs.len(), 200);
        assert!(logs.last().unwrap().contains("line 249"));
    }
}
