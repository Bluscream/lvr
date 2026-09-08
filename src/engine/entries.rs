use std::time::{Duration, Instant};

use crate::config::{AutostartEntry, Config, Trigger};
use crate::procs::{self, ProcSnapshot};
use crate::state::EntryStatus;
use crate::wivrn::WivrnState;

use super::Engine;

/// What the planner decided to do with one entry this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Start,
    Stop,
}

/// Per-entry bookkeeping that is not persisted.
#[derive(Debug, Clone, Default)]
pub struct EntryRuntime {
    pub running: bool,
    pub pids: Vec<u32>,
    /// Stopped by hand: do not auto-start again until the trigger cycles.
    pub suppressed: bool,
    /// Already launched during the current trigger activation.
    pub launched_this_cycle: bool,
    /// A trigger activation is still awaiting its shutdown.
    pub armed: bool,
    pub stop_at: Option<Instant>,
    pub start_at: Option<Instant>,
    pub last_launch: Option<Instant>,
    pub last_error: Option<String>,
}

/// Inputs the planner needs beyond the entry's own configuration.
#[derive(Debug, Clone, Copy)]
pub struct PlanInput {
    pub trigger_active: bool,
    pub running: bool,
    pub now: Instant,
    pub relaunch_debounce: Duration,
}

impl EntryRuntime {
    /// Pure decision function: the heart of the autostart behaviour.
    pub fn plan(&mut self, entry: &AutostartEntry, input: PlanInput) -> Action {
        self.running = input.running;

        if !input.trigger_active {
            self.suppressed = false;
            self.launched_this_cycle = false;
        }

        if !entry.enabled {
            self.start_at = None;
            self.stop_at = None;
            return Action::None;
        }

        if input.trigger_active {
            self.stop_at = None;
            self.armed = true;

            if input.running {
                self.start_at = None;
                self.launched_this_cycle = true;
                return Action::None;
            }
            if self.suppressed {
                self.start_at = None;
                return Action::None;
            }
            if self.launched_this_cycle && !entry.restart_on_exit {
                self.start_at = None;
                return Action::None;
            }
            if let Some(last) = self.last_launch
                && input.now.duration_since(last) < input.relaunch_debounce
            {
                return Action::None;
            }

            match self.start_at {
                Some(at) if input.now < at => Action::None,
                Some(_) => {
                    self.start_at = None;
                    self.mark_launched(input.now);
                    Action::Start
                }
                None => {
                    if entry.start_delay_secs == 0 {
                        self.mark_launched(input.now);
                        Action::Start
                    } else {
                        self.start_at =
                            Some(input.now + Duration::from_secs(entry.start_delay_secs));
                        Action::None
                    }
                }
            }
        } else {
            self.start_at = None;

            if !input.running {
                self.stop_at = None;
                self.armed = false;
                return Action::None;
            }
            if !self.armed {
                self.stop_at = None;
                return Action::None;
            }
            if entry.keeps_running() {
                self.stop_at = None;
                return Action::None;
            }

            match self.stop_at {
                Some(at) if input.now < at => Action::None,
                Some(_) => {
                    self.stop_at = None;
                    self.armed = false;
                    Action::Stop
                }
                None => {
                    if entry.grace_secs == 0 {
                        self.armed = false;
                        Action::Stop
                    } else {
                        self.stop_at =
                            Some(input.now + Duration::from_secs(entry.grace_secs as u64));
                        Action::None
                    }
                }
            }
        }
    }

    fn mark_launched(&mut self, now: Instant) {
        self.last_launch = Some(now);
        self.launched_this_cycle = true;
    }

    pub fn seconds_until(target: Option<Instant>, now: Instant) -> Option<i64> {
        target.map(|at| at.saturating_duration_since(now).as_secs() as i64)
    }
}

impl Engine {
    pub fn trigger_active(
        trigger: &Trigger,
        snapshot: &ProcSnapshot,
        wivrn: &WivrnState,
        vrchat_running: bool,
    ) -> bool {
        match trigger {
            Trigger::Vrchat => vrchat_running,
            Trigger::WivrnRunning => wivrn.running,
            Trigger::HeadsetConnected => wivrn.headset_connected,
            Trigger::Process(pattern) => {
                let pattern = pattern.trim().to_lowercase();
                !pattern.is_empty() && snapshot.any_matching(&[pattern], &[])
            }
            Trigger::Manual => false,
        }
    }

