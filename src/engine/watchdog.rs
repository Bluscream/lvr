use std::time::{Duration, Instant};

use crate::config::Config;
use crate::procs;
use crate::wivrn::WivrnState;

use super::Engine;

pub(super) const WIVRN_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const WIVRN_STARTUP_TIMEOUT: Duration = Duration::from_secs(25);

/// WiVRn watchdog bookkeeping.
#[derive(Debug, Default)]
pub(super) struct WivrnWatch {
    /// The user stopped WiVRn on purpose; do not fight them.
    pub(super) suppressed: bool,
    pub(super) missing_since: Option<Instant>,
    pub(super) last_attempt: Option<Instant>,
    pub(super) failures: u32,
}

impl Engine {
    pub(super) async fn apply_wivrn_watchdog(&mut self, config: &Config, wivrn: &WivrnState) {
        if wivrn.running {
            if self.watch.missing_since.is_some() {
                self.shared.info("WiVRn server is back");
            }
            self.watch.missing_since = None;
            self.watch.failures = 0;
            return;
        }

        if !config.wivrn.watchdog || self.watch.suppressed {
            return;
        }

        let now = Instant::now();
        let missing_since = *self.watch.missing_since.get_or_insert(now);
        if now.duration_since(missing_since) < Duration::from_secs(config.wivrn.restart_delay_secs)
        {
            return;
        }
        if let Some(last) = self.watch.last_attempt
            && now.duration_since(last)
                < Duration::from_secs(config.wivrn.restart_delay_secs.max(5))
        {
            return;
        }
        if config.wivrn.max_consecutive_failures > 0
            && self.watch.failures >= config.wivrn.max_consecutive_failures
        {
            if self.watch.failures == config.wivrn.max_consecutive_failures {
                self.watch.failures += 1;
                self.shared.error(format!(
                    "WiVRn failed to start {} times in a row — watchdog paused. \
                     Fix the start command in Settings, then use Start WiVRn.",
                    config.wivrn.max_consecutive_failures
                ));
                self.watch.suppressed = true;
            }
            return;
        }

        self.watch.last_attempt = Some(now);
        self.watch.failures += 1;
        self.shared.warn("WiVRn is not running — restarting it");
        self.spawn_wivrn(config).await;
    }

    pub(super) async fn spawn_wivrn(&mut self, config: &Config) {
        let command = config.wivrn.start_command.trim();
        if command.is_empty() {
            self.shared.error("No WiVRn start command configured");
            return;
        }
        match procs::spawn_command_line(command) {
            Ok(child) => {
                self.children.insert("__wivrn__", child);
                if self.wivrn.wait_until_up(WIVRN_STARTUP_TIMEOUT).await {
                    self.shared.info("WiVRn server is up");
                    self.watch.failures = 0;
                    self.watch.missing_since = None;
                } else {
                    self.shared
                        .warn("WiVRn did not appear on D-Bus within 25s".to_string());
                }
            }
            Err(err) => self
                .shared
                .error(format!("Launching WiVRn failed: {err:#}")),
        }
    }

    pub(super) async fn start_wivrn(&mut self) {
        let config = self.shared.config_snapshot();
        self.watch.suppressed = false;
        self.watch.failures = 0;
        self.watch.missing_since = None;
        if self.wivrn.is_running().await {
            self.shared.info("WiVRn is already running");
            return;
        }
        self.shared.info("Starting WiVRn");
        self.spawn_wivrn(&config).await;
    }

    /// Stop the server. `by_user` also pauses the watchdog.
    pub(super) async fn stop_wivrn(&mut self, by_user: bool) {
        let config = self.shared.config_snapshot();
        if by_user {
            self.watch.suppressed = true;
        }
        match self.wivrn.quit().await {
            Ok(true) => {
                if !self.wivrn.wait_until_gone(WIVRN_SHUTDOWN_TIMEOUT).await {
                    self.force_kill_wivrn(&config).await;
                } else {
                    self.shared.info("WiVRn stopped");
                }
            }
            Ok(false) => self.shared.info("WiVRn was not running"),
            Err(err) => {
                self.shared
                    .warn(format!("WiVRn Quit() failed ({err:#}) — killing it"));
                self.force_kill_wivrn(&config).await;
            }
        }
        self.children.forget("__wivrn__");
        self.watch.missing_since = None;
    }

    pub(super) async fn force_kill_wivrn(&mut self, config: &Config) {
        let id = config.wivrn.flatpak_id.trim();
        if !id.is_empty()
            && procs::run_command_line(&format!("flatpak kill {id}"))
                .await
                .is_ok()
        {
            self.shared.info(format!("Killed flatpak {id}"));
        }
        let snapshot = self.scanner.scan();
        let pids = snapshot.matching(&["wivrn-server".into(), "wivrn-dashboard".into()], &[]);
        if !pids.is_empty() {
            procs::stop_pids(&pids, Duration::from_secs(5)).await;
            self.shared
                .info(format!("Killed {} leftover WiVRn process(es)", pids.len()));
        }
    }

    pub(super) async fn restart_wivrn(&mut self) {
        self.shared.info("Restarting WiVRn…");
        self.stop_wivrn(false).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let config = self.shared.config_snapshot();
        self.watch.suppressed = false;
        self.watch.failures = 0;
        self.spawn_wivrn(&config).await;
    }
}
