use crate::config::{AutostartRule, Config, TriggerType};
use std::collections::HashMap;
use std::process::{Child, Command as StdCommand};
use std::time::{Duration, Instant};
use sysinfo::System;
use tracing::{error, info};

pub struct ProcessManager {
    sys: System,
    pub active_children: HashMap<String, Child>,
    pub grace_timers: HashMap<String, Instant>,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self {
            sys: System::new_all(),
            active_children: HashMap::new(),
            grace_timers: HashMap::new(),
        }
    }
}

impl ProcessManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refresh process table
    pub fn refresh(&mut self) {
        self.sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    }

    /// Safely check if a process matches a target binary name
    fn process_matches_target(process: &sysinfo::Process, target: &str) -> bool {
        let target_lower = target.to_lowercase();
        let proc_name = process.name().to_string_lossy().to_lowercase();
        if proc_name == target_lower || proc_name.starts_with(&target_lower) {
            return true;
        }

        if let Some(exe_path) = process.exe() {
            if let Some(file_name) = exe_path.file_name() {
                let name = file_name.to_string_lossy().to_lowercase();
                if name == target_lower || name.starts_with(&target_lower) {
                    return true;
                }
            }
        }
        false
    }

    /// Returns true if WiVRn is currently running
    pub fn is_wivrn_running(&self) -> bool {
        let targets = ["wivrn-server", "wivrn", "io.github.wivrn.wivrn"];
        for process in self.sys.processes().values() {
            for target in targets {
                if Self::process_matches_target(process, target) {
                    return true;
                }
            }
        }
        false
    }

    /// Returns true if VRChat is currently running
    pub fn is_vrchat_running(&self, pattern: &str) -> bool {
        let pat = pattern.to_lowercase();
        for process in self.sys.processes().values() {
            if Self::process_matches_target(process, &pat) || Self::process_matches_target(process, "vrchat.exe") {
                return true;
            }
        }
        false
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

        let rule_name = rule.name.to_lowercase();
        let rule_cmd_bin = rule
            .exec_cmd
            .split_whitespace()
            .last()
            .unwrap_or(&rule.exec_cmd)
            .to_lowercase();

        for process in self.sys.processes().values() {
            if Self::process_matches_target(process, &rule_name)
                || Self::process_matches_target(process, &rule_cmd_bin)
            {
                return true;
            }
        }
        false
    }

    /// Launch a rule process
    pub fn spawn_rule(&mut self, rule: &AutostartRule) -> bool {
        info!("Spawning autostart app: '{}' ({})", rule.name, rule.exec_cmd);
        let mut cmd = StdCommand::new("sh");
        cmd.args(["-c", &rule.exec_cmd]);

        match cmd.spawn() {
            Ok(child) => {
                info!("Successfully spawned process for '{}' with PID {:?}", rule.name, child.id());
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

    /// Terminate a managed rule process
    pub fn stop_rule(&mut self, rule: &AutostartRule) {
        info!("Stopping process for rule: '{}'", rule.name);
        self.grace_timers.remove(&rule.id);

        if let Some(mut child) = self.active_children.remove(&rule.id) {
            let _ = child.kill();
            let _ = child.wait();
        }

        let rule_name = rule.name.to_lowercase();
        for process in self.sys.processes().values() {
            if Self::process_matches_target(process, &rule_name) {
                info!("Killing process ({})", process.name().to_string_lossy());
                process.kill();
            }
        }
    }

    /// Process tick updating autostart rules and grace period timers
    pub fn update_rules(&mut self, config: &Config) {
        self.refresh();

        let wivrn_running = self.is_wivrn_running();
        let vrchat_running = self.is_vrchat_running(&config.vrchat_process_pattern);

        let now = Instant::now();

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
                if self.grace_timers.contains_key(&rule.id) {
                    info!("Trigger app for '{}' became active again. Cancelled grace timer.", rule.name);
                    self.grace_timers.remove(&rule.id);
                }

                if !rule_running {
                    self.spawn_rule(rule);
                }
            } else {
                if rule_running {
                    if rule.grace_period_secs < 0 {
                        continue;
                    }

                    if let Some(&expires_at) = self.grace_timers.get(&rule.id) {
                        if now >= expires_at {
                            info!(
                                "Grace period ({}s) expired for '{}'. Terminating app.",
                                rule.grace_period_secs, rule.name
                            );
                            self.stop_rule(rule);
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
    }

    /// Nuke all VR software and restart WiVRn
    pub fn nuke_vr_and_restart(&mut self, config: &Config) {
        info!("--- NUKING ALL VR PROCESSES ---");
        self.refresh();

        let targets = [
            "wivrn-server",
            "wivrn",
            "slimevr",
            "wayvr",
            "vrchat.exe",
            "vrchat",
            "vrcvideocacher",
            "vrcosc",
            "vrcx0",
            "vrcx-extras",
        ];

        for process in self.sys.processes().values() {
            for target in targets {
                if Self::process_matches_target(process, target) {
                    info!("Nuke killing VR process ({})", process.name().to_string_lossy());
                    process.kill();
                }
            }
        }

        self.active_children.clear();
        self.grace_timers.clear();

        info!("Restarting WiVRn using command: '{}'", config.wivrn_command);
        let _ = StdCommand::new("sh")
            .args(["-c", &config.wivrn_command])
            .spawn();
    }
}
