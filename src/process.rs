use crate::config::{AutostartRule, Config, TriggerType};
use std::collections::{HashMap, HashSet};
use std::process::{Child, Command as StdCommand};
use std::time::{Duration, Instant};
use sysinfo::{ProcessesToUpdate, Signal, System};
use tracing::{error, info, warn};

/// One process as seen by a scan: its pid plus a lowercased haystack built from
/// the name, executable path and full command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub haystack: String,
}

/// Does this process match any of the (already lowercased) patterns?
pub fn haystack_matches(haystack: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| !pattern.is_empty() && haystack.contains(pattern))
}

pub struct ProcessManager {
    sys: System,
    procs: Vec<ProcInfo>,
    self_pid: u32,
    pub active_children: HashMap<String, Child>,
    pub grace_timers: HashMap<String, Instant>,
    /// When each rule was last launched, so a slow starter is not launched
    /// again on every poll.
    last_spawn: HashMap<String, Instant>,
    /// Rules whose trigger has been seen active at least once since startup.
    /// Only these may be stopped: an app that was already running before we
    /// ever saw its trigger is not ours to kill.
    armed: HashSet<String>,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self {
            sys: System::new(),
            procs: Vec::new(),
            self_pid: std::process::id(),
            active_children: HashMap::new(),
            grace_timers: HashMap::new(),
            last_spawn: HashMap::new(),
            armed: HashSet::new(),
        }
    }
}

