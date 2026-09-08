//! The supervisor: everything that watches, starts and stops things.
//!
//! It runs on its own tokio runtime thread. The GUI and tray never touch
//! processes directly — they push [`Command`]s and read [`Status`].

mod audio_routing;
mod entries;
mod watchdog;

#[cfg(test)]
mod tests;

pub use entries::EntryRuntime;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;

use crate::audio::{self, Kind};
use crate::config::Config;
use crate::display;
use crate::procs::{ChildRegistry, ProcessScanner};
use crate::state::{Command, EntryStatus, Shared, Status};
use crate::wivrn::WivrnClient;

use self::audio_routing::AudioRouting;
use self::watchdog::WivrnWatch;

const MEDIA_BLOCK_POLL_INTERVAL: Duration = Duration::from_secs(5);

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
    block_state: crate::domain_block::BlockState,
    last_media_block_poll: Option<Instant>,
    virtual_display_created: bool,
    virtual_display_info: Option<String>,
    last_display_count: Option<usize>,
    pending_virtual_display_action: Option<(bool, Instant)>,
}

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
            block_state: crate::domain_block::detect_vrc_prefix("")
                .map(|p| crate::domain_block::read_block_state(&p))
                .unwrap_or_default(),
            last_media_block_poll: None,
            virtual_display_created: false,
            virtual_display_info: None,
            last_display_count: None,
            pending_virtual_display_action: None,
        }
    }

    pub async fn run(mut self) {
        self.shared.info("Supervisor started");
        let domain_cfg = self.shared.config().domain_block.clone();
        tokio::spawn(async move {
            crate::domain_block::init_from_remote_or_fallback_with_config(&domain_cfg).await;
        });
        if self.shared.config().virtual_display.create_on_startup {
            let physical_count = display::get_connected_display_count();
            if physical_count == 0 {
                let res = self.shared.config().virtual_display.resolution.clone();
                if let Some(info) = display::create_virtual_display(&res) {
                    self.virtual_display_created = true;
                    self.virtual_display_info = Some(info.clone());
                    self.shared.info(format!("Created virtual display {info} on app startup (no physical display connected)"));
                }
            } else {
                self.shared.debug(format!("Skipping virtual display creation on startup: {physical_count} physical display(s) connected"));
            }
        }
        self.last_display_count = Some(display::get_connected_display_count());

        let initial_poll = self.shared.config().general.poll_interval_ms;
        let mut interval = tokio::time::interval(Duration::from_millis(initial_poll));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut current_interval_ms = initial_poll;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.tick().await;
                    let desired_ms = self.shared.config().general.poll_interval_ms;
                    if desired_ms != current_interval_ms {
                        current_interval_ms = desired_ms;
                        interval = tokio::time::interval(Duration::from_millis(desired_ms));
                        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    }
                }
                cmd = self.rx.recv() => match cmd {
                    Some(Command::Quit) | None => break,
                    Some(other) => self.handle_command(other).await,
                }
            }
        }

        self.shutdown().await;
    }

    async fn shutdown(&mut self) {
        if self.virtual_display_created {
            display::remove_virtual_display();
            self.shared.info("Cleaned up virtual display on app exit");
        }
        self.shared.info("Supervisor stopped");
    }

    async fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Quit => {}
            Command::Poke => self.tick().await,
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
            Command::ToggleBlockCategory(category) => {
                let current = self.block_state.is_blocked(&category);
                self.set_block_category(category, !current).await;
            }
            Command::ReloadDomainLists => {
                let domain_cfg = self.shared.config().domain_block.clone();
                let lists = crate::domain_block::reload_domain_lists_with_config(&domain_cfg).await;
                if let Some(prefix) = crate::domain_block::detect_vrc_prefix("") {
                    let _ = crate::domain_block::sync_all(&prefix, &self.block_state);
                }
                self.shared.info(format!(
                    "Updated active domain lists: {} videos, {} images, {} strings, {} shared (total: {})",
                    lists.count_for_category(&crate::domain_block::BlockCategory::Video),
                    lists.count_for_category(&crate::domain_block::BlockCategory::Images),
                    lists.count_for_category(&crate::domain_block::BlockCategory::Strings),
                    lists.count_for_category(&crate::domain_block::BlockCategory::Shared),
                    lists.total_count()
                ));

            }
            Command::CreateVirtualDisplay => {
                self.pending_virtual_display_action = None;
                let res = self.shared.config().virtual_display.resolution.clone();
                if let Some(info) = display::create_virtual_display(&res) {
                    self.virtual_display_created = true;
                    self.virtual_display_info = Some(info.clone());
                    self.shared.info(format!("Created virtual display {info}"));
                } else {
                    self.shared.error("Failed to create virtual display");
                }
            }
            Command::RemoveVirtualDisplay => {
                self.pending_virtual_display_action = None;
                display::remove_virtual_display();
                self.virtual_display_created = false;
                self.virtual_display_info = None;
                self.shared.info("Removed virtual display");
            }
            Command::SaveConfig => match self.shared.save_config() {
                Ok(()) => self
                    .shared
                    .info(format!("Saved {}", self.shared.config_path().display())),
                Err(err) => self.shared.error(format!("Saving config failed: {err:#}")),
            },
        }
    }

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
        self.refresh_media_block_cache();

        self.update_virtual_display(&config);

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
            block_state: self.block_state.clone(),
            sinks: self.cached_sinks.clone(),
            sources: self.cached_sources.clone(),
            virtual_display_created: self.virtual_display_created,
            virtual_display_info: self.virtual_display_info.clone(),
        };
        self.shared.set_status(status);
    }

    fn update_virtual_display(&mut self, config: &Config) {
        let current_displays = display::get_connected_display_count();
        let now = Instant::now();
        let debounce_dur = Duration::from_secs(config.virtual_display.debounce_secs);

        if let Some(last_count) = self.last_display_count {
            if last_count > 0 && current_displays == 0 && config.virtual_display.create_on_last_display_unplugged {
                if debounce_dur.is_zero() {
                    self.pending_virtual_display_action = None;
                    self.shared.warn("Last physical display unplugged, creating virtual display...");
                    let res = config.virtual_display.resolution.clone();
                    if let Some(info) = display::create_virtual_display(&res) {
                        self.virtual_display_created = true;
                        self.virtual_display_info = Some(info.clone());
                        self.shared.info(format!("Virtual display {info} created on display disconnect"));
                    }
                } else {
                    self.shared.warn(format!(
                        "Last physical display unplugged; debouncing virtual display creation for {}s...",
                        config.virtual_display.debounce_secs
                    ));
                    self.pending_virtual_display_action = Some((true, now + debounce_dur));
                }
            } else if last_count == 0 && current_displays > 0 && (self.virtual_display_created || matches!(self.pending_virtual_display_action, Some((true, _)))) {
                if debounce_dur.is_zero() {
                    self.pending_virtual_display_action = None;
                    if self.virtual_display_created {
                        self.shared.info("Physical display re-connected, removing virtual display...");
                        if display::remove_virtual_display() {
                            self.virtual_display_created = false;
                            self.virtual_display_info = None;
                            self.shared.info("Virtual display removed");
                        }
                    }
                } else {
                    self.shared.info(format!(
                        "Physical display re-connected; debouncing virtual display removal for {}s...",
                        config.virtual_display.debounce_secs
                    ));
                    self.pending_virtual_display_action = Some((false, now + debounce_dur));
                }
            }
        }

        // Evaluate any pending debounced action
        if let Some((should_create, due_time)) = self.pending_virtual_display_action {
            if should_create {
                if current_displays > 0 {
                    self.shared.info("Physical display reappeared before debounce elapsed; cancelled virtual display creation");
                    self.pending_virtual_display_action = None;
                } else if now >= due_time {
                    self.pending_virtual_display_action = None;
                    if !self.virtual_display_created && config.virtual_display.create_on_last_display_unplugged {
                        self.shared.warn("Debounce elapsed; creating virtual display...");
                        let res = config.virtual_display.resolution.clone();
                        if let Some(info) = display::create_virtual_display(&res) {
                            self.virtual_display_created = true;
                            self.virtual_display_info = Some(info.clone());
                            self.shared.info(format!("Virtual display {info} created on display disconnect"));
                        }
                    }
                }
            } else if current_displays == 0 {
                self.shared.info("Physical display disconnected before debounce elapsed; keeping virtual display");
                self.pending_virtual_display_action = None;
            } else if now >= due_time {
                self.pending_virtual_display_action = None;
                if self.virtual_display_created {
                    self.shared.info("Debounce elapsed; removing virtual display...");
                    if display::remove_virtual_display() {
                        self.virtual_display_created = false;
                        self.virtual_display_info = None;
                        self.shared.info("Virtual display removed");
                    }
                }
            }
        }

        self.last_display_count = Some(current_displays);
    }

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

    fn refresh_media_block_cache(&mut self) {
        if let Some(last) = self.last_media_block_poll
            && last.elapsed() < MEDIA_BLOCK_POLL_INTERVAL
        {
            return;
        }
        self.last_media_block_poll = Some(Instant::now());

        if let Some(prefix) = crate::domain_block::detect_vrc_prefix("") {
            self.block_state = crate::domain_block::read_block_state(&prefix);
        }
    }

    async fn set_block_category(&mut self, category: crate::domain_block::BlockCategory, block: bool) {
        let Some(prefix) = crate::domain_block::detect_vrc_prefix("") else {
            self.shared.error("Could not find VRChat Proton prefix to toggle media blocking");
            return;
        };

        match crate::domain_block::set_category_blocked(&prefix, &category, block) {
            Ok(()) => {
                self.block_state.set_blocked(&category, block);
                let label = category.label();
                if block {
                    self.shared.warn(format!("VRChat {label} BLOCKED"));
                } else {
                    self.shared.info(format!("VRChat {label} ALLOWED (unblocked)"));
                }
            }
            Err(err) => {
                self.shared.error(format!("Failed to update {category:?} blocking state: {err:#}"));
            }
        }
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
        block_state: crate::domain_block::detect_vrc_prefix("")
            .map(|p| crate::domain_block::read_block_state(&p))
            .unwrap_or_default(),
        sinks: audio::list_devices(Kind::Sink).await.unwrap_or_default(),
        sources: audio::list_devices(Kind::Source).await.unwrap_or_default(),
        virtual_display_created: false,
        virtual_display_info: None,
    }
}
