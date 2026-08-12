//! The supervisor: everything that watches, starts and stops things.
//!
//! It runs on its own tokio runtime thread. The GUI and tray never touch
//! processes directly — they push [`Command`]s and read [`Status`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;

use crate::audio::{self, Kind};
use crate::config::{AutostartEntry, Config, Trigger};
use crate::procs::{self, ChildRegistry, ProcSnapshot, ProcessScanner};
use crate::state::{Command, EntryStatus, Shared, Status};
use crate::steam;
use crate::wivrn::{WivrnClient, WivrnState};

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
    /// The trigger has been active at least once since `lvr` started.
    ///
    /// Until then the grace timer stays disarmed, so starting `lvr` while an
    /// app happens to be running never kills something we did not start.
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
            // Trigger cycled off: re-arm both the manual override and the
            // once-per-activation launch guard.
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
                return Action::None;
            }
            if !self.armed {
                // It was already running before we ever saw its trigger, so it
                // is not ours to stop.
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
                    Action::Stop
                }
                None => {
                    if entry.grace_secs == 0 {
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

    fn seconds_until(target: Option<Instant>, now: Instant) -> Option<i64> {
        target.map(|at| at.saturating_duration_since(now).as_secs() as i64)
    }
}

/// Audio routing bookkeeping.
#[derive(Debug, Default)]
struct AudioRouting {
    on_vr: bool,
    saved_sink: Option<String>,
    saved_source: Option<String>,
    initialized: bool,
}

/// WiVRn watchdog bookkeeping.
#[derive(Debug, Default)]
struct WivrnWatch {
    /// The user stopped WiVRn on purpose; do not fight them.
    suppressed: bool,
    missing_since: Option<Instant>,
    last_attempt: Option<Instant>,
    failures: u32,
}

pub struct Engine {
    shared: Shared,
    rx: UnboundedReceiver<Command>,
    scanner: ProcessScanner,
    children: ChildRegistry,
    wivrn: WivrnClient,
    runtimes: HashMap<String, EntryRuntime>,
    audio: AudioRouting,
    watch: WivrnWatch,
    last_audio_poll: Option<Instant>,
    last_device_poll: Option<Instant>,
    cached_sinks: Vec<crate::state::AudioDevice>,
    cached_sources: Vec<crate::state::AudioDevice>,
    cached_default_sink: String,
    cached_default_source: String,
    steam: SteamState,
}

/// Cached Steam profile state, refreshed on a slow timer.
#[derive(Debug, Default)]
struct SteamState {
    profile: Option<String>,
    compat_tool: String,
    switching: bool,
    last_poll: Option<Instant>,
}

const AUDIO_POLL_INTERVAL: Duration = Duration::from_secs(2);
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(15);
const WIVRN_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const WIVRN_STARTUP_TIMEOUT: Duration = Duration::from_secs(25);
const STEAM_POLL_INTERVAL: Duration = Duration::from_secs(10);

impl Engine {
    pub fn new(shared: Shared, rx: UnboundedReceiver<Command>) -> Self {
        Self {
            shared,
            rx,
            scanner: ProcessScanner::new(),
            children: ChildRegistry::default(),
            wivrn: WivrnClient::new(),
            runtimes: HashMap::new(),
            audio: AudioRouting::default(),
            watch: WivrnWatch::default(),
            last_audio_poll: None,
            last_device_poll: None,
            cached_sinks: Vec::new(),
            cached_sources: Vec::new(),
            cached_default_sink: String::new(),
            cached_default_source: String::new(),
            steam: SteamState::default(),
        }
    }

    pub async fn run(mut self) {
        self.shared.info("Supervisor started");
        loop {
            let interval = Duration::from_millis(self.shared.config().general.poll_interval_ms);
            tokio::select! {
                command = self.rx.recv() => {
                    match command {
                        Some(Command::Quit) => break,
                        Some(command) => self.handle(command).await,
                        None => break,
                    }
                }
                _ = tokio::time::sleep(interval) => {}
            }
            if self.shared.is_quitting() {
                break;
            }
            self.tick().await;
        }
        self.shared.info("Supervisor stopped");
    }

    // ---------------------------------------------------------------- commands

    async fn handle(&mut self, command: Command) {
        match command {
            Command::Poke => {}
            Command::Quit => {}
            Command::StartWivrn => self.start_wivrn().await,
            Command::StopWivrn => self.stop_wivrn(true).await,
            Command::RestartWivrn => self.restart_wivrn().await,
            Command::DisconnectHeadset => match self.wivrn.disconnect().await {
                Ok(true) => self.shared.info("Asked WiVRn to disconnect the headset"),
                Ok(false) => self.shared.warn("WiVRn is not running"),
                Err(err) => self.shared.error(format!("Disconnect failed: {err:#}")),
            },
            Command::StartEntry(id) => self.start_entry_manual(&id).await,
            Command::StopEntry(id) => self.stop_entry_manual(&id).await,
            Command::StopAllVr => self.stop_all_vr().await,
            Command::SetAudioVr(on_vr) => {
                if on_vr {
                    self.route_audio_to_vr(true).await;
                } else {
                    self.route_audio_to_desktop(true).await;
                }
            }
            Command::RefreshAudioDevices => {
                self.last_device_poll = None;
                self.last_audio_poll = None;
            }
            Command::SwitchSteamProfile(name) => self.switch_steam_profile(&name).await,
            Command::SaveConfig => match self.shared.save_config() {
                Ok(()) => self
                    .shared
                    .info(format!("Saved {}", self.shared.config_path().display())),
                Err(err) => self.shared.error(format!("Saving config failed: {err:#}")),
            },
        }
    }

    // ------------------------------------------------------------------- tick

    async fn tick(&mut self) {
        self.children.reap();
        self.shared.sync_log_capacity();
        let config = self.shared.config_snapshot();
        let snapshot = self.scanner.scan();
        let wivrn = self.wivrn.poll().await;

        let vrchat_running = snapshot.any_matching(&config.general.vrchat_match, &[]);

        self.apply_wivrn_watchdog(&config, &wivrn).await;
        self.apply_audio(&config, &wivrn).await;
        let entry_status = self
            .apply_entries(&config, &snapshot, &wivrn, vrchat_running)
            .await;
        self.refresh_audio_cache(&config).await;
        self.refresh_steam_cache(&config);

        let status = Status {
            wivrn_running: wivrn.running,
            headset_connected: wivrn.headset_connected,
            headset_name: wivrn.system_name.clone(),
            session_running: wivrn.session_running,
            vrchat_running,
            watchdog_paused: self.watch.suppressed,
            wivrn_failures: self.watch.failures,
            default_sink: self.cached_default_sink.clone(),
            default_source: self.cached_default_source.clone(),
            audio_on_vr: self.audio.on_vr,
            entries: entry_status,
            steam_profile: self.steam.profile.clone(),
            steam_compat_tool: self.steam.compat_tool.clone(),
            steam_switching: self.steam.switching,
            sinks: self.cached_sinks.clone(),
            sources: self.cached_sources.clone(),
            last_tick: Some(chrono::Local::now()),
        };
        self.shared.set_status(status);
    }

    fn trigger_active(
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

    async fn apply_entries(
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

    async fn launch_entry(&mut self, entry: &AutostartEntry, config: &Config) {
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

    async fn stop_entry(&mut self, entry: &AutostartEntry, config: &Config) {
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

    async fn start_entry_manual(&mut self, id: &str) {
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

    async fn stop_entry_manual(&mut self, id: &str) {
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

    // ------------------------------------------------------------------ WiVRn

    async fn apply_wivrn_watchdog(&mut self, config: &Config, wivrn: &WivrnState) {
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

    async fn spawn_wivrn(&mut self, config: &Config) {
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

    async fn start_wivrn(&mut self) {
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
    async fn stop_wivrn(&mut self, by_user: bool) {
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

    async fn force_kill_wivrn(&mut self, config: &Config) {
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

    async fn restart_wivrn(&mut self) {
        self.shared.info("Restarting WiVRn…");
        self.stop_wivrn(false).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let config = self.shared.config_snapshot();
        self.watch.suppressed = false;
        self.watch.failures = 0;
        self.spawn_wivrn(&config).await;
    }

    // ------------------------------------------------------------------ audio

    async fn apply_audio(&mut self, config: &Config, wivrn: &WivrnState) {
        if !config.audio.enabled {
            self.audio.initialized = true;
            return;
        }

        if !self.audio.initialized {
            // First observation: adopt the current world without switching, so
            // starting lvr never yanks audio around unexpectedly.
            self.audio.initialized = true;
            self.audio.on_vr = wivrn.headset_connected;
            if wivrn.headset_connected {
                self.route_audio_to_vr(false).await;
            }
            return;
        }

        if wivrn.headset_connected && !self.audio.on_vr {
            self.route_audio_to_vr(false).await;
        } else if !wivrn.headset_connected && self.audio.on_vr {
            self.route_audio_to_desktop(false).await;
        }
    }

    async fn route_audio_to_vr(&mut self, manual: bool) {
        let config = self.shared.config_snapshot();
        let vr_sink = config.audio.vr_sink.trim().to_string();
        let vr_source = config.audio.vr_source.trim().to_string();

        // Remember where to go back to, but never remember the VR devices.
        if let Ok(current) = audio::get_default(Kind::Sink).await
            && current != vr_sink
        {
            self.audio.saved_sink = Some(current);
        }
        if let Ok(current) = audio::get_default(Kind::Source).await
            && current != vr_source
        {
            self.audio.saved_source = Some(current);
        }

        let outcome =
            audio::route(Some(&vr_sink), Some(&vr_source), config.audio.move_streams).await;
        self.report_route(
            outcome,
            true,
            manual,
            "Could not route audio to VR — check the Audio tab",
        );
    }

    async fn route_audio_to_desktop(&mut self, manual: bool) {
        let config = self.shared.config_snapshot();
        let sink = audio::desktop_target(
            Kind::Sink,
            &config.audio.desktop_sink,
            self.audio.saved_sink.as_deref(),
            &config.audio.vr_sink,
        )
        .await;
        let source = audio::desktop_target(
            Kind::Source,
            &config.audio.desktop_source,
            self.audio.saved_source.as_deref(),
            &config.audio.vr_source,
        )
        .await;

        let outcome = audio::route(
            sink.as_deref(),
            source.as_deref(),
            config.audio.move_streams,
        )
        .await;
        self.report_route(
            outcome,
            false,
            manual,
            "No desktop audio device to switch back to — pick one in the Audio tab",
        );
    }

    fn report_route(
        &mut self,
        outcome: audio::RouteOutcome,
        to_vr: bool,
        manual: bool,
        nothing_happened: &str,
    ) {
        for error in &outcome.errors {
            self.shared.warn(error.clone());
        }
        if !outcome.is_empty() {
            self.audio.on_vr = to_vr;
            let how = match (manual, to_vr) {
                (true, _) => "Audio (manual)",
                (false, true) => "Headset connected — audio",
                (false, false) => "Headset disconnected — audio",
            };
            self.shared.info(format!("{how}: {}", outcome.summary()));
        } else {
            if !to_vr {
                self.audio.on_vr = false;
            }
            if manual {
                self.shared.warn(nothing_happened.to_string());
            }
        }
        self.last_audio_poll = None;
    }

    async fn refresh_audio_cache(&mut self, config: &Config) {
        let now = Instant::now();
        let due_defaults = self
            .last_audio_poll
            .is_none_or(|last| now.duration_since(last) >= AUDIO_POLL_INTERVAL);
        if due_defaults {
            self.last_audio_poll = Some(now);
            if let Ok(sink) = audio::get_default(Kind::Sink).await {
                self.cached_default_sink = sink;
            }
            if let Ok(source) = audio::get_default(Kind::Source).await {
                self.cached_default_source = source;
            }
            if config.audio.enabled {
                // Keep the indicator honest if something else moved the default.
                let vr_sink = config.audio.vr_sink.trim();
                if !vr_sink.is_empty() {
                    self.audio.on_vr = self.cached_default_sink == vr_sink;
                }
            }
        }

        let due_devices = self
            .last_device_poll
            .is_none_or(|last| now.duration_since(last) >= DEVICE_POLL_INTERVAL);
        if due_devices {
            self.last_device_poll = Some(now);
            if let Ok(sinks) = audio::list_devices(Kind::Sink).await {
                self.cached_sinks = sinks;
            }
            if let Ok(sources) = audio::list_devices(Kind::Source).await {
                self.cached_sources = sources;
            }
        }
    }

    // -------------------------------------------------------------- stop all

    async fn stop_all_vr(&mut self) {
        self.shared.info("Stopping everything VR…");
        let config = self.shared.config_snapshot();

        for entry in config.autostart.iter().filter(|e| e.include_in_stop_all) {
            self.stop_entry(entry, &config).await;
            let runtime = self.runtimes.entry(entry.id.clone()).or_default();
            runtime.suppressed = true;
            runtime.launched_this_cycle = true;
        }

        self.stop_wivrn(true).await;

        if config.audio.enabled {
            self.route_audio_to_desktop(false).await;
        }
        self.shared
            .info("Everything VR stopped. WiVRn watchdog is paused until you start it again.");
    }

    // ------------------------------------------------------------ steam profile

    /// Re-read which Proton profile the managed app is pinned to. Cheap enough
    /// (two file reads) but pointless every tick.
    fn refresh_steam_cache(&mut self, config: &Config) {
        if !config.steam.enabled {
            self.steam = SteamState::default();
            return;
        }
        if self.steam.switching {
            return;
        }
        if let Some(last) = self.steam.last_poll
            && last.elapsed() < STEAM_POLL_INTERVAL
        {
            return;
        }
        self.steam.last_poll = Some(Instant::now());
        match steam::SteamPaths::discover(&config.steam, &config.steam.app_id)
            .and_then(|paths| steam::read_setup(&paths, &config.steam.app_id))
        {
            Ok(setup) => {
                self.steam.profile = steam::active_profile(&config.steam, &setup);
                self.steam.compat_tool = setup.compat_tool;
            }
            Err(err) => {
                tracing::debug!("reading the Steam profile failed: {err:#}");
                self.steam.profile = None;
                self.steam.compat_tool.clear();
            }
        }
    }

    async fn switch_steam_profile(&mut self, name: &str) {
        let config = self.shared.config_snapshot();
        if !config.steam.enabled {
            self.shared.warn("Steam profile switching is disabled in the config");
            return;
        }
        let Some(profile) = config.steam.profiles.iter().find(|p| p.name == name) else {
            self.shared.error(format!("No Steam profile named \"{name}\""));
            return;
        };

        self.steam.switching = true;
        let result = self.apply_steam_profile(&config, profile).await;
        self.steam.switching = false;
        self.steam.last_poll = None;

        match result {
            Ok(()) => {
                self.steam.profile = Some(profile.name.clone());
                self.steam.compat_tool = profile.compat_tool.clone();
                self.shared.info(format!(
                    "VRChat now runs on {} ({}). Steam is starting again.",
                    profile.name, profile.compat_tool
                ));
            }
            Err(err) => self
                .shared
                .error(format!("Switching to {} failed: {err:#}", profile.name)),
        }
    }

    /// Quit Steam, rewrite the two VDF files, start Steam again.
    async fn apply_steam_profile(
        &mut self,
        config: &Config,
        profile: &crate::config::SteamProfile,
    ) -> anyhow::Result<()> {
        let paths = steam::SteamPaths::discover(&config.steam, &config.steam.app_id)?;
        let compat_dir = paths.root.join("compatibilitytools.d").join(&profile.compat_tool);
        if !compat_dir.is_dir() {
            self.shared.warn(format!(
                "{} is not installed under {} — Steam may fall back to another Proton",
                profile.compat_tool,
                compat_dir.parent().unwrap_or(&paths.root).display()
            ));
        }

        if steam::steam_running() {
            self.shared.info("Shutting Steam down so it cannot overwrite its config…");
            steam::shutdown_steam(&config.steam).await?;
        }

        let setup = steam::AppSetup {
            compat_tool: profile.compat_tool.clone(),
            launch_options: profile.launch_options.clone(),
        };
        steam::write_setup(&paths, &config.steam.app_id, &setup)?;
        self.shared.info(format!(
            "Set AppID {} to {}",
            config.steam.app_id, profile.compat_tool
        ));

        steam::start_steam(&config.steam).await?;
        Ok(())
    }
}

/// One-shot audio routing for `lvr --audio vr|desktop`, usable without a
/// running instance (handy for a keyboard shortcut).
pub async fn route_audio(config: &Config, to_vr: bool) -> audio::RouteOutcome {
    let (sink, source) = if to_vr {
        (
            Some(config.audio.vr_sink.clone()),
            Some(config.audio.vr_source.clone()),
        )
    } else {
        (
            audio::desktop_target(
                Kind::Sink,
                &config.audio.desktop_sink,
                None,
                &config.audio.vr_sink,
            )
            .await,
            audio::desktop_target(
                Kind::Source,
                &config.audio.desktop_source,
                None,
                &config.audio.vr_source,
            )
            .await,
        )
    };
    audio::route(
        sink.as_deref(),
        source.as_deref(),
        config.audio.move_streams,
    )
    .await
}

/// One read-only pass over the world: no processes are started or stopped and
/// no audio is switched. Used by `lvr --status`.
pub async fn probe(config: &Config) -> Status {
    let mut scanner = ProcessScanner::new();
    let snapshot = scanner.scan();
    let mut wivrn_client = WivrnClient::new();
    let wivrn = wivrn_client.poll().await;
    let vrchat_running = snapshot.any_matching(&config.general.vrchat_match, &[]);

    let entries = config
        .autostart
        .iter()
        .map(|entry| {
            let pids = snapshot.matching(&entry.effective_patterns(), &[]);
            EntryStatus {
                id: entry.id.clone(),
                name: entry.name_or_id().to_string(),
                running: !pids.is_empty(),
                pids,
                trigger_active: Engine::trigger_active(
                    &entry.trigger,
                    &snapshot,
                    &wivrn,
                    vrchat_running,
                ),
                ..Default::default()
            }
        })
        .collect();

    let default_sink = audio::get_default(Kind::Sink).await.unwrap_or_default();
    let default_source = audio::get_default(Kind::Source).await.unwrap_or_default();
    let audio_on_vr =
        !config.audio.vr_sink.trim().is_empty() && default_sink == config.audio.vr_sink.trim();

    let steam_setup = if config.steam.enabled {
        steam::SteamPaths::discover(&config.steam, &config.steam.app_id)
            .and_then(|paths| steam::read_setup(&paths, &config.steam.app_id))
            .ok()
    } else {
        None
    };

    Status {
        wivrn_running: wivrn.running,
        headset_connected: wivrn.headset_connected,
        headset_name: wivrn.system_name.clone(),
        session_running: wivrn.session_running,
        vrchat_running,
        watchdog_paused: false,
        wivrn_failures: 0,
        default_sink,
        default_source,
        audio_on_vr,
        entries,
        steam_profile: steam_setup
            .as_ref()
            .and_then(|setup| steam::active_profile(&config.steam, setup)),
        steam_compat_tool: steam_setup
            .map(|setup| setup.compat_tool)
            .unwrap_or_default(),
        steam_switching: false,
        sinks: audio::list_devices(Kind::Sink).await.unwrap_or_default(),
        sources: audio::list_devices(Kind::Source).await.unwrap_or_default(),
        last_tick: Some(chrono::Local::now()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Trigger;

    fn entry() -> AutostartEntry {
        AutostartEntry {
            id: "test".into(),
            name: "Test".into(),
            enabled: true,
            trigger: Trigger::Vrchat,
            command: "/bin/true".into(),
            grace_secs: 120,
            ..Default::default()
        }
    }

    fn input(trigger_active: bool, running: bool, now: Instant) -> PlanInput {
        PlanInput {
            trigger_active,
            running,
            now,
            relaunch_debounce: Duration::from_secs(30),
        }
    }

    #[test]
    fn starts_when_trigger_appears() {
        let mut runtime = EntryRuntime::default();
        let now = Instant::now();
        assert_eq!(
            runtime.plan(&entry(), input(true, false, now)),
            Action::Start
        );
    }

    #[test]
    fn does_not_start_twice_while_the_app_boots() {
        let mut runtime = EntryRuntime::default();
        let start = Instant::now();
        assert_eq!(
            runtime.plan(&entry(), input(true, false, start)),
            Action::Start
        );
        // Still not visible in the process table a second later.
        assert_eq!(
            runtime.plan(&entry(), input(true, false, start + Duration::from_secs(1))),
            Action::None
        );
        // And not even after the debounce, because it already launched.
        assert_eq!(
            runtime.plan(
                &entry(),
                input(true, false, start + Duration::from_secs(60))
            ),
            Action::None
        );
    }

    #[test]
    fn restart_on_exit_relaunches_after_the_debounce() {
        let config = AutostartEntry {
            restart_on_exit: true,
            ..entry()
        };
        let mut runtime = EntryRuntime::default();
        let start = Instant::now();
        assert_eq!(
            runtime.plan(&config, input(true, false, start)),
            Action::Start
        );
        assert_eq!(
            runtime.plan(&config, input(true, false, start + Duration::from_secs(5))),
            Action::None
        );
        assert_eq!(
            runtime.plan(&config, input(true, false, start + Duration::from_secs(31))),
            Action::Start
        );
    }

    #[test]
    fn stops_only_after_the_grace_period() {
        let config = entry();
        let mut runtime = EntryRuntime::default();
        let start = Instant::now();
        runtime.plan(&config, input(true, true, start));

        assert_eq!(
            runtime.plan(&config, input(false, true, start + Duration::from_secs(1))),
            Action::None
        );
        assert_eq!(
            runtime.plan(
                &config,
                input(false, true, start + Duration::from_secs(119))
            ),
            Action::None
        );
        assert_eq!(
            runtime.plan(
                &config,
                input(false, true, start + Duration::from_secs(121))
            ),
            Action::Stop
        );
    }

    #[test]
    fn an_app_that_was_already_running_at_startup_is_left_alone() {
        let config = entry();
        let mut runtime = EntryRuntime::default();
        let start = Instant::now();
        // lvr starts: VRChat is not running but the app is. We never saw the
        // trigger, so the grace timer must stay disarmed.
        for minutes in [0, 5, 60] {
            assert_eq!(
                runtime.plan(
                    &config,
                    input(false, true, start + Duration::from_secs(minutes * 60))
                ),
                Action::None
            );
        }
        assert!(runtime.stop_at.is_none());

        // Once the trigger has been seen, normal behaviour resumes.
        runtime.plan(
            &config,
            input(true, true, start + Duration::from_secs(3600)),
        );
        assert!(runtime.armed);
        runtime.plan(
            &config,
            input(false, true, start + Duration::from_secs(3601)),
        );
        assert_eq!(
            runtime.plan(
                &config,
                input(false, true, start + Duration::from_secs(3800))
            ),
            Action::Stop
        );
    }

    #[test]
    fn grace_of_zero_stops_immediately() {
        let config = AutostartEntry {
            grace_secs: 0,
            ..entry()
        };
        let mut runtime = EntryRuntime::default();
        let now = Instant::now();
        runtime.plan(&config, input(true, true, now));
        assert_eq!(runtime.plan(&config, input(false, true, now)), Action::Stop);
    }

    #[test]
    fn negative_grace_never_stops() {
        let config = AutostartEntry {
            grace_secs: -1,
            ..entry()
        };
        let mut runtime = EntryRuntime::default();
        let start = Instant::now();
        runtime.plan(&config, input(true, true, start));
        for minutes in [1, 10, 600] {
            assert_eq!(
                runtime.plan(
                    &config,
                    input(false, true, start + Duration::from_secs(minutes * 60))
                ),
                Action::None
            );
        }
        assert_eq!(runtime.stop_at, None);
    }

    #[test]
    fn trigger_returning_cancels_a_pending_stop() {
        let config = entry();
        let mut runtime = EntryRuntime::default();
        let start = Instant::now();
        runtime.plan(&config, input(true, true, start));
        runtime.plan(&config, input(false, true, start + Duration::from_secs(1)));
        assert!(runtime.stop_at.is_some());
        runtime.plan(&config, input(true, true, start + Duration::from_secs(2)));
        assert!(runtime.stop_at.is_none());
        assert_eq!(
            runtime.plan(&config, input(false, true, start + Duration::from_secs(3))),
            Action::None
        );
    }

    #[test]
    fn start_delay_is_honoured() {
        let config = AutostartEntry {
            start_delay_secs: 10,
            ..entry()
        };
        let mut runtime = EntryRuntime::default();
        let start = Instant::now();
        assert_eq!(
            runtime.plan(&config, input(true, false, start)),
            Action::None
        );
        assert_eq!(
            runtime.plan(&config, input(true, false, start + Duration::from_secs(5))),
            Action::None
        );
        assert_eq!(
            runtime.plan(&config, input(true, false, start + Duration::from_secs(11))),
            Action::Start
        );
    }

    #[test]
    fn disabled_entries_do_nothing() {
        let config = AutostartEntry {
            enabled: false,
            ..entry()
        };
        let mut runtime = EntryRuntime::default();
        let now = Instant::now();
        assert_eq!(runtime.plan(&config, input(true, false, now)), Action::None);
        assert_eq!(runtime.plan(&config, input(false, true, now)), Action::None);
    }

    #[test]
    fn suppression_blocks_restart_until_the_trigger_cycles() {
        let config = entry();
        let mut runtime = EntryRuntime::default();
        let start = Instant::now();
        runtime.suppressed = true;
        assert_eq!(
            runtime.plan(&config, input(true, false, start)),
            Action::None
        );

        // Trigger goes away: suppression lifts.
        runtime.plan(&config, input(false, false, start + Duration::from_secs(1)));
        assert!(!runtime.suppressed);
        assert_eq!(
            runtime.plan(&config, input(true, false, start + Duration::from_secs(2))),
            Action::Start
        );
    }

    #[test]
    fn manual_trigger_is_never_active() {
        let snapshot = ProcSnapshot::default();
        let wivrn = WivrnState {
            running: true,
            headset_connected: true,
            ..Default::default()
        };
        assert!(!Engine::trigger_active(
            &Trigger::Manual,
            &snapshot,
            &wivrn,
            true
        ));
    }

    #[test]
    fn trigger_active_maps_each_source() {
        let snapshot = ProcSnapshot {
            procs: vec![crate::procs::ProcInfo {
                pid: 5,
                ppid: None,
                haystack: "some-daemon --run".into(),
            }],
        };
        let off = WivrnState::default();
        let on = WivrnState {
            running: true,
            headset_connected: true,
            ..Default::default()
        };

        assert!(Engine::trigger_active(
            &Trigger::Vrchat,
            &snapshot,
            &off,
            true
        ));
        assert!(!Engine::trigger_active(
            &Trigger::Vrchat,
            &snapshot,
            &off,
            false
        ));
        assert!(Engine::trigger_active(
            &Trigger::WivrnRunning,
            &snapshot,
            &on,
            false
        ));
        assert!(!Engine::trigger_active(
            &Trigger::WivrnRunning,
            &snapshot,
            &off,
            false
        ));
        assert!(Engine::trigger_active(
            &Trigger::HeadsetConnected,
            &snapshot,
            &on,
            false
        ));
        assert!(Engine::trigger_active(
            &Trigger::Process("SOME-daemon".into()),
            &snapshot,
            &off,
            false
        ));
        assert!(!Engine::trigger_active(
            &Trigger::Process("  ".into()),
            &snapshot,
            &off,
            false
        ));
    }

    #[test]
    fn seconds_until_counts_down_and_never_goes_negative() {
        let now = Instant::now();
        assert_eq!(EntryRuntime::seconds_until(None, now), None);
        assert_eq!(
            EntryRuntime::seconds_until(Some(now + Duration::from_secs(42)), now),
            Some(42)
        );
        assert_eq!(
            EntryRuntime::seconds_until(Some(now - Duration::from_secs(42)), now),
            Some(0)
        );
    }
}