impl ProcessManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refresh the process table into `self.procs`.
    pub fn refresh(&mut self) {
        self.sys.refresh_processes(ProcessesToUpdate::All, true);

        let self_pid = self.self_pid;
        self.procs = self
            .sys
            .processes()
            .iter()
            // Threads carry their process' command line, so without this a
            // single Electron app looks like a hundred matches — and would be
            // signalled once per thread.
            .filter(|(_, process)| process.thread_kind().is_none())
            .filter(|(pid, _)| pid.as_u32() != self_pid)
            .map(|(pid, process)| {
                let name = process.name().to_string_lossy().to_lowercase();
                let mut haystack = name.clone();
                if let Some(exe) = process.exe() {
                    haystack.push(' ');
                    haystack.push_str(&exe.to_string_lossy().to_lowercase());
                }
                for arg in process.cmd() {
                    haystack.push(' ');
                    haystack.push_str(&arg.to_string_lossy().to_lowercase());
                }
                ProcInfo {
                    pid: pid.as_u32(),
                    name,
                    haystack,
                }
            })
            .collect();
    }

    fn pids_matching(&self, patterns: &[String]) -> Vec<u32> {
        self.procs
            .iter()
            .filter(|p| haystack_matches(&p.haystack, patterns))
            .map(|p| p.pid)
            .collect()
    }

    fn any_matching(&self, patterns: &[String]) -> bool {
        self.procs
            .iter()
            .any(|p| haystack_matches(&p.haystack, patterns))
    }

    /// Returns true if WiVRn is currently running
    pub fn is_wivrn_running(&self) -> bool {
        self.any_matching(&[
            "wivrn-server".to_string(),
            "wivrn-dashboard".to_string(),
            "io.github.wivrn.wivrn".to_string(),
        ])
    }

    /// Returns true if VRChat is currently running
    pub fn is_vrchat_running(&self, pattern: &str) -> bool {
        let pattern = pattern.trim().to_lowercase();
        let patterns = if pattern.is_empty() {
            vec!["vrchat.exe".to_string()]
        } else {
            vec![pattern]
        };
        self.any_matching(&patterns)
    }

    /// Check if a rule's process is running on the system
    pub fn is_rule_running(&mut self, rule: &AutostartRule) -> bool {
        // First check if child handle is still alive
        if let Some(child) = self.active_children.get_mut(&rule.id) {
            if let Ok(Some(_status)) = child.try_wait() {
                self.active_children.remove(&rule.id);
            } else {
                return true;
            }
        }
        self.any_matching(&rule.effective_patterns())
    }

    /// Launch a rule process
    pub fn spawn_rule(&mut self, rule: &AutostartRule) -> bool {
        info!("Spawning autostart app: '{}' ({})", rule.name, rule.exec_cmd);
        let mut cmd = StdCommand::new("sh");
        cmd.args(["-c", &rule.exec_cmd]);

        self.last_spawn.insert(rule.id.clone(), Instant::now());

        match cmd.spawn() {
            Ok(child) => {
                info!(
                    "Successfully spawned process for '{}' with PID {:?}",
                    rule.name,
                    child.id()
                );
                self.active_children.insert(rule.id.clone(), child);
                self.grace_timers.remove(&rule.id);
                true
            }
            Err(e) => {
                error!("Failed to spawn rule process for '{}': {}", rule.name, e);
                false
            }
        }
    }

    /// Ask a set of processes to quit, then force them if they refuse.
    ///
    /// SIGTERM first matters: VRCX keeps a SQLite database and VRCOSC writes
    /// its config on exit, and SIGKILL gives neither a chance to close cleanly.
    fn terminate_pids(&mut self, pids: &[u32], grace: Duration) {
        if pids.is_empty() {
            return;
        }
        for pid in pids {
            if let Some(process) = self.sys.process(sysinfo::Pid::from_u32(*pid)) {
                process.kill_with(Signal::Term);
            }
        }

        let deadline = Instant::now() + grace;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            self.sys
                .refresh_processes(ProcessesToUpdate::All, true);
            let alive: Vec<u32> = pids
                .iter()
                .copied()
                .filter(|pid| self.sys.process(sysinfo::Pid::from_u32(*pid)).is_some())
                .collect();
            if alive.is_empty() {
                return;
            }
            if Instant::now() >= deadline {
                warn!("Force killing {} process(es) that ignored SIGTERM", alive.len());
                for pid in alive {
                    if let Some(process) = self.sys.process(sysinfo::Pid::from_u32(pid)) {
                        process.kill();
                    }
                }
                return;
            }
        }
    }

    /// Terminate a managed rule process
    pub fn stop_rule(&mut self, rule: &AutostartRule, grace: Duration) {
        info!("Stopping process for rule: '{}'", rule.name);
        self.grace_timers.remove(&rule.id);
        self.last_spawn.remove(&rule.id);
        self.armed.remove(&rule.id);

        if let Some(mut child) = self.active_children.remove(&rule.id) {
            let _ = child.try_wait();
        }

        let pids = self.pids_matching(&rule.effective_patterns());
        self.terminate_pids(&pids, grace);
    }

    /// Process tick updating autostart rules and grace period timers
    pub fn update_rules(&mut self, config: &Config) {
        self.refresh();

        let wivrn_running = self.is_wivrn_running();
        let vrchat_running = self.is_vrchat_running(&config.vrchat_process_pattern);

        let now = Instant::now();
        let debounce = Duration::from_secs(config.spawn_debounce_secs.max(1));
        let stop_grace = Duration::from_secs(config.stop_grace_secs.max(1));

        for rule in &config.autostart_rules {
            if !rule.enabled {
                continue;
            }

            let trigger_active = match rule.trigger {
                TriggerType::VRChat => vrchat_running,
                TriggerType::WiVRn => wivrn_running,
                TriggerType::Always => true,
            };

            let rule_running = self.is_rule_running(rule);

            if trigger_active {
                self.armed.insert(rule.id.clone());

                if self.grace_timers.remove(&rule.id).is_some() {
                    info!(
                        "Trigger app for '{}' became active again. Cancelled grace timer.",
                        rule.name
                    );
                }

                if !rule_running {
                    let recently_spawned = self
                        .last_spawn
                        .get(&rule.id)
                        .map(|at| now.duration_since(*at) < debounce)
                        .unwrap_or(false);
                    if recently_spawned {
                        // Still starting up; do not pile on another instance.
                        continue;
                    }
                    self.spawn_rule(rule);
                }
            } else if rule_running {
                if rule.keeps_running() {
                    continue;
                }
                if !self.armed.contains(&rule.id) {
                    // It was already running before we ever saw its trigger,
                    // so it was not started by us and is not ours to stop.
                    continue;
                }

                if let Some(&expires_at) = self.grace_timers.get(&rule.id) {
                    if now >= expires_at {
                        info!(
                            "Grace period ({}s) expired for '{}'. Terminating app.",
                            rule.grace_period_secs, rule.name
                        );
                        self.stop_rule(rule, stop_grace);
                    }
                } else {
                    let expires_at = now + Duration::from_secs(rule.grace_period_secs as u64);
                    info!(
                        "Trigger app stopped for '{}'. Starting grace period of {}s.",
                        rule.name, rule.grace_period_secs
                    );
                    self.grace_timers.insert(rule.id.clone(), expires_at);
                }
            } else {
                self.grace_timers.remove(&rule.id);
            }
        }
    }

    /// Nuke all VR software and restart WiVRn
    pub fn nuke_vr_and_restart(&mut self, config: &Config) {
        info!("--- NUKING ALL VR PROCESSES ---");
        self.refresh();

        let mut patterns: Vec<String> = vec![
            "wivrn-server".to_string(),
            "wivrn-dashboard".to_string(),
            "slimevr".to_string(),
            "wayvr".to_string(),
            "vrchat.exe".to_string(),
        ];
        // Whatever the user actually configured, rather than a stale hard-coded
        // list that drifts away from their rules.
        for rule in &config.autostart_rules {
            patterns.extend(rule.effective_patterns());
        }
        patterns.sort();
        patterns.dedup();

        let pids = self.pids_matching(&patterns);
        info!("Nuke terminating {} process(es)", pids.len());
        self.terminate_pids(&pids, Duration::from_secs(config.stop_grace_secs.max(1)));

        self.active_children.clear();
        self.grace_timers.clear();
        self.last_spawn.clear();
        self.armed.clear();

        info!("Restarting WiVRn using command: '{}'", config.wivrn_command);
        let _ = StdCommand::new("sh")
            .args(["-c", &config.wivrn_command])
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_finds_a_command_line_substring() {
        let haystack = "sh /run/media/system/data/projects/vrcx-extras/start.sh";
        assert!(haystack_matches(haystack, &["vrcx-extras".to_string()]));
        assert!(haystack_matches(haystack, &["start.sh".to_string()]));
        assert!(!haystack_matches(haystack, &["slimevr".to_string()]));
    }

    #[test]
    fn empty_patterns_never_match() {
        // An empty pattern is a substring of everything, which would make a
        // misconfigured rule match — and kill — every process on the system.
        assert!(!haystack_matches("anything at all", &[String::new()]));
        assert!(!haystack_matches("anything at all", &[]));
    }

    #[test]
    fn a_rules_derived_patterns_match_its_own_process() {
        let rule = AutostartRule {
            name: "VRCX-Extras Companion".to_string(),
            exec_cmd: "/run/media/system/Data/Projects/vrcx-extras/start.sh".to_string(),
            ..Default::default()
        };
        let haystack = "start.sh /bin/bash /run/media/system/data/projects/vrcx-extras/start.sh";
        assert!(
            haystack_matches(haystack, &rule.effective_patterns()),
            "the rule must recognise its own process, or it is respawned forever"
        );
    }

    #[test]
    fn an_appimage_rule_matches_its_running_process() {
        let rule = AutostartRule {
            name: "VRCX-0".to_string(),
            exec_cmd: "/home/blu/AppImages/vrcx0.appimage --appimage-extract-and-run".to_string(),
            ..Default::default()
        };
        let haystack = "vrcx-0 /var/home/blu/appimages/vrcx0.appimage --autostart";
        assert!(haystack_matches(haystack, &rule.effective_patterns()));
    }
}