    pub(super) async fn apply_entries(
        &mut self,
        config: &Config,
        snapshot: &ProcSnapshot,
        wivrn: &WivrnState,
        vrchat_running: bool,
    ) -> Vec<EntryStatus> {
        let now = Instant::now();
        let debounce = Duration::from_secs(config.general.relaunch_debounce_secs);
        let known: Vec<String> = config.autostart.iter().map(|e| e.id.clone()).collect();
        self.runtimes.retain(|id, _| known.contains(id));

        let mut statuses = Vec::with_capacity(config.autostart.len());
        for entry in &config.autostart {
            let patterns = entry.effective_patterns();
            let mut pids = snapshot.matching(&patterns, &[]);
            if let Some(pid) = self.children.live_pid(&entry.id)
                && !pids.contains(&pid)
            {
                pids.push(pid);
            }
            snapshot.expand_children(&mut pids, &[]);
            let running = !pids.is_empty();
            let trigger_active =
                Self::trigger_active(&entry.trigger, snapshot, wivrn, vrchat_running);

            let runtime = self.runtimes.entry(entry.id.clone()).or_default();
            runtime.pids = pids.clone();
            let action = runtime.plan(
                entry,
                PlanInput {
                    trigger_active,
                    running,
                    now,
                    relaunch_debounce: debounce,
                },
            );

            match action {
                Action::Start => {
                    self.launch_entry(entry, config).await;
                }
                Action::Stop => {
                    self.shared.info(format!(
                        "{} — grace period over, stopping",
                        entry.name_or_id()
                    ));
                    self.stop_entry(entry, config).await;
                }
                Action::None => {}
            }

            let runtime = self.runtimes.entry(entry.id.clone()).or_default();
            statuses.push(EntryStatus {
                id: entry.id.clone(),
                name: entry.name_or_id().to_string(),
                running,
                pids,
                trigger_active,
                stop_in_secs: EntryRuntime::seconds_until(runtime.stop_at, now),
                start_in_secs: EntryRuntime::seconds_until(runtime.start_at, now),
                suppressed: runtime.suppressed,
                last_error: runtime.last_error.clone(),
            });
        }
        statuses
    }

    pub(super) async fn launch_entry(&mut self, entry: &AutostartEntry, config: &Config) {
        match procs::launch(entry, &config.general.terminal) {
            Ok(child) => {
                let pid = child.id();
                self.children.insert(&entry.id, child);
                self.shared
                    .info(format!("Started {} (pid {pid})", entry.name_or_id()));
                if let Some(runtime) = self.runtimes.get_mut(&entry.id) {
                    runtime.last_error = None;
                }
            }
            Err(err) => {
                let message = format!("{err:#}");
                self.shared
                    .error(format!("Starting {} failed: {message}", entry.name_or_id()));
                if let Some(runtime) = self.runtimes.get_mut(&entry.id) {
                    runtime.last_error = Some(message);
                }
            }
        }
    }

    pub(super) async fn stop_entry(&mut self, entry: &AutostartEntry, config: &Config) {
        let stop_command = entry.stop_command.trim();
        if !stop_command.is_empty() {
            match procs::run_command_line(stop_command).await {
                Ok(()) => self
                    .shared
                    .info(format!("{}: ran stop command", entry.name_or_id())),
                Err(err) => self.shared.warn(format!(
                    "{}: stop command failed: {err:#}",
                    entry.name_or_id()
                )),
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }

        let snapshot = self.scanner.scan();
        let mut pids = snapshot.matching(&entry.effective_patterns(), &[]);
        if let Some(pid) = self.children.live_pid(&entry.id)
            && !pids.contains(&pid)
        {
            pids.push(pid);
        }
        snapshot.expand_children(&mut pids, &[]);

        if pids.is_empty() {
            self.children.forget(&entry.id);
            return;
        }

        let grace = Duration::from_secs(config.general.stop_grace_secs);
        let forced = procs::stop_pids(&pids, grace).await;
        self.children.forget(&entry.id);
        if forced.is_empty() {
            self.shared.info(format!(
                "Stopped {} ({} process{})",
                entry.name_or_id(),
                pids.len(),
                if pids.len() == 1 { "" } else { "es" }
            ));
        } else {
            self.shared.warn(format!(
                "Stopped {} — force-killed {:?}",
                entry.name_or_id(),
                forced
            ));
        }
        if let Some(runtime) = self.runtimes.get_mut(&entry.id) {
            runtime.running = false;
            runtime.pids.clear();
            runtime.stop_at = None;
        }
    }

    pub(super) async fn start_entry_manual(&mut self, id: &str) {
        let config = self.shared.config_snapshot();
        let Some(entry) = config.entry(id).cloned() else {
            self.shared.warn(format!("Unknown entry `{id}`"));
            return;
        };
        {
            let runtime = self.runtimes.entry(entry.id.clone()).or_default();
            runtime.suppressed = false;
            runtime.last_launch = Some(Instant::now());
            runtime.launched_this_cycle = true;
            runtime.start_at = None;
            runtime.stop_at = None;
        }
        self.launch_entry(&entry, &config).await;
    }

    pub(super) async fn stop_entry_manual(&mut self, id: &str) {
        let config = self.shared.config_snapshot();
        let Some(entry) = config.entry(id).cloned() else {
            self.shared.warn(format!("Unknown entry `{id}`"));
            return;
        };
        self.shared.info(format!("Stopping {}", entry.name_or_id()));
        self.stop_entry(&entry, &config).await;
        let runtime = self.runtimes.entry(entry.id.clone()).or_default();
        runtime.suppressed = true;
        runtime.launched_this_cycle = true;
    }
}
